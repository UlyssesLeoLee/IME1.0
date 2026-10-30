//! Pure translation of Windows key messages into `pime_core::input::KeyEvent` (no COM, no Win32 calls: testable).
//!
//! The COM glue (`tip`) supplies the virtual-key code, the character the key produces with the current layout and the
//! modifier state; everything that decides what the key means is here. Two rules shape the map:
//! * the machine is the only judge of what is eaten: Ctrl/Alt chords keep their flags so it passes them through, and a
//!   key it has no use for is `None` (the host keeps it);
//! * Shift is never a key of its own: a lone Shift tap is detected separately ([`ShiftTapDetector`]).

use pime_core::input::{Key, KeyEvent};

/// Virtual-key codes used by the map (values from winuser.h; kept as plain numbers so this file has no Windows dependency).
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const CONTROL: u32 = 0x11;
    pub const MENU: u32 = 0x12; // Alt
    pub const CAPITAL: u32 = 0x14;
    pub const ESCAPE: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const PRIOR: u32 = 0x21; // PageUp
    pub const NEXT: u32 = 0x22; // PageDown
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const INSERT: u32 = 0x2D;
    pub const DELETE: u32 = 0x2E;
    pub const LWIN: u32 = 0x5B;
    pub const RWIN: u32 = 0x5C;
    pub const NUMLOCK: u32 = 0x90;
    pub const SCROLL: u32 = 0x91;
    pub const LSHIFT: u32 = 0xA0;
    pub const RSHIFT: u32 = 0xA1;
    pub const LCONTROL: u32 = 0xA2;
    pub const RCONTROL: u32 = 0xA3;
    pub const LMENU: u32 = 0xA4;
    pub const RMENU: u32 = 0xA5;
    /// Another input method's pending key (never ours to interpret).
    pub const PROCESSKEY: u32 = 0xE5;
    /// An injected Unicode character (SendInput); it has no key identity.
    pub const PACKET: u32 = 0xE7;
}

/// Modifier state at the time of a key message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Either Windows key. Win chords (Win+Space switches the input method) belong to Windows.
    pub win: bool,
}

pub fn is_shift_key(code: u32) -> bool {
    matches!(code, vk::SHIFT | vk::LSHIFT | vk::RSHIFT)
}

/// Keys that only change modifier or lock state and produce nothing by themselves.
fn is_modifier_key(code: u32) -> bool {
    is_shift_key(code)
        || matches!(
            code,
            vk::CONTROL
                | vk::LCONTROL
                | vk::RCONTROL
                | vk::MENU
                | vk::LMENU
                | vk::RMENU
                | vk::LWIN
                | vk::RWIN
                | vk::CAPITAL
                | vk::NUMLOCK
                | vk::SCROLL
        )
}

/// Translate a key-down into a state-machine event. `ch` is the character the key produces with the current layout and
/// modifiers (None for non-character keys and dead keys). Returns None for keys the machine has no use for.
pub fn translate(code: u32, ch: Option<char>, mods: Modifiers) -> Option<KeyEvent> {
    if mods.win || is_modifier_key(code) || matches!(code, vk::PROCESSKEY | vk::PACKET) {
        return None;
    }
    let key = match code {
        vk::BACK => Key::Backspace,
        vk::TAB => Key::Tab,
        vk::RETURN => Key::Enter,
        vk::ESCAPE => Key::Esc,
        vk::PRIOR => Key::PageUp,
        vk::NEXT => Key::PageDown,
        vk::LEFT => Key::Left,
        vk::UP => Key::Up,
        vk::RIGHT => Key::Right,
        vk::DOWN => Key::Down,
        vk::SPACE => Key::Char(' '),
        _ => match ch.filter(|c| !c.is_control()) {
            Some(c) => Key::Char(c),
            // Ctrl+letter produces a control character, not the letter: the chord still needs a key identity
            None => match code {
                0x30..=0x39 | 0x41..=0x5A => Key::Char(char::from(code as u8).to_ascii_lowercase()),
                _ => return None,
            },
        },
    };
    Some(KeyEvent {
        key,
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
    })
}

