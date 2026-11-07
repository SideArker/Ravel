use crate::{
    geometry::{Display, Monitor},
    protocol::Input,
};
use anyhow::{Context, Result, ensure};
use core_graphics::{
    display::{CGDisplay, CGPoint},
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
};
use keycode::{KeyMap, KeyMapping};
use std::collections::BTreeSet;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
    fn CGEventSourceKeyState(state: CGEventSourceStateID, key: u16) -> bool;
}

pub fn monitors() -> Result<Vec<Monitor>> {
    let main = CGDisplay::main().id;
    let mut monitors = Vec::new();
    for id in CGDisplay::active_displays()
        .map_err(|e| anyhow::anyhow!("Cannot list macOS displays: {e}"))?
    {
        let bounds = CGDisplay::new(id).bounds();
        let monitor = Monitor {
            id: id.to_string(),
            display: Display {
                x: bounds.origin.x,
                y: bounds.origin.y,
                width: bounds.size.width,
                height: bounds.size.height,
                scale: 1.,
            }
            .validate()?,
        };
        if id == main {
            monitors.insert(0, monitor);
        } else {
            monitors.push(monitor);
        }
    }
    ensure!(!monitors.is_empty(), "No active macOS displays found");
    Ok(monitors)
}

pub struct Mac {
    source: CGEventSource,
    desktop: Display,
    point: CGPoint,
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
    caps_lock: bool,
}

