use crate::protocol::Input;
use anyhow::{Context, Result, bail, ensure};
use ashpd::desktop::{
    PersistMode, Session,
    remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions},
};
use futures::{FutureExt, StreamExt};
use reis::{
    ei::{self, button::ButtonState, handshake::ContextType, keyboard::KeyState},
    event::{self, DeviceCapability, EiEvent},
    tokio::EiConvertEventStream,
};
use std::{
    os::unix::net::UnixStream,
    sync::{LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

static RESTORE_TOKEN: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

pub struct Portal {
    _desktop: RemoteDesktop,
    session: Session<RemoteDesktop>,
    context: ei::Context,
    connection: event::Connection,
    events: EiConvertEventStream,
    devices: Vec<event::Device>,
}

impl Portal {
    pub async fn new() -> Result<Self> {
        let desktop = RemoteDesktop::new()
            .await
            .context("RemoteDesktop portal unavailable")?;
        ensure!(
            desktop.version() >= 2,
            "RemoteDesktop portal needs version 2 for ConnectToEIS"
        );
        let available = desktop.available_device_types().await?;
        ensure!(
            available.contains(DeviceType::Keyboard) && available.contains(DeviceType::Pointer),
            "RemoteDesktop portal lacks keyboard or pointer injection"
        );
        let session = desktop.create_session(Default::default()).await?;
        let token = RESTORE_TOKEN.lock().unwrap().take();
        desktop
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(PersistMode::Application)
                    .set_restore_token(token.as_deref()),
            )
            .await?
            .response()
            .context("RemoteDesktop device selection denied")?;
        let granted = desktop
            .start(&session, None, Default::default())
            .await?
            .response()
            .context("RemoteDesktop permission denied")?;
        ensure!(
            granted.devices().contains(DeviceType::Keyboard)
                && granted.devices().contains(DeviceType::Pointer),
            "RemoteDesktop permission did not grant keyboard and pointer control"
        );
        *RESTORE_TOKEN.lock().unwrap() = granted.restore_token().map(str::to_owned);
        let fd = desktop.connect_to_eis(&session, Default::default()).await?;
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(true)?;
        let context = ei::Context::new(stream)?;
        let (connection, events) = context
            .handshake_tokio("Ravel", ContextType::Sender)
            .await?;
        let mut portal = Self {
            _desktop: desktop,
            session,
            context,
            connection,
            events,
            devices: Vec::new(),
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !portal.ready() {
                let event = portal.events.next().await.context("EIS stream closed")??;
                portal.handle(event)?;
            }
            Result::<()>::Ok(())
        })
        .await
        .context("Timed out waiting for EIS input devices")??;
        Ok(portal)
    }

    fn ready(&self) -> bool {
        self.device::<ei::PointerAbsolute>().is_some()
            && self.device::<ei::Button>().is_some()
            && self.device::<ei::Scroll>().is_some()
            && self.device::<ei::Keyboard>().is_some()
    }

    fn device<T: ei::Interface>(&self) -> Option<(&event::Device, T)> {
        self.devices
            .iter()
            .find_map(|d| d.interface::<T>().map(|i| (d, i)))
    }

    fn handle(&mut self, event: EiEvent) -> Result<()> {
        match event {
            EiEvent::SeatAdded(e) => {
                e.seat.bind_capabilities(
                    DeviceCapability::PointerAbsolute
                        | DeviceCapability::Button
                        | DeviceCapability::Scroll
                        | DeviceCapability::Keyboard,
                );
                self.context.flush()?;
            }
            EiEvent::DeviceAdded(_) => {}
            EiEvent::DeviceRemoved(e) => self.devices.retain(|d| d != &e.device),
            EiEvent::DevicePaused(e) => self.devices.retain(|d| d != &e.device),
            EiEvent::DeviceResumed(e) => {
                e.device.device().start_emulating(e.serial, 0);
                if !self.devices.contains(&e.device) {
                    self.devices.push(e.device);
                }
                self.context.flush()?;
            }
            EiEvent::Disconnected(e) => bail!("EIS disconnected: {:?}", e.reason),
            _ => {}
        }
        Ok(())
    }

    pub fn maintenance(&mut self) -> Result<()> {
        while let Some(event) = self.events.next().now_or_never() {
            self.handle(event.context("EIS stream closed")??)?;
        }
        ensure!(self.ready(), "EIS input devices are unavailable");
        Ok(())
    }

    pub fn inject(&mut self, input: Input) -> Result<()> {
        self.maintenance()?;
        let time = now();
        match input {
            Input::Motion { x, y } => {
                let (device, pointer) = self
                    .device::<ei::PointerAbsolute>()
                    .context("EIS absolute pointer unavailable")?;
                let (x, y) = absolute_point(device.regions(), x, y)?;
                pointer.motion_absolute(x, y);
                device.device().frame(self.connection.serial(), time);
            }
            Input::Key { code, down } => {
                let (device, keyboard) = self
                    .device::<ei::Keyboard>()
                    .context("EIS keyboard unavailable")?;
                keyboard.key(
                    code,
                    if down {
                        KeyState::Press
                    } else {
                        KeyState::Released
                    },
                );
                device.device().frame(self.connection.serial(), time);
            }
            Input::Button { code, down } => {
                let (device, button) = self
                    .device::<ei::Button>()
                    .context("EIS buttons unavailable")?;
                button.button(
                    code,
                    if down {
                        ButtonState::Press
                    } else {
                        ButtonState::Released
                    },
                );
                device.device().frame(self.connection.serial(), time);
            }
            Input::Scroll { axis, value } => {
                let (device, scroll) = self
                    .device::<ei::Scroll>()
                    .context("EIS scroll unavailable")?;
                if value.fract() == 0. {
                    if axis == 0 {
                        scroll.scroll_discrete(0, value as i32);
                    } else {
                        scroll.scroll_discrete(value as i32, 0);
                    }
                } else {
                    let pixels = (value / 120. * 15.) as f32;
                    if axis == 0 {
                        scroll.scroll(0., pixels);
                    } else {
                        scroll.scroll(pixels, 0.);
                    }
                }
                device.device().frame(self.connection.serial(), time);
            }
            Input::Modifiers { .. } => {}
        }
        self.context.flush()?;
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        self.session.close().await?;
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn absolute_point(regions: &[event::Region], x: f64, y: f64) -> Result<(f32, f32)> {
    let usable: Vec<_> = regions
        .iter()
        .filter(|r| r.width > 0 && r.height > 0)
        .collect();
    ensure!(!usable.is_empty(), "EIS pointer has no usable region");
    let left = usable
        .iter()
        .map(|r| r.x as f64)
        .fold(f64::INFINITY, f64::min);
    let top = usable
        .iter()
        .map(|r| r.y as f64)
        .fold(f64::INFINITY, f64::min);
    let right = usable
        .iter()
        .map(|r| r.x as f64 + r.width as f64)
        .fold(f64::NEG_INFINITY, f64::max);
    let bottom = usable
        .iter()
        .map(|r| r.y as f64 + r.height as f64)
        .fold(f64::NEG_INFINITY, f64::max);
    let target = (
        left + x * (right - left - 1.),
        top + y * (bottom - top - 1.),
    );
    let nearest = usable
        .iter()
        .map(|r| {
            let px = target.0.clamp(r.x as f64, r.x as f64 + r.width as f64 - 1.);
            let py = target
                .1
                .clamp(r.y as f64, r.y as f64 + r.height as f64 - 1.);
            let distance = (px - target.0).powi(2) + (py - target.1).powi(2);
            (distance, (px as f32, py as f32))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap();
    Ok(nearest.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_motion_stays_in_an_eis_region_when_monitors_have_a_gap() {
        let regions = [
            event::Region {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
                scale: 1.,
                mapping_id: None,
            },
            event::Region {
                x: 200,
                y: 0,
                width: 100,
                height: 100,
                scale: 1.,
                mapping_id: None,
            },
        ];
        assert_eq!(absolute_point(&regions, 0., 0.).unwrap(), (0., 0.));
        assert_eq!(absolute_point(&regions, 1., 1.).unwrap(), (299., 99.));
        let (x, y) = absolute_point(&regions, 0.5, 0.5).unwrap();
        assert!(x <= 99. || x >= 200., "gap coordinate {x}");
        assert_eq!(y, 49.5);
    }
}