/// True for a key that moves the caret or edits the text behind the machine's back, so that what it observed (the typed
/// prefix, the candidates) no longer describes the document: navigation keys, Delete, Tab and every Ctrl/Alt/Win chord
/// (paste, undo, AltGr characters as well: losing a prefix is cheap, offering a stale candidate is not).
/// The caller applies it only to keys the machine passed through.
pub fn breaks_context(code: u32, mods: Modifiers) -> bool {
    if is_modifier_key(code) {
        return false;
    }
    mods.ctrl
        || mods.alt
        || mods.win
        || matches!(
            code,
            vk::LEFT
                | vk::RIGHT
                | vk::UP
                | vk::DOWN
                | vk::HOME
                | vk::END
                | vk::PRIOR
                | vk::NEXT
                | vk::DELETE
                | vk::TAB
        )
}

/// Detects "Shift pressed and released with no other key in between".
#[derive(Debug, Default)]
pub struct ShiftTapDetector {
    armed: bool,
}

impl ShiftTapDetector {
    /// Feed every key-down (`down = true`) and key-up (`down = false`) in order, with the modifier state of that event;
    /// returns true exactly when a tap completed. Windows may report one physical event twice (`OnTestKeyUp` and
    /// `OnKeyUp`), so a completed tap disarms and the repeat reports nothing.
    pub fn feed(&mut self, code: u32, down: bool, mods: Modifiers) -> bool {
        let chord = mods.ctrl || mods.alt || mods.win; // Ctrl+Shift and Alt+Shift switch layouts: not ours
        if !is_shift_key(code) {
            self.armed = false;
            return false;
        }
        if down {
            self.armed = !chord;
            return false;
        }
        std::mem::take(&mut self.armed) && !chord
    }

    /// Forget a half-seen gesture (focus moved to another window).
    pub fn reset(&mut self) {
        self.armed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::input::{InputMachine, KeyOutcome, LocalPredictor, State};

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };

    #[test]
    fn keys_that_produce_characters_become_char_events() {
        assert_eq!(translate(0x41, Some('a'), NONE), Some(KeyEvent::ch('a')));
        assert_eq!(
            translate(0x41, Some('A'), SHIFT),
            Some(KeyEvent::shifted(Key::Char('A')))
        );
        assert_eq!(translate(0x35, Some('5'), NONE), Some(KeyEvent::ch('5')));
        assert_eq!(
            translate(0x35, Some('%'), SHIFT),
            Some(KeyEvent::shifted(Key::Char('%')))
        );
        assert_eq!(translate(0xBC, Some(','), NONE), Some(KeyEvent::ch(',')));
        assert_eq!(
            translate(0x61, Some('1'), NONE),
            Some(KeyEvent::ch('1')),
            "numpad digits pick candidates like the digit row"
        );
        assert_eq!(
            translate(vk::SPACE, Some(' '), NONE),
            Some(KeyEvent::ch(' '))
        );
        assert_eq!(
            translate(vk::SPACE, None, NONE),
            Some(KeyEvent::ch(' ')),
            "Space is a character even when the layout call failed"
        );
        assert_eq!(translate(0x41, Some('あ'), NONE), Some(KeyEvent::ch('あ')));
    }

    #[test]
    fn named_keys_map_one_to_one() {
        for (code, key) in [
            (vk::BACK, Key::Backspace),
            (vk::TAB, Key::Tab),
            (vk::RETURN, Key::Enter),
            (vk::ESCAPE, Key::Esc),
            (vk::PRIOR, Key::PageUp),
            (vk::NEXT, Key::PageDown),
            (vk::LEFT, Key::Left),
            (vk::UP, Key::Up),
            (vk::RIGHT, Key::Right),
            (vk::DOWN, Key::Down),
        ] {
            // the layout reports a control character for some of them: it must not leak into the key
            assert_eq!(
                translate(code, Some('\u{9}'), NONE),
                Some(KeyEvent::plain(key)),
                "{code:#x}"
            );
            assert_eq!(translate(code, None, NONE), Some(KeyEvent::plain(key)));
        }
    }

    #[test]
    fn shift_tab_is_the_shifted_tab_event() {
        assert_eq!(
            translate(vk::TAB, None, SHIFT),
            Some(KeyEvent::shifted(Key::Tab))
        );
    }

