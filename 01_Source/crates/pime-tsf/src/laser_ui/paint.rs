//! The painters of the laser interface: the glow (the slit of light at the mouth), the beam that shoots out of it
//! and the panel with its rows.
//!
//! Every painter gets the whole picture of its window and not just its pixels, because the text of the panel is
//! drawn with GDI (there is no other way to draw a glyph) and GDI does not write the alpha byte: the picture has to
//! be told which pixels the text touched.

use pime_core::style::{BRAND_RAINBOW, Canvas, LaserTheme, PixelRect, Rgb, mix};

use crate::glass::Model;
use crate::laser::{Mouth, PanelItem, mouth_slit};
use crate::layout::{Rect, TextMetrics};
use crate::paint::LayeredSurface;

use super::{PANEL_HANDLE, PANEL_ROW_HEIGHT};

/// How long the light of the mouth takes to breathe once (in milliseconds).
const SLIT_PULSE_MS: u64 = 2600;

/// How deep the gradient of the brand rainbow along the top of the panel reaches.
const PANEL_RAINBOW_BAND: f64 = 9.0;

/// The glow of one mouth of light: the slit of light in the colour of its source.
pub(super) fn paint_glow(surface: &mut LayeredSurface, model: &Model, key: &str) {
    let Some(state) = model.mouth_of(key) else {
        return;
    };
    let Some(mut canvas) = surface.canvas() else {
        return;
    };
    draw_glow(
        &mut canvas,
        state.mouth,
        state.window,
        model.now_ms,
        state.colour,
        state.halo,
    );
}

/// The picture of the glow: a thin slit of pale blue light at the mouth.
///
/// The mouth is drawn **inside** the window: the position of the mouth on the screen is turned into a position inside
/// the picture first. Without that the light would be drawn outside of the window, and the mouth would not be seen.
///
/// The user asked for the mouth to be "barely there, a small piece of pale blue, half transparent, not loud but
/// visible" ("若隐若现有一小截淡蓝色半透明激光"), so it is a hairline of pale blue with a soft halo around it
/// and nothing else: no ring of the brand colours, no white core. It breathes slowly, so that the eye finds it
/// without it asking for attention.
pub(super) fn draw_glow(
    canvas: &mut Canvas,
    mouth: Mouth,
    origin: Rect,
    now_ms: u64,
    colour: Rgb,
    halo_colour: Rgb,
) {
    let slit = mouth_slit(&mouth);
    // The line is put on the centre of a pixel: a line of light that falls between two pixels would light two of them
    // and the light would be twice as wide as it should be.
    // The slit lies **on the edge of the screen** (the user asked for it on 2026-11-12), so the place of the light
    // inside the picture is the place of the edge, not the centre of the mouth.
    let centre_x = (slit.x - origin.left) as f64 + 0.5;
    let centre_y = (slit.y - origin.top) as f64 + 0.5;
    let half = slit.length as f64 / 2.0;
    let halo = slit.halo as f64;
    // The light breathes: it is there, but it does not demand the eye.
    let breathe = 0.78 + 0.22 * ((now_ms as f64 / SLIT_PULSE_MS as f64) * std::f64::consts::TAU).sin();
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            // How far the pixel is from the line of the slit, and how far along it it is.
            let (across, along) = if slit.vertical {
                ((px - centre_x).abs(), (py - centre_y).abs())
            } else {
                ((py - centre_y).abs(), (px - centre_x).abs())
            };
            if along > half {
                continue;
            }
            // The two ends of the slit fade away: the light does not stop with a hard edge.
            let ends = (1.0 - (along / half).powi(3)).clamp(0.0, 1.0);
            // The halo: the blue that leaks into the air around the line.
            let halo_cover = (1.0 - across / halo).clamp(0.0, 1.0);
            if halo_cover > 0.0 {
                canvas.blend(
                    x,
                    y,
                    halo_colour,
                    halo_cover * halo_cover * 0.26 * breathe * ends,
                );
            }
            // The core: a hairline of pale blue, translucent like everything else.
            let core_cover = (1.0 - across / slit.width as f64).clamp(0.0, 1.0);
            if core_cover > 0.0 {
                canvas.blend(x, y, colour, core_cover * 0.78 * breathe * ends);
            }
        }
    }
}

/// The beam that shoots out of the mouth.
pub(super) fn paint_beam(surface: &mut LayeredSurface, model: &Model, _key: &str) {
    let Some(beam) = model.beam else {
        return;
    };
    let origin = model.beam_window;
    let from_x = (beam.from.0 - origin.left) as f64;
    let from_y = (beam.from.1 - origin.top) as f64;
    let target_x = (beam.to.0 - origin.left) as f64;
    let target_y = (beam.to.1 - origin.top) as f64;
    let progress = beam.progress(model.now_ms);
    let strength = beam.strength(model.now_ms);
    if strength <= 0.0 {
        return;
    }
    let Some(mut canvas) = surface.canvas() else {
        return;
    };
    // the beam has the colour of the mouth that fired it
    canvas.fill_beam_coloured(
        from_x,
        from_y,
        target_x,
        target_y,
        progress,
        strength,
        beam.colour,
    );
    // the light that hits the character is in the same colour
    if progress >= 1.0 {
        canvas.fill_glow_coloured(target_x, target_y, 26.0, strength * 0.8, beam.colour);
    }
}

