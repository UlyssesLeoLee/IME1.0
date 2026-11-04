//! The floating bar in the laser-rainbow style of the product.
//!
//! The bar is a **layered window**: a 32-bit picture with an alpha channel per pixel is handed to
//! `UpdateLayeredWindow`, so the bar has soft corners, a shadow under it and a rim of the brand rainbow. GDI cannot
//! draw that: a window that is painted with GDI has hard edges and no transparency.
//!
//! The bar still offers the other three skins ("system", "light", "dark"); they are drawn into the same picture, so
//! the window stays a layered window whatever the user chose.
//!
//! The shapes and the colours come from the core (`pime_core::style`) — the same code the install wizard, the glow on
//! the left of the screen and the candidate list use — so the whole product is one thing.

use pime_core::style::{BRAND_RAINBOW, LaserTheme, PixelRect, Rgb, Shadow};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::HFONT;

use crate::floating::{BarButtonKind, BarLayout, BarState, SkinColors};
use crate::paint::LayeredSurface;

/// The space around the bar that the shadow and the soft corners need.
pub const PADDING: i32 = 10;

/// The radius of the corners of the bar.
const RADIUS: i32 = 10;
/// The radius of the corners of the pill under the highlighted button.
const PILL_RADIUS: i32 = 6;
/// How deep the gradient of the brand rainbow along the top of the bar reaches.
const RAINBOW_EDGE: f64 = 6.0;

/// Draw the bar into the picture of the window.
///
/// The picture must be the size of the window: the bar plus [`PADDING`] on every side.
pub fn paint(
    surface: &mut LayeredSurface,
    layout: &BarLayout,
    state: &BarState,
    font: HFONT,
    skin: &SkinColors,
) {
    let (w, h) = (layout.width, layout.height);
    if w <= 0 || h <= 0 {
        return;
    }
    let pad = PADDING;
    let panel = PixelRect::new(pad, pad, pad + w, pad + h);
    let laser = state.skin == "laser";
    surface.clear();
    {
        let Some(mut canvas) = surface.canvas() else {
            return;
        };
        if laser {
            // the shadow the bar casts on the page below it
            canvas.fill_shadow(
                panel,
                RADIUS,
                LaserTheme::PAGE,
                Shadow {
                    blur: 10,
                    offset_x: 0,
                    offset_y: 4,
                    strength: 0.5,
                },
            );
            // the glass of the bar: 80% opaque, so the page below it shows through
            canvas.fill_rounded(panel, RADIUS, LaserTheme::PANEL, LaserTheme::MATERIAL_ALPHA);
            canvas.fill_rounded(
                PixelRect::new(panel.left, panel.top, panel.right, panel.top + h / 2),
                RADIUS,
                LaserTheme::PANEL_LIT,
                0.3,
            );
            // The rim of the brand rainbow along the top: the signature of the product.
            // A **gradient**: the whole rainbow runs across the edge of the bar and fades out downwards.
            canvas.fill_rainbow_edge(panel, RADIUS, RAINBOW_EDGE, &BRAND_RAINBOW, 0.85);
            // the line around the bar
            canvas.stroke_rounded(panel, RADIUS, LaserTheme::RIM, 1, 0.5);
        } else {
            // the other skins are solid bars of their own colour
            canvas.fill_rounded(panel, RADIUS, to_rgb(skin.background), 1.0);
            canvas.stroke_rounded(panel, RADIUS, to_rgb(skin.frame), 1, 1.0);
        }
        // the pill under the button that is on
        for button in &layout.buttons {
            if button.kind != BarButtonKind::Mode || !state.input_mode {
                continue;
            }
            let lit = PixelRect::new(
                button.rect.left + pad,
                button.rect.top + pad,
                button.rect.right + pad,
                button.rect.bottom + pad,
            );
            if laser {
                // The pill under the mode button is a **gradient** of the brand rainbow: the whole rainbow runs
                // across it, so the button that is on glows with the colours of the logo.
                canvas.fill_rainbow_rounded(lit, PILL_RADIUS, &BRAND_RAINBOW, 1.0, 0.92, 240);
            } else {
                canvas.fill_rounded(lit, PILL_RADIUS, to_rgb(skin.highlight), 0.95);
            }
        }
    }
    // The text is drawn with GDI (there is no other way to draw a glyph).
    let (title_colour, button_colour) = if laser {
        (LaserTheme::TEXT, LaserTheme::TEXT)
    } else {
        (to_rgb(skin.text), to_rgb(skin.text))
    };
    let highlighted_text = if laser {
        LaserTheme::ON_LIGHT
    } else {
        to_rgb(skin.highlight_text)
    };
    surface.draw_text(font, title_colour, layout.title.left + pad, layout.title.top + pad, &state.title());
    for button in &layout.buttons {
        let on = button.kind == BarButtonKind::Mode && state.input_mode;
        let colour = if on { highlighted_text } else { button_colour };
        surface.draw_text(
            font,
            colour,
            button.rect.left + pad,
            button.rect.top + pad,
            state.label(button.kind),
        );
    }
}

/// The colour of a `COLORREF` (0x00BBGGRR) as the core wants it (r, g, b).
fn to_rgb(value: COLORREF) -> Rgb {
    Rgb::from_bgr(value.0)
}
