//! Pure geometry of the candidate window: sizes, row rectangles, placement relative to the caret and the monitor
//! work area (flip above when it would not fit, clamp horizontally). No Win32 calls: testable with synthetic metrics.
//!
//! Design anchors: DD-08 / SCR-01 (the window never covers the caret line and never grows over the whole screen),
//! UT-055 (stay inside the work area; when it cannot be placed, do not show it), FR-043 (DPI 100-250 %, multiple
//! monitors), UT-057 (information is carried by text, never by colour alone: row labels, a selection marker and a
//! page indicator in digits).

use crate::ui::{Anchor, CandidateView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect::new(0, 0, 0, 0);

    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> i32 {
        self.right.saturating_sub(self.left)
    }

    pub fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
}

/// Text measurements supplied by the caller (from GDI in production, synthetic in tests).
pub trait TextMetrics {
    fn text_width(&self, text: &str) -> i32;
    fn line_height(&self) -> i32;
}

/// The marker drawn in front of the highlighted row. Text, not colour: it stays visible in high-contrast themes and for
/// users who cannot tell the highlight colour apart.
pub const SELECTION_MARKER: &str = ">";
pub const ELLIPSIS: &str = "\u{2026}";

/// Design-time sizes in pixels at 96 dpi (scale 1.0); multiplied by the DPI scale and rounded.
const MIN_WIDTH: i32 = 200;
const MAX_WIDTH: i32 = 520;
const BORDER: i32 = 1;
const PAD_X: i32 = 8;
const PAD_Y: i32 = 4;
const ROW_PAD_Y: i32 = 3;
const COLUMN_GAP: i32 = 8;
const DETAIL_GAP: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Under the caret line (the normal case).
    Below,
    /// Above the caret line because the space below is too small.
    Above,
    /// Nothing to show, or no position keeps the window inside the work area without covering the caret line.
    Hidden,
}

/// Text of one row as it will be drawn: single line, already shortened with an ellipsis to its column, so that what is
/// measured is exactly what is painted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    pub label: String,
    pub text: String,
    pub detail: String,
}

/// Left edges (client coordinates) of the columns of a row: marker, label, snippet text, detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub marker_x: i32,
    pub label_x: i32,
    pub text_x: i32,
    pub detail_x: i32,
}

/// Result of [`compute`]. `window` is in screen coordinates; everything else is in client coordinates of the window
/// (origin at its top-left corner).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Rect,
    pub rows: Vec<Rect>,
    pub footer: Option<Rect>,
    pub placement: Placement,
    pub texts: Vec<RowText>,
    pub footer_text: Option<String>,
    pub columns: Columns,
    /// Border thickness in device pixels.
    pub border: i32,
    /// Height of one line of text as measured, for centring text inside a row.
    pub line_height: i32,
}

