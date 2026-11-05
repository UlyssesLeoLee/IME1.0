//! Opens the windows of the input method (the settings window and the laser interface: the glow on the left of the
//! screen, the beam, the panel with the list of the candidates, and the logo in the corner), keeps them on screen for a
//! while and reports what it opened. A screenshot of the desktop taken while it runs is the evidence for the native look
//! of the input method.
//!
//!   cargo run --example ime_ui_probe -- [seconds to stay open]
//!
//! Why a probe and not a test: a test that opens a window on the desktop of the user must not run unattended. The
//! probe is therefore a separate program; the screenshot script (`tests/smoke/ime-ui-capture.ps1`) takes the picture
//! while it runs, and `tests/smoke/ime-ui-check.js` checks afterwards that the picture exists and is not a blank
//! rectangle (a window that painted nothing is a defect the unit tests cannot see).
//!
//! What is on the screen, and why:
//!
//! * the **settings window on the page of the plugins** (the tab strip is a real `SysTabControl32`, so the probe asks
//!   it to select the page and tells the window about it, the way the mouse would);
//! * the **list of the candidates in the panel of the laser interface** - there is no floating window next to the caret
//!   any more (the Sogou-style window was in the way of the code the user writes), so the list is drawn in the panel
//!   that hangs on the mouth of the light;
//! * the **beam** of the light, fired at the place where a character was written.
//!
//! The probe writes what it found into `02_Verification/ime-ui/probe-report.json`: the check script reads it and
//! refuses a picture that was taken with an empty strip of pages or without a list in the panel (a picture alone cannot
//! say whether the pages are there - they are only a few pixels tall).
//!
//! Nothing is registered, no profile is activated, no text is inserted anywhere.

use pime_tsf::imesettings::ImeSettings;
use pime_tsf::laser::PanelItem;
use pime_tsf::laser_candidates::LaserCandidates;
use pime_tsf::laser_ui::{LaserEvent, LaserUi};
use pime_tsf::settings_window::{SettingsEvent, SettingsPage, SettingsWindow, PAGES};
use pime_tsf::ui::{Anchor, CandidateRow, CandidateUi, CandidateView};
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DispatchMessageW, GetDlgItem, GetSystemMetrics, GetWindowRect, HWND_TOPMOST, PeekMessageW,
    PostQuitMessage, SendMessageW, SetWindowPos, TranslateMessage, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOMOVE, SWP_NOSIZE, WM_NOTIFY, WM_QUIT,
};

/// The messages of the tab control (the `windows` crate does not bind them: its `Win32_UI_Controls` module is enormous
/// and rustc runs out of stack on it).
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_GETITEMCOUNT: u32 = 0x1304;
/// The notification of the strip of the pages that says which page is now shown (-551).
const TCN_SELCHANGE: u32 = 0xFFFF_FDD9;
/// The id of the strip of the pages (the one of the settings window).
const ID_TABS: usize = 9004;

/// The header of a notification (`NMHDR`).
#[repr(C)]
struct NmHdr {
    /// The window that sent the notification (the tab control).
    hwnd_from: HWND,
    /// The id of the control that sent it.
    id_from: usize,
    /// What happened.
    code: u32,
}

