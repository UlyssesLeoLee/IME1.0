//! The visual language of the product: **laser rainbow**.
//!
//! One design language for everything the user sees: the install wizard, the candidate window, the floating
//! window, the settings window, the new left-hand glow with its laser beam, and the logo in the corner. This module
//! is the single source of that language, and it lives in the core for one reason: the installer (`pime-setup`) and
//! the text service (`pime-tsf`) are two separate crates with two separate toolchains, and both of them depend on
//! `pime-core`. A token or a curve that is defined here is literally the same code in both programs, so the wizard
//! and the input method cannot drift apart.
//!
//! What is here:
//!
//! * **colour** — [`Rgb`] and mixing in linear light (the middle of two colours is the colour the eye expects, not
//!   the arithmetic middle of two numbers);
//! * **shape** — signed distance functions for rounded boxes and segments, so every edge is covered by an analytic
//!   coverage function and is smooth instead of stair-stepped;
//! * **pixels** — [`Canvas`], a borrowed RGBA buffer with the shapes of the design language;
//! * **tokens** — the brand rainbow, the laser palette and the curves of the glow and of the beam.
//!
//! Nothing here talks to Windows: the buffer is borrowed, so the same code paints a GDI DIB of the installer and the
//! layered window of the input method.

/// A colour, in the order the screen shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// A colour from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// A colour from a `COLORREF` value (`0x00BBGGRR`: red in the lowest byte, blue in the highest one).
    pub fn from_bgr(value: u32) -> Rgb {
        Rgb {
            r: (value & 0xFF) as u8,
            g: ((value >> 8) & 0xFF) as u8,
            b: ((value >> 16) & 0xFF) as u8,
        }
    }

    /// The `COLORREF` value of this colour (`0x00BBGGRR`).
    pub const fn to_bgr(self) -> u32 {
        (self.b as u32) << 16 | (self.g as u32) << 8 | (self.r as u32)
    }

    /// The three channels as linear light (the way a light is added, not the way a number is added).
    pub fn to_linear(self) -> [f64; 3] {
        [
            srgb_to_linear(self.r as f64 / 255.0),
            srgb_to_linear(self.g as f64 / 255.0),
            srgb_to_linear(self.b as f64 / 255.0),
        ]
    }

    /// The three channels back from linear light.
    pub fn from_linear(value: [f64; 3]) -> Rgb {
        Rgb {
            r: (linear_to_srgb(value[0]) * 255.0).round() as u8,
            g: (linear_to_srgb(value[1]) * 255.0).round() as u8,
            b: (linear_to_srgb(value[2]) * 255.0).round() as u8,
        }
    }
}

/// The curve between two numbers of light and of darkness.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// The curve back from light to the number the screen shows.
fn linear_to_srgb(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Mix two colours (`t` = 0 gives `a`, 1 gives `b`).
///
/// The mix happens in linear light: the middle of two colours is then the colour the eye expects in the middle, not
/// the arithmetic middle of two numbers (which looks darker and muddier than it should).
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let (la, lb) = (a.to_linear(), b.to_linear());
    Rgb::from_linear([
        la[0] + (lb[0] - la[0]) * t,
        la[1] + (lb[1] - la[1]) * t,
        la[2] + (lb[2] - la[2]) * t,
    ])
}

/// A colour that is `amount` deeper (0 = the same colour, 1 = black).
///
/// The factor is applied to the linear light, so the colour keeps its hue instead of going grey.
pub fn darken(value: Rgb, amount: f64) -> Rgb {
    if amount <= 0.0 {
        return value;
    }
    scale(value, 1.0 - amount.clamp(0.0, 1.0))
}

/// A colour that is `factor` as bright (1 = the same colour, 0 = black, more than 1 = brighter).
pub fn scale(value: Rgb, factor: f64) -> Rgb {
    let linear = value.to_linear();
    Rgb::from_linear([
        linear[0] * factor,
        linear[1] * factor,
        linear[2] * factor,
    ])
}

/// The colour of a smooth ramp that runs through `stops`.
///
/// The stops are control points of a Catmull-Rom curve: the ramp passes exactly through every one of them and does
/// not change its speed at a stop, so the rainbow of the brand flows instead of showing seven colours with a corner
/// between them.
pub fn spline_sample(stops: &[Rgb], t: f64) -> Rgb {
    if stops.is_empty() {
        return Rgb { r: 0, g: 0, b: 0 };
    }
    if stops.len() == 1 {
        return stops[0];
    }
    let t = t.clamp(0.0, 1.0);
    let last = (stops.len() - 1) as f64;
    let position = t * last;
    let index = (position.floor() as usize).min(stops.len() - 2);
    let local = position - index as f64;
    let p0 = stops[index.saturating_sub(1)];
    let p1 = stops[index];
    let p2 = stops[index + 1];
    let p3 = stops[(index + 2).min(stops.len() - 1)];
    let mut out = [0u8; 3];
    for (channel, slot) in out.iter_mut().enumerate() {
        let (p0, p1, p2, p3) = (
            channel_value(p0, channel),
            channel_value(p1, channel),
            channel_value(p2, channel),
            channel_value(p3, channel),
        );
        // the Catmull-Rom basis
        let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
        let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c = -0.5 * p0 + 0.5 * p2;
        let value = ((a * local + b) * local + c) * local + p1;
        *slot = value.clamp(0.0, 255.0).round() as u8;
    }
    Rgb {
        r: out[0],
        g: out[1],
        b: out[2],
    }
}

/// One channel of a colour.
fn channel_value(value: Rgb, channel: usize) -> f64 {
    match channel {
        0 => value.r as f64,
        1 => value.g as f64,
        _ => value.b as f64,
    }
}

