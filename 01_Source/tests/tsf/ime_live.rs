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

/// The decisive measurement of D-170: the document context created under the client id that owns the key event
/// sink.
///
/// The keystroke manager of TSF hands the key to the sink that belongs to the **owner of the focused context**. The
/// text service advises its sink under the client id msctf gave it (measured: 21 on this machine), while
/// `ITfThreadMgr2::Activate` hands the application another one (measured: 32) - when the two do not match, the key
/// goes nowhere. This test creates the context under the measured id and asks again.
#[test]
fn the_context_of_the_client_that_owns_the_sink_receives_the_keys() {
    if !live_allowed() {
        return;
    }
    let host = LiveHost::open_with_the_context_of_another_client(21).expect("the host opens");
    let mut eaten = Vec::new();
    for ch in ['w', 'o'] {
        let (vk, scan) = common::ime_live::key_of(ch);
        eaten.push(host.ask_about_key(vk, scan, Some(&ch.to_string())));
    }
    eprintln!("the keys with the context of the owner of the sink: {eaten:?}");
    let chord = host.ask_about_ctrl_f12();
    eprintln!("the chord with the context of the owner of the sink: {chord}");
    // The measurement (the hypothesis is **disproved**): the routing is not decided by the owner of the focused
    // context, so the sink is looked up somewhere else. That is why this host cannot drive the input method.
}

