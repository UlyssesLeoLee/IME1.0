//! The logo of the input method in the corner of the screen and the resident tray it lives in.
//!
//! The logo is drawn at run time with the same design language as the rest of the product (`pime_core::style`): the
//! rainbow of the brand in a disc with the light of the laser around it. Nothing is loaded from disk, so there is no
//! resource to keep in step with the code.
//!
//! When one of the languages of the product is the active one, the disc carries its mark in the middle (`中`, `日`,
//! `EN`): the user sees at a glance which language the input method is in, the way Sogou does it. The mark is drawn
//! with GDI text, because the pixel canvas of the core has no glyphs. A language that is not one of ours gets no
//! mark at all: the tray must never show a language that is not the active one.
//!
//! The tray itself is the Windows notification area (the place where Sogou keeps its logo too): the system gives it
//! the place, so it can never cover the clock.

use std::ffi::c_void;
use std::ptr;

use pime_core::style::{mix, BRAND_RAINBOW, Canvas, LaserTheme, Rgb};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HANDLE, SIZE};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    ETO_OPTIONS, ExtTextOutW, FF_SWISS, FW_BOLD, GGI_MARK_NONEXISTING_GLYPHS, GetGlyphIndicesW,
    GetTextExtentPoint32W, GetTextMetricsW, HBITMAP, HDC, HGDIOBJ, HFONT, OUT_TT_PRECIS, SelectObject, SetBkMode,
    SetTextColor, TEXTMETRICW, TRANSPARENT,
};

/// The size of the icon, in pixels.
pub const ICON_SIZE: i32 = 32;

/// The part of the icon the mark of the language is drawn in.
const MARKER_BOX: f64 = 0.56;

/// The ink of the mark: the darkest colour of the brand, so that the mark is readable on every colour of the
/// rainbow.
const MARKER_INK: Rgb = LaserTheme::ON_LIGHT;

/// A language of the product, as the tray shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// Chinese (zh-CN).
    Zh,
    /// English (en-US).
    En,
    /// Japanese (ja-JP).
    Ja,
}

impl Language {
    /// The mark of the language: the one thing the user sees in the corner of the screen.
    pub fn marker(self) -> &'static str {
        match self {
            Language::Zh => "中",
            Language::En => "EN",
            Language::Ja => "日",
        }
    }

    /// The name of the language, in the language of the product.
    pub fn name(self) -> &'static str {
        match self {
            Language::Zh => "简体中文",
            Language::En => "英语",
            Language::Ja => "日语",
        }
    }

    /// The Windows language id of the language.
    pub fn langid(self) -> u16 {
        match self {
            Language::Zh => 0x0804,
            Language::En => 0x0409,
            Language::Ja => 0x0411,
        }
    }

    /// The faces of the fonts the mark is drawn with, in the order they are tried.
    ///
    /// The mark is a Chinese or a Japanese character on one hand and two Latin letters on the other: every Windows
    /// has its own fonts, so the first face that really has the glyph is the one that is used.
    fn fonts(self) -> &'static [&'static str] {
        match self {
            Language::Zh => &[
                "Microsoft YaHei UI",
                "Microsoft YaHei",
                "SimSun",
                "Yu Gothic UI",
                "MS Gothic",
                "Segoe UI",
            ],
            Language::Ja => &[
                "Yu Gothic UI",
                "Yu Gothic",
                "Meiryo",
                "MS Gothic",
                "Microsoft YaHei UI",
                "SimSun",
            ],
            Language::En => &[
                "Segoe UI",
                "Tahoma",
                "Arial",
                "Microsoft YaHei UI",
                "Yu Gothic UI",
            ],
        }
    }
}

/// The language of a Windows language id.
///
/// None for a language the product does not ship: the tray must never show a language that is not the active one.
pub fn language_for_langid(langid: u16) -> Option<Language> {
    [Language::Zh, Language::En, Language::Ja]
        .into_iter()
        .find(|language| language.langid() == langid)
}

