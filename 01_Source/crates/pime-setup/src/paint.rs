//! Painting with smooth gradients and anti-aliased edges.
//!
//! The wizard draws itself, so the shapes are drawn here as well. GDI is not enough for that: `RoundRect` and
//! `Ellipse` are stair-stepped (there is no anti-aliasing), and a GDI gradient is a stack of flat bands, so the
//! colour changes in steps that the eye sees. This module keeps a 32-bit pixel buffer (a GDI DIB section, so every
//! pixel can be addressed) and draws into it:
//!
//! * every edge is covered by an **analytic** coverage function (the distance from the pixel to the outline of the
//!   shape), so the edges are smooth instead of stair-stepped;
//! * every gradient is computed **per pixel** in **linear light**, so there are no steps in the colour and the mix of
//!   two colours looks like the mix of two lights and not the mix of two numbers;
//! * a shadow is the same shape with a penumbra, which is what gives the window its depth.
//!
//! Nothing here needs a library that the program does not already use: the buffer is a GDI DIB section and every shape
//! is computed by this module.
//!
//! **The shapes themselves live in the core** (`pime_core::style`): the installer and the text service are two
//! separate programs, and both of them draw in the laser-rainbow style of the product, so the shapes, the colours
//! and the curves must be the *same code* and not two copies of it. What is left here is the part that is Windows:
//! the DIB, the DC and the `COLORREF` conversions.

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{COLORREF, HANDLE, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};

pub use pime_core::style::{
    BRAND_RAINBOW, ChannelOrder, LaserTheme, PixelRect, Rgb, Shadow, coverage, darken,
    distance_to_rounded_box, distance_to_segment, laser_ramp, mix, scale, shadow_coverage,
    spline_sample,
};
use pime_core::style::Canvas;

/// The conversions between a colour and the `COLORREF` the rest of the program uses.
///
/// A `COLORREF` is `0x00BBGGRR`: the **red** is in the lowest byte and the **blue** in the highest one. Getting this
/// the wrong way round swaps red and blue everywhere, which is why it is written down in one place.
pub trait ColorRef {
    /// Take a colour apart from the `COLORREF` the rest of the program uses.
    fn from_colorref(value: COLORREF) -> Rgb;
    /// The `COLORREF` the rest of the program uses.
    fn to_colorref(self) -> COLORREF;
}

impl ColorRef for Rgb {
    fn from_colorref(value: COLORREF) -> Rgb {
        Rgb::from_bgr(value.0)
    }

    fn to_colorref(self) -> COLORREF {
        COLORREF(self.to_bgr())
    }
}

/// A 32-bit picture the wizard draws into.
///
/// The pixels are addressed one by one, which is what makes the smooth edges and the smooth gradients possible.
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Surface {
    /// A picture of `width` x `height` pixels that is compatible with `reference`.
    pub fn new(reference: HDC, width: i32, height: i32) -> Option<Surface> {
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
            Some(Surface {
                dc,
                bitmap,
                previous,
                bits: bits as *mut u8,
                width,
                height,
            })
        }
    }

    /// The DC the picture can be drawn into (the text and the icon are drawn with GDI as usual).
    pub fn dc(&self) -> HDC {
        self.dc
    }

    /// The picture as a canvas of the core (the shapes are the ones of the design language).
    fn canvas(&mut self) -> Option<Canvas<'_>> {
        let len = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        if self.bits.is_null() {
            return None;
        }
        // SAFETY: the DIB owns `len` bytes of pixel data, and the borrow of `self` keeps them alive.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        Canvas::with_order(self.width, self.height, pixels, ChannelOrder::Bgra)
    }

    /// Fill a rectangle with one colour.
    pub fn fill_rect(&mut self, rect: RECT, colour: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_rect(to_pixel_rect(rect), colour, 255);
    }

    /// Fill a rectangle with rounded corners (the edge is smooth).
    pub fn fill_rounded(&mut self, rect: RECT, radius: i32, colour: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_rounded(to_pixel_rect(rect), radius, colour, 1.0);
    }

    /// Draw the line around a rounded rectangle, `width` pixels wide, just inside its edge.
    pub fn stroke_rounded(&mut self, rect: RECT, radius: i32, colour: Rgb, width: i32) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.stroke_rounded(to_pixel_rect(rect), radius, colour, width, 1.0);
    }

    /// The shadow under a shape: the shape itself, moved a little down and to the right, with a soft edge that fades
    /// away around it.
    pub fn fill_shadow(&mut self, rect: RECT, radius: i32, colour: Rgb, shadow: Shadow) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_shadow(to_pixel_rect(rect), radius, colour, shadow);
    }

    /// Fill a circle (the edge is smooth).
    pub fn fill_circle(&mut self, centre_x: f64, centre_y: f64, radius: f64, colour: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_circle(centre_x, centre_y, radius, colour, 1.0);
    }

    /// Draw a line with a given width (the edge is smooth).
    pub fn stroke_segment(&mut self, ax: f64, ay: f64, bx: f64, by: f64, width: f64, colour: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.stroke_segment(ax, ay, bx, by, width, colour, 1.0);
    }

    /// Fill a rectangle with a gradient from top to bottom (computed for every pixel).
    pub fn fill_gradient_v(&mut self, rect: RECT, top: Rgb, bottom: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_gradient_v(to_pixel_rect(rect), top, bottom, 255);
    }

    /// Fill a rectangle with a vertical gradient from `top` to `bottom`, with rounded corners.
    ///
    /// The gradient is clipped to the rounded outline, so a button is a round shape that is lit from above.
    pub fn fill_gradient_rounded(&mut self, rect: RECT, radius: i32, top: Rgb, bottom: Rgb) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_gradient_rounded(to_pixel_rect(rect), radius, top, bottom, 255);
    }

    /// Fill a rectangle with the rainbow of the brand: a smooth ramp that runs through `stops`, with a vertical
    /// shading from `top_factor` to `bottom_factor` (both in linear light).
    pub fn fill_rainbow(&mut self, rect: RECT, stops: &[Rgb], top_factor: f64, bottom_factor: f64) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.fill_rainbow(
            to_pixel_rect(rect),
            stops,
            top_factor,
            bottom_factor,
            255,
        );
    }

    /// Make every pixel of the picture opaque (GDI can leave the alpha byte of the pixels it drew at zero; the
    /// picture is shown with `BitBlt`, which ignores the alpha, but an opaque picture keeps the text that is drawn
    /// later from blending against a transparent background).
    pub fn make_opaque(&mut self) {
        let Some(mut canvas) = self.canvas() else {
            return;
        };
        canvas.make_opaque();
    }
}

/// The rectangle of the core, from the one of Windows.
fn to_pixel_rect(rect: RECT) -> PixelRect {
    PixelRect::new(rect.left, rect.top, rect.right, rect.bottom)
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the objects are ours; the bitmap must leave the DC before it can be deleted.
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}