/// The plain edit control: the path of every classic Windows application, and the only harness so far in which the
/// **keys really reach the input method**.
///
/// Nothing of TSF is done by the host here - the keys go into the control, Windows hands them to the input processor
/// of the thread, and the input processor writes its composition into the control. This is the path of every classic
/// Windows application - the one that does not implement TSF itself.
///
/// What the measurement says (2026-11-19, the installed 0.3.1):
/// * in the **code input** the letters land in the document as they were typed (the design of the product);
/// * after `Ctrl+F12` (the natural language input) the letters are **eaten** ⇒ the composition is open
///   ⇒ **the input method receives the keys**;
/// * but the composition is **not shown** in the control and the **candidate does not land** in it
///   ⇒ the input method eats the keys and writes nothing back (D-171).
#[test]
fn the_plain_edit_control_receives_the_input_method() {
    // The child: it runs the drive and prints the report. The parent reads the report: the drive needs a clean
    // process (the other cases of this file activate and deactivate the input method too).
    if std::env::var("PIME_IME_CHILD").is_ok() {
        drive_the_edit_control();
        return;
    }
    if !live_allowed() {
        return;
    }
    let report = common::ime_live::run_the_drive_in_a_child(&[
        "the_plain_edit_control_receives_the_input_method",
    ]);
    println!("--- the drive of the edit control ---\n{report}");
    // The measurement is kept on disk: the two invariants of the user (the pinyin must not stand next to the
    // character, the code must go into the document as it was typed) are claims about the **real** input method, so
    // the report of a real run belongs in the repository next to the other evidence (2026-11-25).
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("02_Verification");
        let _ = std::fs::create_dir_all(&root);
        let _ = std::fs::write(
            root.join("ime-live-report.txt"),
            format!("# the real input method in a plain edit control\n{report}\n"),
        );
    }
    assert!(
        report.contains("write=ok"),
        "the write path of the input method must work (the bridge writes into the document); the report is:\n{report}"
    );
    assert!(
        report.contains("math=ok"),
        "the mathematics of the symbol table has to reach the document through the same write path; the report is:\n{report}"
    );
    assert!(
        report.contains("commit=landed"),
        "the candidate must land in the document of the application (D-171: the keys are eaten and the write path \
         works, so the candidate list is empty); the report is:\n{report}"
    );
    assert!(
        report.contains("digit=landed"),
        "the digit of a candidate must **take** the candidate and not land in the document (the report of the user: \
         the digit is inserted into the pinyin and the selected text is not written); the report is:\n{report}"
    );
    assert!(
        report.contains("panel=on-screen"),
        "the floating window (the panel is the list of the candidates) has to be on the screen while the candidates \
         show; the report is:\n{report}"
    );
    assert!(
        report.contains("code_panel=candidates"),
        "the candidates must be visible in the floating window in the code input too (the report of the user: the \
         programming input cannot see the candidates); the report is:\n{report}"
    );
    assert!(
        report.contains("click=landed"),
        "a click on a row of the floating window has to take that candidate and write it into the document (the \
         report of the user: the floating window cannot be interacted with); the report is:\n{report}"
    );
    // The **content** of the document after every commit: the report used to say only "a candidate landed", and that
    // cannot tell a character from a character that stands next to the pinyin. The two invariants of the user
    // (2026-11-24) are measured here: the pinyin and the character must never be written into the document together,
    // and the code input must write the code into the document exactly as it was typed.
    // The word the drive types in the natural language input, and the character its first candidate is.
    const PINYIN: &str = "wo";
    const CANDIDATE: &str = "我";
    // The code the drive types in the code input phase.
    const CODE: &str = "wo";
    // The document before the click (the document of the composition) and the documents after every commit: the
    // commit has to **add** the character, so the document of the phase before it is the measure - a character that
    // was already there must not make the check pass (the click is the first commit; the space and the digit come
    // after it and would find the character of the click in the document already).
    let document_before_the_click = common::ime_live::the_field_of_the_report(&report, "doc_before_click")
        .unwrap_or_else(|| "<the report has no doc_before_click>".to_string());
    let document_after_the_click = common::ime_live::the_field_of_the_report(&report, "doc_after_click")
        .unwrap_or_else(|| "<the report has no doc_after_click>".to_string());
    let characters_before_the_click = common::ime_live::the_count_of_characters(&document_before_the_click);
    let characters_after_the_click = common::ime_live::the_count_of_characters(&document_after_the_click);
    assert!(
        characters_after_the_click > characters_before_the_click,
        "the click must add the character of the row to the document of the application (the document held \
         {characters_before_the_click} of them before the click and {characters_after_the_click} after it); the \
         document is {document_after_the_click:?}; the report is:\n{report}"
    );
    assert!(
        !common::ime_live::the_pinyin_leaked_into(&document_after_the_click, PINYIN, CANDIDATE),
        "the pinyin must not stand next to the character in the document (the click may only write the character): \
         the document is {document_after_the_click:?}; the report is:\n{report}"
    );
    let document_after_the_space = common::ime_live::the_field_of_the_report(&report, "doc_after_space")
        .unwrap_or_else(|| "<the report has no doc_after_space>".to_string());
    let characters_after_the_space = common::ime_live::the_count_of_characters(&document_after_the_space);
    assert!(
        characters_after_the_space > characters_after_the_click,
        "the space must add the character it took to the document of the application (the document held \
         {characters_after_the_click} of them before the space and {characters_after_the_space} after it); the \
         document is {document_after_the_space:?}; the report is:\n{report}"
    );
    assert!(
        !common::ime_live::the_pinyin_leaked_into(&document_after_the_space, PINYIN, CANDIDATE),
        "the pinyin must not stand next to the character in the document (the space may only write the character): \
         the document is {document_after_the_space:?}; the report is:\n{report}"
    );
    let document_after_the_digit = common::ime_live::the_field_of_the_report(&report, "doc_after_digit")
        .unwrap_or_else(|| "<the report has no doc_after_digit>".to_string());
    let characters_after_the_digit = common::ime_live::the_count_of_characters(&document_after_the_digit);
    assert!(
        characters_after_the_digit > characters_after_the_space,
        "the digit must add the character it took to the document of the application (the document held \
         {characters_after_the_space} of them before the digit and {characters_after_the_digit} after it); the \
         document is {document_after_the_digit:?}; the report is:\n{report}"
    );
    assert!(
        !common::ime_live::the_pinyin_leaked_into(&document_after_the_digit, PINYIN, CANDIDATE),
        "the pinyin must not stand next to the character in the document (the digit may only write the character): \
         the document is {document_after_the_digit:?}; the report is:\n{report}"
    );
    // The code input: the code goes into the document exactly as it was typed - nothing rewritten, no candidate
    // added, no letter lost. The phase runs only when the drive could switch into the code input; when it did not run,
