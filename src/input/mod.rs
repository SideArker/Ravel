use crate::{
    geometry::{Display, Side},
    protocol::Input,
};
use anyhow::{Result, ensure};
use futures::StreamExt;
pub use input_capture::CaptureEvent;
use input_capture::InputCapture;
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

pub struct Backend {
    platform: Platform,
    pub display: Display,
    pub label: &'static str,
    held: Held,
    armed: bool,
}
enum Platform {
    #[cfg(target_os = "linux")]
    Wayland {
        capture: InputCapture,
        injector: wayland::Wayland,
    },
    #[cfg(target_os = "linux")]
    X11(x11::X11),
    #[cfg(windows)]
    Windows(InputCapture),
}
impl Backend {
    pub async fn new() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            {
                let mut capture =
                    InputCapture::new(Some(input_capture::Backend::LayerShell)).await?;
                // Dispatch output metadata without registering an edge or capturing input.
                let _ = tokio::time::timeout(std::time::Duration::from_millis(150), capture.next())
                    .await;
                let bounds = capture.bounds().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Wayland requires exactly one active output with logical geometry"
                    )
                })?;
                let display = Display {
                    x: bounds[0] as f64,
                    y: bounds[1] as f64,
                    width: bounds[2] as f64,
                    height: bounds[3] as f64,
                    scale: 1.,
                }
                .validate()?;
                let injector = wayland::Wayland::new()?;
                return Ok(Self {
                    platform: Platform::Wayland { capture, injector },
                    display,
                    label: "Wayland: layer-shell + wlroots virtual input",
                    held: Held::default(),
                    armed: false,
                });
            }
            let backend = x11::X11::new()?;
            let display = backend.display();
            Ok(Self {
                platform: Platform::X11(backend),
                display,
                label: "Native X11: grabs + XTest",
                held: Held::default(),
                armed: false,
            })
        }
        #[cfg(windows)]
        {
            let display = windows::display()?;
            let capture = InputCapture::new(Some(input_capture::Backend::Windows)).await?;
            Ok(Self {
                platform: Platform::Windows(capture),
                display,
                label: "Windows: low-level hooks + SendInput",
                held: Held::default(),
                armed: false,
            })
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            anyhow::bail!("This OS is unsupported")
        }
    }
    pub async fn arm(&mut self, side: Option<Side>) -> Result<()> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.arm(side)?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => arm_native(capture, &mut self.armed, side).await?,
            #[cfg(windows)]
            Platform::Windows(capture) => arm_native(capture, &mut self.armed, side).await?,
        }
        Ok(())
    }
    pub async fn next(&mut self) -> Result<CaptureEvent> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.next().await,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => next_native(capture).await,
            #[cfg(windows)]
            Platform::Windows(capture) => next_native(capture).await,
        }
    }
    pub async fn release_capture(&mut self) -> Result<()> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.release()?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => capture.release().await?,
            #[cfg(windows)]
            Platform::Windows(capture) => capture.release().await?,
        }
        Ok(())
    }
    pub fn check_geometry(&self) -> Result<()> {
        match &self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.check_geometry()?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => {
                let b = capture
                    .bounds()
                    .ok_or_else(|| anyhow::anyhow!("Output configuration changed"))?;
                ensure!(
                    self.display.width == b[2] as f64 && self.display.height == b[3] as f64,
                    "Output size changed; reconnect sharing"
                );
            }
            #[cfg(windows)]
            Platform::Windows(_) => ensure!(
                windows::display()? == self.display,
                "Display changed; reconnect sharing"
            ),
        }
        Ok(())
    }
    pub fn inject(&mut self, event: Input) -> Result<()> {
        event.validate()?;
        if !self.held.update(event) {
            return Ok(());
        }
        self.inject_raw(event)
    }
    fn inject_raw(&mut self, event: Input) -> Result<()> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.inject(event),
            #[cfg(target_os = "linux")]
            Platform::Wayland { injector, .. } => injector.inject(event),
            #[cfg(windows)]
            Platform::Windows(_) => windows::inject(event),
        }
    }
    pub fn release_input(&mut self) -> Result<()> {
        let events = self.held.releases();
        let mut first = None;
        // Try every release even when a device refuses one event.
        for event in events {
            if let Err(error) = self.inject_raw(event) {
                first.get_or_insert(error);
            }
        }
        if let Some(error) = first {
            return Err(error);
        }
        Ok(())
    }
    pub async fn close(&mut self) -> Result<()> {
        let released = self.release_input();
        let disarmed = self.arm(None).await;
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => capture.terminate().await?,
            #[cfg(windows)]
            Platform::Windows(capture) => capture.terminate().await?,
            #[cfg(target_os = "linux")]
            Platform::X11(_) => {}
        }
        released?;
        disarmed?;
        Ok(())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.release_input();
    }
}
async fn arm_native(
    capture: &mut InputCapture,
    armed: &mut bool,
    side: Option<Side>,
) -> Result<()> {
    if *armed {
        capture.release().await?;
        capture.destroy(0).await?;
        *armed = false;
    }
    if let Some(side) = side {
        capture
            .create(
                0,
                match side {
                    Side::Left => input_capture::Position::Left,
                    Side::Right => input_capture::Position::Right,
                    Side::Top => input_capture::Position::Top,
                    Side::Bottom => input_capture::Position::Bottom,
                },
            )
            .await?;
        *armed = true;
    }
    Ok(())
}
async fn next_native(capture: &mut InputCapture) -> Result<CaptureEvent> {
    let (_, e) = capture
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("Input capture stream closed"))??;
    Ok(e)
}

#[derive(Default)]
pub struct Held {
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
}
impl Held {
    pub fn update(&mut self, event: Input) -> bool {
        match event {
            Input::Key { code, down } => {
                if down {
                    self.keys.insert(code)
                } else {
                    self.keys.remove(&code)
                }
            }
            Input::Button { code, down } => {
                if down {
                    self.buttons.insert(code)
                } else {
                    self.buttons.remove(&code)
                }
            }
            _ => true,
        }
    }
    pub fn emergency_stop(&self) -> bool {
        self.keys.contains(&1)
            && [29, 97].iter().any(|k| self.keys.contains(k))
            && [56, 100].iter().any(|k| self.keys.contains(k))
            && [42, 54].iter().any(|k| self.keys.contains(k))
    }
    pub fn releases(&mut self) -> Vec<Input> {
        let mut events = Vec::new();
        for code in std::mem::take(&mut self.keys) {
            events.push(Input::Key { code, down: false });
        }
        for code in std::mem::take(&mut self.buttons) {
            events.push(Input::Button { code, down: false });
        }
        events.push(Input::Modifiers {
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
        });
        events
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn releases_keys_and_buttons_and_detects_stop_chord() {
        let mut h = Held::default();
        for code in [1, 29, 56, 42] {
            h.update(Input::Key { code, down: true });
        }
        h.update(Input::Button {
            code: 0x110,
            down: true,
        });
        assert!(h.emergency_stop());
        let released = h.releases();
        assert_eq!(released.len(), 6);
        assert!(!h.emergency_stop());
        assert_eq!(h.releases().len(), 1);
    }
}
