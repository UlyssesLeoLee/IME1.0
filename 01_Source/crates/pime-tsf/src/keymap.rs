//! Pure translation of Windows key messages into `pime_core::input::KeyEvent` (no COM, no Win32 calls: testable).
//!
//! OWNER: the "TIP" work item. Stub: the real mapping (virtual-key codes, modifier state, Shift-tap detection,
//! which keys must never be eaten) is implemented there.

use pime_core::input::{Key, KeyEvent};

/// Virtual-key codes used by the map (values from winuser.h; kept as plain numbers so this file has no Windows dependency).
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const ESCAPE: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const PRIOR: u32 = 0x21; // PageUp
    pub const NEXT: u32 = 0x22; // PageDown
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
}

/// Modifier state at the time of a key message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Translate a key-down into a state-machine event. `ch` is the character the key produces with the current layout and
/// modifiers (None for non-character keys). Returns None for keys the machine has no use for.
pub fn translate(_vk: u32, _ch: Option<char>, _mods: Modifiers) -> Option<KeyEvent> {
    let _ = Key::Other(0);
    None
}

/// Detects "Shift pressed and released with no other key in between".
#[derive(Debug, Default)]
pub struct ShiftTapDetector {
    armed: bool,
}

impl ShiftTapDetector {
    /// Feed every key-down (`down = true`) and key-up (`down = false`) in order; returns true exactly when a tap completed.
    pub fn feed(&mut self, _vk: u32, _down: bool) -> bool {
        let _ = self.armed;
        false
    }
}