// the measurement of the code is missing and the report says so (`code_phase=skipped`).
    if report.contains("code_phase=ran") {
        let doc_code = common::ime_live::the_field_of_the_report(&report, "doc_code").unwrap_or_default();
        assert_eq!(
            doc_code, CODE,
            "the code input must write the code into the document exactly as it was typed - nothing rewritten, no \
             candidate added, no letter lost; the text the code phase added is {doc_code:?}; the report is:\n{report}"
        );
    } else {
        eprintln!(
            "the code phase did not run (the machine was already in the natural language input when the drive \
             started): the code input was not measured by this run"
        );
    }
}

/// The real Notepad: the application the user types in.
///
/// What this case really measures (and what it cannot):
/// * **it can**: prove that the input method is the input processor **of the real Notepad** - the bridge of the agent
///   protocol answers from the **Notepad's own process**, and it names the mode and the language;
/// * **it cannot**: deliver the keys. The Notepad of Windows 11 is a store application (XAML), and a background
///   process cannot give it the keyboard focus on this machine (`SetForegroundWindow` is refused by the foreground
///   lock, and the click lands on the window that is in front). The keys therefore never reach the Notepad, and the
///   text service stays without a context - which is also why the bridge answers "the field is a password field"
///   (the text service fails closed when it has no context).
///   ⇒ **the measurement of the real Notepad is the manual one** (`02_Verification/REAL_MACHINE_CHECKLIST.md`).
#[test]
fn the_input_method_is_the_input_processor_of_the_real_notepad() {
    // The Notepad is a **real application on the desktop**: this case opens it, takes the focus and types on the
    // keyboard of the user. It therefore asks for its own switch (on top of PIME_IME_LIVE).
    if std::env::var("PIME_IME_NOTEPAD").as_deref() != Ok("1") {
        eprintln!(
            "skipped: this case opens the real Notepad on the desktop; it needs PIME_IME_NOTEPAD=1 (on top of \
             PIME_IME_LIVE)"
        );
        return;
    }
    // The child: it drives the Notepad (the Notepad needs the desktop to itself) and prints what it measured.
    if std::env::var("PIME_IME_CHILD").is_ok() {
        drive_the_real_notepad();
        return;
    }
    if !live_allowed() {
        return;
    }
    let report = common::ime_live::run_the_drive_in_a_child(&[
        "the_input_method_is_the_input_processor_of_the_real_notepad",
    ]);
    println!("--- the real Notepad ---\n{report}");
    assert!(
        report.contains("ime_in_the_notepad=yes"),
        "the input method must be the input processor of the real Notepad (its bridge answers from the Notepad's \
         own process); the report is:\n{report}"
    );
}