/// The colour of the rainbow at a place on the circle (0.0 = the top, 1.0 = the top again).
///
/// The seven colours of the brand are mixed in between, in linear light, so that the disc has no hard edges.
pub fn rainbow_at(angle: f64) -> Rgb {
    let count = BRAND_RAINBOW.len();
    let position = angle.rem_euclid(1.0) * count as f64;
    let index = (position.floor() as usize) % count;
    let between = position - position.floor();
    mix(
        BRAND_RAINBOW[index],
        BRAND_RAINBOW[(index + 1) % count],
        between,
    )
}

/// The picture of the logo: the rainbow disc with the light of the laser around it, as BGRA pixels.
pub fn logo_pixels(size: i32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let Some(mut canvas) = Canvas::new_bgra(size, size, &mut pixels) else {
        return pixels;
    };
    let centre = size as f64 / 2.0;
    let radius = size as f64 * 0.34;
    // the halo of the laser around the disc
    canvas.fill_glow(centre, centre, radius * 1.9, 0.55);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - centre;
            let dy = y as f64 + 0.5 - centre;
            let distance = (dx * dx + dy * dy).sqrt();
            let inside = ((radius - distance) / 1.2).clamp(0.0, 1.0);
            if inside <= 0.0 {
                continue;
            }
            let angle = (dy.atan2(dx) + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                / std::f64::consts::TAU;
            // the core of the disc is white, the rim is the rainbow
            let colour = mix(
                rainbow_at(angle),
                LaserTheme::GLOW_CORE,
                (1.0 - distance / radius).powi(3) * 0.75,
            );
            canvas.blend(x, y, colour, inside);
        }
    }
    pixels
}

/// The picture of the logo of a language: the rainbow disc with the mark of the language in its middle.
///
/// The plain disc for a language that is not one of ours (and for a mark that cannot be drawn): the tray must never
/// show a language that is not the active one.
pub fn logo_pixels_for(language: Option<Language>, size: i32) -> Vec<u8> {
    let mut pixels = logo_pixels(size);
    let Some(language) = language else {
        return pixels;
    };
    let Some(coverage) = marker_coverage(language, size) else {
        return pixels;
    };
    let Some(mut canvas) = Canvas::new_bgra(size, size, &mut pixels) else {
        return pixels;
    };
    for (index, pixel) in coverage.iter().enumerate() {
        let alpha = *pixel as f64 / 255.0;
        if alpha <= 0.0 {
            continue;
        }
        canvas.blend(
            (index as i32) % size,
            (index as i32) / size,
            MARKER_INK,
            alpha,
        );
    }
    pixels
}

/// The logo as an icon file (the ICO format): one picture, 32 bits per pixel, with its mask.
pub fn icon_bytes(size: i32) -> Vec<u8> {
    icon_file(&logo_pixels(size), size)
}

/// The logo of a language as an icon file (the ICO format).
///
/// The plain disc for a language that is not one of ours.
pub fn icon_bytes_for(language: Option<Language>, size: i32) -> Vec<u8> {
    match language {
        Some(language) => icon_file(&logo_pixels_for(Some(language), size), size),
        None => icon_bytes(size),
    }
}

/// The icon of the tray, as the bytes a `CreateIconFromResourceEx` call wants.
pub fn icon_resource() -> Vec<u8> {
    // the icon file without its header: that is what the resource functions read.
    let mut bytes = icon_bytes(ICON_SIZE);
    bytes.drain(..22);
    bytes
}

/// The icon of the tray for a language, as the bytes a `CreateIconFromResourceEx` call wants.
pub fn icon_resource_for(language: Option<Language>) -> Vec<u8> {
    match language {
        Some(language) => {
            // the icon file without its header: that is what the resource functions read.
            let mut bytes = icon_bytes_for(Some(language), ICON_SIZE);
            bytes.drain(..22);
            bytes
        }
        None => icon_resource(),
    }
}

