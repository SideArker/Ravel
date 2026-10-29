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
    #[cfg(windows)]
    repeat: Option<(u32, std::time::Instant)>,
}
// A single backend lives for the session; boxing would add indirection to save a few hundred bytes.
#[allow(clippy::large_enum_variant)]
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
                repeat: None,
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
    pub fn maintenance(&mut self) -> Result<bool> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.emergency_stop(),
            #[cfg(target_os = "linux")]
            Platform::Wayland { .. } => Ok(false),
            #[cfg(windows)]
            Platform::Windows(_) => {
                if let Some((code, deadline)) = self.repeat
                    && std::time::Instant::now() >= deadline
                {
                    windows::inject(Input::Key { code, down: true })?;
                    self.repeat = Some((
                        code,
                        std::time::Instant::now() + std::time::Duration::from_millis(32),
                    ));
                }
                Ok(windows::emergency_stop())
            }
        }
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
        #[cfg(windows)]
        if let Input::Key { code, down } = event {
            if down && ![29, 42, 54, 56, 97, 100, 125, 126, 58, 69, 70].contains(&code) {
                self.repeat = Some((
                    code,
                    std::time::Instant::now() + std::time::Duration::from_millis(500),
                ));
            } else if !down && self.repeat.is_some_and(|(key, _)| key == code) {
                self.repeat = None;
            }
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
        #[cfg(windows)]
        {
            self.repeat = None;
        }
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

#[cfg(all(test, target_os = "linux"))]
mod native_tests {
    use super::*;
    use x11rb::{
        connection::Connection,
        protocol::{
            xproto::{ConnectionExt as _, GrabMode, GrabStatus, KeyButMask},
            xtest::ConnectionExt as _,
        },
    };
    #[tokio::test]
    #[ignore = "requires isolated Xvfb, WAYLAND_DISPLAY unset and RAVEL_TEST_X11=1"]
    async fn x11_edge_capture_injection_stop_and_cleanup() {
        assert_eq!(std::env::var("RAVEL_TEST_X11").as_deref(), Ok("1"));
        let (observer, screen) = x11rb::connect(None).unwrap();
        let root = observer.setup().roots[screen].root;
        let mut b = Backend::new().await.unwrap();
        assert_eq!(b.label, "Native X11: grabs + XTest");
        b.arm(Some(Side::Right)).await.unwrap();
        b.inject(Input::Motion { x: 1., y: 0.5 }).unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), b.next())
            .await
            .unwrap()
            .unwrap();
        let CaptureEvent::Begin { point } = event else {
            panic!("Expected edge capture");
        };
        assert_eq!(point[0], b.display.width - 1.);
        observer
            .xtest_fake_input(
                x11rb::protocol::xproto::KEY_PRESS_EVENT,
                38,
                0,
                root,
                0,
                0,
                0,
            )
            .unwrap()
            .check()
            .unwrap();
        observer.flush().unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), b.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            CaptureEvent::Input(input_event::Event::Keyboard(
                input_event::KeyboardEvent::Key {
                    key: 30,
                    state: 1,
                    ..
                }
            ))
        ));
        observer
            .xtest_fake_input(
                x11rb::protocol::xproto::KEY_RELEASE_EVENT,
                38,
                0,
                root,
                0,
                0,
                0,
            )
            .unwrap()
            .check()
            .unwrap();
        b.inject(Input::Key {
            code: 30,
            down: true,
        })
        .unwrap();
        b.inject(Input::Button {
            code: 0x110,
            down: true,
        })
        .unwrap();
        let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
        assert!(keys[38 / 8] & (1 << (38 % 8)) != 0);
        for code in [1, 29, 56, 42] {
            b.inject(Input::Key { code, down: true }).unwrap();
        }
        assert!(b.maintenance().unwrap());
        b.close().await.unwrap();
        let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
        for code in [30u8, 1, 29, 56, 42] {
            let key = code + 8;
            assert_eq!(keys[key as usize / 8] & (1 << (key % 8)), 0);
        }
        assert!(
            !observer
                .query_pointer(root)
                .unwrap()
                .reply()
                .unwrap()
                .mask
                .contains(KeyButMask::BUTTON1)
        );
        let grabbed = observer
            .grab_keyboard(false, root, 0u32, GrabMode::ASYNC, GrabMode::ASYNC)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(grabbed.status, GrabStatus::SUCCESS);
        observer.ungrab_keyboard(0u32).unwrap().check().unwrap();
    }
}
