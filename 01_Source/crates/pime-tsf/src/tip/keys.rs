//! The key path: the key events that TSF hands over, and what only Windows can tell about them.
//!
//! The key itself is not decided here: the controller (`crate::controller`) owns the state machine of the input, the
//! chords, the input mode and the vertical writing mode. This module only says what the host pressed: the virtual
//! key, the character it produces with the active layout, the modifier state and the time.

use std::time::Instant;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, GetKeyboardLayout, ToUnicodeEx};
use windows::Win32::UI::TextServices::ITfContext;

use crate::controller::KeyInput;
use crate::keymap::{Modifiers, vk};

use super::Tip;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyKind {
    TestDown,
    Down,
    TestUp,
    Up,
}

impl KeyKind {
    fn is_down(self) -> bool {
        matches!(self, KeyKind::TestDown | KeyKind::Down)
    }
}

fn pressed(code: u32) -> bool {
    // SAFETY: GetKeyState has no preconditions; the high bit of the result is the key's down state.
    unsafe { GetKeyState(code as i32) < 0 }
}

/// The character a key press produces with the active layout and the current modifiers, without disturbing the
/// layout's dead-key state (flag 4: Windows 10 1607 and later; earlier versions ignore it).
fn produced_char(code: u32, lparam: isize) -> Option<char> {
    let scan = ((lparam >> 16) & 0xFF) as u32;
    let mut state = [0u8; 256];
    for key in [
        vk::SHIFT,
        vk::CONTROL,
        vk::MENU,
        vk::CAPITAL,
        vk::LSHIFT,
        vk::RSHIFT,
        vk::LCONTROL,
        vk::RCONTROL,
        vk::LMENU,
        vk::RMENU,
    ] {
        // SAFETY: as in `pressed`; the low bit is the toggle state (Caps Lock).
        let raw = unsafe { GetKeyState(key as i32) };
        state[key as usize] = if raw < 0 { 0x80 } else { 0 } | u8::from(raw & 1 != 0);
    }
    let mut buffer = [0u16; 8];
    // SAFETY: the buffers are live for the call; GetKeyboardLayout(0) is the calling thread's layout.
    let written = unsafe { ToUnicodeEx(code, scan, &state, &mut buffer, 4, GetKeyboardLayout(0)) };
    // one UTF-16 unit and a real character: a dead key (-1) or a control code has no text
    match written {
        1 => char::from_u32(u32::from(buffer[0])).filter(|c| !c.is_control()),
        2 => char::decode_utf16(buffer[..2].iter().copied()).next()?.ok(),
        _ => None,
    }
}

fn key_input(wparam: WPARAM, lparam: LPARAM, started: Instant, down: bool) -> KeyInput {
    let code = wparam.0 as u32;
    KeyInput {
        code,
        lparam: lparam.0,
        ch: if down {
            produced_char(code, lparam.0)
        } else {
            None
        },
        mods: Modifiers {
            shift: pressed(vk::SHIFT),
            ctrl: pressed(vk::CONTROL),
            alt: pressed(vk::MENU),
            win: pressed(vk::LWIN) || pressed(vk::RWIN),
        },
        now_ms: started.elapsed().as_millis() as u64,
    }
}

impl Tip {
    /// Forward one key event to the controller; false (not eaten) whenever anything is missing.
    pub(super) fn key(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
        kind: KeyKind,
    ) -> bool {
        self.with(false, |inner| {
            inner.controller.host_mut().set_context(pic);
            let input = key_input(wparam, lparam, inner.started, kind.is_down());
            // The user is working in the application again: the panel of the settings has no business on the
            // screen any more (the user asked for that on 2026-11-11). The click somewhere else is not ours to
            // see, but the typing that follows it is.
            if kind == KeyKind::Down
                && input.code != 0
                && input.ch.is_some()
                && let Ok(mut laser) = inner.laser.try_borrow_mut()
            {
                laser.close_panel_unless_list();
            }
            let eaten = match kind {
                KeyKind::TestDown => inner.controller.on_test_key_down(input),
                KeyKind::Down => inner.controller.on_key_down(input),
                KeyKind::TestUp => inner.controller.on_test_key_up(input),
                KeyKind::Up => inner.controller.on_key_up(input),
            };
            eaten
        })
    }
}