/// The icon file of a picture (the ICO format): one picture, 32 bits per pixel, with its mask.
fn icon_file(pixels: &[u8], size: i32) -> Vec<u8> {
    let mut image = Vec::new();
    // the header of a bitmap inside an icon: the height counts the mask as well
    image.extend_from_slice(&40u32.to_le_bytes());
    image.extend_from_slice(&size.to_le_bytes());
    image.extend_from_slice(&(size * 2).to_le_bytes());
    image.extend_from_slice(&1u16.to_le_bytes());
    image.extend_from_slice(&32u16.to_le_bytes());
    image.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    let mask_size = ((size as usize).div_ceil(32) * 4) * size as usize;
    image.extend_from_slice(&((pixels.len() + mask_size) as u32).to_le_bytes());
    image.extend_from_slice(&0u32.to_le_bytes()); // the resolution is not used
    image.extend_from_slice(&0u32.to_le_bytes());
    image.extend_from_slice(&0u32.to_le_bytes()); // the number of colours
    image.extend_from_slice(&0u32.to_le_bytes()); // the important colours
    // the picture, bottom up, as the format wants it
    for row in (0..size).rev() {
        let start = (row * size * 4) as usize;
        image.extend_from_slice(&pixels[start..start + (size * 4) as usize]);
    }
    // the mask: every pixel of the disc is opaque, the rest is transparent
    let mut mask = vec![0u8; mask_size];
    let stride = (size as usize).div_ceil(32) * 4;
    for row in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - size as f64 / 2.0;
            let dy = row as f64 + 0.5 - size as f64 / 2.0;
            if (dx * dx + dy * dy).sqrt() <= size as f64 * 0.40 {
                mask[row as usize * stride + (x as usize / 8)] |= 0x80 >> (x % 8);
            }
        }
    }
    image.extend_from_slice(&mask);

    let mut icon = Vec::new();
    icon.extend_from_slice(&0u16.to_le_bytes()); // reserved
    icon.extend_from_slice(&1u16.to_le_bytes()); // an icon
    icon.extend_from_slice(&1u16.to_le_bytes()); // one picture
    icon.push(size as u8); // the width
    icon.push(size as u8); // the height
    icon.push(0); // no palette
    icon.push(0); // reserved
    icon.extend_from_slice(&1u16.to_le_bytes()); // the planes
    icon.extend_from_slice(&32u16.to_le_bytes()); // the bits per pixel
    icon.extend_from_slice(&(image.len() as u32).to_le_bytes());
    icon.extend_from_slice(&22u32.to_le_bytes()); // where the picture starts
    icon.extend_from_slice(&image);
    icon
}

