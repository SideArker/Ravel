use crate::{geometry::Display, protocol::Input};
use anyhow::{Result, ensure};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::*,
    WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_CXSCREEN, SM_CYSCREEN},
};

pub fn display() -> Result<Display> {
    unsafe {
        ensure!(
            GetSystemMetrics(SM_CMONITORS) == 1,
            "Exactly one active Windows monitor is required"
        );
        Display {
            x: 0.,
            y: 0.,
            width: GetSystemMetrics(SM_CXSCREEN) as f64,
            height: GetSystemMetrics(SM_CYSCREEN) as f64,
            scale: 1.,
        }
        .validate()
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
            (x * 65535.) as i32,
            (y * 65535.) as i32,
            0,
            MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE,
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