/// The panel with the settings.
pub(super) fn paint_panel(surface: &mut LayeredSurface, model: &Model, _key: &str) {
    let panel = &model.panel;
    if !panel.open {
        return;
    }
    let reveal = panel.reveal;
    let font = model.font;
    {
        let Some(mut canvas) = surface.canvas() else {
            return;
        };
        let height = canvas.height as f64;
        // the glass of the panel
        let rect = PixelRect::new(0, 0, canvas.width, canvas.height);
        canvas.fill_shadow(
            rect,
            16,
            LaserTheme::PAGE,
            pime_core::style::Shadow {
                blur: 18,
                offset_x: 0,
                offset_y: 6,
                strength: 0.45,
            },
        );
        canvas.fill_rounded(rect, 16, LaserTheme::PANEL, LaserTheme::MATERIAL_ALPHA);
        // The rim of the brand rainbow along the top: a **gradient**, not a stripe. The whole rainbow runs across
        // the edge of the panel and fades out downwards, clipped by the rounded outline of the panel.
        canvas.fill_rainbow_edge(rect, 16, PANEL_RAINBOW_BAND, &BRAND_RAINBOW, 0.8);
        // the rows: the light wipes them open from the top
        let scan = reveal * height;
        for (index, (item, rect)) in panel.layout.rows.iter().enumerate() {
            let row_y = rect.top as f64 + rect.height() as f64 * 0.5;
            if row_y > scan {
                break;
            }
            let hot = panel.hot == Some(index);
            // the candidate that is highlighted is lit as well: the user has to see at once which one
            // the digits and the Enter key would take
            let selected = matches!(item, PanelItem::Candidate(candidate) if candidate.selected);
            if hot || selected {
                canvas.fill_rounded(
                    PixelRect::new(
                        rect.left - 6,
                        rect.top - 2,
                        rect.right + 6,
                        rect.bottom + 2,
                    ),
                    8,
                    LaserTheme::PANEL_LIT,
                    if selected { 0.7 } else { 0.9 },
                );
            }
            // the switch of a toggle
            if let PanelItem::Toggle { on, .. } = item {
                let switch_right = rect.right - 4;
                let switch_left = switch_right - 34;
                let centre_y = row_y;
                let shape = PixelRect::new(
                    switch_left,
                    centre_y as i32 - 9,
                    switch_right,
                    centre_y as i32 + 9,
                );
                canvas.fill_rounded(shape, 9, LaserTheme::PAGE, 0.85);
                let knob_x = if *on { switch_right - 9 } else { switch_left + 9 };
                let knob = if *on {
                    LaserTheme::ACCENT
                } else {
                    LaserTheme::DIM
                };
                canvas.fill_circle(knob_x as f64, centre_y, 6.5, knob, 0.95);
                if *on {
                    canvas.fill_glow(knob_x as f64, centre_y, 14.0, 0.5);
                }
            }
        }
    }
    // the text of the rows (GDI: there is no other way to draw a glyph)
    if let Some(font) = font {
        let layout = panel.layout.clone();
        let metrics = crate::popup::GdiMetrics::new(font);
        let height = surface.height() as f64;
        for (index, (item, rect)) in layout.rows.iter().enumerate() {
            let row_y = rect.top as f64 + rect.height() as f64 * 0.5;
            let scan = reveal * height;
            if row_y > scan {
                break;
            }
            let hot = panel.hot == Some(index);
            if let PanelItem::Candidate(candidate) = item {
                // a candidate: the number that takes it, the text of it, and where it comes from
                let colour = if candidate.selected || hot {
                    LaserTheme::ACCENT
                } else {
                    LaserTheme::TEXT
                };
                surface.draw_text(
                    font,
                    LaserTheme::DIM,
                    rect.left,
                    rect.top + 6,
                    &candidate.label,
                );
                let number = metrics
                    .as_ref()
                    .map(|metrics| metrics.text_width(&candidate.label))
                    .unwrap_or(0);
                surface.draw_text(
                    font,
                    colour,
                    rect.left + number + 10,
                    rect.top + 6,
                    &candidate.text,
                );
                let detail = &candidate.detail;
                let width = metrics
                    .as_ref()
                    .map(|metrics| metrics.text_width(detail))
                    .unwrap_or(0);
                surface.draw_text(
                    font,
                    LaserTheme::DIM,
                    rect.right - width,
                    rect.top + 6,
                    detail,
                );
                continue;
            }
            let colour = if hot {
                LaserTheme::ACCENT
            } else {
                LaserTheme::TEXT
            };
            surface.draw_text(font, colour, rect.left, rect.top + 6, item.label());
            if let PanelItem::Toggle { .. } = item {
                // the switch carries its own state; the text is only the name of it
            } else {
                let value = item.value();
                let width = metrics
                    .as_ref()
                    .map(|metrics| metrics.text_width(&value))
                    .unwrap_or(0);
                surface.draw_text(
                    font,
                    LaserTheme::DIM,
                    rect.right - width,
                    rect.top + 6,
                    &value,
                );
            }
            let _ = index;
        }
        // The status of the panel: the one line under the rows, in the room that is left for it.
        if let Some(status) = layout.status.as_deref() {
            let left = layout
                .rows
                .first()
                .map(|(_, rect)| rect.left)
                .unwrap_or(PANEL_HANDLE);
            let top = PANEL_HANDLE + layout.rows.len() as i32 * PANEL_ROW_HEIGHT + 4;
            surface.draw_text(font, LaserTheme::DIM, left, top, status);
        }
    }
}

/// The colour of the light, in one place (the core has the mix already).
#[allow(dead_code)]
fn light_colour(t: f64) -> Rgb {
    mix(LaserTheme::GLOW_MID, LaserTheme::GLOW_EDGE, t)
}