/// The coverage of the mark of a language at every pixel (0 = nothing, 255 = the whole ink).
///
/// The mark is drawn with GDI text in white on a black picture, so the red channel of a pixel tells how much of the
/// mark is there. None when the mark cannot be drawn at all (no font of this machine has the glyph).
fn marker_coverage(language: Language, size: i32) -> Option<Vec<u8>> {
    let wide: Vec<u16> = language.marker().encode_utf16().collect();
    if wide.is_empty() || size <= 0 {
        return None;
    }
    let box_size = ((size as f64) * MARKER_BOX).round().max(1.0) as i32;
    // SAFETY: every call below is a plain GDI call on the objects this function owns.
    unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return None;
        }
        let dc = MemDc(dc);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        // a negative height means the first row of the buffer is the top row of the picture
        info.bmiHeader.biHeight = -size;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = match CreateDIBSection(
            dc.0,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE(ptr::null_mut()),
            0,
        ) {
            Ok(bitmap) => bitmap,
            Err(_) => return None,
        };
        let picture = Picture { bitmap };
        let length = (size as usize).saturating_mul(size as usize).saturating_mul(4);
        if bits.is_null() {
            return None;
        }
        // the picture is black and transparent before the mark is drawn on it
        ptr::write_bytes(bits, 0, length);
        let previous = SelectObject(dc.0, HGDIOBJ(picture.bitmap.0));
        let _selection = Selection {
            hdc: dc.0,
            previous,
        };

        let font = marker_font(dc.0, language, &wide, box_size)?;
        let font = GdiObject(HGDIOBJ(font.0));
        let _font = Selection::new(dc.0, font.0);

        // the mark is white on black: the colour of a pixel is then the coverage of the mark there
        let _ = SetBkMode(dc.0, TRANSPARENT);
        let _ = SetTextColor(dc.0, COLORREF(0x00FF_FFFF));
        let mut metrics = TEXTMETRICW::default();
        let _ = GetTextMetricsW(dc.0, &mut metrics);
        let mut extent = SIZE::default();
        let _ = GetTextExtentPoint32W(dc.0, &wide, &mut extent);
        // the mark is in the middle of the picture: `y` is the top of the cell of the letters, not their baseline
        let x = (size - extent.cx) / 2;
        let y = (size - (metrics.tmAscent + metrics.tmDescent)) / 2;
        let _ = ExtTextOutW(
            dc.0,
            x,
            y,
            ETO_OPTIONS(0),
            None,
            PCWSTR(wide.as_ptr()),
            wide.len() as u32,
            None,
        );

        // the picture is BGRA: the red channel is the third byte of a pixel
        let slice = std::slice::from_raw_parts(bits as *const u8, length);
        let coverage: Vec<u8> = (0..length / 4).map(|pixel| slice[pixel * 4 + 2]).collect();
        if coverage.iter().all(|value| *value == 0) {
            return None; // nothing was drawn: no mark at all
        }
        Some(coverage)
    }
}

/// The font of the mark: the first face that really has the glyph, made as small as the box allows.
fn marker_font(dc: HDC, language: Language, wide: &[u16], box_size: i32) -> Option<HFONT> {
    for face in language.fonts() {
        let font = create_font(-box_size, face);
        // SAFETY: the DC is live and the font is live.
        let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
        let has_glyph = has_glyph(dc, wide);
        // SAFETY: the font that was there before is selected back.
        let _ = unsafe { SelectObject(dc, previous) };
        if !has_glyph {
            // SAFETY: the font is not used any more.
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
            continue;
        }
        // the font has the glyph: the mark is drawn with one that is as small as the box allows
        // SAFETY: the font is not used any more.
        let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        return fitted_font(dc, face, wide, box_size);
    }
    None
}

/// The font of a face, made as small as the box allows.
///
/// The text is measured with the font that is drawn, so that the mark always fits into the box.
fn fitted_font(dc: HDC, face: &str, wide: &[u16], box_size: i32) -> Option<HFONT> {
    let mut height = -box_size;
    let mut font = create_font(height, face);
    for _ in 0..4 {
        // SAFETY: the DC is live and the font is live.
        let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: the metrics are a live local.
        let _ = unsafe { GetTextMetricsW(dc, &mut metrics) };
        let mut extent = SIZE::default();
        // SAFETY: the text is a live slice and the extent is a live local.
        let _ = unsafe { GetTextExtentPoint32W(dc, wide, &mut extent) };
        // SAFETY: the font that was there before is selected back.
        let _ = unsafe { SelectObject(dc, previous) };
        let width = extent.cx.max(1);
        let tall = (metrics.tmAscent + metrics.tmDescent).max(1);
        if width <= box_size && tall <= box_size {
            return Some(font);
        }
        let scale = (box_size as f64 / width as f64).min(box_size as f64 / tall as f64);
        let next = ((height as f64) * scale).round() as i32;
        let smaller = create_font(next.clamp(-box_size, -1), face);
        // SAFETY: the old font is not used any more.
        let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        font = smaller;
        if next == height {
            break;
        }
        height = next;
    }
    Some(font)
}

