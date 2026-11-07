use crate::{
    geometry::{Display, Monitor, Side, monitor_bounds, selected_monitor},
    protocol::Input,
};
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, ensure};
use futures::StreamExt;
pub use input_capture::CaptureEvent;
use input_capture::InputCapture;
use std::collections::BTreeSet;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

pub struct Backend {
    platform: Platform,
    pub display: Display,
    pub desktop: Display,
    pub monitors: Vec<Monitor>,
    pub label: &'static str,
    held: Held,
    armed: bool,
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
    Portal {
        capture: InputCapture,
        injector: portal::Portal,
    },
    #[cfg(target_os = "linux")]
    X11(x11::X11),
    #[cfg(windows)]
    Windows(InputCapture),
    #[cfg(target_os = "macos")]
    MacOS {
        capture: InputCapture,
        injector: macos::Mac,
    },
    #[cfg(test)]
    Test {
        events: tokio::sync::mpsc::Receiver<CaptureEvent>,
        trace: std::rc::Rc<TestTrace>,
    },
}
#[cfg(target_os = "linux")]
fn wayland_monitors(capture: &InputCapture) -> Result<Vec<Monitor>> {
    let monitors: Vec<_> = capture
        .outputs()
        .into_iter()
        .map(|(id, bounds)| {
            Ok(Monitor {
                id,
                display: Display {
                    x: bounds[0] as f64,
                    y: bounds[1] as f64,
                    width: bounds[2] as f64,
                    height: bounds[3] as f64,
                    scale: 1.,
                }
                .validate()?,
            })
        })
        .collect::<Result<_>>()?;
    ensure!(
        !monitors.is_empty(),
        "Wayland output geometry is unavailable"
    );
    ensure!(
        monitors.iter().all(|m| !m.id.is_empty()),
        "Wayland output names are unavailable"
    );
    Ok(monitors)
}
impl Backend {
    pub async fn new() -> Result<Self> {
        Self::with_monitor(None).await
    }
    pub async fn with_monitor(selected: Option<&str>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            {
                let wlroots = wayland::Wayland::new();
                let mut capture = if wlroots.is_ok() {
                    InputCapture::new(Some(input_capture::Backend::LayerShell))
                        .await
                        .ok()
                } else {
                    None
                };
                if let (Some(mut capture), Ok(injector)) = (capture.take(), wlroots) {
                    // Dispatch output metadata without registering an edge or capturing input.
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(150), capture.next())
                            .await;
                    let monitors = wayland_monitors(&capture)?;
                    let display = selected_monitor(&monitors, selected)?;
                    let desktop = monitor_bounds(&monitors)?.validate()?;
                    capture.set_target([
                        display.x as i32,
                        display.y as i32,
                        display.width as i32,
                        display.height as i32,
                    ]);
                    return Ok(Self {
                        platform: Platform::Wayland { capture, injector },
                        display,
                        desktop,
                        monitors,
                        label: "Wayland: layer-shell + wlroots virtual input",
                        held: Held::default(),
                        armed: false,
                        repeat: None,
                    });
                }
                let mut capture =
                    InputCapture::new(Some(input_capture::Backend::InputCapturePortal))
                        .await
                        .context("wlroots protocols unavailable and InputCapture portal failed")?;
                let setup = (|| -> Result<_> {
                    let monitors = wayland_monitors(&capture)?;
                    let display = selected_monitor(&monitors, selected)?;
                    let desktop = monitor_bounds(&monitors)?.validate()?;
                    Ok((monitors, display, desktop))
                })();
                let (monitors, display, desktop) = match setup {
                    Ok(value) => value,
                    Err(error) => {
                        capture.terminate().await?;
                        return Err(error);
                    }
                };
                capture.set_target([
                    display.x as i32,
                    display.y as i32,
                    display.width as i32,
                    display.height as i32,
                ]);
                let injector = match portal::Portal::new().await {
                    Ok(value) => value,
                    Err(error) => {
                        capture.terminate().await?;
                        return Err(error);
                    }
                };
                return Ok(Self {
                    platform: Platform::Portal { capture, injector },
                    display,
                    desktop,
                    monitors,
                    label: "Wayland: InputCapture + RemoteDesktop portals",
                    held: Held::default(),
                    armed: false,
                    repeat: None,
                });
            }
            let backend = x11::X11::new(selected)?;
            let display = backend.display();
            let monitors = backend.monitors();
            let desktop = monitor_bounds(&monitors)?.validate()?;
            Ok(Self {
                platform: Platform::X11(backend),
                display,
                desktop,
                monitors,
                label: "Native X11: grabs + XTest",
                held: Held::default(),
                armed: false,
                repeat: None,
            })
        }
        #[cfg(windows)]
        {
            let monitors = windows::monitors()?;
            let display = selected_monitor(&monitors, selected)?;
            let desktop = monitor_bounds(&monitors)?.validate()?;
            let mut capture = InputCapture::new(Some(input_capture::Backend::Windows)).await?;
            capture.set_target([
                display.x as i32,
                display.y as i32,
                display.width as i32,
                display.height as i32,
            ]);
            Ok(Self {
                platform: Platform::Windows(capture),
                display,
                desktop,
                monitors,
                label: "Windows: low-level hooks + SendInput",
                held: Held::default(),
                armed: false,
                repeat: None,
            })
        }
        #[cfg(target_os = "macos")]
        {
            let monitors = macos::monitors()?;
            let display = selected_monitor(&monitors, selected)?;
            let desktop = monitor_bounds(&monitors)?.validate()?;
            let mut capture = InputCapture::new(Some(input_capture::Backend::MacOs)).await?;
            capture.set_target([
                display.x as i32,
                display.y as i32,
                display.width as i32,
                display.height as i32,
            ]);
            let injector = match macos::Mac::new(desktop) {
                Ok(value) => value,
                Err(error) => {
                    capture.terminate().await?;
                    return Err(error);
                }
            };
            Ok(Self {
                platform: Platform::MacOS { capture, injector },
                display,
                desktop,
                monitors,
                label: "macOS: Core Graphics event tap + posting",
                held: Held::default(),
                armed: false,
                repeat: None,
            })
        }
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            anyhow::bail!("This OS is unsupported")
        }
    }
    pub async fn arm(&mut self, side: Option<Side>) -> Result<()> {
        if let Some(side) = side {
            ensure!(
                self.display
                    .is_outer_edge(monitor_bounds(&self.monitors)?, side),
                "Selected handoff monitor is not on the outer {side:?} edge"
            );
        }
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.arm(side)?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => arm_native(capture, &mut self.armed, side).await?,
            #[cfg(target_os = "linux")]
            Platform::Portal { capture, .. } => arm_native(capture, &mut self.armed, side).await?,
            #[cfg(windows)]
            Platform::Windows(capture) => arm_native(capture, &mut self.armed, side).await?,
            #[cfg(target_os = "macos")]
            Platform::MacOS { capture, .. } => arm_native(capture, &mut self.armed, side).await?,
            #[cfg(test)]
            Platform::Test { trace, .. } => {
                trace.state.borrow_mut().armed = side;
                trace.changed.notify_one();
            }
        }
        Ok(())
    }
    pub async fn next(&mut self) -> Result<CaptureEvent> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.next().await,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => next_native(capture).await,
            #[cfg(target_os = "linux")]
            Platform::Portal { capture, .. } => next_native(capture).await,
            #[cfg(windows)]
            Platform::Windows(capture) => next_native(capture).await,
            #[cfg(target_os = "macos")]
            Platform::MacOS { capture, .. } => next_native(capture).await,
            #[cfg(test)]
            Platform::Test { events, .. } => events
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("Test capture closed")),
        }
    }
    pub async fn release_capture(&mut self) -> Result<()> {
        match &mut self.platform {
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.release()?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => capture.release().await?,
            #[cfg(target_os = "linux")]
            Platform::Portal { capture, .. } => capture.release().await?,
            #[cfg(windows)]
            Platform::Windows(capture) => capture.release().await?,
            #[cfg(target_os = "macos")]
            Platform::MacOS { capture, .. } => capture.release().await?,
            #[cfg(test)]
            Platform::Test { .. } => {}
        }
        Ok(())
    }
    pub fn maintenance(&mut self) -> Result<bool> {
        match &mut self.platform {
            #[cfg(test)]
            Platform::Test { .. } => Ok(false),
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.emergency_stop(),
            #[cfg(target_os = "linux")]
            Platform::Wayland { .. } => Ok(false),
            #[cfg(target_os = "linux")]
            Platform::Portal { injector, .. } => {
                injector.maintenance()?;
                if let Some((code, deadline)) = self.repeat
                    && std::time::Instant::now() >= deadline
                {
                    injector.inject(Input::Key { code, down: true })?;
                    self.repeat = Some((
                        code,
                        std::time::Instant::now() + std::time::Duration::from_millis(32),
                    ));
                }
                Ok(false)
            }
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
            #[cfg(target_os = "macos")]
            Platform::MacOS { injector, .. } => {
                if let Some((code, deadline)) = self.repeat
                    && std::time::Instant::now() >= deadline
                {
                    injector.inject(Input::Key { code, down: true })?;
                    self.repeat = Some((
                        code,
                        std::time::Instant::now() + std::time::Duration::from_millis(32),
                    ));
                }
                macos::Mac::maintenance()
            }
        }
    }
    pub fn check_geometry(&self) -> Result<()> {
        match &self.platform {
            #[cfg(test)]
            Platform::Test { .. } => {}
            #[cfg(target_os = "linux")]
            Platform::X11(x) => x.check_geometry()?,
            #[cfg(target_os = "linux")]
            Platform::Wayland { capture, .. } => {
                ensure!(
                    wayland_monitors(capture)? == self.monitors,
                    "Output configuration changed; reconnect sharing"
                );
            }
            #[cfg(target_os = "linux")]
            Platform::Portal { capture, .. } => ensure!(
                wayland_monitors(capture)? == self.monitors,
                "Portal zones changed; reconnect sharing"
            ),
            #[cfg(windows)]
            Platform::Windows(_) => ensure!(
                windows::monitors()? == self.monitors,
                "Display changed; reconnect sharing"
            ),
            #[cfg(target_os = "macos")]
            Platform::MacOS { .. } => ensure!(
                macos::monitors()? == self.monitors,
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
            if down && repeatable(code) {
                self.repeat = Some((
                    code,
                    std::time::Instant::now() + std::time::Duration::from_millis(500),
                ));
            } else if !down && self.repeat.is_some_and(|(key, _)| key == code) {
                self.repeat = None;
            }
        }
        #[cfg(target_os = "macos")]
        if let Input::Key { code, down } = event {
            if down && repeatable(code) {
                self.repeat = Some((
                    code,
                    std::time::Instant::now() + std::time::Duration::from_millis(500),
                ));
            } else if !down && self.repeat.is_some_and(|(key, _)| key == code) {
                self.repeat = None;
            }
        }
        #[cfg(target_os = "linux")]
        if matches!(self.platform, Platform::Portal { .. })
            && let Input::Key { code, down } = event
        {
            if down && repeatable(code) {
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
            Platform::X11(x) => x.inject(event, self.desktop),
            #[cfg(target_os = "linux")]
            Platform::Wayland { injector, .. } => injector.inject(event),
            #[cfg(target_os = "linux")]
            Platform::Portal { injector, .. } => injector.inject(event),
            #[cfg(windows)]
            Platform::Windows(_) => windows::inject(event),
            #[cfg(target_os = "macos")]
            Platform::MacOS { injector, .. } => injector.inject(event),
            #[cfg(test)]
            Platform::Test { trace, .. } => {
                let mut state = trace.state.borrow_mut();
                state.injected.push(event);
                let fail = std::mem::take(&mut state.fail_next);
                drop(state);
                trace.changed.notify_one();
                ensure!(!fail, "Test injection failure");
                Ok(())
            }
        }
    }
    pub fn release_input(&mut self) -> Result<()> {
        self.repeat = None;
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
            #[cfg(target_os = "linux")]
            Platform::Portal { capture, injector } => {
                let captured = capture.terminate().await;
                let injected = injector.close().await;
                captured?;
                injected?;
            }
            #[cfg(windows)]
            Platform::Windows(capture) => capture.terminate().await?,
            #[cfg(target_os = "macos")]
            Platform::MacOS { capture, .. } => capture.terminate().await?,
            #[cfg(test)]
            Platform::Test { trace, .. } => {
                trace.state.borrow_mut().closed = true;
                trace.changed.notify_one();
            }
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

fn repeatable(code: u32) -> bool {
    ![29, 42, 54, 56, 97, 100, 125, 126, 58, 69, 70].contains(&code)
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
    #[tokio::test]
    async fn internal_monitor_edge_is_not_armed_for_remote_handoff() {
        let (mut backend, _, _) = Backend::test();
        backend.monitors.push(Monitor {
            id: "right".into(),
            display: Display {
                x: 1000.,
                y: 0.,
                width: 1000.,
                height: 800.,
                scale: 1.,
            },
        });
        assert!(backend.arm(Some(Side::Right)).await.is_err());
        backend.arm(Some(Side::Left)).await.unwrap();
    }
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

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestState {
    pub injected: Vec<Input>,
    pub armed: Option<Side>,
    pub closed: bool,
    pub fail_next: bool,
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestTrace {
    pub state: std::cell::RefCell<TestState>,
    pub changed: tokio::sync::Notify,
}
#[cfg(test)]
impl TestTrace {
    pub async fn wait_for(&self, condition: impl Fn(&TestState) -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if condition(&self.state.borrow()) {
                    return;
                }
                changed.await;
            }
        })
        .await
        .expect("Session did not reach the expected input state");
    }
}
#[cfg(test)]
impl Backend {
    pub(crate) fn test() -> (
        Self,
        tokio::sync::mpsc::Sender<CaptureEvent>,
        std::rc::Rc<TestTrace>,
    ) {
        let (tx, events) = tokio::sync::mpsc::channel(128);
        let trace = std::rc::Rc::new(TestTrace::default());
        (
            Self {
                platform: Platform::Test {
                    events,
                    trace: trace.clone(),
                },
                display: Display {
                    x: 0.,
                    y: 0.,
                    width: 1000.,
                    height: 800.,
                    scale: 1.,
                },
                desktop: Display {
                    x: 0.,
                    y: 0.,
                    width: 1000.,
                    height: 800.,
                    scale: 1.,
                },
                monitors: vec![Monitor {
                    id: "test".into(),
                    display: Display {
                        x: 0.,
                        y: 0.,
                        width: 1000.,
                        height: 800.,
                        scale: 1.,
                    },
                }],
                label: "Test input",
                held: Held::default(),
                armed: false,
                repeat: None,
            },
            tx,
            trace,
        )
    }
}
