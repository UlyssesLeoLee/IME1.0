//! Renders the mouth of the human in the **two modes** into one picture: the evidence of the sign of the mode.
//!
//!   cargo run --example laser_mode_probe -- <out.raw> [scale]
//!
//! The left half is the **code input** (one pale blue line) and the right half the **natural language input** (a
//! double amber line). The picture is drawn by the production code (`laser_ui::draw_glow`), so it is the picture of
//! the screen and not a drawing of a drawing.
//!
//! The crate has no dependencies (it may not have an image encoder), so the raw buffer is written and a script turns
//! it into a PNG.

use pime_core::style::{Canvas, LaserTheme, PixelRect};
use pime_tsf::laser::{Edge, MouthPlace, mouth_at};
use pime_tsf::laser_ui::draw_glow;
use pime_tsf::layout::Rect;
use pime_tsf::mouths::{MOUTH_RADIUS, WritingMode};

/// How wide the window of the glow is (the value of `laser_ui::GLOW_SIZE`).
const GLOW_SIZE: i32 = 40;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        eprintln!("usage: laser_mode_probe <out.raw> [scale]");
        return std::process::ExitCode::FAILURE;
    };
    let scale: i32 = args.next().and_then(|value| value.parse().ok()).unwrap_or(6);

    // The mouth of the human on the **left edge** of the screen: the slit of light is vertical there.
    let work = Rect::new(0, 0, 1920, 1040);
    let place = MouthPlace {
        edge: Edge::Left,
        offset: 520,
    };
    let mouth = mouth_at(work, place, MOUTH_RADIUS);
    let origin = Rect::new(
        mouth.x - GLOW_SIZE / 2,
        mouth.y - GLOW_SIZE / 2,
        mouth.x + GLOW_SIZE / 2,
        mouth.y + GLOW_SIZE / 2,
    );

    // Two windows of the glow side by side: the code input on the left, the natural language input on the right.
    let width = GLOW_SIZE * 2;
    let height = GLOW_SIZE;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    {
        let Some(mut canvas) = Canvas::new(width, height, &mut pixels) else {
            eprintln!("the picture does not fit into the buffer");
            return std::process::ExitCode::FAILURE;
        };
        // the background is the page: the light of the mouth is seen against it
        canvas.fill_rect(PixelRect::new(0, 0, width, height), LaserTheme::PAGE, 255);
        // the code input: one pale blue line
        draw_glow(
            &mut canvas,
            mouth,
            origin,
            0,
            LaserTheme::MOUTH_SLIT,
            LaserTheme::MOUTH_HALO,
            Some(WritingMode::Code),
        );
        // the natural language input: a double amber line (the mouth is open). The origin moves with it, so the
        // second mouth is drawn in the right half of the picture.
        draw_glow(
            &mut canvas,
            mouth,
            Rect::new(origin.left - GLOW_SIZE, origin.top, origin.right - GLOW_SIZE, origin.bottom),
            0,
            LaserTheme::MOUTH_SLIT_NATURAL,
            LaserTheme::MOUTH_HALO_NATURAL,
            Some(WritingMode::Natural),
        );
    }

    // The picture is scaled up (a hairline is one pixel wide: it would be hard to see), nearest neighbour, so that
    // the pixels of the light stay the pixels of the light.
    let scaled_width = width * scale;
    let scaled_height = height * scale;
    let mut scaled = vec![0u8; (scaled_width * scaled_height * 4) as usize];
    for y in 0..scaled_height {
        for x in 0..scaled_width {
            let from = ((y / scale) * width + x / scale) as usize * 4;
            let to = (y * scaled_width + x) as usize * 4;
            scaled[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
        }
    }
    if let Err(error) = std::fs::write(&out, &scaled) {
        eprintln!("the picture could not be written: {error}");
        return std::process::ExitCode::FAILURE;
    }
    println!("{out} {scaled_width} {scaled_height}");
    std::process::ExitCode::SUCCESS
}