/// The distance from a point to the outline of a rounded rectangle (negative inside the shape).
pub fn distance_to_rounded_box(
    x: f64,
    y: f64,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    radius: f64,
) -> f64 {
    let radius = radius.clamp(0.0, ((right - left).min(bottom - top)) * 0.5);
    let centre_x = (left + right) * 0.5;
    let centre_y = (top + bottom) * 0.5;
    let half_x = (right - left) * 0.5 - radius;
    let half_y = (bottom - top) * 0.5 - radius;
    let dx = (x - centre_x).abs() - half_x;
    let dy = (y - centre_y).abs() - half_y;
    let outside = (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

/// The distance from a point to a line between two points.
pub fn distance_to_segment(x: f64, y: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let pax = x - ax;
    let pay = y - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let length = bax * bax + bay * bay;
    let along = if length <= f64::EPSILON {
        0.0
    } else {
        ((pax * bax + pay * bay) / length).clamp(0.0, 1.0)
    };
    let dx = pax - bax * along;
    let dy = pay - bay * along;
    (dx * dx + dy * dy).sqrt()
}

/// How much of a pixel a shape covers (1 = the whole pixel is inside, 0 = none of it is).
///
/// The transition is one pixel wide, which is what turns a stair-stepped outline into a smooth one.
pub fn coverage(distance: f64) -> f64 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// How much shadow falls on a pixel (the penumbra of a shape with a blur of `blur` pixels around it).
///
/// The shadow is at its strongest on the outline of the shape and fades away over `blur` pixels; under the shape
/// itself it does not matter, because the shape is drawn over it.
pub fn shadow_coverage(distance: f64, blur: f64) -> f64 {
    if blur <= 0.0 {
        return 0.0;
    }
    let t = (distance / blur).clamp(0.0, 1.0);
    let smooth = t * t * (3.0 - 2.0 * t);
    1.0 - smooth
}

/// How a shadow is cast.
///
/// The shadow is the shape itself, moved a little down and to the right, with a penumbra around it. It is drawn
/// **under** the shape, so only the part that peeks out at the bottom and on the right is seen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// How wide the soft edge around the shape is (in pixels).
    pub blur: i32,
    /// How far the shadow is moved to the right (in pixels).
    pub offset_x: i32,
    /// How far the shadow is moved down (in pixels).
    pub offset_y: i32,
    /// How dark the shadow is at its darkest (0 = no shadow at all, 1 = as dark as the colour).
    pub strength: f64,
}

impl Shadow {
    /// A shadow of the given blur, moved down by `offset` pixels.
    pub fn soft(blur: i32, offset: i32) -> Shadow {
        Shadow {
            blur,
            offset_x: 0,
            offset_y: offset,
            strength: 0.24,
        }
    }
}

/// A rectangle in pixels (left, top, right, bottom; `right`/`bottom` are exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    /// A rectangle from its four edges.
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// How wide the rectangle is (never negative).
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    /// How tall the rectangle is (never negative).
    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

// ---------------------------------------------------------------------------------------------
// The tokens of the design language
// ---------------------------------------------------------------------------------------------

/// The rainbow of the brand: the seven soft colours of the logo, in the order they run.
///
/// They are the colours the whole window is built from: the band of the wizard, the rim of the candidate window, the
/// light of the beam.
pub const BRAND_RAINBOW: [Rgb; 7] = [
    Rgb::new(0x8F, 0xB8, 0xEE), // soft blue
    Rgb::new(0x79, 0xD4, 0xC6), // mint
    Rgb::new(0xA8, 0xDC, 0x8A), // fresh green
    Rgb::new(0xF3, 0xD6, 0x75), // soft yellow
    Rgb::new(0xF4, 0xB3, 0x78), // orange
    Rgb::new(0xF0, 0x9B, 0xBE), // rose
    Rgb::new(0xC3, 0xA6, 0xEE), // violet
];

/// The laser palette: the colours of the dark "glass" surfaces and of the light they carry.
///
/// The surfaces are dark on purpose: the glow and the beam are light, and light on light is invisible. The text is
/// near-white, the secondary text is dimmed but still readable on the glass.
pub struct LaserTheme;

impl LaserTheme {
    /// The deepest background (the page of the wizard, the glass of the panels).
    pub const PAGE: Rgb = Rgb::new(0x0B, 0x10, 0x20);
    /// The panel of glass that floats over the page.
    pub const PANEL: Rgb = Rgb::new(0x13, 0x1B, 0x33);
    /// The panel of glass that is lit (the row under the mouse).
    pub const PANEL_LIT: Rgb = Rgb::new(0x1C, 0x28, 0x4A);
    /// The primary text.
    pub const TEXT: Rgb = Rgb::new(0xEA, 0xF2, 0xFF);
    /// The secondary text (the hint, the value of a switch).
    pub const DIM: Rgb = Rgb::new(0x93, 0xA6, 0xC8);
    /// The accent: the laser cyan.
    pub const ACCENT: Rgb = Rgb::new(0x35, 0xE0, 0xFF);
    /// The second accent: the magenta of the far end of the beam.
    pub const ACCENT_2: Rgb = Rgb::new(0xFF, 0x4F, 0xD8);
    /// The rim around a panel (a thin line of the brand rainbow).
    pub const RIM: Rgb = Rgb::new(0x6F, 0xA8, 0xFF);
    /// The core of the glow: nearly white.
    pub const GLOW_CORE: Rgb = Rgb::new(0xDF, 0xFB, 0xFF);
    /// The middle of the glow: the laser cyan.
    pub const GLOW_MID: Rgb = Rgb::new(0x2E, 0xC8, 0xFF);
    /// The edge of the glow: the violet it fades into.
    pub const GLOW_EDGE: Rgb = Rgb::new(0x7A, 0x4F, 0xE8);
    /// The core of the beam: a pale blue, never white — a laser is **light**, not a solid wire.
    pub const BEAM_CORE: Rgb = Rgb::new(0xD4, 0xF0, 0xFF);
    /// The middle of the beam: cyan.
    pub const BEAM_MID: Rgb = Rgb::new(0x59, 0xE8, 0xFF);
    /// The edge of the beam: magenta.
    pub const BEAM_EDGE: Rgb = Rgb::new(0xC0, 0x5F, 0xFF);
    /// The head of the beam: the pale cyan it leaves the mouth of the glow with.
    pub const BEAM_HEAD: Rgb = Rgb::new(0xA6, 0xEC, 0xFF);
    /// The tail of the beam: the blue-violet it fades into at the far end.
    pub const BEAM_TAIL: Rgb = Rgb::new(0x62, 0x5C, 0xFF);
    /// The ink of the light surfaces (the text on a bright button).
    pub const ON_LIGHT: Rgb = Rgb::new(0x06, 0x12, 0x1E);
    /// How opaque the glass of the panels is. Every floating surface of the product uses the same material, so the
    /// panels look like one piece of glass and not like three different windows.
    pub const MATERIAL_ALPHA: f64 = 0.80;
}

/// The colour of the laser ramp at `t` (0 = at the mouth of the glow, 1 = at the character).
///
/// The ramp stays in the **blue** family the whole way: a pale cyan at the mouth, the laser cyan, then blue and
/// blue-violet at the far end. It never turns magenta — the beam is one blue laser that fades, not a rainbow ribbon.
pub fn laser_ramp(t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let stops = [
        LaserTheme::BEAM_HEAD,
        LaserTheme::BEAM_MID,
        Rgb::new(0x4C, 0x94, 0xFF),
        LaserTheme::BEAM_TAIL,
    ];
    spline_sample(&stops, t)
}

/// How bright the beam is at the point `t` of its way (0 = at the mouth of the glow, 1 = at the character).
///
/// The beam is at its brightest right behind the mouth, dips a little in the middle (it is a beam of light in the
/// air, not a solid rod) and flares up again where it hits the character.
pub fn beam_profile(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let body = 0.55 + 0.45 * (1.0 - t).powf(0.6);
    let flare = 0.35 * (1.0 - ((t - 1.0).abs() / 0.12).min(1.0)).powi(2);
    (body + flare).min(1.4)
}

/// How bright the glow is at the distance `d` from its centre (0 = the centre, `radius` = the edge).
///
/// A soft core with a long tail: the light is strongest in the middle and dies away over the whole radius.
pub fn glow_profile(d: f64, radius: f64) -> f64 {
    if radius <= 0.0 {
        return 0.0;
    }
    let t = (d / radius).clamp(0.0, 1.0);
    let core = (1.0 - t).powf(2.6);
    let tail = 0.35 * (1.0 - t).powf(0.9);
    (core + tail).min(1.0)
}

// ---------------------------------------------------------------------------------------------
// The canvas
// ---------------------------------------------------------------------------------------------

/// The order of the three colour channels in the buffer of a [`Canvas`].
///
/// GDI (a 32-bit `BI_RGB` DIB) and a layered window both expect **blue, green, red, alpha**; a buffer that is
/// written by other code may be red first. The canvas asks, so the same drawing code paints both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelOrder {
    /// Red, green, blue, alpha.
    Rgba,
    /// Blue, green, red, alpha (what GDI and a layered window want).
    Bgra,
}