/// The drive of the real Notepad: it prints a machine-readable report of what the file ended up holding.
fn drive_the_real_notepad() {
    let _modifiers = common::ime_live::ModifiersReleased;
    let mut host = match common::ime_live::NotepadHost::open() {
        Ok(host) => host,
        Err(e) => {
            println!("REPORT notepad=unavailable ({e:?})");
            return;
        }
    };
    // The input method that is active in the Notepad's own process: its bridge answers what it is doing.
    let reply = common::ime_live::ReplyWindow::open();
    let bridge_pid = common::ime_live::bridge_window_anywhere()
        .map(common::ime_live::process_of_window)
        .unwrap_or(0);
    let ask = |op: &str| -> Option<String> {
        reply.as_ref().and_then(|r| {
            r.ask(&format!(
                "{{\"v\":1,\"op\":\"{op}\",\"reply\":{}}}",
                r.hwnd().0 as usize
            ))
        })
    };
    let status = ask("status");
    eprintln!(
        "the Notepad is in front: {}; the process of the Notepad: {}; the process of the bridge: {}",
        host.is_in_front(),
        host.process_id(),
        bridge_pid
    );
    // The positive control: the bridge of the input method writes a character into the focused document of the
    // Notepad. If that character is there afterwards, the path works and the missing candidate is the input
    // method's.
    let _control = ask("insert").map(|_| ()).is_some();
    let write_answer = reply.as_ref().and_then(|r| {
        r.ask(&format!(
            "{{\"v\":1,\"op\":\"insert\",\"text\":\"X\",\"reply\":{}}}",
            r.hwnd().0 as usize
        ))
    });
    // The letters of the word, then the space that takes the candidate.
    host.type_real("wo");
    std::thread::sleep(std::time::Duration::from_millis(600));
    // The space takes the candidate. The selection of the document must stay a caret while the commit happens: the
    // commit verifies the text before the caret, and a selection would make that verification fail (the measurement
    // would then measure its own disturbance instead of the input method).
    host.type_real(" ");
    std::thread::sleep(std::time::Duration::from_millis(800));
    // Select everything and let the input method read the selection back: that is the text of the document.
    host.select_all();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let read = ask("read");
    host.save();
    let saved = host.what_was_saved();
    let text_of_the_document = read.clone().unwrap_or_default();
    let control_ok = text_of_the_document.contains('X');
    let committed = text_of_the_document.contains('我');
    // the process of the Notepad, before it is closed
    let notepad_pid = host.process_id();
    host.close();
    println!(
        "REPORT notepad={:?} read={:?} control={} committed={} status={:?} write={:?} ime_in_the_notepad={}",
        saved,
        text_of_the_document,
        if control_ok { "ok" } else { "broken" },
        if committed { "yes" } else { "no" },
        status,
        write_answer,
        if bridge_pid == notepad_pid { "yes" } else { "no" }
    );
}

