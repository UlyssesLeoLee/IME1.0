//! The Windows layer of the laser-rainbow style: a layered window that is painted pixel by pixel.
//!
//! GDI cannot draw a glow: a window that is painted with GDI has hard edges and no transparency, and a beam of light
//! is nothing but transparency. So the windows of this style (the glow on the left, the beam, the laser panel, the
//! logo in the corner) are **layered windows**: a 32-bit picture with an alpha channel per pixel is handed to
//! `UpdateLayeredWindow`, which puts it on the screen with that alpha.
//!
//! The picture itself is a [`pime_core::style::Canvas`] — the same canvas the installer draws the wizard with — so
//! the shapes, the colours and the curves of the product are literally the same code in both programs.
//!
//! One Windows detail matters here: GDI writes text into a 32-bit DIB without touching the alpha byte, so a glyph
//! would come out fully transparent. [`LayeredSurface::draw_text`] therefore remembers the picture before the text is
//! drawn and gives every pixel the text touched an alpha of 255 afterwards.

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, ExtTextOutW,
    HBITMAP, HDC, HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use pime_core::style::{Canvas, ChannelOrder, Rgb};

use crate::popup::Selection;

/// A 32-bit picture that is shown on the screen with its own alpha (a layered window).
pub struct LayeredSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl LayeredSurface {
    /// A picture of `width` x `height` pixels that is compatible with `reference`.
    pub fn new(reference: HDC, width: i32, height: i32) -> Option<LayeredSurface> {
        if width <= 0 || height <= 0 {
            return None;
        }
        // SAFETY: every call below is a plain GDI call on objects this function owns.
        unsafe {
            let dc = CreateCompatibleDC(reference);
            if dc.is_invalid() {
                return None;
            }
            let mut info = BITMAPINFO::default();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            // a negative height means the first row of the buffer is the top row of the picture
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut c_void = ptr::null_mut();
            let bitmap = match CreateDIBSection(
                reference,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                HANDLE(ptr::null_mut()),
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            Some(LayeredSurface {
                dc,
                bitmap,
                previous,
                bits: bits as *mut u8,
                width,
                height,
            })
        }
    }

    /// How wide the picture is.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// How tall the picture is.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// The picture as a canvas of the core: the shapes are the ones of the design language.
    pub fn canvas(&mut self) -> Option<Canvas<'_>> {
        if self.bits.is_null() {
            return None;
        }
        let len = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        // SAFETY: the DIB owns `len` bytes of pixel data, and the borrow of `self` keeps them alive.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        Canvas::with_order(self.width, self.height, pixels, ChannelOrder::Bgra)
    }

    /// Make every pixel of the picture transparent.
    pub fn clear(&mut self) {
        if let Some(mut canvas) = self.canvas() {
            canvas.clear();
        }
    }

    /// Put the picture on the screen at (`screen_x`, `screen_y`).
    ///
    /// The window must have the `WS_EX_LAYERED` style; this is the call that puts a picture with an alpha channel
    /// on the screen.
    pub fn upload(&self, hwnd: HWND, screen_x: i32, screen_y: i32) -> bool {
        let destination = POINT { x: screen_x, y: screen_y };
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: the memory DC holds the picture, the size and the point describe it, and the window is ours.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                HDC::default(),
                Some(&destination),
                Some(&size),
                self.dc,
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .is_ok()
        }
    }

    /// Draw a line of text with GDI and keep it visible.
    ///
    /// GDI does not write the alpha byte, so every pixel the text changed gets an alpha of 255 afterwards (the
    /// pixels it did not touch keep the alpha they had).
    pub fn draw_text(&mut self, font: windows::Win32::Graphics::Gdi::HFONT, colour: Rgb, x: i32, y: i32, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(before) = self.snapshot() else {
            return;
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return;
        }
        let count = u32::try_from(wide.len()).unwrap_or(0);
        // SAFETY: the DC is live, `wide` holds `count` UTF-16 units and outlives the call.
        unsafe {
            let _font = Selection::new(self.dc, HGDIOBJ(font.0));
            let _ = SetBkMode(self.dc, TRANSPARENT);
            let _ = SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(colour.to_bgr()));
            let _ = ExtTextOutW(
                self.dc,
                x,
                y,
                windows::Win32::Graphics::Gdi::ETO_OPTIONS(0),
                None,
                windows::core::PCWSTR(wide.as_ptr()),
                count,
                None,
            );
        }
        self.give_alpha_to_what_changed(&before);
    }

    /// A copy of the picture (the pixels, not the alpha of the text).
    fn snapshot(&self) -> Option<Vec<u8>> {
        if self.bits.is_null() {
            return None;
        }
        let len = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        // SAFETY: the DIB owns `len` bytes of pixel data.
        let slice = unsafe { std::slice::from_raw_parts(self.bits, len) };
        Some(slice.to_vec())
    }

    /// Give every pixel the text changed an alpha of 255; the others keep the alpha they had.
    fn give_alpha_to_what_changed(&mut self, before: &[u8]) {
        if self.bits.is_null() {
            return;
        }
        let len = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        // SAFETY: the DIB owns `len` bytes of pixel data, and `before` has the same length.
        unsafe {
            let now = std::slice::from_raw_parts_mut(self.bits, len);
            for index in (0..len.min(before.len())).step_by(4) {
                let changed = now[index] != before[index]
                    || now[index + 1] != before[index + 1]
                    || now[index + 2] != before[index + 2];
                if changed {
                    now[index + 3] = 255;
                } else {
                    now[index + 3] = before[index + 3];
                }
            }
        }
    }
}

impl Drop for LayeredSurface {
    fn drop(&mut self) {
        // SAFETY: the objects are ours; the bitmap must leave the DC before it can be deleted.
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// The rectangle of a window, in screen coordinates (`None` when the handle is not a window).
pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    // SAFETY: a plain read of the window rectangle.
    let mut rect = RECT::default();
    // SAFETY: the call only writes into the local rectangle.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}
