//! Keeps the laser beam of the input method on the screen, so that a picture of the desktop shows it.
//!
//! The beam of the product is short: it shoots out of the mouth of the glow when a character goes on the screen and
//! it is gone in about half a second. A picture that is taken once would almost never catch it, so this probe fires
//! it again and again — the beam of the picture is the beam of the product, drawn by the same code.
//!
//!   cargo run --example laser_beam_probe -- [seconds to stay open]
//!
//! Nothing is registered, no profile is activated, no text is inserted anywhere.

use pime_tsf::laser_ui::{LaserEvent, LaserUi};
use pime_tsf::ui::Anchor;
use std::cell::RefCell;
use std::rc::Rc;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetSystemMetrics, PeekMessageW, PostQuitMessage, TranslateMessage, MSG,
    PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, WM_QUIT,
};

/// How often the beam is fired again (in milliseconds).
///
/// The beam lives about half a second, so firing it every 400 milliseconds keeps it on the screen without a gap.
const REFIRE_MS: u64 = 400;

fn main() -> std::process::ExitCode {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);

    // SAFETY: the metrics of the primary monitor are a plain read.
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    // The character that was written: the beam shoots at it.
    let anchor = Anchor {
        x: (screen_w * 2 / 3).max(0),
        y: (screen_h / 2).max(0),
        line_height: 20,
    };

    let sink: Rc<dyn Fn(LaserEvent)> = Rc::new(|_: LaserEvent| {});
    let laser = LaserUi::create(sink);
    {
        let Ok(mut laser) = laser.try_borrow_mut() else {
            return std::process::ExitCode::FAILURE;
        };
        laser.show(pime_tsf::layout::Rect::new(0, 0, screen_w, screen_h));
        // The panel is open, so that the mouth of the glow is on the screen.
        laser.open_panel();
    }

    println!("screen: {screen_w}x{screen_h}");
    println!("READY");
    pump_for(seconds, &laser, anchor);
    println!("done");
    std::process::ExitCode::SUCCESS
}

/// Pump the messages of this thread for `seconds`, firing the beam again and again.
///
/// The messages of this thread are pumped, because the animation of the laser interface is driven by a timer.
fn pump_for(seconds: u64, laser: &Rc<RefCell<LaserUi>>, anchor: Anchor) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut next = std::time::Instant::now();
    loop {
        if std::time::Instant::now() >= deadline {
            return;
        }
        // SAFETY: the message is a local; the call only fills it.
        let mut message = MSG::default();
        // SAFETY: a plain read of the queue of this thread.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                // The time is checked here and not only outside of the loop: the timer of the interface posts a
                // message every 16 milliseconds and the drawing of the panel takes longer than that, so the queue of
                // the thread is never empty and a check outside of the loop would never be reached.
                if std::time::Instant::now() >= deadline {
                    return;
                }
                // The beam is fired from here and not from outside of the loop: the queue of the thread is never
                // empty (see above), so the code outside of the loop would never come back.
                if std::time::Instant::now() >= next {
                    if let Ok(mut laser) = laser.try_borrow_mut() {
                        laser.fire(anchor);
                    }
                    next = std::time::Instant::now() + std::time::Duration::from_millis(REFIRE_MS);
                }
                if message.message == WM_QUIT {
                    PostQuitMessage(0);
                    return;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}