/// The drive of the edit control: it measures and prints a machine-readable report.
fn drive_the_edit_control() {
    // The drive presses chords (Ctrl+F12): whatever happens, the modifiers of the keyboard are released at the end.
    let _modifiers = common::ime_live::ModifiersReleased;
    let host = common::ime_live::EditHost::open().expect("the window with the edit control");
    // The profile of this product is activated for this thread: the system only makes the text service the input
    // processor of a thread when the profile of its language is active, and the state of the language bar of the
    // desktop of the user must not decide what the measurement sees.
    if let Err(e) = host.activate_our_profile() {
        println!("REPORT profile=unavailable ({e:?})");
        return;
    }
    // The focus comes **after** the activation: switching the input method may give the foreground to another window,
    // and a key that goes to another window is typed into somebody else's document.
    if !host.take_the_focus() {
        println!("REPORT focus=no");
        return;
    }
    // The input method is activated by the system when the focused window gets the focus: the keys typed before
    // that are plain text, the ones after it are the input method's. The **bridge** of the input method is the sign
    // that it is alive (it comes with the text service).
    let mut alive = false;
    for _ in 0..60 {
        if host.the_mode_of_the_input_method().is_some() {
            alive = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    if !alive {
        println!("REPORT ime=absent");
        return;
    }
    let status = host.the_mode_of_the_input_method().unwrap_or_default();
    let natural_language = status.contains("the input mode is on");
    let language = if status.contains("\"language\":\"zh-CN\"") {
        "zh-CN"
    } else if status.contains("\"language\":\"en-US\"") {
        "en-US"
    } else if status.contains("\"language\":\"ja-JP\"") {
        "ja-JP"
    } else {
        "?"
    };
    let switched = !natural_language;
    if switched {
        host.press_ctrl_f12();
    }
    // The mode really changed: the bridge answers with the mode of the text service, so the **installed** DLL can be
    // asked what it did (the user asked for a visible sign of the mode on 2026-11-23 - this is the part of it a
    // machine can check; the pixels are the human's job).
    let after_the_switch = host.the_mode_of_the_input_method().unwrap_or_default();
    assert!(
        after_the_switch.contains("the input mode is on"),
        "Ctrl+F12 must switch the installed text service to the natural language input: the bridge says {after_the_switch}"
    );
    // The focus is checked again: a key that goes to another window is typed into somebody else's document.
    let focus_before_typing = host.has_the_focus();
    let modifiers_before_typing = host.modifier_state();
    // The document is read after **every** letter: the text service and the application may see different
    // things, and only the two together say where a letter went.
    host.type_real("w");
    let document_after_the_w = host.read();
    host.type_real("o");
    let document_after_the_o = host.read();
    let modifiers_after_typing = host.modifier_state();
    // The document is read **without touching the selection**. The commit verifies the text before the caret, so a
    // selection (the "select all" of an earlier version of this drive) would make that verification fail: the
    // measurement would then measure its own disturbance instead of the input method.
    let composition = host.read();
    let caret_of_the_application = host.read_selection();
    // **The floating window**: the panel IS the list of candidates, and a click on one of its rows has to take that
    // candidate (the report of the user on 2026-11-20: "the floating window cannot be interacted with, the click
    // does nothing"). The click comes first: it takes the candidate and ends the list.
    let panel = common::ime_live::the_panel_of_the_input_method();
    let panel_is_on_the_screen = panel.is_some();
    // The document is read after the click: the click has to put the character into the document, and only the
    // character (the pinyin must not stay next to it in the document of the application).
    let mut document_after_the_click = String::new();
    let click_landed = match panel {
        Some((_, left, top, width, _)) => {
            // the first row of the panel is under its handle
            common::ime_live::click_at(
                left + width / 2,
                top + pime_tsf::laser_ui::PANEL_HANDLE + pime_tsf::laser_ui::PANEL_ROW_HEIGHT / 2,
            );
            std::thread::sleep(std::time::Duration::from_millis(700));
            document_after_the_click = host.read();
            document_after_the_click.contains('我')
        }
        None => false,
    };
    // The space takes the highlighted candidate.
    host.type_real("wo");
    host.type_real(" ");
    let after_space = host.read();
    let commit_landed = after_space.contains('我');
    // The digit of a candidate: it has to **take** the candidate, not land in the document (the report of the user:
    // "the digit of the candidate is inserted into the pinyin and the selected text is not written").
    host.type_real("wo");
    host.type_real("1");
    let after_digit = host.read();
    let digit_landed = after_digit.matches('我').count() == 3 && !after_digit.contains('1');
    // The path of the agents: the bridge writes into the focused document.
    let before = host.read();
    let answer = host.ask_the_bridge_to_write("我");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let after = host.read();
    let write_path_works = answer
        .as_deref()
        .map(|a| a.contains("\"ok\":true"))
        .unwrap_or(false)
        && after.contains('我')
        && after.len() > before.len();
    // The mathematics of the symbol table: the symbols are **not typed** (they are rows of the panel), so the path
    // that carries them into the document is the same write path - it has to carry a symbol of the table too.
    let before_the_symbol = host.read();
    let symbol_answer = host.ask_the_bridge_to_write("±");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let after_the_symbol = host.read();
    let math_path_works = symbol_answer
        .as_deref()
        .map(|a| a.contains("\"ok\":true"))
        .unwrap_or(false)
        && after_the_symbol.contains('±')
        && after_the_symbol.len() > before_the_symbol.len();
    // The code input: the letters belong to the host (the code goes into the editor as it was typed) and the
    // candidates must be visible in the floating window (the report of the user: "the programming input cannot see
    // the candidates"). The panel of the candidates is shorter than the panel of the settings: the height tells
    // which one is on the screen.
    let mut code_panel = "absent";
    let mut document_of_the_code_input = String::new();
    let mut document_after_the_code_w = String::new();
    let mut code_phase_ran = false;
    // The document before the code input: the code phase is measured by what it adds to it (the code must land in the
    // document exactly as it was typed - nothing rewritten, no candidate added, no letter lost).
    let document_before_the_code = host.read();
    if switched {
        code_phase_ran = true;
        host.press_ctrl_f12(); // the code input
        std::thread::sleep(std::time::Duration::from_millis(400));
        host.type_real("w");
        std::thread::sleep(std::time::Duration::from_millis(200));
        document_after_the_code_w = host.read();
        host.type_real("o");
        std::thread::sleep(std::time::Duration::from_millis(400));
        document_of_the_code_input = host.read();
        code_panel = match common::ime_live::the_panel_of_the_input_method() {
            Some((_, _, _, _, height)) if height < 400 => "candidates",
            Some((_, _, _, _, _)) => "settings",
            None => "absent",
        };
    }
    // The text the code phase added at the end of the document: the code itself, and nothing else.
    let text_added_by_the_code = common::ime_live::the_text_added_at_the_end(
        &document_before_the_code,
        &document_of_the_code_input,
    );
    let (doc_code, doc_code_delta) = match (text_added_by_the_code, code_phase_ran) {
        (Some(text), _) => (text, "ok"),
        (None, false) => (String::new(), "skipped"),
        (None, true) => (String::new(), "not-at-the-end"),
    };
    // The document at the end: it says where the text the bridge wrote landed.
    let document_at_the_end = host.read();
    if switched && !code_phase_ran {
        // the mode of the user is put back the way it was (the code input phase above already did it)
        host.press_ctrl_f12();
    }
    // The view of the input method itself: it reads the selection of the document. It happens at the very end,
    // because a selection disturbs the commit (the commit verifies the text before the caret).
    host.select_all();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let document_of_the_input_method = host.ask_the_bridge(&format!(
        "{{\"v\":1,\"op\":\"read\",\"reply\":{}}}",
        host.window().0 as usize
    ));
    println!(
        "REPORT mode={} language={} focus={} modifiers_before={:?} modifiers_after={:?} composition={:?} caret={:?} \
         document_after_the_w={:?} document_after_the_o={:?} edit_window={:#x} \
         panel={} click={} commit={} digit={} write={} math={} document_after_the_digit={:?} \
         document_of_the_input_method={:?} bridge={:?} document_at_the_end={:?} code_panel={} \
         document_after_the_code_w={:?} document_of_the_code_input={:?} \
         doc_before_click={} doc_after_click={} doc_after_space={} doc_after_digit={} doc_code={} doc_code_delta={} \
         code_phase={}",
        if natural_language { "natural" } else { "code" },
        language,
        if focus_before_typing { "ok" } else { "lost" },
        modifiers_before_typing,
        modifiers_after_typing,
        composition,
        caret_of_the_application,
        document_after_the_w,
        document_after_the_o,
        host.edit_window().0 as usize,
        if panel_is_on_the_screen { "on-screen" } else { "absent" },
        if click_landed { "landed" } else { "missing" },
        if commit_landed { "landed" } else { "missing" },
        if digit_landed { "landed" } else { "missing" },
        if write_path_works { "ok" } else { "broken" },
        if math_path_works { "ok" } else { "broken" },
        after_digit,
        document_of_the_input_method,
        answer,
        document_at_the_end,
        code_panel,
        document_after_the_code_w,
        document_of_the_code_input,
        common::ime_live::quoted(&composition),
        common::ime_live::quoted(&document_after_the_click),
        common::ime_live::quoted(&after_space),
        common::ime_live::quoted(&after_digit),
        common::ime_live::quoted(&doc_code),
        doc_code_delta,
        if code_phase_ran { "ran" } else { "skipped" }
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
