//! The regression test of the **real** input method: the installed DLL, the real `msctf`, a real window with the
//! keyboard focus and the real keyboard.
//!
//! The unit tests and the scenario suite of the crate (`ime_scenarios.rs`) drive the **state machine** of the text
//! service; this one drives the **installed text service** the way an application does. It is the layer that the
//! VS Code bridge could not replace: the bridge proves that the engine writes text into an editor, not that the
//! input method eats a key.
//!
//! ## When it runs
//!
//! The live part takes the focus of the desktop for a moment (the laser interface of the input method comes on the
//! screen), so it is **opt-in**: `PIME_IME_LIVE=1 cargo test --test ime_live -- --nocapture`. Without the variable
//! every test here skips itself and says why.
//!
//! The tests also skip themselves when the zh-CN profile of this product is **not registered** on the machine (a
//! machine without the installer): the profile is looked up, nothing is written.
//!
//! ## What it proves, and what it does not
//!
//! * `the_input_method_comes_alive_when_its_profile_is_activated` — msctf loads the installed `pime_tsf.dll` into the
//!   process and the text service opens its own windows (the glow of the laser interface, the bridge of the agents).
//!   **Measured: yes.**
//! * `the_text_service_holds_the_key_event_sink_of_the_thread` — msctf gave the key event sink of the client id of
//!   the thread to the text service (a second advise of the host is refused). **Measured: yes.**
//! * `the_keys_reach_the_text_service` — the key events of the real keyboard reach the text service. **Measured: no**
//!   (the text service answers "not mine" even for `Ctrl+F12`, the chord it must always take): see D-170 in
//!   `02_Verification/DEFECT_SUBMISSION_2026-11-07.md`. This test is the one that turns green when the defect is
//!   fixed; until then it fails with the measurement in the message.

mod common;

use common::ime_live::{KEY_F12, KEY_SPACE, LiveHost, GLOW_CLASS};

/// Whether the live part may run: the user asks for it, and the profile of this product is registered on this machine.
fn live_allowed() -> bool {
    if std::env::var("PIME_IME_LIVE").as_deref() != Ok("1") {
        eprintln!(
            "skipped: the live part needs PIME_IME_LIVE=1 (it takes the focus of the desktop and the laser \
             interface of the input method comes on the screen)"
        );
        return false;
    }
    if !LiveHost::profile_registered() {
        eprintln!(
            "skipped: the zh-CN profile of this product is not registered on this machine (the installer was \
             never run here); nothing was registered by the test"
        );
        return false;
    }
    true
}

/// The installed text service really comes alive: msctf loads the DLL and the text service opens its own windows.
#[test]
fn the_input_method_comes_alive_when_its_profile_is_activated() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open().expect("the host opens");
    assert!(
        host.installed_dll_loaded(),
        "msctf must load the installed pime_tsf.dll into the process of the host"
    );
    let windows = host.windows();
    assert!(
        windows.iter().any(|w| w == GLOW_CLASS),
        "the text service must open the glow of its laser interface; the windows of the thread are {windows:?}"
    );
    assert!(
        host.key_sink_is_taken(),
        "the text service must hold the key event sink of the client id of the thread"
    );
    // The diagnosis of D-170: one sink per client id (the text service's sink is on the client id of the thread) or
    // one sink per thread (it is somewhere else and the keystroke manager cannot find it for this context).
    eprintln!(
        "the diagnosis: another client id may have a sink = {}; the focus is our document manager = {}",
        host.another_client_id_may_have_a_sink(),
        host.focus_is_our_document_manager()
    );
}

/// The key events of the real keyboard reach the text service, and the candidate lands in the document.
///
/// This is the test that measures the whole chain. It fails today (D-170): the text service never sees a key.
#[test]
fn the_keys_reach_the_text_service_and_the_candidate_lands_in_the_document() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open().expect("the host opens");

    // The factory mode is the **code input**: the letters go into the document as they were typed, and the text
    // service only watches. The host writes them itself, the way an application does.
    let mut eaten = Vec::new();
    for ch in ['w', 'o'] {
        let (vk, scan) = common::ime_live::key_of(ch);
        eaten.push(host.ask_about_key(vk, scan, Some(&ch.to_string())));
    }
    assert_eq!(
        host.document(),
        "wo",
        "in the code input the document holds what was typed"
    );

    // Ctrl+F12 switches to the natural language input: the chord belongs to the input method, always.
    let chord = host.ask_about_ctrl_f12();
    assert!(
        chord,
        "Ctrl+F12 must be eaten by the text service (it switches the input mode); the key events do not reach \
         it. **D-170**: the same host does not drive the Microsoft Chinese IME either, so the chain of our text \
         service is **unproven** by this host (not disproven) - the missing step is on the host side"
    );

    // Now the letters are eaten and the composition is open.
    for ch in ['w', 'o'] {
        let (vk, scan) = common::ime_live::key_of(ch);
        assert!(
            host.ask_about_key(vk, scan, Some(&ch.to_string())),
            "the letter {ch:?} must be eaten by the text service in the natural language input (D-170)"
        );
    }

    // The space takes the highlighted candidate, and the text service writes it into the document of the host.
    let (vk, scan) = KEY_SPACE;
    assert!(
        host.ask_about_key(vk, scan, None),
        "the space must be eaten by the text service (it takes the candidate) (D-170)"
    );
    let document = host.document();
    assert!(
        document.contains('我'),
        "the document must hold the character the input method wrote; it holds {document:?}"
    );
}

