use crate::protocol::Input;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::FileExt,
    },
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_keyboard::{self, WlKeyboard},
        wl_registry,
        wl_seat::WlSeat,
    },
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 as KeyboardManager,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1 as Keyboard,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1 as PointerManager,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1 as Pointer,
};
use xkbcommon::xkb;

struct State {
    keymap: Option<(OwnedFd, u32, String)>,
}
pub struct Wayland {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    pointer: Pointer,
    keyboard: Keyboard,
    xkb: xkb::State,
}
impl Wayland {
    pub fn new() -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let seat: WlSeat = globals
            .bind(&qh, 7..=8, ())
            .context("Wayland seat protocol missing")?;
        let pm: PointerManager = globals
            .bind(&qh, 1..=2, ())
            .context("wlr virtual-pointer protocol missing; this compositor is unsupported")?;
        let km: KeyboardManager = globals
            .bind(&qh, 1..=1, ())
            .context("virtual-keyboard protocol missing; this compositor is unsupported")?;
        let physical = seat.get_keyboard(&qh, ());
        let mut state = State { keymap: None };
        queue.roundtrip(&mut state)?;
        let (fd, size, keymap) = state
            .keymap
            .take()
            .context("Compositor did not provide an XKB keymap")?;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let map = xkb::Keymap::new_from_string(
            &context,
            keymap,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .context("Invalid compositor XKB keymap")?;
        let xkb = xkb::State::new(&map);
        let pointer = pm.create_virtual_pointer(Some(&seat), &qh, ());
        let keyboard = km.create_virtual_keyboard(&seat, &qh, ());
        keyboard.keymap(1, fd.as_fd(), size);
        physical.release();
        conn.flush()?;
        Ok(Self {
            conn,
            queue,
            state,
            pointer,
            keyboard,
            xkb,
        })
    }
    pub fn inject(&mut self, input: Input) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        match input {
            Input::Motion { x, y } => {
                self.pointer.motion_absolute(
                    0,
                    (x * 1_000_000.) as u32,
                    (y * 1_000_000.) as u32,
                    1_000_000,
                    1_000_000,
                );
                self.pointer.frame();
            }
            Input::Button { code, down } => {
                self.pointer.button(
                    0,
                    code,
                    if down {
                        wl_pointer_state(true)
                    } else {
                        wl_pointer_state(false)
                    },
                );
                self.pointer.frame();
            }
            Input::Scroll { axis, value } => {
                use wayland_client::protocol::wl_pointer::{Axis, AxisSource};
                self.pointer.axis_source(AxisSource::Wheel);
                self.pointer.axis(
                    0,
                    if axis == 0 {
                        Axis::VerticalScroll
                    } else {
                        Axis::HorizontalScroll
                    },
                    value / 120. * 15.,
                );
                self.pointer.frame();
            }
            Input::Key { code, down } => {
                self.xkb.update_key(
                    xkb::Keycode::new(code + 8),
                    if down {
                        xkb::KeyDirection::Down
                    } else {
                        xkb::KeyDirection::Up
                    },
                );
                self.keyboard.key(0, code, u32::from(down));
                self.modifiers();
            }
            Input::Modifiers { .. } => {
                // Derive modifiers from physical key transitions in the receiver's
                // keymap. Source keymap modifier bit positions are not portable.
                self.modifiers();
            }
        }
        self.conn.flush()?;
        Ok(())
    }
    fn modifiers(&self) {
        self.keyboard.modifiers(
            self.xkb.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            self.xkb.serialize_mods(xkb::STATE_MODS_LATCHED),
            self.xkb.serialize_mods(xkb::STATE_MODS_LOCKED),
            self.xkb.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        );
    }
}
fn wl_pointer_state(down: bool) -> wayland_client::protocol::wl_pointer::ButtonState {
    if down {
        wayland_client::protocol::wl_pointer::ButtonState::Pressed
    } else {
        wayland_client::protocol::wl_pointer::ButtonState::Released
    }
}
impl Drop for Wayland {
    fn drop(&mut self) {
        self.keyboard.destroy();
        self.pointer.destroy();
        let _ = self.conn.flush();
    }
}
impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap {
            format: wayland_client::WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = event
            && let Ok(text) = read_keymap(&fd, size)
        {
            state.keymap = Some((fd, size, text));
        }
    }
}
delegate_noop!(State: ignore wl_registry::WlRegistry);
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore PointerManager);
delegate_noop!(State: ignore KeyboardManager);
delegate_noop!(State: ignore Pointer);
delegate_noop!(State: ignore Keyboard);
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn read_keymap(fd: &OwnedFd, size: u32) -> Result<String> {
    ensure!((1..=1_048_576).contains(&size), "Invalid keymap size");
    let mut data = vec![0; size as usize];
    // SCM_RIGHTS and dup share the file offset; never consume the compositor's fd.
    File::from(fd.try_clone()?).read_exact_at(&mut data, 0)?;
    if data.last() == Some(&0) {
        data.pop();
    }
    Ok(String::from_utf8(data)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};
    #[test]
    fn keymap_read_does_not_consume_or_depend_on_shared_fd_offset() {
        let mut file = tempfile::tempfile().unwrap();
        let text = b"xkb_keymap { regression fixture };\0";
        file.write_all(text).unwrap();
        let position = file.stream_position().unwrap();
        let fd: OwnedFd = file.try_clone().unwrap().into();
        for _ in 0..2 {
            assert_eq!(
                read_keymap(&fd, text.len() as u32).unwrap().as_bytes(),
                &text[..text.len() - 1]
            );
        }
        assert_eq!(file.stream_position().unwrap(), position);
        assert!(read_keymap(&fd, 1_048_577).is_err());
    }
}