/// What the probe found. It is written next to the picture, so that the check script can read it.
struct Report {
    /// Whether the settings window was opened.
    settings_window: bool,
    /// How many pages the strip of the settings window has.
    settings_pages: i32,
    /// The page that is shown (the index in the order of the pages).
    selected_page: i32,
    /// The title of the page that is shown.
    selected_page_title: &'static str,
    /// How many rows the list in the panel of the light has.
    candidate_rows: usize,
    /// Where the settings window is on the screen (left, top, right, bottom).
    settings_rect: [i32; 4],
    /// The size of the screen.
    screen: [i32; 2],
}

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

    // ---- the settings window (on the page of the plugins) ---------------------------
    let sink: Rc<dyn Fn(SettingsEvent)> = Rc::new(|_: SettingsEvent| {});
    let settings = SettingsWindow::open(ImeSettings::default(), sink);
    let (pages, selected) = match settings.as_ref() {
        Some(window) => {
            let pages = select_page(window.handle(), SettingsPage::Plugins);
            (pages.0, pages.1)
        }
        None => (0, -1),
    };

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
            PanelItem::Menu {
                key: "language",
                label: "语言",
                value: "中文".to_string(),
            },
            PanelItem::Menu {
                key: "keymap",
                label: "按键方案",
                value: "程序员".to_string(),
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

    // ---- the list of the candidates, in the panel of the light ----------------------
    // The list is not a window next to the caret any more: it is drawn in the panel of the laser interface.
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
    let mut list = LaserCandidates::new(Rc::clone(&laser));
    let list_shown = list.show(&view, anchor);
    let list_rows = laser.borrow().candidate_rows().len();

    let report = Report {
        settings_window: settings.is_some(),
        settings_pages: pages,
        selected_page: selected,
        selected_page_title: page_title_of(selected),
        candidate_rows: list_rows,
        settings_rect: settings_rect(settings.as_ref().map(|w| w.handle())),
        screen: [screen_w, screen_h],
    };
    write_report(&report);

    println!("screen: {screen_w}x{screen_h}");
    println!(
        "settings window: {}",
        if report.settings_window {
            "shown"
        } else {
            "NOT SHOWN"
        }
    );
    println!(
        "pages of the settings: {} (page {}: {})",
        report.settings_pages, report.selected_page, report.selected_page_title
    );
    println!(
        "candidate list in the laser panel: {} ({} rows)",
        if list_shown { "shown" } else { "NOT SHOWN" },
        list_rows
    );
    println!("READY");
    // The windows stay on screen while the screenshot is taken. The messages of this thread are pumped, because the
    // animation of the laser interface is driven by a timer.
    pump_for(seconds, settings.as_ref().map(|w| w.handle()));
    println!("done");
    // The probe fails when the windows are not what they must be: the gate then refuses the picture.
    if report.settings_pages != PAGES.len() as i32 || report.candidate_rows == 0 {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

/// The title of the page with the index (the empty text when the index is not one of ours).
fn page_title_of(index: i32) -> &'static str {
    if index < 0 {
        return "";
    }
    PAGES
        .get(index as usize)
        .map(|page| pime_tsf::settings_window::page_title(*page))
        .unwrap_or("")
}

/// Where the settings window is on the screen.
fn settings_rect(hwnd: Option<HWND>) -> [i32; 4] {
    let Some(hwnd) = hwnd else {
        return [0, 0, 0, 0];
    };
    let mut rect = RECT::default();
    // SAFETY: a plain read of the rectangle of our own window.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok();
    [rect.left, rect.top, rect.right, rect.bottom]
}

/// Put the settings window on a page, the way the mouse would.
///
/// The strip of the pages is a real tab control: the probe asks it to select the page and then tells the window about
/// the change, the way the control does when the user clicks a tab.
///
/// Returns how many pages the strip has and which page is shown now.
fn select_page(hwnd: HWND, page: SettingsPage) -> (i32, i32) {
    let Some(index) = PAGES.iter().position(|p| *p == page) else {
        return (0, -1);
    };
    // SAFETY: a plain read of a child window by its id.
    let tabs = unsafe { GetDlgItem(hwnd, ID_TABS as i32) }.unwrap_or_default();
    if tabs.is_invalid() {
        return (0, -1);
    }
    // SAFETY: the tab control is a child of our window; the message only changes which tab is selected.
    unsafe {
        SendMessageW(tabs, TCM_SETCURSEL, WPARAM(index as usize), LPARAM(0));
    }
    let header = NmHdr {
        hwnd_from: tabs,
        id_from: ID_TABS,
        code: TCN_SELCHANGE,
    };
    // SAFETY: the header is a live local and the message reads it during the call.
    unsafe {
        SendMessageW(
            hwnd,
            WM_NOTIFY,
            WPARAM(ID_TABS),
            LPARAM((&header as *const NmHdr) as isize),
        );
        let count = SendMessageW(tabs, TCM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0;
        let current = SendMessageW(tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0;
        (count as i32, current as i32)
    }
}

/// Write what the probe found, next to the picture the check script reads.
fn write_report(report: &Report) {
    let path = std::path::Path::new("E:/IME1.0/02_Verification/ime-ui/probe-report.json");
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let text = format!(
        "{{\n  \"settingsWindow\": {},\n  \"settingsPages\": {},\n  \"selectedPage\": {},\n  \"selectedPageTitle\": \"{}\",\n  \"candidateRows\": {},\n  \"settingsRect\": [{}, {}, {}, {}],\n  \"screen\": [{}, {}]\n}}\n",
        report.settings_window,
        report.settings_pages,
        report.selected_page,
        report.selected_page_title,
        report.candidate_rows,
        report.settings_rect[0],
        report.settings_rect[1],
        report.settings_rect[2],
        report.settings_rect[3],
        report.screen[0],
        report.screen[1]
    );
    let _ = std::fs::write(path, text);
}

/// Pump the messages of this thread for `seconds` (the timer of the laser interface needs them).
///
/// The settings window is put on top again every half second: the user is working on the same screen, and a window
/// that is pushed behind the window of the user cannot be judged (and cannot be shown in the picture).
fn pump_for(seconds: u64, settings: Option<HWND>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut next_raise = std::time::Instant::now();
    loop {
        if std::time::Instant::now() >= deadline {
            return;
        }
        if let Some(hwnd) = settings {
            let now = std::time::Instant::now();
            if now >= next_raise {
                next_raise = now + std::time::Duration::from_millis(500);
                // SAFETY: our own window; the flags keep it where it is.
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE,
                    );
                    let _ = BringWindowToTop(hwnd);
                }
            }
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
