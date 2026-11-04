//! The candidate list in the laser-rainbow style of the product.
//!
//! The list is a **layered window**: a 32-bit picture with an alpha channel per pixel is handed to
//! `UpdateLayeredWindow`, so the panel has soft corners, a shadow under it and a rim of the brand rainbow. GDI cannot
//! draw that: a window that is painted with GDI has hard edges, no transparency and no shadow.
//!
//! The shapes and the colours come from the core (`pime_core::style`) — the same code the install wizard and the glow
//! on the left of the screen use — so the whole product is one thing and cannot drift apart.
//!
//! Two rules of the design are kept:
//!
//! * the **selection is carried by text as well as by colour**: the `>` marker in front of the highlighted row is
//!   still there, so the list still works in a high-contrast theme and for a user who cannot tell the colours apart;
//! * the **text is drawn with GDI** (`LayeredSurface::draw_text`), because there is no other way to draw a glyph, and
//!   the picture is told afterwards which pixels the text touched (GDI does not write the alpha byte).

use pime_core::style::{BRAND_RAINBOW, LaserTheme, PixelRect, Rgb, Shadow};
use windows::Win32::Graphics::Gdi::HFONT;

use crate::layout::Layout;
use crate::paint::LayeredSurface;

/// The space around the list that the shadow and the soft corners need.
///
/// The window is this much bigger than the list on every side: the shadow falls outside the panel, and the corners of
/// the panel have to be able to fade out.
pub const PADDING: i32 = 14;

/// The radius of the corners of the list.
const RADIUS: i32 = 12;
/// The radius of the corners of the highlighted row.
const ROW_RADIUS: i32 = 7;
/// How deep the gradient of the brand rainbow along the top of the list reaches.
const RAINBOW_EDGE: f64 = 8.0;

/// The colours of the rows.
struct Palette {
    /// The text of a row that is not highlighted.
    text: Rgb,
    /// The text of the row that is highlighted.
    highlighted: Rgb,
    /// The text of the detail column (the kind of the candidate, the score).
    detail: Rgb,
    /// The marker in front of the highlighted row.
    marker: Rgb,
}

/// The colours of the laser-rainbow style.
fn palette() -> Palette {
    Palette {
        text: LaserTheme::TEXT,
        highlighted: LaserTheme::GLOW_CORE,
        detail: LaserTheme::DIM,
        marker: LaserTheme::ACCENT,
    }
}

/// Draw the candidate list into the picture of the window.
///
/// The picture must be the size of the window: the list plus [`PADDING`] on every side.
pub fn paint(surface: &mut LayeredSurface, layout: &Layout, selected: usize, font: HFONT) {
    if !layout.is_visible() {
        return;
    }
    let (w, h) = (layout.window.width(), layout.window.height());
    if w <= 0 || h <= 0 {
        return;
    }
    let pad = PADDING;
    let panel = PixelRect::new(pad, pad, pad + w, pad + h);
    let colours = palette();
    surface.clear();
    {
        let Some(mut canvas) = surface.canvas() else {
            return;
        };
        // the shadow the list casts on the page below it
        canvas.fill_shadow(
            panel,
            RADIUS,
            LaserTheme::PAGE,
            Shadow {
                blur: 12,
                offset_x: 0,
                offset_y: 5,
                strength: 0.55,
            },
        );
        // the glass of the list: 80% opaque, so the page below it shows through
        canvas.fill_rounded(panel, RADIUS, LaserTheme::PANEL, LaserTheme::MATERIAL_ALPHA);
        // the top of the list catches a little more light than the bottom
        canvas.fill_rounded(
            PixelRect::new(panel.left, panel.top, panel.right, panel.top + h / 2),
            RADIUS,
            LaserTheme::PANEL_LIT,
            0.35,
        );
        // The rim of the brand rainbow along the top of the list: the signature of the product.
        // It is a **gradient**, not a painted stripe: the whole rainbow runs across the edge of the panel and fades
        // out downwards, and the rounded outline of the panel clips it.
        canvas.fill_rainbow_edge(panel, RADIUS, RAINBOW_EDGE, &BRAND_RAINBOW, 0.85);
        // the line around the list
        canvas.stroke_rounded(panel, RADIUS, LaserTheme::RIM, 1, 0.5);
        // the highlighted row: it is lit, not just coloured
        let selected = selected.min(layout.rows.len().saturating_sub(1));
        for (index, row) in layout.rows.iter().enumerate() {
            if index != selected {
                continue;
            }
            let lit = PixelRect::new(
                row.left + pad,
                row.top + pad,
                row.right + pad,
                row.bottom + pad,
            );
            canvas.fill_rounded(lit, ROW_RADIUS, LaserTheme::PANEL_LIT, 0.95);
            // The bar of light on the left of the row: the **whole** rainbow of the brand, running down the bar.
            let bar = PixelRect::new(lit.left + 3, lit.top + 3, lit.left + 7, lit.bottom - 3);
            canvas.fill_rainbow_vertical(bar, 2, &BRAND_RAINBOW, 242);
            // the soft light that the bar throws around itself
            let glow = PixelRect::new(lit.left + 1, lit.top + 1, lit.left + 10, lit.bottom - 1);
            canvas.fill_rounded(glow, 4, colours.marker, 0.18);
        }
        // the line above the footer
        if let Some(footer) = layout.footer {
            canvas.fill_rect(
                PixelRect::new(panel.left + 1, footer.top + pad, panel.right - 1, footer.top + pad + 1),
                LaserTheme::RIM,
                110,
            );
        }
    }
    draw_text(surface, layout, selected, font, colours);
}

/// Draw the text of the list (the glyphs are GDI's work).
///
/// The columns and the rows of the layout are in the coordinates of the **list**; the picture of the window is
/// [`PADDING`] bigger on every side, so every position is moved by that padding.
fn draw_text(
    surface: &mut LayeredSurface,
    layout: &Layout,
    selected: usize,
    font: HFONT,
    colours: Palette,
) {
    let pad = PADDING;
    let columns = layout.columns;
    let selected = selected.min(layout.rows.len().saturating_sub(1));
    for (index, (row, text)) in layout.rows.iter().zip(&layout.texts).enumerate() {
        let on = index == selected;
        let y = row.top + (row.height() - layout.line_height) / 2 + pad;
        if on {
            surface.draw_text(font, colours.marker, columns.marker_x + pad, y, ">");
        }
        let body = if on {
            colours.highlighted
        } else {
            colours.text
        };
        for (content, x, colour) in [
            (&text.label, columns.label_x, body),
            (&text.text, columns.text_x, body),
            (&text.detail, columns.detail_x, colours.detail),
        ] {
            if !content.is_empty() {
                surface.draw_text(font, colour, x + pad, y, content);
            }
        }
    }
    if let (Some(footer), Some(content)) = (layout.footer, layout.footer_text.as_deref()) {
        let y = footer.top + (footer.height() - layout.line_height) / 2 + pad;
        surface.draw_text(font, colours.detail, columns.marker_x + pad, y, content);
    }
}
