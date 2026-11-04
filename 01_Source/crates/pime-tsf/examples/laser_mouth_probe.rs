//! Drags the mouth of the glow to the top edge of the screen, so that a picture of the desktop shows where it went.
//!
//!   cargo run --example laser_mouth_probe -- [seconds to stay open]
//!
//! The probe presses the mouth with the mouse, moves the mouse to the top edge and lets it go. It prints the place
//! the mouth was given, so that the behaviour of the drag can be read from the output of the program and not only
//! from the picture.
//!
//! Why a probe and not a test: a test that opens a window on the desktop of the user must not run unattended. The
//! probe is a separate program; the screenshot script takes the picture while it runs.
//!
//! Nothing is registered, no profile is activated, no text is inserted anywhere.

use pime_tsf::glass::{GlassEvent, Role};
use pime_tsf::laser::{MouthPlace, PanelItem, PanelOrigin};
use pime_tsf::laser_ui::{LaserEvent, LaserUi};
use pime_tsf::layout::Rect;
use std::cell::RefCell;
use std::rc::Rc;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetSystemMetrics, PeekMessageW, PostQuitMessage, TranslateMessage, MSG,
    PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, WM_QUIT,
};

/// How wide the window of the glow is.
///
/// The mouth sits in the middle of it, so the mouse is pressed in the middle of the window. The value is the one of
/// `laser_ui::GLOW_SIZE`.
const GLOW_SIZE: i32 = 64;

/// The radius of the lens of the mouth (the value of `laser_ui::MOUTH_RADIUS`).
const MOUTH_RADIUS: i32 = 15;

fn main() -> std::process::ExitCode {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);

    // SAFETY: the metrics of the primary monitor are a plain read.
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let work = Rect::new(0, 0, screen_w, screen_h);

    // The place the mouth is given is reported by the interface; the probe writes it down.
    let moved: Rc<RefCell<Option<MouthPlace>>> = Rc::new(RefCell::new(None));
    let sink: Rc<dyn Fn(LaserEvent)> = Rc::new({
        let moved = Rc::clone(&moved);
        move |event| {
            if let LaserEvent::MouthMoved { edge, offset } = event {
                *moved.borrow_mut() = Some(MouthPlace { edge, offset });
            }
        }
    });

    let laser = LaserUi::create(sink);
    {
        let Ok(mut laser) = laser.try_borrow_mut() else {
            return std::process::ExitCode::FAILURE;
        };
        laser.set_items(vec![
            PanelItem::Toggle {
                key: "input_mode",
                label: "输入模式",
                on: true,
            },
            PanelItem::Toggle {
                key: "space_confirms",
                label: "空格上屏",
                on: false,
            },
            PanelItem::Menu {
                key: "language",
                label: "语言",
                value: "中文".to_string(),
            },
            PanelItem::Menu {
                key: "skin",
                label: "外观",
                value: "激光彩虹".to_string(),
            },
            PanelItem::Settings,
        ]);
        laser.show(work);
        // The panel is open, so that the picture shows it next to the mouth.
        laser.open_panel(PanelOrigin::Glow);
    }
    // The windows need a moment to appear on the screen.
    pump_for(0.6);

    // ---- the drag of the mouth ------------------------------------------------
    // The mouth starts on the left edge, in the middle of the work area.
    let start = (work.left + MOUTH_RADIUS, work.top + work.height() / 2);
    // The mouse goes to the top edge of the screen, in the middle of it.
    let target = (screen_w / 2, work.top + 30);
    {
        let Ok(mut laser) = laser.try_borrow_mut() else {
            return std::process::ExitCode::FAILURE;
        };
        // The mouse is pressed in the middle of the window of the glow: that is the mouth itself.
        laser.handle(
            Role::Glow,
            GlassEvent::Press {
                x: GLOW_SIZE / 2,
                y: GLOW_SIZE / 2,
            },
        );
        // The mouse moves to the top edge, in a few steps, like a hand that drags.
        for step in 1..=8 {
            let x = start.0 + (target.0 - start.0) * step / 8;
            let y = start.1 + (target.1 - start.1) * step / 8;
            laser.handle(Role::Glow, GlassEvent::Drag { x, y });
        }
        // The button is let go.
        laser.handle(
            Role::Glow,
            GlassEvent::Release {
                x: target.0,
                y: target.1,
                screen_x: target.0,
                screen_y: target.1,
            },
        );
    }

    match moved.borrow().as_ref() {
        Some(place) => println!("mouth place: {}", place.to_text()),
        None => println!("mouth place: NONE (the drag did not finish)"),
    }
    println!("READY");
    // The windows stay on the screen while the picture is taken.
    pump_for(seconds as f64);
    println!("done");
    std::process::ExitCode::SUCCESS
}

/// Pump the messages of this thread for `seconds` (the timer of the laser interface needs them).
///
/// The time is checked inside the loop over the messages and not only outside of it: the timer of the interface posts
/// a message every 16 milliseconds and the drawing of the panel takes longer than that, so the queue of the thread is
/// never empty and a check outside of the loop would never be reached.
fn pump_for(seconds: f64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds);
    loop {
        if std::time::Instant::now() >= deadline {
            return;
        }
        // SAFETY: the message is a local; the call only fills it.
        let mut message = MSG::default();
        // SAFETY: a plain read of the queue of this thread.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if std::time::Instant::now() >= deadline {
                    return;
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