impl Mac {
    pub fn new(desktop: Display) -> Result<Self> {
        ensure!(
            unsafe { AXIsProcessTrusted() },
            "macOS Accessibility permission denied; enable Ravel in Privacy & Security"
        );
        ensure!(
            unsafe { CGPreflightPostEventAccess() || CGRequestPostEventAccess() },
            "macOS Input Control permission denied; enable Ravel in Privacy & Security"
        );
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|_| anyhow::anyhow!("Cannot create macOS event source"))?;
        let point = CGEvent::new(source.clone())
            .map_err(|_| anyhow::anyhow!("Cannot read macOS cursor position"))?
            .location();
        Ok(Self {
            source,
            desktop,
            point,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            caps_lock: false,
        })
    }

    pub fn inject(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Motion { x, y } => {
                let [x, y] = self.desktop.denormalize([x, y]);
                self.point = CGPoint::new(x, y);
                let kind = if self.buttons.contains(&0x110) {
                    CGEventType::LeftMouseDragged
                } else if self.buttons.contains(&0x111) {
                    CGEventType::RightMouseDragged
                } else if !self.buttons.is_empty() {
                    CGEventType::OtherMouseDragged
                } else {
                    CGEventType::MouseMoved
                };
                self.mouse(kind, CGMouseButton::Left, 0)?
                    .post(CGEventTapLocation::HID);
            }
            Input::Key { code, down } => {
                let mac = mac_key(code).context("Scancode unsupported on macOS")?;
                if code == 58 && down {
                    self.caps_lock = !self.caps_lock;
                }
                if down {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
                let event =
                    CGEvent::new_keyboard_event(self.source.clone(), mac, down || modifier(code))
                        .map_err(|_| anyhow::anyhow!("Cannot create macOS key event"))?;
                if modifier(code) {
                    event.set_type(CGEventType::FlagsChanged);
                }
                let mut flags = if modifier(code) {
                    self.modifier_flags()
                } else {
                    self.flags()
                };
                if matches!(mac, 0x7b..=0x7e) {
                    flags |=
                        CGEventFlags::CGEventFlagNumericPad | CGEventFlags::CGEventFlagSecondaryFn;
                }
                event.set_flags(flags);
                event.post(CGEventTapLocation::HID);
            }
            Input::Button { code, down } => {
                let (kind, button, number) = match (code, down) {
                    (0x110, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0),
                    (0x110, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
                    (0x111, true) => (CGEventType::RightMouseDown, CGMouseButton::Right, 1),
                    (0x111, false) => (CGEventType::RightMouseUp, CGMouseButton::Right, 1),
                    (0x112..=0x114, true) => (
                        CGEventType::OtherMouseDown,
                        CGMouseButton::Center,
                        (code - 0x110) as i64,
                    ),
                    (0x112..=0x114, false) => (
                        CGEventType::OtherMouseUp,
                        CGMouseButton::Center,
                        (code - 0x110) as i64,
                    ),
                    _ => anyhow::bail!("Unsupported macOS mouse button"),
                };
                if down {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
                let event = self.mouse(kind, button, number)?;
                event.post(CGEventTapLocation::HID);
            }
            Input::Scroll { axis, value } => {
                let (unit, amount) = if value % 120. == 0. {
                    (ScrollEventUnit::LINE, (value / 120.) as i32)
                } else {
                    (ScrollEventUnit::PIXEL, (value / 8.).round() as i32)
                };
                let event = CGEvent::new_scroll_event(
                    self.source.clone(),
                    unit,
                    if axis == 0 { 1 } else { 2 },
                    if axis == 0 { -amount } else { 0 },
                    if axis == 1 { -amount } else { 0 },
                    0,
                )
                .map_err(|_| anyhow::anyhow!("Cannot create macOS scroll event"))?;
                event.post(CGEventTapLocation::HID);
            }
            Input::Modifiers { .. } => {}
        }
        Ok(())
    }

    fn mouse(&self, kind: CGEventType, button: CGMouseButton, number: i64) -> Result<CGEvent> {
        let event = CGEvent::new_mouse_event(self.source.clone(), kind, self.point, button)
            .map_err(|_| anyhow::anyhow!("Cannot create macOS mouse event"))?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, number);
        event.set_flags(self.flags());
        Ok(event)
    }

    fn flags(&self) -> CGEventFlags {
        let mut flags = CGEventFlags::empty();
        for code in &self.keys {
            flags |= match *code {
                42 | 54 => CGEventFlags::CGEventFlagShift,
                29 | 97 => CGEventFlags::CGEventFlagControl,
                56 | 100 => CGEventFlags::CGEventFlagAlternate,
                125 | 126 => CGEventFlags::CGEventFlagCommand,
                _ => CGEventFlags::empty(),
            };
        }
        if self.caps_lock {
            flags |= CGEventFlags::CGEventFlagAlphaShift;
        }
        flags
    }

    fn modifier_flags(&self) -> CGEventFlags {
        let mut bits = 0u64;
        for code in &self.keys {
            bits |= match *code {
                29 | 97 => 0x0000_0001,
                42 | 54 => 0x0000_0002,
                125 | 126 => 0x0000_0008,
                56 => 0x0000_0020,
                100 => 0x0000_0040,
                _ => 0,
            };
        }
        CGEventFlags::from_bits_retain(
            self.flags().bits() | bits | CGEventFlags::CGEventFlagNonCoalesced.bits(),
        )
    }

    pub fn emergency_stop() -> bool {
        let pressed =
            |code| unsafe { CGEventSourceKeyState(CGEventSourceStateID::HIDSystemState, code) };
        pressed(0x35) // Escape
            && (pressed(0x3b) || pressed(0x3e)) // Control
            && (pressed(0x3a) || pressed(0x3d)) // Option
            && (pressed(0x38) || pressed(0x3c)) // Shift
    }

    pub fn maintenance() -> Result<bool> {
        ensure!(
            unsafe { AXIsProcessTrusted() },
            "macOS Accessibility permission was revoked"
        );
        ensure!(
            unsafe { CGPreflightPostEventAccess() },
            "macOS Input Control permission was revoked"
        );
        Ok(Self::emergency_stop())
    }
}

fn modifier(code: u32) -> bool {
    matches!(code, 42 | 54 | 29 | 97 | 56 | 100 | 125 | 126 | 58)
}

fn mac_key(code: u32) -> Option<u16> {
    match code {
        99 => Some(0x69),  // Print Screen -> F13
        70 => Some(0x6b),  // Scroll Lock -> F14
        119 => Some(0x71), // Pause -> F15
        _ => KeyMap::from_key_mapping(KeyMapping::Evdev(code as u16))
            .ok()
            .map(|k| k.mac),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_virtual_codes_map_to_wire_scancodes() {
        for code in [
            1, 2, 3, 16, 17, 18, 29, 30, 31, 42, 54, 56, 57, 97, 100, 125, 126,
        ] {
            let mac = mac_key(code).expect("wire key maps to a macOS virtual key");
            let back = KeyMap::from_key_mapping(KeyMapping::Mac(mac))
                .unwrap()
                .evdev;
            assert_eq!(back as u32, code, "round trip for wire key {code}");
        }
        assert_eq!(mac_key(99), Some(0x69));
        assert_eq!(mac_key(70), Some(0x6b));
        assert_eq!(mac_key(119), Some(0x71));
    }
}
