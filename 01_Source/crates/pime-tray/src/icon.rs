//! The logo of the input method in the corner of the screen and the resident tray it lives in.
//!
//! The logo is drawn at run time with the same design language as the rest of the product (`pime_core::style`): the
//! rainbow of the brand in a disc with the light of the laser around it. Nothing is loaded from disk, so there is no
//! resource to keep in step with the code.
//!
//! The tray itself is the Windows notification area (the place where Sogou keeps its logo too): the system gives it
//! the place, so it can never cover the clock.

use pime_core::style::{mix, BRAND_RAINBOW, Canvas, LaserTheme, Rgb};

/// The size of the icon, in pixels.
pub const ICON_SIZE: i32 = 32;

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
    )}

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

/// The logo as an icon file (the ICO format): one picture, 32 bits per pixel, with its mask.
pub fn icon_bytes(size: i32) -> Vec<u8> {
    let pixels = logo_pixels(size);
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

/// The icon of the tray, as the bytes a `CreateIconFromResourceEx` call wants.
pub fn icon_resource() -> Vec<u8> {
    // the icon file without its header: that is what the resource functions read.
    let mut bytes = icon_bytes(ICON_SIZE);
    bytes.drain(..22);
    bytes
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
}
