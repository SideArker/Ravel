use crate::{
    geometry::{Display, Monitor},
    protocol::Input,
};
use anyhow::{Result, ensure};
use windows::{
    Win32::{
        Foundation::FALSE,
        Graphics::Gdi::{
            DEVMODEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE,
            DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplaySettingsW,
        },
        UI::Input::KeyboardAndMouse::*,
    },
    core::PCWSTR,
};

pub fn monitors() -> Result<Vec<Monitor>> {
    unsafe {
        let mut result = Vec::new();
        for index in 0.. {
            let mut device = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if EnumDisplayDevicesW(None, index, &mut device, Default::default()) == FALSE {
                break;
            }
            if !device
                .StateFlags
                .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
            {
                continue;
            }
            let mut mode = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            ensure!(
                EnumDisplaySettingsW(
                    PCWSTR::from_raw(device.DeviceName.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut mode
                ) != FALSE,
                "Cannot read active Windows monitor geometry"
            );
            let pos = mode.Anonymous1.Anonymous2.dmPosition;
            let name_len = device
                .DeviceName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(device.DeviceName.len());
            let monitor = Monitor {
                id: String::from_utf16_lossy(&device.DeviceName[..name_len]),
                display: Display {
                    x: pos.x as f64,
                    y: pos.y as f64,
                    width: mode.dmPelsWidth as f64,
                    height: mode.dmPelsHeight as f64,
                    scale: 1.,
                }
                .validate()?,
            };
            if device.StateFlags.contains(DISPLAY_DEVICE_PRIMARY_DEVICE) {
                result.insert(0, monitor);
            } else {
                result.push(monitor);
            }
        }
        ensure!(!result.is_empty(), "No active Windows monitors found");
        Ok(result)
    }
}
pub fn inject(event: Input) -> Result<()> {
    let input = match event {
        Input::Key { code, down } => {
            let linux = input_event::scancode::Linux::try_from(code)?;
            let scan = input_event::scancode::Windows::try_from(linux)
                .map_err(|_| anyhow::anyhow!("Scancode unsupported on Windows"))?
                as u32;
            let flags = KEYEVENTF_SCANCODE
                | if down {
                    KEYBD_EVENT_FLAGS(0)
                } else {
                    KEYEVENTF_KEYUP
                }
                | if scan & 0xe000 != 0 {
                    KEYEVENTF_EXTENDEDKEY
                } else {
                    KEYBD_EVENT_FLAGS(0)
                };
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: (scan & 0xff) as u16,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            }
        }
        Input::Motion { x, y } => mouse(
            (x * 65535.).round() as i32,
            (y * 65535.).round() as i32,
            0,
            MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
        ),
        Input::Button { code, down } => {
            let (flag, data) = match (code, down) {
                (0x110, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (0x110, false) => (MOUSEEVENTF_LEFTUP, 0),
                (0x111, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (0x111, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (0x112, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (0x112, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (0x113, true) => (MOUSEEVENTF_XDOWN, 1),
                (0x113, false) => (MOUSEEVENTF_XUP, 1),
                (0x114, true) => (MOUSEEVENTF_XDOWN, 2),
                (0x114, false) => (MOUSEEVENTF_XUP, 2),
                _ => anyhow::bail!("Unsupported mouse button"),
            };
            mouse(0, 0, data, flag)
        }
        Input::Scroll { axis, value } => mouse(
            0,
            0,
            (if axis == 0 { -value } else { value }) as i32 as u32,
            if axis == 0 {
                MOUSEEVENTF_WHEEL
            } else {
                MOUSEEVENTF_HWHEEL
            },
        ),
        Input::Modifiers { .. } => return Ok(()),
    };
    ensure!(
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) } == 1,
        "Windows refused SendInput (elevated or secure desktop?)"
    );
    Ok(())
}
fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

pub fn emergency_stop() -> bool {
    unsafe {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_ESCAPE]
            .iter()
            .all(|key| GetAsyncKeyState(i32::from(key.0)) < 0)
    }
}