/// The real keyboard reaches the window procedure of the host, and the window procedure asks TSF about the key.
///
/// This part of the chain works today: it is the part **after** it that is broken (D-170).
#[test]
fn the_real_keyboard_reaches_the_window_procedure_of_the_host() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open().expect("the host opens");
    let seen = host.type_on_the_real_keyboard("wo");
    assert_eq!(
        seen.len(),
        2,
        "the window procedure must see both letters of the real keyboard; it saw {seen:?}"
    );
    // What the window procedure writes into the document depends on the keyboard layout of the machine, so it is not
    // asserted here: the point of this test is that the key reaches the window procedure and the question of TSF.
}

/// The text service of **this build**, activated by the host itself: does the key chain work with the code of this
/// repository?
///
/// This is the variant that separates the two halves of D-170: msctf activating a **registered** text service (the
/// installed DLL) versus the host activating the text service itself. Measured: msctf refuses the key event sink of
/// a text service that is not the registered input processor of the thread (`E_INVALIDARG`), so this variant cannot
/// work - it is kept as a diagnostic.
#[test]
fn the_text_service_of_this_build_cannot_take_the_key_sink() {
    if !live_allowed() {
        return;
    }
    match LiveHost::open_with_our_own_text_service() {
        Ok(host) => {
            let mut eaten = Vec::new();
            for ch in ['w', 'o'] {
                let (vk, scan) = common::ime_live::key_of(ch);
                eaten.push(host.ask_about_key(vk, scan, Some(&ch.to_string())));
            }
            eprintln!("the keys of the text service of this build: {eaten:?}");
        }
        Err(reason) => {
            // The measurement: msctf only lets the **registered** input processor of the thread hold the key event
            // sink. The host cannot take it for a text service of its own.
            eprintln!("the text service of this build was refused: {reason}");
        }
    }
}

/// The control experiment of D-170: the **Microsoft** Chinese text service, in the same host.
///
/// The host is only a valid measuring instrument if it can drive *some* text service. The Microsoft Chinese IME of
/// Windows 11 runs as a separate process (`ChsIME.exe`) with an in-process proxy, so it is a weak baseline - but
/// the measurement is still worth having: today the host drives neither.
#[test]
fn the_microsoft_text_service_takes_the_keys_in_the_same_host() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open_without_any_profile().expect("the host opens");
    host.activate_the_profile_of(
        &common::ime_live::CLSID_MS_CHINESE,
        &common::ime_live::PROFILE_MS_CHINESE,
    )
    .expect("the profile of the Microsoft text service");
    std::thread::sleep(std::time::Duration::from_millis(600));
    let mut eaten = Vec::new();
    for ch in ['w', 'o'] {
        let (vk, scan) = common::ime_live::key_of(ch);
        eaten.push(host.ask_about_key(vk, scan, Some(&ch.to_string())));
    }
    eprintln!("the keys of the Microsoft text service: {eaten:?}");
    eprintln!("the windows of the thread: {:?}", host.windows());
    assert!(
        eaten.iter().any(|e| *e),
        "the host must be able to drive **some** text service, otherwise it cannot prove anything about ours \
         (D-170): the Microsoft Chinese IME of Windows 11 runs as a separate process, so this baseline is weak, \
         but the measurement says the host drives neither today"
    );
}

/// The mode chord of the product belongs to the text service: it is the way from the code input to the natural
/// language input. Measured with the direct call of TSF (the one the window procedure makes).
#[test]
fn the_mode_chord_is_eaten_by_the_text_service() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open().expect("the host opens");
    let chord = host.ask_about_ctrl_f12();
    // The diagnosis: which client id holds the key event sink of this thread. The keystroke manager of TSF hands the
    // key to the sink of the focused context, and that context belongs to the client id that created it.
    eprintln!(
        "the diagnosis: the sink of this thread is under the client id {:?} (the client id of the thread is {})",
        host.client_id_that_holds_the_sink(),
        host.client_id()
    );
    assert!(
        chord,
        "the text service must take Ctrl+F12; the key event sink of the thread is held by the text service \
         (the sink is taken: {}), but the key does not arrive. **D-170**: the same host does not drive the \
         Microsoft Chinese IME either, so the chain of our text service is **unproven** by this host (not \
         disproven) - the missing step is on the host side",
        host.key_sink_is_taken()
    );
    assert_eq!((KEY_F12.0, KEY_F12.1), (0x7B, 0x58));
}
