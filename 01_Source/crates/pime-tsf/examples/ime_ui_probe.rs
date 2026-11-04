//! Opens the windows of the input method (the candidate window, the settings window and the laser interface: the glow
//! on the left of the screen, the beam, the panel and the logo in the corner), keeps them on screen for a while and
//! reports what it opened. A screenshot of the desktop taken while it runs is the evidence for the native look of the
//! input method.
//!
//!   cargo run --example ime_ui_probe -- [seconds to stay open]
//!
//! Why a probe and not a test: a test that opens a window on the desktop of the user must not run unattended. The
//! probe is therefore a separate program; the screenshot script (`tests/smoke/ime-ui-capture.ps1`) takes the picture
//! while it runs, and `tests/smoke/ime-ui-check.js` checks afterwards that the picture exists and is not a blank
//! rectangle (a window that painted nothing is a defect the unit tests cannot see).
//!
//! Nothing is registered, no profile is activated, no text is inserted anywhere.

use pime_tsf::imesettings::ImeSettings;
use pime_tsf::laser::PanelItem;
use pime_tsf::laser_ui::{LaserEvent, LaserUi};
use pime_tsf::popup::PopupWindow;
use pime_tsf::settings_window::{SettingsEvent, SettingsWindow};
use pime_tsf::ui::{Anchor, CandidateRow, CandidateUi, CandidateView};
use std::rc::Rc;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetSystemMetrics, PeekMessageW, PostQuitMessage, TranslateMessage, MSG,
    PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, WM_QUIT,
};

fn main() -> std::process::ExitCode {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);

    // The windows are placed in the middle of the primary monitor: a fixed corner would collide with whatever the
    // user has open, and an overlapped window cannot be judged.
    // SAFETY: the metrics of the primary monitor are a plain read.
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let anchor = Anchor {
        x: (screen_w / 2).max(0),
        y: (screen_h / 2).max(0),
        line_height: 20,
    };

    // ---- the candidate window ------------------------------------------------
    let view = CandidateView {
        rows: (1..=5)
            .map(|i| CandidateRow {
                label: i.to_string(),
                text: format!("fn main() {{ … }}   candidate {i}"),
                detail: "rule · valid".to_string(),
            })
            .collect(),
        selected_in_page: 1,
        page: 0,
        page_count: 3,
    };
    let mut popup = PopupWindow::new();
    let popup_shown = popup.show(&view, anchor);

    // ---- the settings window -------------------------------------------------
    let sink: Rc<dyn Fn(SettingsEvent)> = Rc::new(|_: SettingsEvent| {});
    let settings = SettingsWindow::open(ImeSettings::default(), sink);

    // ---- the laser interface (the glow, the beam, the panel and the logo) ----
    let laser_sink: Rc<dyn Fn(LaserEvent)> = Rc::new(|_: LaserEvent| {});
    let laser = LaserUi::create(laser_sink);
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
            PanelItem::Toggle {
                key: "auto_compose",
                label: "自动开组合",
                on: false,
            },
            PanelItem::Toggle {
                key: "tab_cycles",
                label: "Tab 确认候选",
                on: true,
            },
            PanelItem::Toggle {
                key: "tab_to_host",
                label: "Tab 归宿主",
                on: false,
            },
            PanelItem::Menu {
                key: "language",
                label: "语言",
                value: "中文".to_string(),
            },
            PanelItem::Menu {
                key: "effort",
                label: "AI 力度",
                value: "中".to_string(),
            },
            PanelItem::Menu {
                key: "skin",
                label: "外观",
                value: "激光彩虹".to_string(),
            },
            PanelItem::Settings,
        ]);
        // The work area of the primary monitor.
        laser.show(pime_tsf::layout::Rect::new(0, 0, screen_w, screen_h));
        // The panel is open, so that the picture shows it.
        laser.open_panel(pime_tsf::laser::PanelOrigin::Glow);
        // A beam is fired at the candidate window: the light of the character.
        laser.fire(anchor);
    }

    println!("screen: {screen_w}x{screen_h}");
    println!(
        "candidate window: {}",
        if popup_shown { "shown" } else { "NOT SHOWN" }
    );
    println!(
        "settings window: {}",
        if settings.is_some() {
            "shown"
        } else {
            "NOT SHOWN"
        }
    );
    println!(
        "rects: candidate={:?} settings={:?}",
        popup.window_rect(),
        settings.as_ref().map(|w| w.handle().0 as i64)
    );
    println!("READY");
    // The windows stay on screen while the screenshot is taken. The messages of this thread are pumped, because the
    // animation of the laser interface is driven by a timer.
    pump_for(seconds);
    println!("done");
    std::process::ExitCode::SUCCESS
}

/// Pump the messages of this thread for `seconds` (the timer of the laser interface needs them).
fn pump_for(seconds: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
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