/// A font of the given face, as tall as `height` pixels (negative: the height of the letter, not of its cell).
fn create_font(height: i32, face: &str) -> HFONT {
    let name: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the name is NUL-terminated and outlives the call.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            ANTIALIASED_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32 | FF_SWISS.0 as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

/// Whether the font that is selected into the DC really has the glyph of the text.
fn has_glyph(dc: HDC, wide: &[u16]) -> bool {
    let mut indices = vec![0u16; wide.len()];
    // SAFETY: the DC is live, the text is a live slice and the indices are a live local.
    let fetched = unsafe {
        GetGlyphIndicesW(
            dc,
            PCWSTR(wide.as_ptr()),
            wide.len() as i32,
            indices.as_mut_ptr(),
            GGI_MARK_NONEXISTING_GLYPHS,
        )
    };
    fetched == wide.len() as u32 && indices.iter().all(|index| *index != 0xFFFF)
}

/// A memory DC, deleted on drop.
struct MemDc(HDC);

impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: the DC was created here; the objects selected into it are gone by now.
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// A picture in memory, deleted on drop.
struct Picture {
    bitmap: HBITMAP,
}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the bitmap was created here and is not selected into any DC any more.
        let _ = unsafe { DeleteObject(HGDIOBJ(self.bitmap.0)) };
    }
}

/// A GDI object, deleted on drop.
struct GdiObject(HGDIOBJ);

impl Drop for GdiObject {
    fn drop(&mut self) {
        // SAFETY: the object was created here and is not used any more.
        let _ = unsafe { DeleteObject(self.0) };
    }
}

/// An object selected into a DC; the previous one is selected back on drop.
struct Selection {
    hdc: HDC,
    previous: HGDIOBJ,
}