    #[test]
    fn chords_keep_their_flags_so_the_machine_passes_them_through() {
        let ctrl_c = translate(0x43, Some('\u{3}'), CTRL).unwrap();
        assert_eq!(
            ctrl_c,
            KeyEvent {
                key: Key::Char('c'),
                ctrl: true,
                alt: false,
                shift: false
            },
            "Ctrl+C: identity from the virtual key, flag kept"
        );
        let alt_f4 = translate(0x73, None, ALT);
        assert_eq!(alt_f4, None, "F4 has no use for the machine");
        let both = translate(
            vk::TAB,
            None,
            Modifiers {
                ctrl: true,
                shift: true,
                ..NONE
            },
        )
        .unwrap();
        assert!(both.ctrl && both.shift && both.key == Key::Tab);
        assert_eq!(
            translate(0x31, Some('1'), ALT).map(|e| (e.key, e.alt)),
            Some((Key::Char('1'), true))
        );
    }

    #[test]
    fn a_translated_chord_is_never_consumed_even_with_candidates_and_capture() {
        let mut m = InputMachine::new(LocalPredictor, "rust");
        m.grant_observation(true);
        m.set_candidate_capture(true);
        for c in "fn".chars() {
            m.on_key(KeyEvent::ch(c), 0);
        }
        assert_eq!(m.state(), State::Candidates);
        for (code, ch, mods) in [
            (vk::TAB, None, CTRL),
            (vk::RETURN, None, ALT),
            (vk::NEXT, None, CTRL),
            (0x31, Some('1'), CTRL),
            (vk::ESCAPE, None, ALT),
        ] {
            let ev = translate(code, ch, mods).unwrap();
            assert!(!m.would_consume(ev), "{code:#x}");
            assert_eq!(m.on_key(ev, 1).outcome, KeyOutcome::PassThrough);
        }
        assert_eq!(m.state(), State::Candidates, "nothing was disturbed");
    }

    #[test]
    fn keys_the_machine_has_no_use_for_are_none() {
        for code in [
            vk::SHIFT,
            vk::LSHIFT,
            vk::RSHIFT,
            vk::CONTROL,
            vk::LCONTROL,
            vk::RCONTROL,
            vk::MENU,
            vk::LMENU,
            vk::RMENU,
            vk::LWIN,
            vk::RWIN,
            vk::CAPITAL,
            vk::NUMLOCK,
            vk::SCROLL,
            vk::PROCESSKEY,
            vk::PACKET,
        ] {
            assert_eq!(translate(code, None, NONE), None, "{code:#x}");
            assert_eq!(translate(code, Some('x'), NONE), None, "{code:#x}");
        }
        for code in [0x70, 0x7B, vk::HOME, vk::END, vk::INSERT, vk::DELETE, 0xDF] {
            assert_eq!(translate(code, None, NONE), None, "{code:#x}");
        }
        assert_eq!(
            translate(0x41, None, NONE),
            Some(KeyEvent::ch('a')),
            "a letter whose character could not be computed still has an identity"
        );
        assert_eq!(
            translate(0xBA, None, NONE),
            None,
            "dead key: no character, no identity"
        );
    }

    #[test]
    fn windows_key_chords_belong_to_windows() {
        let win = Modifiers { win: true, ..NONE };
        for code in [vk::SPACE, 0x41, vk::TAB, vk::RETURN] {
            assert_eq!(translate(code, Some('a'), win), None, "{code:#x}");
        }
    }

    #[test]
    fn context_breakers_are_navigation_editing_tab_and_chords() {
        for code in [
            vk::LEFT,
            vk::RIGHT,
            vk::UP,
            vk::DOWN,
            vk::HOME,
            vk::END,
            vk::PRIOR,
            vk::NEXT,
            vk::DELETE,
            vk::TAB,
        ] {
            assert!(breaks_context(code, NONE), "{code:#x}");
        }
        assert!(breaks_context(0x56, CTRL), "Ctrl+V");
        assert!(breaks_context(0x5A, CTRL), "Ctrl+Z");
        assert!(breaks_context(0x51, ALT));
        assert!(breaks_context(0x41, Modifiers { win: true, ..NONE }));
        for code in [
            0x41,
            0x35,
            vk::SPACE,
            vk::RETURN,
            vk::BACK,
            vk::ESCAPE,
            vk::SHIFT,
            vk::CONTROL,
            vk::MENU,
            vk::CAPITAL,
        ] {
            assert!(!breaks_context(code, NONE), "{code:#x}");
        }
        assert!(
            !breaks_context(vk::CONTROL, CTRL),
            "pressing Ctrl itself breaks nothing"
        );
        assert!(!breaks_context(0x41, SHIFT), "a capital letter is text");
    }

