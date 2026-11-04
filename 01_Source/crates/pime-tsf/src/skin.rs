//! The colours of the product: the panel of the laser interface and the menus that hang on it.
//!
//! The floating bar is gone (the laser interface at the left of the screen is the one floating interface of the
//! product), but its colours are still the ones of the product: "laser" is the look of the laser interface, "system"
//! follows the theme of Windows, the other two are fixed.

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_WINDOWFRAME,
};

use pime_core::style::LaserTheme;

use crate::popup::colour;

/// The colours of the panel and of its menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinColors {
    pub background: COLORREF,
    pub text: COLORREF,
    pub dim: COLORREF,
    pub highlight: COLORREF,
    pub highlight_text: COLORREF,
    pub frame: COLORREF,
}

/// The colours of a skin: "laser" is the look of the product, "system" follows the theme of Windows, the other two are
/// fixed.
pub fn skin_colors(skin: &str) -> SkinColors {
    match skin {
        // 0x00BBGGRR
        "laser" => SkinColors {
            background: COLORREF(LaserTheme::PANEL.to_bgr()),
            text: COLORREF(LaserTheme::TEXT.to_bgr()),
            dim: COLORREF(LaserTheme::DIM.to_bgr()),
            highlight: COLORREF(LaserTheme::ACCENT.to_bgr()),
            highlight_text: COLORREF(LaserTheme::ON_LIGHT.to_bgr()),
            frame: COLORREF(LaserTheme::RIM.to_bgr()),
        },
        "light" => SkinColors {
            background: COLORREF(0x00F7F3EC),
            text: COLORREF(0x00201A15),
            dim: COLORREF(0x00706860),
            highlight: COLORREF(0x00C8DCF0),
            highlight_text: COLORREF(0x00201A15),
            frame: COLORREF(0x00A8A196),
        },
        "dark" => SkinColors {
            background: COLORREF(0x00262320),
            text: COLORREF(0x00EDE7DF),
            dim: COLORREF(0x00A79E93),
            highlight: COLORREF(0x005A6E82),
            highlight_text: COLORREF(0x00EDE7DF),
            frame: COLORREF(0x00555048),
        },
        _ => SkinColors {
            background: colour(COLOR_BTNFACE),
            text: colour(COLOR_BTNTEXT),
            dim: colour(COLOR_GRAYTEXT),
            highlight: colour(COLOR_HIGHLIGHT),
            highlight_text: colour(COLOR_HIGHLIGHTTEXT),
            frame: colour(COLOR_WINDOWFRAME),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skins_have_readable_colours() {
        for skin in ["system", "light", "dark", "laser"] {
            let colors = skin_colors(skin);
            assert_ne!(colors.background, colors.text, "{skin}");
            assert_ne!(colors.highlight, colors.highlight_text, "{skin}");
        }
    }
}