impl Layout {
    pub fn hidden() -> Layout {
        Layout {
            window: Rect::EMPTY,
            rows: Vec::new(),
            footer: None,
            placement: Placement::Hidden,
            texts: Vec::new(),
            footer_text: None,
            columns: Columns {
                marker_x: 0,
                label_x: 0,
                text_x: 0,
                detail_x: 0,
            },
            border: 0,
            line_height: 0,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.placement != Placement::Hidden
    }
}

/// [`compute_scaled`] at 96 dpi.
pub fn compute(
    view: &CandidateView,
    anchor: Anchor,
    work_area: Rect,
    metrics: &dyn TextMetrics,
) -> Layout {
    compute_scaled(view, anchor, work_area, metrics, 1.0)
}

/// Layout of `view` for a caret at `anchor` on a monitor whose usable area is `work_area` (all screen coordinates).
///
/// `scale` is the DPI scale (dpi / 96); it multiplies the paddings and gaps. `metrics` must already measure with the
/// font of that DPI. The window is placed below the caret line, above it when it does not fit below, and clamped
/// horizontally into the work area. When it fits nowhere (or `view` has no rows) the result is
/// [`Placement::Hidden`] rather than a window that leaves the work area or covers the caret.
pub fn compute_scaled(
    view: &CandidateView,
    anchor: Anchor,
    work_area: Rect,
    metrics: &dyn TextMetrics,
    scale: f32,
) -> Layout {
    let work_w = work_area.width();
    let work_h = work_area.height();
    if view.rows.is_empty() || work_w <= 0 || work_h <= 0 {
        return Layout::hidden();
    }
    let sizes = Sizes::new(scale);
    let line_h = metrics.line_height().max(1);
    let row_h = line_h.saturating_add(sizes.row_pad.saturating_mul(2));
    let row_count = i32::try_from(view.rows.len()).unwrap_or(i32::MAX);
    let footer_text = footer_text(view);
    let footer_h = if footer_text.is_some() { row_h } else { 0 };
    let rows_h = row_h.saturating_mul(row_count);
    let top_inset = sizes.border + sizes.pad_y;
    let window_h = rows_h
        .saturating_add(footer_h)
        .saturating_add(top_inset.saturating_mul(2));
    if window_h > work_h {
        return Layout::hidden();
    }

    let width = |s: &str| metrics.text_width(s).max(0);
    let mut texts: Vec<RowText> = view
        .rows
        .iter()
        .map(|row| RowText {
            label: one_line(&row.label),
            text: one_line(&row.text),
            detail: one_line(&row.detail),
        })
        .collect();
    let marker_w = width(SELECTION_MARKER);
    let label_w = texts.iter().map(|t| width(&t.label)).max().unwrap_or(0);
    let text_w = texts.iter().map(|t| width(&t.text)).max().unwrap_or(0);
    let detail_w = texts.iter().map(|t| width(&t.detail)).max().unwrap_or(0);
    let footer_w = footer_text.as_deref().map_or(0, width);

    let edge = sizes.border + sizes.pad_x;
    // saturating: the widths come from the caller's metrics and the text from the host's snippets
    let before_text = marker_w
        .saturating_add(sizes.column_gap)
        .saturating_add(label_w)
        .saturating_add(sizes.column_gap);
    let detail_gap = if detail_w > 0 { sizes.detail_gap } else { 0 };
    let natural_inner = before_text
        .saturating_add(text_w)
        .saturating_add(detail_gap)
        .saturating_add(detail_w)
        .max(footer_w);
    let window_w = natural_inner
        .saturating_add(edge * 2)
        .clamp(sizes.min_width, sizes.max_width)
        .min(work_w);
    let inner_w = window_w - edge * 2;

    // The detail keeps its natural width unless the window had to be narrowed; then it gets at most a third of the row
    // so that the snippet, the part the user picks by, keeps most of the space.
    let detail_w = if inner_w >= natural_inner {
        detail_w
    } else {
        detail_w.min(inner_w.max(0) / 3)
    };
    let detail_gap = if detail_w > 0 { sizes.detail_gap } else { 0 };
    let text_avail = inner_w
        .saturating_sub(before_text)
        .saturating_sub(detail_gap)
        .saturating_sub(detail_w)
        .max(0);
    for t in &mut texts {
        t.text = ellipsize(&t.text, text_avail, metrics);
        t.detail = ellipsize(&t.detail, detail_w, metrics);
    }
    let footer_text = footer_text.map(|t| ellipsize(&t, inner_w.max(0), metrics));

    let Some((placement, left, top)) = place(anchor, work_area, window_w, window_h) else {
        return Layout::hidden();
    };

    let rows = (0..row_count)
        .map(|i| {
            let y = top_inset + i * row_h;
            Rect::new(sizes.border, y, window_w - sizes.border, y + row_h)
        })
        .collect();
    let footer = footer_text.as_ref().map(|_| {
        let y = top_inset + rows_h;
        Rect::new(sizes.border, y, window_w - sizes.border, y + footer_h)
    });
    Layout {
        window: Rect::new(left, top, left + window_w, top + window_h),
        rows,
        footer,
        placement,
        texts,
        footer_text,
        columns: Columns {
            marker_x: edge,
            label_x: edge
                .saturating_add(marker_w)
                .saturating_add(sizes.column_gap)
                .min(window_w),
            text_x: edge.saturating_add(before_text).min(window_w),
            detail_x: window_w - edge - detail_w,
        },
        border: sizes.border,
        line_height: line_h,
    }
}

/// Where a window of `w` x `h` goes: below the caret line when it fits, else above it, x clamped into the work area.
/// `None` when neither side fits (the caret line is never covered).
fn place(anchor: Anchor, work: Rect, w: i32, h: i32) -> Option<(Placement, i32, i32)> {
    // 64-bit so that absurd anchors (the host reports whatever it likes) cannot overflow.
    let (y, h) = (i64::from(anchor.y), i64::from(h));
    let (work_top, work_bottom) = (i64::from(work.top), i64::from(work.bottom));
    let caret_bottom = y + i64::from(anchor.line_height.max(0));
    let (placement, top) = if caret_bottom >= work_top && caret_bottom + h <= work_bottom {
        (Placement::Below, caret_bottom)
    } else if y - h >= work_top && y <= work_bottom {
        (Placement::Above, y - h)
    } else {
        return None;
    };
    // `w` never exceeds the work width, so the clamp cannot invert.
    let left = anchor.x.min(work.right - w).max(work.left);
    Some((placement, left, i32::try_from(top).ok()?))
}

struct Sizes {
    border: i32,
    pad_x: i32,
    pad_y: i32,
    row_pad: i32,
    column_gap: i32,
    detail_gap: i32,
    min_width: i32,
    max_width: i32,
}

impl Sizes {
    fn new(scale: f32) -> Sizes {
        // A broken DPI value must not collapse or explode the window.
        let s = if scale.is_finite() && scale > 0.0 {
            scale.clamp(0.5, 8.0)
        } else {
            1.0
        };
        let px = |design: i32| (design as f32 * s).round() as i32;
        Sizes {
            border: px(BORDER).max(1),
            pad_x: px(PAD_X),
            pad_y: px(PAD_Y),
            row_pad: px(ROW_PAD_Y),
            column_gap: px(COLUMN_GAP),
            detail_gap: px(DETAIL_GAP),
            min_width: px(MIN_WIDTH),
            max_width: px(MAX_WIDTH),
        }
    }
}

/// The page indicator, shown only when there is more than one page: digits and the keys, no colour, e.g.
/// "2/3  PgUp/PgDn". `view.page` is zero-based.
pub fn footer_text(view: &CandidateView) -> Option<String> {
    if view.page_count <= 1 {
        return None;
    }
    let page = view.page.min(view.page_count - 1) + 1;
    Some(format!("{}/{}  PgUp/PgDn", page, view.page_count))
}

/// Turn a snippet into a one-line preview: line breaks and other control characters become a single space, runs of
/// spaces right after a break collapse, and the bidirectional formatting characters (which could make the preview read
/// differently from what would be inserted) are replaced by U+FFFD. Only the preview changes, never the inserted text.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_break = false;
    for c in text.chars() {
        if c.is_control() {
            after_break = true;
        } else if after_break && c == ' ' {
            // collapsed
        } else {
            if after_break && !out.is_empty() {
                out.push(' ');
            }
            after_break = false;
            out.push(if is_bidi_control(c) { '\u{FFFD}' } else { c });
        }
    }
    out
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// `text` if it fits `max_width`, else the longest prefix (cut on a `char` boundary) followed by an ellipsis that fits.
/// Empty when not even the ellipsis fits. Width is assumed to grow with the prefix length.
pub fn ellipsize(text: &str, max_width: i32, metrics: &dyn TextMetrics) -> String {
    if metrics.text_width(text) <= max_width {
        return text.to_string();
    }
    if metrics.text_width(ELLIPSIS) > max_width {
        return String::new();
    }
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let candidate = |chars: usize| {
        let end = ends.get(chars).copied().unwrap_or(text.len());
        format!("{}{ELLIPSIS}", text[..end].trim_end())
    };
    // lo chars + ellipsis fits (0 does), hi chars + ellipsis does not (the whole text did not fit)
    let (mut lo, mut hi) = (0, ends.len() - 1);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if metrics.text_width(&candidate(mid)) <= max_width {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    candidate(lo)
}

/// What a screen reader should say about the window (UT-057): the page, the position of the selected candidate, which
/// key picks it, its text and its source/validation in words. Set as the window name by the popup.
pub fn accessible_name(view: &CandidateView) -> String {
    let mut out = String::from("Candidates");
    if view.page_count > 1 {
        let page = view.page.min(view.page_count - 1) + 1;
        out.push_str(&format!(", page {page} of {}", view.page_count));
    }
    if let Some(row) = view.rows.get(view.selected_in_page) {
        let text: String = one_line(&row.text).chars().take(120).collect();
        out.push_str(&format!(
            ". Selected {} of {}, press {} to choose: {text}, {}",
            view.selected_in_page + 1,
            view.rows.len(),
            one_line(&row.label),
            one_line(&row.detail)
        ));
    }
    out
}

/// What a rectangle is filled with. The popup maps each tone to a system colour, so high-contrast themes apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillTone {
    Window,
    Frame,
    Selection,
    Separator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTone {
    Normal,
    /// Secondary text (source and validation).
    Dim,
    /// Text drawn on a [`FillTone::Selection`] fill.
    OnSelection,
}

/// One painting step, in client coordinates and in painting order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawOp {
    Fill {
        rect: Rect,
        tone: FillTone,
    },
    /// `text` is drawn with its top-left at (`x`, `y`) and clipped to `clip`.
    Text {
        text: String,
        x: i32,
        y: i32,
        clip: Rect,
        tone: TextTone,
    },
}

/// The paint list for `layout` with row `selected` (clamped) highlighted. The highlighted row is marked twice, by the
/// [`FillTone::Selection`] fill and by [`SELECTION_MARKER`] text in front of its label, so it does not depend on colour.
pub fn draw_ops(layout: &Layout, selected: usize) -> Vec<DrawOp> {
    let mut ops = Vec::new();
    if !layout.is_visible() {
        return ops;
    }
    let (w, h, b) = (layout.window.width(), layout.window.height(), layout.border);
    let fill = |rect, tone| DrawOp::Fill { rect, tone };
    ops.push(fill(Rect::new(0, 0, w, h), FillTone::Window));
    for edge in [
        Rect::new(0, 0, w, b),
        Rect::new(0, h - b, w, h),
        Rect::new(0, 0, b, h),
        Rect::new(w - b, 0, w, h),
    ] {
        ops.push(fill(edge, FillTone::Frame));
    }
    let text = |s: &str, x: i32, right: i32, band: Rect, tone| DrawOp::Text {
        text: s.to_string(),
        x,
        y: band.top + (band.height() - layout.line_height) / 2,
        clip: Rect::new(x, band.top, right, band.bottom),
        tone,
    };
    let cols = layout.columns;
    let selected = selected.min(layout.rows.len().saturating_sub(1));
    for (i, (row, t)) in layout.rows.iter().zip(&layout.texts).enumerate() {
        let on = i == selected;
        let (normal, dim) = if on {
            (TextTone::OnSelection, TextTone::OnSelection)
        } else {
            (TextTone::Normal, TextTone::Dim)
        };
        if on {
            ops.push(fill(*row, FillTone::Selection));
            ops.push(text(
                SELECTION_MARKER,
                cols.marker_x,
                cols.label_x,
                *row,
                normal,
            ));
        }
        for (s, x, right, tone) in [
            (&t.label, cols.label_x, cols.text_x, normal),
            (&t.text, cols.text_x, cols.detail_x, normal),
            (&t.detail, cols.detail_x, row.right - b, dim),
        ] {
            if !s.is_empty() {
                ops.push(text(s, x, right, *row, tone));
            }
        }
    }
    if let (Some(f), Some(s)) = (layout.footer, layout.footer_text.as_deref()) {
        ops.push(fill(
            Rect::new(f.left, f.top, f.right, f.top + b),
            FillTone::Separator,
        ));
        ops.push(text(s, cols.marker_x, f.right, f, TextTone::Normal));
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::CandidateRow;

    /// Fixed-pitch metrics: ASCII is `cw` wide, anything else (CJK, ellipsis, middle dot, emoji) is twice that.
    struct Fixed {
        cw: i32,
        lh: i32,
    }

    impl Fixed {
        fn at(scale: f32) -> Fixed {
            Fixed {
                cw: (8.0 * scale).round() as i32,
                lh: (16.0 * scale).round() as i32,
            }
        }
    }

    impl TextMetrics for Fixed {
        fn text_width(&self, text: &str) -> i32 {
            text.chars()
                .map(|c| if c.is_ascii() { self.cw } else { self.cw * 2 })
                .sum()
        }
        fn line_height(&self) -> i32 {
            self.lh
        }
    }

    fn row(i: usize, text: &str, detail: &str) -> CandidateRow {
        CandidateRow {
            label: (i + 1).to_string(),
            text: text.to_string(),
            detail: detail.to_string(),
        }
    }

    fn view(rows: usize, page: usize, page_count: usize) -> CandidateView {
        CandidateView {
            rows: (0..rows)
                .map(|i| row(i, "fn main() { \u{2026} }", "rule \u{b7} valid"))
                .collect(),
            selected_in_page: 0,
            page,
            page_count,
        }
    }

    fn anchor(x: i32, y: i32) -> Anchor {
        Anchor {
            x,
            y,
            line_height: 20,
        }
    }

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);
    const M: Fixed = Fixed { cw: 8, lh: 16 };

    // With the fixed metrics at scale 1.0 a three-row single-page view is 298 x 76:
    // width = 2*(border 1 + pad 8) + marker 8 + gap 8 + label 8 + gap 8 + text 128 + gap 16 + detail 104,
    // height = 2*(border 1 + pad 4) + 3 rows of (16 + 2*3).
    const W3: i32 = 298;
    const H3: i32 = 76;

    #[test]
    fn ut_055_places_the_window_directly_below_the_caret_line() {
        let l = compute(&view(3, 0, 1), anchor(100, 100), SCREEN, &M);
        assert_eq!(l.placement, Placement::Below);
        assert_eq!(l.window, Rect::new(100, 120, 100 + W3, 120 + H3));
    }

    #[test]
    fn ut_055_flips_above_the_caret_when_the_bottom_edge_is_too_close() {
        // exactly fits below: caret bottom 1004 + 76 = 1080
        let l = compute(&view(3, 0, 1), anchor(100, 984), SCREEN, &M);
        assert_eq!(l.placement, Placement::Below);
        assert_eq!(l.window.bottom, 1080);
        // one pixel lower and it flips; the window then ends exactly at the caret line
        let l = compute(&view(3, 0, 1), anchor(100, 985), SCREEN, &M);
        assert_eq!(l.placement, Placement::Above);
        assert_eq!(l.window, Rect::new(100, 985 - H3, 100 + W3, 985));
    }

    #[test]
    fn ut_055_top_edge_stays_below_and_a_caret_above_the_work_area_is_not_shown() {
        let l = compute(&view(3, 0, 1), anchor(100, 0), SCREEN, &M);
        assert_eq!((l.placement, l.window.top), (Placement::Below, 20));
        let l = compute(&view(3, 0, 1), anchor(100, -100), SCREEN, &M);
        assert_eq!(l.placement, Placement::Hidden);
    }

    #[test]
    fn ut_055_clamps_into_the_work_area_at_the_right_edge() {
        let l = compute(&view(3, 0, 1), anchor(1900, 100), SCREEN, &M);
        assert_eq!((l.window.right, l.window.left), (1920, 1920 - W3));
        // and when the caret is far beyond the edge
        let l = compute(&view(3, 0, 1), anchor(50_000, 100), SCREEN, &M);
        assert_eq!(l.window.right, 1920);
    }

    #[test]
    fn ut_055_clamps_into_the_work_area_at_the_left_edge_including_negative_monitors() {
        let left_monitor = Rect::new(-1920, 0, 0, 1080);
        let l = compute(&view(3, 0, 1), anchor(-3000, 100), left_monitor, &M);
        assert_eq!(l.window.left, -1920);
        let l = compute(&view(3, 0, 1), anchor(-10, 100), left_monitor, &M);
        assert_eq!((l.window.left, l.window.right), (-W3, 0));
        let l = compute(&view(3, 0, 1), anchor(-1000, 100), left_monitor, &M);
        assert_eq!(l.window.left, -1000);
    }

    #[test]
    fn ut_055_monitor_above_the_primary_one_uses_its_own_work_area() {
        let upper = Rect::new(0, -1080, 1920, 0);
        let l = compute(&view(3, 0, 1), anchor(10, -30), upper, &M);
        assert_eq!(l.placement, Placement::Above);
        assert_eq!(l.window.bottom, -30);
    }

    #[test]
    fn ut_055_a_window_that_fits_neither_side_is_not_shown() {
        let short = Rect::new(0, 0, 1920, 100);
        // taller than the whole work area
        assert_eq!(
            compute(&view(9, 0, 1), anchor(0, 0), short, &M).placement,
            Placement::Hidden
        );
        // 76 px high in a 100 px work area with the caret in the middle: 60 below and 40 above are too little
        let l = compute(&view(3, 0, 1), anchor(0, 40), short, &M);
        assert_eq!(l.placement, Placement::Hidden);
        assert_eq!(l.window, Rect::EMPTY);
        assert!(l.rows.is_empty() && l.footer.is_none() && l.texts.is_empty());
    }

    #[test]
    fn ut_055_degenerate_work_areas_and_extreme_anchors_do_not_panic() {
        for work in [
            Rect::new(0, 0, 0, 0),
            Rect::new(10, 10, 5, 5),
            Rect::new(0, 0, 1, 1),
            Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        ] {
            for (x, y) in [(0, 0), (i32::MAX, i32::MAX), (i32::MIN, i32::MIN)] {
                let a = Anchor {
                    x,
                    y,
                    line_height: i32::MAX,
                };
                let l = compute(&view(3, 0, 2), a, work, &M);
                if l.is_visible() {
                    assert!(l.window.left >= work.left && l.window.right <= work.right);
                }
            }
        }
    }

    #[test]
    fn ut_055_absurd_text_widths_from_the_metrics_do_not_overflow() {
        struct Huge;
        impl TextMetrics for Huge {
            fn text_width(&self, _: &str) -> i32 {
                i32::MAX
            }
            fn line_height(&self) -> i32 {
                16
            }
        }
        let l = compute(&view(3, 0, 2), anchor(100, 100), SCREEN, &Huge);
        assert_eq!(l.window.width(), 520);
        assert!(l.texts.iter().all(|t| t.text.is_empty()));
    }

    #[test]
    fn ut_055_empty_view_shows_nothing() {
        let empty = CandidateView {
            rows: Vec::new(),
            selected_in_page: 0,
            page: 0,
            page_count: 3,
        };
        let l = compute(&empty, anchor(10, 10), SCREEN, &M);
        assert!(!l.is_visible());
        assert_eq!(l, Layout::hidden());
    }

    #[test]
    fn ut_055_a_narrow_work_area_narrows_the_window_and_shortens_the_text() {
        let narrow = Rect::new(0, 0, 120, 600);
        let l = compute(&view(3, 0, 2), anchor(100, 100), narrow, &M);
        assert_eq!(l.window.width(), 120);
        assert_eq!((l.window.left, l.window.right), (0, 120));
        for t in &l.texts {
            assert!(t.text.ends_with('\u{2026}'), "{t:?}");
        }
        for t in &l.texts {
            assert!(l.columns.text_x + M.text_width(&t.text) <= l.columns.detail_x - 16);
            assert!(l.columns.detail_x + M.text_width(&t.detail) <= 120 - 9);
        }
    }

    #[test]
    fn very_long_snippets_are_cut_with_an_ellipsis_and_the_window_stays_at_its_maximum_width() {
        let mut v = view(2, 0, 1);
        v.rows[0].text = "a".repeat(200);
        v.rows[1].text = "\u{89c4}\u{5219}".repeat(100); // multi-byte characters
        let l = compute(&v, anchor(100, 100), SCREEN, &M);
        assert_eq!(l.window.width(), 520);
        // 502 inner width - 32 before the text - 16 gap - 104 detail = 350 for the snippet
        for t in &l.texts {
            assert!(t.text.ends_with('\u{2026}'));
            assert!(M.text_width(&t.text) <= 350, "{}", M.text_width(&t.text));
        }
        // the longest prefix that fits: one more character would not
        assert_eq!(l.texts[0].text, format!("{}\u{2026}", "a".repeat(41)));
        assert_eq!(M.text_width(&l.texts[0].text), 41 * 8 + 16);
        // the detail is intact
        assert_eq!(l.texts[0].detail, "rule \u{b7} valid");
    }

    #[test]
    fn ellipsize_is_char_safe_and_handles_zero_width() {
        let text = "\u{1f600}\u{89c4}\u{5219}x"; // 4-byte, 3-byte, 3-byte, 1-byte
        assert_eq!(ellipsize(text, 1000, &M), text);
        assert_eq!(ellipsize(text, 15, &M), ""); // not even the ellipsis (16) fits
        assert_eq!(ellipsize(text, 16, &M), "\u{2026}");
        assert_eq!(ellipsize(text, 47, &M), "\u{1f600}\u{2026}");
        assert_eq!(ellipsize(text, 48, &M), "\u{1f600}\u{89c4}\u{2026}");
        assert_eq!(ellipsize("", 0, &M), "");
        assert_eq!(ellipsize("abc  def", 8 * 3 + 16, &M), "abc\u{2026}"); // trailing space dropped
    }

    #[test]
    fn one_line_flattens_breaks_and_neutralises_bidi_controls() {
        assert_eq!(one_line("fn a() {\n    x\r\n}"), "fn a() { x }");
        assert_eq!(one_line("\n\tlead\t\ttrail\n"), "lead trail");
        assert_eq!(one_line("a  b"), "a  b"); // ordinary spaces inside a line are kept
        assert_eq!(one_line("a\u{202e}b\u{2066}c"), "a\u{fffd}b\u{fffd}c");
        assert_eq!(one_line("\u{89c4}\u{5219}"), "\u{89c4}\u{5219}");
    }

    #[test]
    fn single_page_has_no_footer_and_multi_page_shows_the_page_in_digits() {
        let one = compute(&view(3, 0, 1), anchor(100, 100), SCREEN, &M);
        assert!(one.footer.is_none() && one.footer_text.is_none());
        assert_eq!(one.window.height(), H3);

        let two = compute(&view(3, 1, 3), anchor(100, 100), SCREEN, &M);
        assert_eq!(two.footer_text.as_deref(), Some("2/3  PgUp/PgDn"));
        let footer = two.footer.unwrap();
        assert_eq!(footer.top, two.rows[2].bottom);
        assert_eq!(footer.height(), two.rows[0].height());
        assert_eq!(two.window.height(), H3 + footer.height());
        assert_eq!(
            footer_text(&view(1, 0, 4)).as_deref(),
            Some("1/4  PgUp/PgDn")
        );
        assert_eq!(
            footer_text(&view(1, 9, 4)).as_deref(),
            Some("4/4  PgUp/PgDn")
        );
    }

    #[test]
    fn the_footer_can_widen_the_window_beyond_the_rows() {
        let mut v = view(1, 0, 12);
        v.rows[0].text = "x".to_string();
        v.rows[0].detail = "r".to_string();
        // wide glyphs: the 15-character indicator is 240 px, the row only 96 px, so the footer sets the width
        let wide = Fixed { cw: 16, lh: 16 };
        let l = compute(&v, anchor(0, 0), SCREEN, &wide);
        assert_eq!(l.window.width(), 240 + 18);
        // and without a footer the same row is just the minimum width
        v.page_count = 1;
        let l = compute(&v, anchor(0, 0), SCREEN, &wide);
        assert_eq!(l.window.width(), 200);
    }

    #[test]
    fn rows_stack_inside_the_window_without_overlap() {
        let l = compute(&view(5, 0, 2), anchor(10, 10), SCREEN, &M);
        assert_eq!(l.rows.len(), 5);
        let client = Rect::new(0, 0, l.window.width(), l.window.height());
        let mut previous_bottom = l.border + 4;
        for r in l.rows.iter().chain(l.footer.iter()) {
            assert_eq!(r.top, previous_bottom);
            assert!(r.left >= client.left && r.right <= client.right);
            assert!(r.bottom <= client.bottom);
            previous_bottom = r.bottom;
        }
        assert_eq!(client.bottom, previous_bottom + l.border + 4);
    }

    #[test]
    fn ut_057_the_selected_row_has_room_for_a_text_marker_in_front_of_the_label() {
        let l = compute(&view(3, 0, 1), anchor(100, 100), SCREEN, &M);
        assert_eq!(l.columns.marker_x, 9);
        assert!(l.columns.label_x >= l.columns.marker_x + M.text_width(SELECTION_MARKER));
        assert!(l.columns.text_x > l.columns.label_x);
        assert!(l.columns.detail_x > l.columns.text_x + 128);
        assert!(!SELECTION_MARKER.trim().is_empty());
    }

    #[test]
    fn ut_057_the_accessible_name_carries_position_key_text_and_source_in_words() {
        let mut v = view(3, 1, 3);
        v.selected_in_page = 1;
        v.rows[1].text = "fn main() -> Result<()> {\n}".to_string();
        let name = accessible_name(&v);
        assert!(name.contains("page 2 of 3"), "{name}");
        assert!(name.contains("Selected 2 of 3"), "{name}");
        assert!(name.contains("press 2 to choose"), "{name}");
        assert!(name.contains("fn main() -> Result<()> {"), "{name}");
        assert!(name.contains("rule \u{b7} valid"), "{name}");
        assert!(!accessible_name(&view(2, 0, 1)).contains("page"));
        // a stale selection index does not panic
        v.selected_in_page = 99;
        assert_eq!(accessible_name(&v), "Candidates, page 2 of 3");
    }

    fn texts_of(ops: &[DrawOp]) -> Vec<(&str, TextTone)> {
        ops.iter()
            .filter_map(|op| match op {
                DrawOp::Text { text, tone, .. } => Some((text.as_str(), *tone)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ut_057_the_selected_row_is_marked_by_highlight_and_by_a_text_marker_not_colour_alone() {
        let l = compute(&view(3, 0, 1), anchor(100, 100), SCREEN, &M);
        let ops = draw_ops(&l, 1);
        // highlight: exactly one selection fill, covering row 1
        let fills: Vec<Rect> = ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Fill {
                    rect,
                    tone: FillTone::Selection,
                } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(fills, vec![l.rows[1]]);
        // text marker: exactly one, inside row 1, in front of the label
        let markers: Vec<&DrawOp> = ops
            .iter()
            .filter(|op| matches!(op, DrawOp::Text { text, .. } if text == SELECTION_MARKER))
            .collect();
        assert_eq!(markers.len(), 1);
        match markers[0] {
            DrawOp::Text {
                x, y, clip, tone, ..
            } => {
                assert_eq!(*x, l.columns.marker_x);
                assert!(*x < l.columns.label_x);
                assert!(*y >= l.rows[1].top && *y + l.line_height <= l.rows[1].bottom);
                assert!(clip.right <= l.columns.label_x);
                assert_eq!(*tone, TextTone::OnSelection);
            }
            other => panic!("not a text op: {other:?}"),
        }
        // the other rows carry the marker's absence and the normal tones: 2 rows x (label, text, detail)
        let normal: Vec<_> = texts_of(&ops)
            .into_iter()
            .filter(|(_, tone)| *tone != TextTone::OnSelection)
            .collect();
        assert_eq!(normal.len(), 6);
        assert_eq!(
            normal.iter().filter(|(_, t)| *t == TextTone::Dim).count(),
            2
        );
    }

    #[test]
    fn draw_ops_clamp_a_stale_selection_and_skip_empty_strings() {
        let mut v = view(2, 0, 1);
        v.rows[0].label = String::new();
        let l = compute(&v, anchor(0, 0), SCREEN, &M);
        let ops = draw_ops(&l, 99);
        assert!(ops.contains(&DrawOp::Fill {
            rect: l.rows[1],
            tone: FillTone::Selection
        }));
        assert!(texts_of(&ops).iter().all(|(text, _)| !text.is_empty()));
        assert!(draw_ops(&Layout::hidden(), 0).is_empty());
    }

    #[test]
    fn draw_ops_paint_background_then_frame_then_rows_then_footer() {
        let l = compute(&view(2, 1, 2), anchor(0, 0), SCREEN, &M);
        let ops = draw_ops(&l, 0);
        let (w, h) = (l.window.width(), l.window.height());
        assert_eq!(
            ops[0],
            DrawOp::Fill {
                rect: Rect::new(0, 0, w, h),
                tone: FillTone::Window
            }
        );
        let frames = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    DrawOp::Fill {
                        tone: FillTone::Frame,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(frames, 4);
        // the page indicator is the last thing painted and is plain text
        let footer = l.footer.unwrap();
        assert!(matches!(
            &ops[ops.len() - 2],
            DrawOp::Fill { rect, tone: FillTone::Separator } if rect.top == footer.top
        ));
        assert!(matches!(
            ops.last(),
            Some(DrawOp::Text { text, tone: TextTone::Normal, .. }) if text == "2/2  PgUp/PgDn"
        ));
        // a single page has no separator and no indicator
        let single = draw_ops(&compute(&view(2, 0, 1), anchor(0, 0), SCREEN, &M), 0);
        assert!(!single.iter().any(|op| matches!(
            op,
            DrawOp::Fill {
                tone: FillTone::Separator,
                ..
            }
        )));
    }

    #[test]
    fn every_paint_op_stays_inside_the_client_area() {
        for work in [SCREEN, Rect::new(0, 0, 120, 600)] {
            let l = compute(&view(4, 2, 5), anchor(10, 10), work, &M);
            let client = Rect::new(0, 0, l.window.width(), l.window.height());
            for op in draw_ops(&l, 2) {
                let rect = match op {
                    DrawOp::Fill { rect, .. } => rect,
                    DrawOp::Text { clip, .. } => clip,
                };
                assert!(
                    rect.left >= client.left
                        && rect.top >= client.top
                        && rect.right <= client.right
                        && rect.bottom <= client.bottom,
                    "{op:?} outside {client:?}"
                );
            }
        }
    }

    #[test]
    fn dpi_two_doubles_every_dimension() {
        let one = compute_scaled(
            &view(3, 0, 1),
            anchor(100, 100),
            SCREEN,
            &Fixed::at(1.0),
            1.0,
        );
        let two = compute_scaled(
            &view(3, 0, 1),
            anchor(100, 100),
            SCREEN,
            &Fixed::at(2.0),
            2.0,
        );
        assert_eq!(two.window.width(), one.window.width() * 2);
        assert_eq!(two.window.height(), one.window.height() * 2);
        assert_eq!(two.rows[0].height(), 44);
        assert_eq!(two.border, 2);
        assert_eq!(two.columns.marker_x, 2 * one.columns.marker_x);
    }

    #[test]
    fn dpi_one_and_a_half_rounds_each_padding_to_whole_pixels() {
        let l = compute_scaled(
            &view(3, 0, 1),
            anchor(100, 100),
            SCREEN,
            &Fixed::at(1.5),
            1.5,
        );
        // border 2, paddings 12/6/5, gaps 12/24, characters 12 px, line 24 px
        assert_eq!(l.border, 2);
        assert_eq!(l.rows[0].height(), 24 + 2 * 5);
        assert_eq!(l.window.width(), 448);
        assert_eq!(l.window.height(), 118);
        assert_eq!(l.window.top, 120);
    }

    #[test]
    fn dpi_scales_the_minimum_width_and_flips_with_the_taller_window() {
        let mut v = view(1, 0, 1);
        v.rows[0].text = "x".to_string();
        v.rows[0].detail = "r".to_string();
        let l = compute_scaled(&v, anchor(0, 0), SCREEN, &Fixed::at(2.0), 2.0);
        assert_eq!(l.window.width(), 400);
        // at 200 % the three-row window is 152 px high: it no longer fits below a caret at y = 930
        let low = compute_scaled(&view(3, 0, 1), anchor(0, 930), SCREEN, &Fixed::at(2.0), 2.0);
        assert_eq!(low.placement, Placement::Above);
        let low = compute_scaled(&view(3, 0, 1), anchor(0, 930), SCREEN, &M, 1.0);
        assert_eq!(low.placement, Placement::Below);
    }

    #[test]
    fn a_broken_scale_behaves_like_one() {
        let base = compute(&view(3, 0, 1), anchor(100, 100), SCREEN, &M);
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let l = compute_scaled(&view(3, 0, 1), anchor(100, 100), SCREEN, &M, bad);
            assert_eq!(l, base, "scale {bad}");
        }
    }

    #[test]
    fn the_window_stays_in_the_work_area_and_off_the_caret_line_everywhere() {
        let areas = [
            SCREEN,
            Rect::new(-1920, 0, 0, 1080),
            Rect::new(0, -1080, 1920, 0),
            Rect::new(100, 50, 900, 650),
            Rect::new(0, 0, 330, 260),
        ];
        let views = [view(1, 0, 1), view(9, 0, 3)];
        for work in areas {
            for v in &views {
                for ax in (work.left - 200..work.right + 200).step_by(97) {
                    for ay in (work.top - 200..work.bottom + 200).step_by(41) {
                        let a = anchor(ax, ay);
                        let l = compute(v, a, work, &M);
                        if !l.is_visible() {
                            continue;
                        }
                        let w = l.window;
                        assert!(
                            w.left >= work.left
                                && w.right <= work.right
                                && w.top >= work.top
                                && w.bottom <= work.bottom,
                            "{work:?} {a:?} -> {w:?}"
                        );
                        assert!(
                            w.bottom <= a.y || w.top >= a.y + a.line_height,
                            "covers the caret line: {a:?} -> {w:?}"
                        );
                    }
                }
            }
        }
    }
}