    fn tap(d: &mut ShiftTapDetector, events: &[(u32, bool)]) -> usize {
        events
            .iter()
            .filter(|&&(code, down)| d.feed(code, down, NONE))
            .count()
    }

    #[test]
    fn shift_down_then_up_is_exactly_one_tap() {
        let mut d = ShiftTapDetector::default();
        assert!(
            !d.feed(vk::SHIFT, true, NONE),
            "the tap completes on release"
        );
        assert!(d.feed(vk::SHIFT, false, NONE));
        assert!(
            !d.feed(vk::SHIFT, false, NONE),
            "a repeated release (Test + real) is not a second tap"
        );
        assert_eq!(
            tap(&mut d, &[(vk::SHIFT, true), (vk::SHIFT, false)]),
            1,
            "and the next tap works again"
        );
    }

    #[test]
    fn any_other_key_in_between_disarms() {
        let mut d = ShiftTapDetector::default();
        assert_eq!(
            tap(
                &mut d,
                &[
                    (vk::SHIFT, true),
                    (0x41, true),
                    (0x41, false),
                    (vk::SHIFT, false)
                ]
            ),
            0,
            "Shift+A is a capital letter"
        );
        assert_eq!(
            tap(
                &mut d,
                &[(vk::SHIFT, true), (vk::TAB, true), (vk::SHIFT, false)]
            ),
            0,
            "Shift+Tab walks the candidates backwards"
        );
        assert_eq!(
            tap(
                &mut d,
                &[
                    (0x41, true),
                    (vk::SHIFT, true),
                    (0x41, false),
                    (vk::SHIFT, false)
                ]
            ),
            0,
            "releasing an earlier key during the Shift press is not a tap"
        );
        assert_eq!(tap(&mut d, &[(vk::SHIFT, false)]), 0, "a stray release");
        assert_eq!(
            tap(
                &mut d,
                &[
                    (0x41, true),
                    (0x41, false),
                    (vk::SHIFT, true),
                    (vk::SHIFT, false)
                ]
            ),
            1,
            "a key before the press is irrelevant"
        );
    }

    #[test]
    fn auto_repeat_and_both_shift_keys_still_make_one_tap() {
        let mut d = ShiftTapDetector::default();
        assert_eq!(
            tap(
                &mut d,
                &[
                    (vk::SHIFT, true),
                    (vk::SHIFT, true),
                    (vk::SHIFT, true),
                    (vk::SHIFT, false)
                ]
            ),
            1
        );
        assert_eq!(
            tap(&mut d, &[(vk::LSHIFT, true), (vk::LSHIFT, false)]),
            1,
            "left Shift"
        );
        assert_eq!(
            tap(&mut d, &[(vk::RSHIFT, true), (vk::RSHIFT, false)]),
            1,
            "right Shift"
        );
    }

    #[test]
    fn shift_with_ctrl_alt_or_win_is_a_windows_shortcut_not_a_tap() {
        for held in [CTRL, ALT, Modifiers { win: true, ..NONE }] {
            let mut d = ShiftTapDetector::default();
            assert!(!d.feed(vk::SHIFT, true, held));
            assert!(
                !d.feed(vk::SHIFT, false, held),
                "{held:?} at press and release"
            );
            // chord released before Shift: Ctrl down, Shift down, Ctrl up, Shift up (the layout-switch gesture)
            let mut d = ShiftTapDetector::default();
            assert!(!d.feed(vk::SHIFT, true, held));
            assert!(!d.feed(vk::CONTROL, false, NONE));
            assert!(
                !d.feed(vk::SHIFT, false, NONE),
                "{held:?} held at the press"
            );
        }
    }

    #[test]
    fn a_half_seen_gesture_is_forgotten_on_reset() {
        let mut d = ShiftTapDetector::default();
        d.feed(vk::SHIFT, true, NONE);
        d.reset();
        assert!(
            !d.feed(vk::SHIFT, false, NONE),
            "the Shift press belonged to another window"
        );
    }
}
