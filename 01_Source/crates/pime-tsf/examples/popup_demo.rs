//! Shows the native candidate popup for a moment and, on request, saves ONLY the popup's own rectangle as a BMP.
//!
//!   cargo run --release --example popup_demo -- --ms 1500 --bmp <path>
//!
//! Side effects (this is the only place the popup ever appears without a text service driving it): one window, on the
//! primary monitor, for at most 2 seconds, which never takes focus; the capture reads the screen pixels of that window's
//! rectangle and nothing else. Nothing is registered and no other process is touched.
//!
//! The crate is a `cdylib`, so the example compiles the three modules it needs straight from `src/`.

#[allow(dead_code)]
#[path = "../src/layout.rs"]
mod layout;
#[allow(dead_code)]
#[path = "../src/popup.rs"]
mod popup;
#[allow(dead_code)]
#[path = "../src/ui.rs"]
mod ui;

use std::ffi::c_void;
use std::mem::size_of;
use std::time::{Duration, Instant};

use layout::Rect;
use ui::{Anchor, CandidateRow, CandidateUi, CandidateView};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

/// The longest the window may stay up, whatever `--ms` says.
const MAX_MS: u64 = 2000;
/// The popup is painted by the time its first messages have been pumped; capture a little later to be safe.
const CAPTURE_AFTER: Duration = Duration::from_millis(400);

fn row(label: &str, text: &str, detail: &str) -> CandidateRow {
    CandidateRow {
        label: label.to_string(),
        text: text.to_string(),
        detail: detail.to_string(),
    }
}

/// Page 2 of 3: five rows, the second one highlighted; one very long snippet (ellipsis) and a Chinese detail text.
fn sample_view() -> CandidateView {
    CandidateView {
        rows: vec![
            row("1", "fn main() { \u{2026} }", "rule \u{b7} valid"),
            row(
                "2",
                "fn main() -> Result<(), Box<dyn Error>> { \u{2026} }",
                "\u{4e0a}\u{4e0b}\u{6587}\u{5019}\u{9009} \u{b7} valid",
            ),
            row(
                "3",
                "async fn main() { \u{2026} }",
                "AI \u{b7} not validated",
            ),
            row(
                "4",
                "pub async fn handle_request(&self, request: Request<Body>, state: Arc<AppState>, \
                 cancel: CancellationToken) -> Result<Response<Body>, ServiceError> { \u{2026} }",
                "rule \u{b7} valid",
            ),
            row("5", "#[tokio::main]", "rule \u{b7} valid"),
        ],
        selected_in_page: 1,
        page: 1,
        page_count: 3,
    }
}

struct Args {
    ms: u64,
    bmp: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        ms: 1500,
        bmp: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--ms" => {
                let v = it.next().ok_or("--ms needs a number")?;
                args.ms = v.parse().map_err(|_| format!("bad --ms value: {v}"))?;
            }
            "--bmp" => args.bmp = Some(it.next().ok_or("--bmp needs a path")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    args.ms = args.ms.min(MAX_MS);
    Ok(args)
}

/// Delivers the window's own messages (paint) the way the host's message loop would.
fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a live local; the loop only dispatches messages of this thread's windows.
    unsafe {
        while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Copies the screen pixels of `rect` (the popup's own rectangle, nothing else) into a 24-bit BMP file.
fn capture(rect: Rect, path: &str) -> Result<(), String> {
    let (w, h) = (rect.width(), rect.height());
    if w <= 0 || h <= 0 {
        return Err("empty rectangle".to_string());
    }
    // SAFETY: a null window means the screen DC; the DC and bitmap below are released at the end of this function.
    let (screen, mem, bitmap) = unsafe {
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        (screen, mem, CreateCompatibleBitmap(screen, w, h))
    };
    // SAFETY: the handles were just created; only the rectangle of the popup is read from the screen.
    let (previous, copied) = unsafe {
        let previous = SelectObject(mem, HGDIOBJ(bitmap.0));
        (
            previous,
            BitBlt(mem, 0, 0, w, h, screen, rect.left, rect.top, SRCCOPY),
        )
    };
    // a bitmap must not be selected into a DC while GetDIBits reads it
    // SAFETY: restores the DC's original bitmap.
    unsafe { SelectObject(mem, previous) };
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: h, // positive: bottom-up rows, as BMP stores them
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: `bits` holds a w x h 32-bit image and outlives the call; `info` describes exactly that format.
    let lines = unsafe {
        GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            Some(bits.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    // SAFETY: releases what was created above, exactly once.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
    }
    copied.map_err(|e| format!("BitBlt: {e}"))?;
    if lines != h {
        return Err(format!("GetDIBits returned {lines} of {h} lines"));
    }
    std::fs::write(path, bmp_24(w as usize, h as usize, &bits)).map_err(|e| format!("{path}: {e}"))
}

/// A 24-bit bottom-up BMP from 32-bit BGRA rows (already bottom-up).
fn bmp_24(w: usize, h: usize, bgra: &[u8]) -> Vec<u8> {
    let stride = (w * 3).div_ceil(4) * 4;
    let image = stride * h;
    let mut out = Vec::with_capacity(54 + image);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + image) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // BI_RGB
    out.extend_from_slice(&(image as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    for y in 0..h {
        let row = &bgra[y * w * 4..(y + 1) * w * 4];
        for px in row.as_chunks::<4>().0 {
            out.extend_from_slice(&px[..3]);
        }
        out.resize(54 + (y + 1) * stride, 0);
    }
    out
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("popup_demo: {e}\nusage: popup_demo [--ms N (max {MAX_MS})] [--bmp path]");
            std::process::exit(2);
        }
    };
    // Real device pixels for the anchor, the layout and the capture.
    // SAFETY: a process-wide setting made before any window exists.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    // SAFETY: a plain query.
    let foreground_before = unsafe { GetForegroundWindow() };
    let mut popup = popup::PopupWindow::new();
    popup.show(
        &sample_view(),
        Anchor {
            x: 300,
            y: 200,
            line_height: 22,
        },
    );
    if popup.window_rect().is_none() {
        eprintln!("popup_demo: the popup could not be shown");
        std::process::exit(1);
    }

    let started = Instant::now();
    let end = started + Duration::from_millis(args.ms);
    let mut pending = args.bmp.clone();
    let mut failed = false;
    while Instant::now() < end {
        pump();
        if started.elapsed() >= CAPTURE_AFTER
            && let Some(path) = pending.clone()
            && let Some(rect) = popup.window_rect()
        {
            pending = None;
            match capture(rect, &path) {
                Ok(()) => println!(
                    "popup rectangle {}x{} at ({}, {}) saved to {path}",
                    rect.width(),
                    rect.height(),
                    rect.left,
                    rect.top
                ),
                Err(e) => {
                    eprintln!("popup_demo: capture failed: {e}");
                    failed = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // the popup is still up: it must not have become the foreground window
    // SAFETY: a plain query.
    if unsafe { GetForegroundWindow() } == foreground_before {
        println!("focus: the foreground window did not change while the popup was shown");
    } else {
        eprintln!("popup_demo: the foreground window CHANGED while the popup was shown");
        failed = true;
    }
    popup.hide();
    drop(popup); // destroys the window before the process ends
    pump();
    if pending.is_some() {
        eprintln!("popup_demo: --ms was too short to capture");
        failed = true;
    }
    std::process::exit(i32::from(failed));
}