impl Selection {
    /// Select an object into a DC.
    fn new(hdc: HDC, object: HGDIOBJ) -> Selection {
        // SAFETY: both handles are live.
        let previous = unsafe { SelectObject(hdc, object) };
        Selection { hdc, previous }
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        // SAFETY: the DC is live and the object that was there before is still alive.
        let _ = unsafe { SelectObject(self.hdc, self.previous) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rainbow_goes_all_the_way_round() {
        let start = rainbow_at(0.0);
        let end = rainbow_at(1.0);
        assert_eq!(start, end, "the circle closes");
        // the colours of the brand are all there
        let mut seen = Vec::new();
        for step in 0..70 {
            let colour = rainbow_at(step as f64 / 70.0);
            if !seen.contains(&colour) {
                seen.push(colour);
            }
        }
        assert!(seen.len() >= 5, "the disc has the colours of the brand: {seen:?}");
    }

    #[test]
    fn the_logo_has_pixels_and_the_corners_are_empty() {
        let pixels = logo_pixels(ICON_SIZE);
        assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let alpha_at = |x: i32, y: i32| pixels[((y * ICON_SIZE + x) * 4 + 3) as usize];
        assert!(alpha_at(0, 0) < 40, "the corner is nearly empty");
        assert!(alpha_at(ICON_SIZE / 2, ICON_SIZE / 2) > 200, "the centre is solid");
    }

    #[test]
    fn the_icon_is_a_file_a_program_can_read() {
        let icon = icon_bytes(ICON_SIZE);
        assert_eq!(&icon[..2], &[0, 0], "reserved");
        assert_eq!(&icon[2..4], &[1, 0], "an icon");
        assert_eq!(&icon[4..6], &[1, 0], "one picture");
        assert_eq!(icon[6] as i32, ICON_SIZE);
        assert_eq!(icon[7] as i32, ICON_SIZE);
        let size = u32::from_le_bytes(icon[14..18].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(icon[18..22].try_into().unwrap()) as usize;
        assert_eq!(size, icon.len() - offset, "the picture fills the rest");
        assert_eq!(&icon[offset..offset + 4], &[40, 0, 0, 0], "a bitmap header");
    }

    #[test]
    fn a_language_id_gives_the_language_of_the_product() {
        assert_eq!(language_for_langid(0x0804), Some(Language::Zh));
        assert_eq!(language_for_langid(0x0409), Some(Language::En));
        assert_eq!(language_for_langid(0x0411), Some(Language::Ja));
        // a language the product does not ship has no mark at all
        assert_eq!(language_for_langid(0x0412), None);
        assert_eq!(language_for_langid(0x0000), None);
        assert_eq!(language_for_langid(0x1234), None);
    }

    #[test]
    fn the_mark_of_a_language_is_the_one_of_the_owner() {
        assert_eq!(Language::Zh.marker(), "中");
        assert_eq!(Language::Ja.marker(), "日");
        assert_eq!(Language::En.marker(), "EN");
        // the language id of a language is the one Windows uses
        assert_eq!(Language::Zh.langid(), 0x0804);
        assert_eq!(Language::En.langid(), 0x0409);
        assert_eq!(Language::Ja.langid(), 0x0411);
    }

    #[test]
    fn the_three_languages_have_a_logo_of_their_own() {
        let plain = logo_pixels(ICON_SIZE);
        let zh = logo_pixels_for(Some(Language::Zh), ICON_SIZE);
        let en = logo_pixels_for(Some(Language::En), ICON_SIZE);
        let ja = logo_pixels_for(Some(Language::Ja), ICON_SIZE);
        assert_ne!(zh, plain, "the Chinese mark is on the disc");
        assert_ne!(en, plain, "the English mark is on the disc");
        assert_ne!(ja, plain, "the Japanese mark is on the disc");
        assert_ne!(zh, en, "the marks are not the same");
        assert_ne!(zh, ja, "the marks are not the same");
        assert_ne!(en, ja, "the marks are not the same");
    }

    #[test]
    fn a_language_that_is_not_one_of_ours_has_the_plain_disc() {
        assert_eq!(logo_pixels_for(None, ICON_SIZE), logo_pixels(ICON_SIZE));
        assert_eq!(icon_bytes_for(None, ICON_SIZE), icon_bytes(ICON_SIZE));
        // and the icon of a language that is not one of ours is the plain disc as well
        let other = language_for_langid(0x0412);
        assert_eq!(logo_pixels_for(other, ICON_SIZE), logo_pixels(ICON_SIZE));
    }

    #[test]
    fn the_same_language_always_gives_the_same_picture() {
        for language in [Language::Zh, Language::En, Language::Ja] {
            let first = logo_pixels_for(Some(language), ICON_SIZE);
            let second = logo_pixels_for(Some(language), ICON_SIZE);
            assert_eq!(first, second, "the picture of a language is always the same");
            let icon = icon_bytes_for(Some(language), ICON_SIZE);
            assert_eq!(icon, icon_bytes_for(Some(language), ICON_SIZE));
        }
    }    #[test]
    fn the_mark_is_in_the_middle_of_the_disc() {
        for language in [Language::Zh, Language::En, Language::Ja] {
            let plain = logo_pixels(ICON_SIZE);
            let marked = logo_pixels_for(Some(language), ICON_SIZE);
            let mut changed = 0;
            for y in 0..ICON_SIZE {
                for x in 0..ICON_SIZE {
                    let index = ((y * ICON_SIZE + x) * 4) as usize;
                    if marked[index..index + 3] == plain[index..index + 3] {
                        continue;
                    }
                    changed += 1;
                    assert!(
                        (ICON_SIZE / 4..ICON_SIZE - ICON_SIZE / 4).contains(&x)
                            && (ICON_SIZE / 4..ICON_SIZE - ICON_SIZE / 4).contains(&y),
                        "the mark of {:?} stays in the middle of the disc",
                        language
                    );
                }
            }
            assert!(
                changed > 20,
                "the mark of {:?} is drawn: {changed} pixels",
                language
            );
        }
    }
}