/// A borrowed 32-bit picture: four bytes per pixel.
///
/// The pixels are addressed one by one, which is what makes the smooth edges and the smooth gradients possible.
/// The buffer belongs to the caller, so the same code paints a GDI DIB and a layered window.
pub struct Canvas<'a> {
    pub width: i32,
    pub height: i32,
    /// The pixels: `width * height * 4` bytes, top row first.
    pub pixels: &'a mut [u8],
    /// The order of the colour channels in the buffer.
    pub order: ChannelOrder,
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`, red first; `None` when the buffer is too small for the picture.
    pub fn new(width: i32, height: i32, pixels: &'a mut [u8]) -> Option<Canvas<'a>> {
        Canvas::with_order(width, height, pixels, ChannelOrder::Rgba)
    }

    /// A canvas over `pixels` in the order GDI wants (blue, green, red, alpha).
    pub fn new_bgra(width: i32, height: i32, pixels: &'a mut [u8]) -> Option<Canvas<'a>> {
        Canvas::with_order(width, height, pixels, ChannelOrder::Bgra)
    }

    /// A canvas over `pixels` with a given channel order; `None` when the buffer is too small.
    pub fn with_order(
        width: i32,
        height: i32,
        pixels: &'a mut [u8],
        order: ChannelOrder,
    ) -> Option<Canvas<'a>> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let needed = (width as usize).saturating_mul(height as usize).saturating_mul(4);
        if pixels.len() < needed {
            return None;
        }
        Some(Canvas {
            width,
            height,
            pixels,
            order,
        })
    }

    /// The index of the red, green and blue byte of one pixel.
    fn channels(&self, index: usize) -> (usize, usize, usize) {
        match self.order {
            ChannelOrder::Rgba => (index, index + 1, index + 2),
            ChannelOrder::Bgra => (index + 2, index + 1, index),
        }
    }

    /// Put one colour into one pixel (the alpha of the pixel becomes the alpha of the colour).
    pub fn put(&mut self, x: i32, y: i32, colour: Rgb, alpha: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let index = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let (r, g, b) = self.channels(index);
        self.pixels[r] = colour.r;
        self.pixels[g] = colour.g;
        self.pixels[b] = colour.b;
        self.pixels[index + 3] = alpha;
    }

    /// The colour of one pixel (transparent black when the pixel is outside the picture).
    pub fn at(&self, x: i32, y: i32) -> (Rgb, u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return (Rgb::new(0, 0, 0), 0);
        }
        let index = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let (r, g, b) = self.channels(index);
        (
            Rgb::new(self.pixels[r], self.pixels[g], self.pixels[b]),
            self.pixels[index + 3],
        )
    }

    /// Mix a colour into one pixel (the source-over rule, in straight alpha).
    pub fn blend(&mut self, x: i32, y: i32, colour: Rgb, alpha: f64) {
        if alpha <= 0.0 || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let alpha = alpha.min(1.0);
        let index = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let keep = 1.0 - alpha;
        let out_a = alpha + (self.pixels[index + 3] as f64 / 255.0) * keep;
        let (r, g, b) = self.channels(index);
        for (slot, source) in [(r, colour.r), (g, colour.g), (b, colour.b)] {
            let dst = self.pixels[slot] as f64;
            self.pixels[slot] = (source as f64 * alpha + dst * keep).round() as u8;
        }
        self.pixels[index + 3] = (out_a * 255.0).round().min(255.0) as u8;
    }

    /// Mix a colour into one pixel and keep the picture **opaque** (the alpha of the pixel stays 255).
    ///
    /// The installer paints on a DIB that is shown with `BitBlt`, which ignores the alpha: there the picture must
    /// stay opaque, or the text that is drawn over it later blends against a transparent background.
    pub fn blend_opaque(&mut self, x: i32, y: i32, colour: Rgb, alpha: f64) {
        if alpha <= 0.0 || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let alpha = alpha.min(1.0);
        let index = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let keep = 1.0 - alpha;
        let (r, g, b) = self.channels(index);
        for (slot, source) in [(r, colour.r), (g, colour.g), (b, colour.b)] {
            let dst = self.pixels[slot] as f64;
            self.pixels[slot] = (source as f64 * alpha + dst * keep).round() as u8;
        }
        self.pixels[index + 3] = 255;
    }

    /// Make every pixel of the picture fully transparent.
    pub fn clear(&mut self) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            pixel[3] = 0;
        }
    }

    /// Make every pixel of the picture opaque (the picture is then shown without any transparency).
    pub fn make_opaque(&mut self) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }

    /// Fill a rectangle with one colour.
    pub fn fill_rect(&mut self, rect: PixelRect, colour: Rgb, alpha: u8) {
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            for x in rect.left.max(0)..rect.right.min(self.width) {
                self.put(x, y, colour, alpha);
            }
        }
    }

    /// Fill a rectangle with rounded corners (the edge is smooth).
    pub fn fill_rounded(&mut self, rect: PixelRect, radius: i32, colour: Rgb, alpha: f64) {
        let radius = radius.max(0) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    rect.left as f64,
                    rect.top as f64,
                    rect.right as f64,
                    rect.bottom as f64,
                    radius,
                );
                let cover = coverage(distance);
                if cover > 0.0 {
                    self.blend(x, y, colour, cover * alpha);
                }
            }
        }
    }

    /// Draw the line around a rounded rectangle, `width` pixels wide, just inside its edge.
    pub fn stroke_rounded(&mut self, rect: PixelRect, radius: i32, colour: Rgb, width: i32, alpha: f64) {
        let width = width.max(1);
        let half = width as f64 * 0.5;
        // The outline of the shape, a little way inside the edge: the band that is drawn is `width` pixels wide and
        // follows the outline, it does not fill the shape.
        let inset = PixelRect::new(
            rect.left + width,
            rect.top + width,
            rect.right - width,
            rect.bottom - width,
        );
        let radius = (radius.max(0) as f64 - half).max(0.0);
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    inset.left as f64,
                    inset.top as f64,
                    inset.right as f64,
                    inset.bottom as f64,
                    radius,
                );
                let cover = (half + 0.5 - distance.abs()).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, colour, cover * alpha);
                }
            }
        }
    }

    /// The shadow under a shape: the shape itself, moved a little down and to the right, with a soft edge that fades
    /// away around it.
    pub fn fill_shadow(&mut self, rect: PixelRect, radius: i32, colour: Rgb, shadow: Shadow) {
        let blur = shadow.blur.max(1) as f64;
        let shape = PixelRect::new(
            rect.left + shadow.offset_x,
            rect.top + shadow.offset_y,
            rect.right + shadow.offset_x,
            rect.bottom + shadow.offset_y,
        );
        let grown = PixelRect::new(
            shape.left - blur as i32,
            shape.top - blur as i32,
            shape.right + blur as i32,
            shape.bottom + blur as i32,
        );
        for y in grown.top.max(0)..grown.bottom.min(self.height) {
            for x in grown.left.max(0)..grown.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    shape.left as f64,
                    shape.top as f64,
                    shape.right as f64,
                    shape.bottom as f64,
                    radius.max(0) as f64,
                );
                let alpha = shadow.strength * shadow_coverage(distance, blur);
                if alpha > 0.0 {
                    self.blend(x, y, colour, alpha);
                }
            }
        }
    }

    /// Fill a circle (the edge is smooth).
    pub fn fill_circle(&mut self, centre_x: f64, centre_y: f64, radius: f64, colour: Rgb, alpha: f64) {
        let left = (centre_x - radius).floor() as i32;
        let right = (centre_x + radius).ceil() as i32;
        let top = (centre_y - radius).floor() as i32;
        let bottom = (centre_y + radius).ceil() as i32;
        for y in top.max(0)..bottom.min(self.height) {
            for x in left.max(0)..right.min(self.width) {
                let dx = x as f64 + 0.5 - centre_x;
                let dy = y as f64 + 0.5 - centre_y;
                let cover = coverage((dx * dx + dy * dy).sqrt() - radius);
                if cover > 0.0 {
                    self.blend(x, y, colour, cover * alpha);
                }
            }
        }
    }

    /// Draw a line with a given width (the edge is smooth).
    pub fn stroke_segment(
        &mut self,
        ax: f64,
        ay: f64,
        bx: f64,
        by: f64,
        width: f64,
        colour: Rgb,
        alpha: f64,
    ) {
        let half = width * 0.5;
        let left = (ax.min(bx) - half - 1.0).floor() as i32;
        let right = (ax.max(bx) + half + 1.0).ceil() as i32;
        let top = (ay.min(by) - half - 1.0).floor() as i32;
        let bottom = (ay.max(by) + half + 1.0).ceil() as i32;
        for y in top.max(0)..bottom.min(self.height) {
            for x in left.max(0)..right.min(self.width) {
                let distance = distance_to_segment(x as f64 + 0.5, y as f64 + 0.5, ax, ay, bx, by);
                let cover = (half + 0.5 - distance).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, colour, cover * alpha);
                }
            }
        }
    }

    /// Fill a rectangle with a gradient from top to bottom (computed for every pixel).
    pub fn fill_gradient_v(&mut self, rect: PixelRect, top: Rgb, bottom: Rgb, alpha: u8) {
        let height = (rect.bottom - rect.top).max(1) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            let t = ((y as f64 + 0.5 - rect.top as f64) / height).clamp(0.0, 1.0);
            let colour = mix(top, bottom, t);
            for x in rect.left.max(0)..rect.right.min(self.width) {
                self.put(x, y, colour, alpha);
            }
        }
    }

    /// Fill a rectangle with the rainbow of the brand: a smooth ramp that runs through `stops`, with a vertical
    /// shading from `top_factor` to `bottom_factor` (both in linear light).
    pub fn fill_rainbow(
        &mut self,
        rect: PixelRect,
        stops: &[Rgb],
        top_factor: f64,
        bottom_factor: f64,
        alpha: u8,
    ) {
        let width = (rect.right - rect.left).max(1) as f64;
        let height = (rect.bottom - rect.top).max(1) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            let ty = ((y as f64 + 0.5 - rect.top as f64) / height).clamp(0.0, 1.0);
            let shading = top_factor + (bottom_factor - top_factor) * ty;
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let tx = ((x as f64 + 0.5 - rect.left as f64) / width).clamp(0.0, 1.0);
                let colour = scale(spline_sample(stops, tx), shading);
                self.put(x, y, colour, alpha);
            }
        }
    }

    /// Fill a rounded rectangle with a vertical gradient from `top` to `bottom`.
    ///
    /// The gradient is clipped to the rounded outline, so a **button** can be a rounded shape that is lit from
    /// above: a hard-edged rectangle would be the one square corner in a window of round ones.
    pub fn fill_gradient_rounded(
        &mut self,
        rect: PixelRect,
        radius: i32,
        top: Rgb,
        bottom: Rgb,
        alpha: u8,
    ) {
        let height = (rect.bottom - rect.top).max(1) as f64;
        let radius = radius.max(0) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            let ty = ((y as f64 + 0.5 - rect.top as f64) / height).clamp(0.0, 1.0);
            let colour = mix(top, bottom, ty);
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    rect.left as f64,
                    rect.top as f64,
                    rect.right as f64,
                    rect.bottom as f64,
                    radius,
                );
                let cover = coverage(distance);
                if cover <= 0.0 {
                    continue;
                }
                self.blend(x, y, colour, cover * (alpha as f64 / 255.0));
            }
        }
    }

    /// Fill a rounded rectangle with the rainbow of the brand: a smooth ramp that runs **across** the rectangle,
    /// with a vertical shading from `top_factor` to `bottom_factor` (both in linear light).
    ///
    /// The band is clipped to the rounded outline, so a rainbow strip can sit on the edge of a glass panel without
    /// its corners sticking out.
    pub fn fill_rainbow_rounded(
        &mut self,
        rect: PixelRect,
        radius: i32,
        stops: &[Rgb],
        top_factor: f64,
        bottom_factor: f64,
        alpha: u8,
    ) {
        let width = (rect.right - rect.left).max(1) as f64;
        let height = (rect.bottom - rect.top).max(1) as f64;
        let radius = radius.max(0) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            let ty = ((y as f64 + 0.5 - rect.top as f64) / height).clamp(0.0, 1.0);
            let shading = top_factor + (bottom_factor - top_factor) * ty;
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    rect.left as f64,
                    rect.top as f64,
                    rect.right as f64,
                    rect.bottom as f64,
                    radius,
                );
                let cover = coverage(distance);
                if cover <= 0.0 {
                    continue;
                }
                let tx = ((x as f64 + 0.5 - rect.left as f64) / width).clamp(0.0, 1.0);
                let colour = scale(spline_sample(stops, tx), shading);
                self.blend(x, y, colour, cover * (alpha as f64 / 255.0));
            }
        }
    }

    /// Fill a rounded rectangle with the rainbow of the brand: a smooth ramp that runs **downwards** through it.
    ///
    /// This is the narrow vertical bar that marks the row the user is on: the whole rainbow, from the first colour
    /// of the brand at the top to the last one at the bottom.
    pub fn fill_rainbow_vertical(
        &mut self,
        rect: PixelRect,
        radius: i32,
        stops: &[Rgb],
        alpha: u8,
    ) {
        let height = (rect.bottom - rect.top).max(1) as f64;
        let radius = radius.max(0) as f64;
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            let ty = ((y as f64 + 0.5 - rect.top as f64) / height).clamp(0.0, 1.0);
            let colour = spline_sample(stops, ty);
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    rect.left as f64,
                    rect.top as f64,
                    rect.right as f64,
                    rect.bottom as f64,
                    radius,
                );
                let cover = coverage(distance);
                if cover <= 0.0 {
                    continue;
                }
                self.blend(x, y, colour, cover * (alpha as f64 / 255.0));
            }
        }
    }

    /// Fill a **band along the top edge** of a rounded panel with the rainbow of the brand.
    ///
    /// The band is clipped by the rounded outline of the panel (so its corners follow the panel) and it fades out
    /// towards the bottom: it reads as a light that lies on the edge, not as a painted stripe.
    pub fn fill_rainbow_edge(
        &mut self,
        panel: PixelRect,
        radius: i32,
        band: f64,
        stops: &[Rgb],
        alpha: f64,
    ) {
        let width = (panel.right - panel.left).max(1) as f64;
        let radius = radius.max(0) as f64;
        let top = panel.top.max(0);
        let bottom = (panel.top as f64 + band).ceil() as i32;
        for y in top..bottom.min(self.height) {
            // the band fades out towards the bottom
            let fall = 1.0 - ((y as f64 + 0.5 - panel.top as f64) / band).clamp(0.0, 1.0);
            for x in panel.left.max(0)..panel.right.min(self.width) {
                let distance = distance_to_rounded_box(
                    x as f64 + 0.5,
                    y as f64 + 0.5,
                    panel.left as f64,
                    panel.top as f64,
                    panel.right as f64,
                    panel.bottom as f64,
                    radius,
                );
                let cover = coverage(distance);
                if cover <= 0.0 {
                    continue;
                }
                let tx = ((x as f64 + 0.5 - panel.left as f64) / width).clamp(0.0, 1.0);
                let colour = spline_sample(stops, tx);
                self.blend(x, y, colour, cover * fall * alpha);
            }
        }
    }

    /// A soft light: a circle whose brightness falls off with the distance, in the colours of the glow.
    ///
    /// This is the shape of the light around the mouth of the glow and the halo of the laser.
    pub fn fill_glow(&mut self, centre_x: f64, centre_y: f64, radius: f64, strength: f64) {
        let left = (centre_x - radius).floor() as i32;
        let right = (centre_x + radius).ceil() as i32;
        let top = (centre_y - radius).floor() as i32;
        let bottom = (centre_y + radius).ceil() as i32;
        for y in top.max(0)..bottom.min(self.height) {
            for x in left.max(0)..right.min(self.width) {
                let dx = x as f64 + 0.5 - centre_x;
                let dy = y as f64 + 0.5 - centre_y;
                let distance = (dx * dx + dy * dy).sqrt();
                let profile = glow_profile(distance, radius);
                if profile <= 0.0 {
                    continue;
                }
                // the core is nearly white, the tail is the colour of the light
                let t = (distance / radius).clamp(0.0, 1.0);
                let colour = mix(
                    LaserTheme::GLOW_CORE,
                    mix(LaserTheme::GLOW_MID, LaserTheme::GLOW_EDGE, t),
                    t.min(1.0),
                );
                self.blend(x, y, colour, profile * strength);
            }
        }
    }

    /// A laser beam from `(ax, ay)` to `(bx, by)`: a **translucent blue** beam in the air.
    ///
    /// The beam is light, not a solid rod: the whole of it stays see-through (the page behind it shows through), the
    /// core is a thin pale-blue line and the halo around it is a wide, soft blue that fades along the way.
    ///
    /// `progress` is how far the beam has travelled (0 = at the mouth, 1 = at the character) and `strength` how
    /// bright it is overall.
    pub fn fill_beam(
        &mut self,
        ax: f64,
        ay: f64,
        bx: f64,
        by: f64,
        progress: f64,
        strength: f64,
    ) {
        let progress = progress.clamp(0.0, 1.0);
        let length = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        if length <= 0.5 {
            return;
        }
        // The halo is soft and the core is thin: light in the air, not a rod. The user asked for a thinner beam
        // ("激光太粗了"), so the halo is half as wide as it was and the core is a hairline.
        let halo = 7.0;
        let core = 1.4;
        let left = (ax.min(bx) - halo - 1.0).floor() as i32;
        let right = (ax.max(bx) + halo + 1.0).ceil() as i32;
        let top = (ay.min(by) - halo - 1.0).floor() as i32;
        let bottom = (ay.max(by) + halo + 1.0).ceil() as i32;
        for y in top.max(0)..bottom.min(self.height) {
            for x in left.max(0)..right.min(self.width) {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let distance = distance_to_segment(px, py, ax, ay, bx, by);
                // how far along the beam this pixel is (0 at the mouth, 1 at the character)
                let along = (((px - ax) * (bx - ax) + (py - ay) * (by - ay)) / (length * length))
                    .clamp(0.0, 1.0);
                // the head of the beam only has reached `progress`
                if along > progress {
                    continue;
                }
                let profile = beam_profile(along);
                let halo_cover = (halo - distance).clamp(0.0, 1.0);
                if halo_cover <= 0.0 {
                    continue;
                }
                // the halo: a soft blue that never becomes opaque
                let halo_alpha = halo_cover * halo_cover * profile * strength * 0.30;
                let colour = laser_ramp(along);
                self.blend(x, y, colour, halo_alpha);
                // the core: thin, pale blue and still translucent
                let core_cover = (core - distance).clamp(0.0, 1.0);
                if core_cover > 0.0 {
                    self.blend(
                        x,
                        y,
                        LaserTheme::BEAM_CORE,
                        core_cover * core_cover * profile * strength * 0.40,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_survives_the_trip_through_linear_light() {
        let colour = Rgb::new(12, 200, 77);
        let back = Rgb::from_linear(colour.to_linear());
        assert!((back.r as i32 - colour.r as i32).abs() <= 1);
        assert!((back.g as i32 - colour.g as i32).abs() <= 1);
        assert!((back.b as i32 - colour.b as i32).abs() <= 1);
    }

    #[test]
    fn the_mix_of_two_colours_is_the_one_the_eye_expects() {
        let mixed = mix(Rgb::new(0, 0, 0), Rgb::new(255, 255, 255), 0.5);
        // in linear light the middle is much brighter than the arithmetic middle of the numbers
        assert!(mixed.r > 180, "the middle of black and white is light, got {}", mixed.r);
        assert_eq!(mixed.r, mixed.g);
        assert_eq!(mixed.g, mixed.b);
    }

    #[test]
    fn a_deeper_colour_keeps_its_hue() {
        let deep = darken(Rgb::new(200, 100, 40), 0.5);
        assert!(deep.r > deep.g && deep.g > deep.b, "the order of the channels is kept");
        assert!(deep.r < 200);
    }

    #[test]
    fn the_rainbow_ramp_runs_through_every_stop() {
        for (index, stop) in BRAND_RAINBOW.iter().enumerate() {
            let t = index as f64 / (BRAND_RAINBOW.len() - 1) as f64;
            let sampled = spline_sample(&BRAND_RAINBOW, t);
            assert!(
                (sampled.r as i32 - stop.r as i32).abs() <= 2
                    && (sampled.g as i32 - stop.g as i32).abs() <= 2
                    && (sampled.b as i32 - stop.b as i32).abs() <= 2,
                "the ramp passes through {stop:?}, got {sampled:?}"
            );
        }
    }

    #[test]
    fn the_rainbow_ramp_does_not_jump() {
        let mut previous = spline_sample(&BRAND_RAINBOW, 0.0);
        for step in 1..=200 {
            let current = spline_sample(&BRAND_RAINBOW, step as f64 / 200.0);
            let jump = (current.r as i32 - previous.r as i32)
                .abs()
                .max((current.g as i32 - previous.g as i32).abs())
                .max((current.b as i32 - previous.b as i32).abs());
            assert!(jump <= 24, "the ramp jumps by {jump} at step {step}");
            previous = current;
        }
    }

    #[test]
    fn the_edge_of_a_shape_is_covered_partly() {
        assert_eq!(coverage(-1.0), 1.0);
        assert_eq!(coverage(1.0), 0.0);
        let middle = coverage(0.0);
        assert!(middle > 0.4 && middle < 0.6, "the edge is half covered, got {middle}");
    }

    #[test]
    fn the_distance_to_a_rounded_box_is_the_distance_to_its_outline() {
        // the centre of a 100x100 box with a radius of 10 is 50 pixels inside the outline
        let distance = distance_to_rounded_box(50.0, 50.0, 0.0, 0.0, 100.0, 100.0, 10.0);
        assert!((distance + 50.0).abs() < 0.001, "the centre is 50 inside, got {distance}");
        // a point on the outline has a distance of zero
        let on_the_edge = distance_to_rounded_box(0.0, 50.0, 0.0, 0.0, 100.0, 100.0, 10.0);
        assert!(on_the_edge.abs() < 0.001, "the edge is at zero, got {on_the_edge}");
        let outside = distance_to_rounded_box(150.0, 50.0, 0.0, 0.0, 100.0, 100.0, 10.0);
        assert!((outside - 50.0).abs() < 0.001, "the far side is 50 outside, got {outside}");
    }

    #[test]
    fn a_shadow_is_darker_near_the_shape_and_gone_far_away() {
        assert!(shadow_coverage(0.0, 10.0) > shadow_coverage(5.0, 10.0));
        assert!(shadow_coverage(5.0, 10.0) > shadow_coverage(20.0, 10.0));
        assert_eq!(shadow_coverage(20.0, 10.0), 0.0);
    }

    #[test]
    fn the_canvas_refuses_a_buffer_that_is_too_small() {
        let mut pixels = vec![0u8; 16];
        assert!(Canvas::new(4, 4, &mut pixels).is_none());
        let mut big = vec![0u8; 64];
        assert!(Canvas::new(4, 4, &mut big).is_some());
    }

    #[test]
    fn a_rounded_rectangle_is_soft_at_its_edge_and_opaque_in_the_middle() {
        let mut pixels = vec![0u8; 40 * 40 * 4];
        {
            let mut canvas = Canvas::new(40, 40, &mut pixels).expect("the buffer fits");
            canvas.fill_rounded(
                PixelRect::new(5, 5, 35, 35),
                8,
                Rgb::new(255, 0, 0),
                1.0,
            );
            let (centre, centre_alpha) = canvas.at(20, 20);
            assert_eq!(centre_alpha, 255);
            assert_eq!(centre.r, 255);
            // the straight edge falls exactly on the pixel border, so that pixel is covered completely
            let (_, edge_alpha) = canvas.at(5, 20);
            assert_eq!(edge_alpha, 255, "the straight edge is on the pixel border");
            // the rounded corner is covered partly by every pixel it crosses: that is what makes it smooth
            let (_, arc_alpha) = canvas.at(7, 7);
            assert!(
                arc_alpha > 0 && arc_alpha < 255,
                "the corner is smooth: {arc_alpha}"
            );
            // nothing is drawn outside the box
            let (_, outside_alpha) = canvas.at(38, 38);
            assert_eq!(outside_alpha, 0);
        }
        assert_eq!(pixels.len(), 40 * 40 * 4);
    }

    #[test]
    fn blending_keeps_the_picture_transparent_where_nothing_was_drawn() {
        let mut pixels = vec![0u8; 10 * 10 * 4];
        let mut canvas = Canvas::new(10, 10, &mut pixels).expect("the buffer fits");
        canvas.blend(5, 5, Rgb::new(255, 255, 255), 0.5);
        let (_, alpha) = canvas.at(5, 5);
        assert!((120..=135).contains(&alpha), "half covered, got {alpha}");
        let (_, empty) = canvas.at(0, 0);
        assert_eq!(empty, 0);
    }

    #[test]
    fn the_glow_is_brightest_in_its_middle() {
        let mut pixels = vec![0u8; 60 * 60 * 4];
        let mut canvas = Canvas::new(60, 60, &mut pixels).expect("the buffer fits");
        canvas.fill_glow(30.0, 30.0, 20.0, 1.0);
        let (_, centre) = canvas.at(30, 30);
        let (_, middle) = canvas.at(38, 30);
        let (_, edge) = canvas.at(49, 30);
        assert!(centre > middle, "the core is the brightest: {centre} vs {middle}");
        assert!(middle > edge, "the light dies away: {middle} vs {edge}");
    }

    #[test]
    fn the_beam_only_reaches_what_it_has_travelled() {
        let mut pixels = vec![0u8; 100 * 40 * 4];
        {
            let mut canvas = Canvas::new(100, 40, &mut pixels).expect("the buffer fits");
            canvas.fill_beam(5.0, 20.0, 95.0, 20.0, 0.5, 1.0);
            // the part of the beam that was already travelled is lit
            let (_, travelled) = canvas.at(30, 20);
            assert!(travelled > 0, "the beam lit the way it came");
            // the part it has not reached yet is dark
            let (_, ahead) = canvas.at(80, 20);
            assert_eq!(ahead, 0, "the beam did not reach further than it went");
        }
        assert!(pixels.iter().any(|byte| *byte > 0));
    }

    #[test]
    fn the_beam_is_brightest_near_the_mouth_and_flares_at_the_character() {
        assert!(beam_profile(0.0) > beam_profile(0.5));
        assert!(beam_profile(1.0) > beam_profile(0.5));
        assert!(beam_profile(-1.0) == beam_profile(0.0));
        assert!(beam_profile(2.0) == beam_profile(1.0));
    }

    #[test]
    fn the_glow_profile_falls_to_zero_at_its_edge() {
        assert!(glow_profile(0.0, 10.0) > 0.9);
        assert_eq!(glow_profile(10.0, 10.0), 0.0);
        assert_eq!(glow_profile(1.0, 0.0), 0.0);
    }

    #[test]
    fn the_gdi_order_puts_blue_first() {
        let mut pixels = vec![0u8; 4];
        let mut canvas = Canvas::new_bgra(1, 1, &mut pixels).expect("the buffer fits");
        canvas.put(0, 0, Rgb::new(0x11, 0x22, 0x33), 255);
        assert_eq!(&pixels[0..4], &[0x33, 0x22, 0x11, 0xFF]);
    }

    #[test]
    fn the_colour_of_a_colorref_survives_the_trip() {
        let colour = Rgb::from_bgr(0x00_EE_B8_8F);
        assert_eq!((colour.r, colour.g, colour.b), (0x8F, 0xB8, 0xEE));
        assert_eq!(colour.to_bgr(), 0x00_EE_B8_8F);
    }

    #[test]
    fn the_laser_ramp_stays_in_the_blue_family() {
        let start = laser_ramp(0.0);
        let end = laser_ramp(1.0);
        assert!(start.g > start.r, "the ramp starts in cyan: {start:?}");
        assert!(end.b > end.r, "the ramp ends in blue-violet: {end:?}");
        // the whole way it is a blue laser: the blue channel is never the weakest one
        for step in 0..=20 {
            let colour = laser_ramp(step as f64 / 20.0);
            assert!(
                colour.b >= colour.r,
                "the beam turns warm at step {step}: {colour:?}"
            );
        }
    }

    #[test]
    fn the_beam_is_translucent_not_a_solid_rod() {
        let mut pixels = vec![0u8; 100 * 40 * 4];
        let mut canvas = Canvas::new(100, 40, &mut pixels).expect("the buffer fits");
        canvas.fill_beam(5.0, 20.0, 95.0, 20.0, 1.0, 1.0);
        let (colour, alpha) = canvas.at(50, 20);
        assert!(alpha > 0, "the beam lit the way");
        assert!(
            alpha < 255,
            "the beam is translucent: the page behind it shows through, got {alpha}"
        );
        // and it is blue: more blue than red
        assert!(colour.b > colour.r, "the beam is blue: {colour:?}");
    }

    #[test]
    fn a_rainbow_band_is_clipped_to_its_rounded_outline() {
        let mut pixels = vec![0u8; 40 * 40 * 4];
        let mut canvas = Canvas::new(40, 40, &mut pixels).expect("the buffer fits");
        let rect = PixelRect::new(5, 5, 35, 15);
        canvas.fill_rainbow_rounded(rect, 5, &BRAND_RAINBOW, 1.0, 1.0, 255);
        let (left, left_alpha) = canvas.at(6, 10);
        let (right, right_alpha) = canvas.at(34, 10);
        assert!(left_alpha > 0 && right_alpha > 0, "the band is there");
        assert!(
            left.r != right.r || left.g != right.g || left.b != right.b,
            "the band really is a gradient: {left:?} vs {right:?}"
        );
        // the rounded corner is soft: the pixel in the corner of the box is only partly covered
        let (_, corner_alpha) = canvas.at(5, 5);
        assert!(
            corner_alpha < 255,
            "the corner of the band is soft, got {corner_alpha}"
        );
        // nothing outside the band
        let (_, outside_alpha) = canvas.at(20, 20);
        assert_eq!(outside_alpha, 0);
    }

    #[test]
    fn a_vertical_rainbow_runs_from_the_first_stop_to_the_last() {
        let mut pixels = vec![0u8; 40 * 40 * 4];
        let mut canvas = Canvas::new(40, 40, &mut pixels).expect("the buffer fits");
        canvas.fill_rainbow_vertical(PixelRect::new(10, 5, 20, 35), 4, &BRAND_RAINBOW, 255);
        let (top, top_alpha) = canvas.at(15, 6);
        let (bottom, bottom_alpha) = canvas.at(15, 34);
        assert!(top_alpha > 0 && bottom_alpha > 0);
        assert!(
            top.r != bottom.r || top.g != bottom.g || top.b != bottom.b,
            "the bar really is a gradient: {top:?} vs {bottom:?}"
        );
    }
}
