use super::CaptureEvent;
use crate::{
    geometry::{Display, Monitor, Side, selected_monitor},
    protocol::Input,
};
use anyhow::{Context, Result, ensure};
use input_event::{Event as InputEvent, KeyboardEvent, PointerEvent};
use std::collections::VecDeque;
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        randr::ConnectionExt as _,
        xproto::{self, ConnectionExt as _, EventMask, GrabMode, GrabStatus},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

pub struct X11 {
    conn: RustConnection,
    root: u32,
    display: Display,
    monitors: Vec<Monitor>,
    side: Option<Side>,
    active: bool,
    entry: [f64; 2],
    pending: VecDeque<CaptureEvent>,
    scroll: [f64; 2],
}
impl X11 {
    pub fn new(selected: Option<&str>) -> Result<Self> {
        ensure!(
            std::env::var_os("WAYLAND_DISPLAY").is_none(),
            "Xwayland is not native X11; use the Wayland backend"
        );
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        conn.xtest_get_version(2, 2)?
            .reply()
            .context("XTest extension required")?;
        let monitors = Self::query_monitors(&conn, root)?;
        let display = selected_monitor(&monitors, selected)?;
        Ok(Self {
            conn,
            root,
            display,
            monitors,
            side: None,
            active: false,
            entry: [0.; 2],
            pending: VecDeque::new(),
            scroll: [0.; 2],
        })
    }
    fn query_monitors(conn: &RustConnection, root: u32) -> Result<Vec<Monitor>> {
        let info = conn
            .randr_get_monitors(root, true)?
            .reply()
            .context("RandR 1.5 required")?
            .monitors;
        let mut monitors = Vec::new();
        for (index, m) in info.into_iter().enumerate() {
            let name = if m.name == 0 {
                format!("monitor-{index}")
            } else {
                String::from_utf8_lossy(&conn.get_atom_name(m.name)?.reply()?.name).into_owned()
            };
            let monitor = Monitor {
                id: name,
                display: Display {
                    x: m.x as f64,
                    y: m.y as f64,
                    width: m.width as f64,
                    height: m.height as f64,
                    scale: 1.,
                }
                .validate()?,
            };
            if m.primary {
                monitors.insert(0, monitor);
            } else {
                monitors.push(monitor);
            }
        }
        ensure!(!monitors.is_empty(), "No active X11 monitors found");
        Ok(monitors)
    }
    pub fn display(&self) -> Display {
        self.display
    }
    pub fn monitors(&self) -> Vec<Monitor> {
        self.monitors.clone()
    }
    pub fn emergency_stop(&self) -> Result<bool> {
        let keys = self.conn.query_keymap()?.reply()?.keys;
        let pressed = |code: u8| {
            let key = code + 8;
            keys[key as usize / 8] & (1 << (key % 8)) != 0
        };
        Ok(pressed(1)
            && (pressed(29) || pressed(97))
            && (pressed(56) || pressed(100))
            && (pressed(42) || pressed(54)))
    }
    pub fn check_geometry(&self) -> Result<()> {
        ensure!(
            Self::query_monitors(&self.conn, self.root)? == self.monitors,
            "Display changed; reconnect sharing"
        );
        Ok(())
    }
    pub fn arm(&mut self, side: Option<Side>) -> Result<()> {
        self.release()?;
        self.side = side;
        Ok(())
    }
    pub fn release(&mut self) -> Result<()> {
        if self.active {
            self.conn.ungrab_keyboard(x11rb::CURRENT_TIME)?.check()?;
            self.conn.ungrab_pointer(x11rb::CURRENT_TIME)?.check()?;
            self.active = false;
            self.conn.flush()?;
        }
        self.pending.clear();
        Ok(())
    }
    pub async fn next(&mut self) -> Result<CaptureEvent> {
        loop {
            if let Some(event) = self.poll()? {
                return Ok(event);
            }
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        }
    }
    fn poll(&mut self) -> Result<Option<CaptureEvent>> {
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        if !self.active {
            // Drain old synthetic events before considering a fresh physical edge.
            while self.conn.poll_for_event()?.is_some() {}
            let Some(side) = self.side else {
                return Ok(None);
            };
            let p = self.conn.query_pointer(self.root)?.reply()?;
            let point = [p.root_x as f64, p.root_y as f64];
            let at = self.display.on_edge(point, side);
            if !at {
                return Ok(None);
            }
            let mask =
                EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
            ensure!(
                self.conn
                    .grab_pointer(
                        false,
                        self.root,
                        mask,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                        self.root,
                        x11rb::NONE,
                        x11rb::CURRENT_TIME
                    )?
                    .reply()?
                    .status
                    == GrabStatus::SUCCESS,
                "X11 pointer grab refused"
            );
            if self
                .conn
                .grab_keyboard(
                    false,
                    self.root,
                    x11rb::CURRENT_TIME,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .reply()?
                .status
                != GrabStatus::SUCCESS
            {
                self.conn.ungrab_pointer(x11rb::CURRENT_TIME)?.check()?;
                anyhow::bail!("X11 keyboard grab refused");
            }
            self.active = true;
            self.entry = point;
            self.center()?;
            return Ok(Some(CaptureEvent::Begin { point }));
        }
        while let Some(event) = self.conn.poll_for_event()? {
            let event = match event {
                Event::KeyPress(e) => Some(InputEvent::Keyboard(KeyboardEvent::Key {
                    time: e.time,
                    key: u32::from(e.detail.saturating_sub(8)),
                    state: 1,
                })),
                Event::KeyRelease(e) => Some(InputEvent::Keyboard(KeyboardEvent::Key {
                    time: e.time,
                    key: u32::from(e.detail.saturating_sub(8)),
                    state: 0,
                })),
                Event::ButtonPress(e) | Event::ButtonRelease(e) => {
                    let down = e.response_type & 0x7f == xproto::BUTTON_PRESS_EVENT;
                    match e.detail {
                        4..=7 if down => Some(InputEvent::Pointer(PointerEvent::AxisDiscrete120 {
                            axis: u8::from(e.detail >= 6),
                            value: if e.detail == 4 || e.detail == 6 {
                                -120
                            } else {
                                120
                            },
                        })),
                        4..=7 => None,
                        detail => button_code(detail).map(|button| {
                            InputEvent::Pointer(PointerEvent::Button {
                                time: e.time,
                                button,
                                state: u32::from(down),
                            })
                        }),
                    }
                }
                Event::MotionNotify(e) => {
                    let center = self.center_point();
                    let dx = e.root_x as f64 - center[0];
                    let dy = e.root_y as f64 - center[1];
                    if dx == 0. && dy == 0. {
                        None
                    } else {
                        self.center()?;
                        Some(InputEvent::Pointer(PointerEvent::Motion {
                            time: e.time,
                            dx,
                            dy,
                        }))
                    }
                }
                _ => None,
            };
            if let Some(event) = event {
                return Ok(Some(CaptureEvent::Input(event)));
            }
        }
        Ok(None)
    }
    fn center_point(&self) -> [f64; 2] {
        [
            self.display.x + (self.display.width / 2.).floor(),
            self.display.y + (self.display.height / 2.).floor(),
        ]
    }
    fn center(&self) -> Result<()> {
        let p = self.center_point();
        self.warp(p)
    }
    fn warp(&self, p: [f64; 2]) -> Result<()> {
        self.conn
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, p[0] as i16, p[1] as i16)?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }
    pub fn inject(&mut self, input: Input, desktop: Display) -> Result<()> {
        match input {
            Input::Motion { x, y } => self.warp(desktop.denormalize([x, y]))?,
            Input::Key { code, down } => {
                ensure!(code <= 247, "Scancode outside X11 range");
                self.fake(
                    if down {
                        xproto::KEY_PRESS_EVENT
                    } else {
                        xproto::KEY_RELEASE_EVENT
                    },
                    (code + 8) as u8,
                )?;
            }
            Input::Button { code, down } => {
                let button = match code {
                    0x110 => 1,
                    0x111 => 3,
                    0x112 => 2,
                    0x113 => 8,
                    0x114 => 9,
                    _ => anyhow::bail!("Unsupported mouse button"),
                };
                self.fake(
                    if down {
                        xproto::BUTTON_PRESS_EVENT
                    } else {
                        xproto::BUTTON_RELEASE_EVENT
                    },
                    button,
                )?;
            }
            Input::Scroll { axis, value } => {
                self.scroll[axis as usize] += value;
                while self.scroll[axis as usize].abs() >= 120. {
                    let negative = self.scroll[axis as usize] < 0.;
                    let button = match (axis, negative) {
                        (0, true) => 4,
                        (0, false) => 5,
                        (_, true) => 6,
                        (_, false) => 7,
                    };
                    self.fake(xproto::BUTTON_PRESS_EVENT, button)?;
                    self.fake(xproto::BUTTON_RELEASE_EVENT, button)?;
                    self.scroll[axis as usize] -= if negative { -120. } else { 120. };
                }
            }
            Input::Modifiers { .. } => {}
        }
        self.conn.flush()?;
        Ok(())
    }
    fn fake(&self, event: u8, detail: u8) -> Result<()> {
        self.conn
            .xtest_fake_input(event, detail, 0, self.root, 0, 0, 0)?
            .check()?;
        Ok(())
    }
}
fn button_code(detail: u8) -> Option<u32> {
    match detail {
        1 => Some(0x110),
        3 => Some(0x111),
        2 => Some(0x112),
        8 => Some(0x113),
        9 => Some(0x114),
        _ => None,
    }
}
impl Drop for X11 {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
