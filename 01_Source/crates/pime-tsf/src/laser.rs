//! The model of the laser interface: the blue glow on the left of the screen, the beam that shoots out of it when
//! a character is written, and the panel that opens when the mouse rests on the mouth of the glow.
//!
//! Everything in this file is pure: where the mouth is, how far the beam has come, what the panel shows and which
//! row is under the mouse. The windows that show it are in `glow`, `panel` and `logo`; the geometry and the timing
//! live here so that they can be tested without a screen.

use crate::layout::Rect;

/// How long the light shines on the character after it was written (in milliseconds).
///
/// The user asked for "half a second or so": long enough for the eye to catch the character it lit up, short enough
/// not to get in the way of typing.
pub const BEAM_DURATION_MS: u64 = 520;

/// How long the mouse has to rest on the mouth before the panel opens (in milliseconds).
pub const HOVER_OPEN_MS: u64 = 220;

/// How long the mouse may be away before the panel closes again (in milliseconds).
pub const HOVER_CLOSE_MS: u64 = 420;

/// The mouth of the glow: the opening the light comes out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouth {
    /// The x of the centre of the mouth (it sits on the left edge of the work area).
    pub x: i32,
    /// The y of the centre of the mouth.
    pub y: i32,
    /// How big the opening is (the radius of the lens).
    pub radius: i32,
    /// The edge of the screen the mouth sits on: the slit of light follows it.
    pub edge: Edge,
}

impl MouthPlace {
    /// The default place: the left edge, in the middle of the work area.
    pub fn default_in(work: Rect) -> MouthPlace {
        MouthPlace {
            edge: Edge::Left,
            offset: work.top + work.height() / 2,
        }
    }

    /// The place as it is written in the settings file ("left:520").
    pub fn to_text(&self) -> String {
        let name = match self.edge {
            Edge::Left => "left",
            Edge::Top => "top",
            Edge::Right => "right",
        };
        format!("{name}:{}", self.offset)
    }

    /// The place out of the text of the settings file. `None` when the text is not one.
    pub fn from_text(text: &str) -> Option<MouthPlace> {
        let (edge, offset) = text.split_once(':')?;
        let edge = match edge.trim() {
            "left" => Edge::Left,
            "top" => Edge::Top,
            "right" => Edge::Right,
            _ => return None,
        };
        let offset: i32 = offset.trim().parse().ok()?;
        Some(MouthPlace { edge, offset })
    }
}

impl Mouth {
    /// The mouth on the left edge of a work area, at the height of `centre_y`.
    pub fn on_the_left_edge(work: Rect, centre_y: i32, radius: i32) -> Mouth {
        Mouth {
            x: work.left + radius,
            y: centre_y.clamp(work.top + radius, (work.bottom - radius).max(work.top + radius)),
            radius: radius.max(1),
            edge: Edge::Left,
        }
    }

    /// Whether the point `(x, y)` is on the mouth (the lens itself, not the whole band of light).
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let dx = (x - self.x) as f64;
        let dy = (y - self.y) as f64;
        let radius = self.radius as f64 * 1.35; // a little more than the lens: the mouse does not have to be exact
        dx * dx + dy * dy <= radius * radius
    }
}

/// The edge of the screen the mouth sits on.
///
/// The bottom edge is not one of them: the taskbar is there, and the clock of the system is under it. The mouth is
/// the handle of the panel, and the panel must never cover the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
}

/// Where the mouth is on the screen: on an edge, and how far along it.
///
/// The mouth is never in the middle of the screen and never off it: it belongs to the border of the screen, and the
/// person can drag it along that border.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouthPlace {
    /// The edge the mouth is on.
    pub edge: Edge,
    /// How far the mouth is from the start of the edge: the left end for the top edge, the top end for the sides.
    pub offset: i32,
}

/// The mouth of a place, in screen coordinates.
///
/// The mouth sits **on** the edge: its centre is one radius away from it, so the lens is never cut off by the border
/// of the screen.
pub fn mouth_at(work: Rect, place: MouthPlace, radius: i32) -> Mouth {
    let radius = radius.max(1);
    match place.edge {
        Edge::Left => Mouth {
            x: work.left + radius,
            y: place.offset.clamp(work.top + radius, (work.bottom - radius).max(work.top + radius)),
            radius,
            edge: Edge::Left,
        },
        Edge::Right => Mouth {
            x: work.right - radius,
            y: place.offset.clamp(work.top + radius, (work.bottom - radius).max(work.top + radius)),
            radius,
            edge: Edge::Right,
        },
        Edge::Top => Mouth {
            x: place.offset.clamp(work.left + radius, (work.right - radius).max(work.left + radius)),
            y: work.top + radius,
            radius,
            edge: Edge::Top,
        },
    }
}

/// How long the slit of light at the mouth is (in pixels).
///
/// The user asked for "a small piece of it" ("一小截淡蓝色半透明激光"): long enough to be found at a glance, short
/// enough to be a detail of the screen and not a decoration of it.
pub const SLIT_LENGTH: i32 = 26;

/// How wide the bright core of the slit is (in pixels): a hairline, not a bar.
pub const SLIT_WIDTH: i32 = 1;

/// How far the soft light around the slit reaches (in pixels).
pub const SLIT_HALO: i32 = 5;

/// The visible part of the mouth: a thin slit of light along the edge of the screen.
///
/// The geometry is pure, so that "what the mouth looks like" can be tested without a screen: the painter only has
/// to draw the line this describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouthSlit {
    /// The centre of the slit (the same point as the mouth).
    pub x: i32,
    pub y: i32,
    /// How long the slit is, along the edge of the screen.
    pub length: i32,
    /// How wide the bright core of it is.
    pub width: i32,
    /// How far the soft light around it reaches.
    pub halo: i32,
    /// Whether the slit runs from top to bottom (the mouth is on the left or the right edge) or from left to right
    /// (the mouth is on the top edge).
    pub vertical: bool,
}

/// The slit of light of a mouth.
///
/// It follows the edge the mouth sits on: a vertical slit on the left and the right edge, a horizontal one on the top.
pub fn mouth_slit(mouth: &Mouth) -> MouthSlit {
    MouthSlit {
        x: mouth.x,
        y: mouth.y,
        length: SLIT_LENGTH,
        width: SLIT_WIDTH,
        halo: SLIT_HALO,
        vertical: !matches!(mouth.edge, Edge::Top),
    }
}

/// The place of a mouth that was dropped at the point `(x, y)`.
///
/// The mouth goes to the edge that is nearest: the left, the top or the right. It never leaves the screen, and it
/// never ends up in the middle of it.
pub fn place_at(work: Rect, x: i32, y: i32, radius: i32) -> MouthPlace {
    let radius = radius.max(1);
    let to_left = (x - work.left).abs();
    let to_right = (work.right - x).abs();
    let to_top = (y - work.top).abs();
    // The nearest of the three edges wins. On a tie the left one does (the mouth starts there).
    let edge = if to_left <= to_right && to_left <= to_top {
        Edge::Left
    } else if to_right <= to_top {
        Edge::Right
    } else {
        Edge::Top
    };
    match edge {
        Edge::Left => MouthPlace {
            edge,
            offset: y.clamp(work.top + radius, (work.bottom - radius).max(work.top + radius)),
        },
        Edge::Right => MouthPlace {
            edge,
            offset: y.clamp(work.top + radius, (work.bottom - radius).max(work.top + radius)),
        },
        Edge::Top => MouthPlace {
            edge,
            offset: x.clamp(work.left + radius, (work.right - radius).max(work.left + radius)),
        },
    }
}

/// Where the panel goes when it was dragged: it hangs on the edge that is nearest.
///
/// The panel never covers the clock of the system: the bottom edge is not one of them.
pub fn panel_place(work: Rect, x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    let to_left = (x - work.left).abs();
    let to_right = (work.right - (x + width)).abs();
    let to_top = (y - work.top).abs();
    let max_y = (work.bottom - height).max(work.top);
    if to_left <= to_right && to_left <= to_top {
        (work.left, y.clamp(work.top, max_y))
    } else if to_right <= to_top {
        (work.right - width, y.clamp(work.top, max_y))
    } else {
        (
            x.clamp(work.left, (work.right - width).max(work.left)),
            work.top,
        )
    }
}

/// The place of the mouth of the panel: the panel hangs on the edge the mouth is on.
///
/// A mouth on the left opens the panel to the right of it, a mouth on the right opens it to the left, and a mouth on
/// the top opens it below itself.
pub fn panel_beside(place: MouthPlace, mouth: Mouth, panel: (i32, i32), work: Rect) -> (i32, i32) {
    let (width, height) = panel;
    match place.edge {
        Edge::Left => (mouth.x + mouth.radius + 12, mouth.y - height / 2),
        Edge::Right => (mouth.x - mouth.radius - 12 - width, mouth.y - height / 2),
        Edge::Top => (mouth.x - width / 2, mouth.y + mouth.radius + 12),
    }
    .max_inside(work, width, height)
}

/// Moves a rectangle so that it lies inside the work area.
trait Inside {
    /// The same point, but inside the work area.
    fn max_inside(self, work: Rect, width: i32, height: i32) -> (i32, i32);
}

impl Inside for (i32, i32) {
    fn max_inside(self, work: Rect, width: i32, height: i32) -> (i32, i32) {
        let x = self.0.clamp(work.left, (work.right - width).max(work.left));
        let y = self.1.clamp(work.top, (work.bottom - height).max(work.top));
        (x, y)
    }
}

/// A beam that is on its way to a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beam {
    /// Where the beam starts (the mouth of the glow).
    pub from: (i32, i32),
    /// Where the beam ends (the character that was written).
    pub to: (i32, i32),
    /// When the beam was fired (the clock of the input method, in milliseconds).
    pub started_ms: u64,
}

impl Beam {
    /// A beam from the mouth to a point.
    pub fn new(from: (i32, i32), to: (i32, i32), started_ms: u64) -> Beam {
        Beam {
            from,
            to,
            started_ms,
        }
    }

    /// How long the beam has been alive.
    pub fn elapsed(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.started_ms)
    }

    /// Whether the beam is over.
    pub fn is_finished(&self, now_ms: u64) -> bool {
        self.elapsed(now_ms) >= BEAM_DURATION_MS
    }

    /// How far the head of the beam has travelled (0 = at the mouth, 1 = at the character).
    ///
    /// The head races over the first part of the way and then stays on the character: the light does not travel
    /// slowly, it arrives and then shines.
    pub fn progress(&self, now_ms: u64) -> f64 {
        let elapsed = self.elapsed(now_ms) as f64;
        let travel = (BEAM_DURATION_MS as f64) * 0.45;
        (elapsed / travel).min(1.0)
    }

    /// How bright the beam is (0 = nothing, 1 = the full light).
    ///
    /// The light comes on at once, holds while it travels and then dies away over the character.
    pub fn strength(&self, now_ms: u64) -> f64 {
        let elapsed = self.elapsed(now_ms) as f64;
        if elapsed >= BEAM_DURATION_MS as f64 {
            return 0.0;
        }
        let travel = (BEAM_DURATION_MS as f64) * 0.45;
        let rise = (elapsed / 40.0).min(1.0);
        // the light dies away over the second half of its life
        let fall = 1.0 - ((elapsed - travel) / (BEAM_DURATION_MS as f64 - travel)).clamp(0.0, 1.0);
        (rise * fall).clamp(0.0, 1.0)
    }
}

/// One candidate of the list, as the panel shows it.
///
/// A candidate is a row of the panel like the settings are: the number that takes it, the text of it and
/// where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelCandidate {
    /// The number of the candidate (the key that takes it).
    pub label: String,
    /// The text of the candidate.
    pub text: String,
    /// Where the candidate comes from (the rule, the context, the validation).
    pub detail: String,
    /// Whether this one is the highlighted one.
    pub selected: bool,
}

/// One row of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelItem {
    /// A switch that is on or off.
    Toggle {
        /// What the switch changes (the key the settings file uses).
        key: &'static str,
        /// The text of the row.
        label: &'static str,
        /// Whether the switch is on.
        on: bool,
    },
    /// A row that opens a list.
    Menu {
        /// What the list is for.
        key: &'static str,
        /// The text of the row.
        label: &'static str,
        /// The value that is chosen right now.
        value: String,
    },
    /// The row that opens the settings window.
    Settings,
    /// One candidate of the list.
    Candidate(PanelCandidate),
}

impl PanelItem {
    /// The text of the row.
    pub fn label(&self) -> &str {
        match self {
            PanelItem::Toggle { label, .. } => label,
            PanelItem::Menu { label, .. } => label,
            PanelItem::Settings => "设置",
            PanelItem::Candidate(candidate) => &candidate.text,
        }
    }

    /// The value that is shown on the right of the row.
    pub fn value(&self) -> String {
        match self {
            PanelItem::Toggle { on, .. } => {
                if *on { "开" } else { "关" }
            }
            .to_string(),
            PanelItem::Menu { value, .. } => value.clone(),
            PanelItem::Settings => "…".to_string(),
            PanelItem::Candidate(candidate) => candidate.detail.clone(),
        }
    }

    /// Whether the row is a switch (it is drawn with a light of its own).
    pub fn is_toggle(&self) -> bool {
        matches!(self, PanelItem::Toggle { .. })
    }
}

/// What the user asked the panel to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Switch a setting on or off.
    Toggle { key: &'static str, on: bool },
    /// Open one of the lists.
    Menu { key: &'static str },
    /// Open the settings window.
    Settings,
    /// Take the candidate of the row (the index is the one of the row in the panel).
    Candidate { index: usize },
}

/// The rows of the panel and where they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelLayout {
    /// The size of the panel.
    pub width: i32,
    pub height: i32,
    /// The rectangle of every row, in the order the rows are drawn.
    pub rows: Vec<(PanelItem, Rect)>,
}

impl PanelLayout {
    /// The geometry of a panel with `items` rows.
    pub fn compute(items: &[PanelItem], row_height: i32, padding: i32, width: i32) -> PanelLayout {
        let mut rows = Vec::with_capacity(items.len());
        let mut y = padding;
        for item in items {
            let rect = Rect::new(padding, y, width - padding, y + row_height);
            rows.push((item.clone(), rect));
            y += row_height;
        }
        PanelLayout {
            width,
            height: y + padding,
            rows,
        }
    }

    /// The row under the point `(x, y)`.
    pub fn hit(&self, x: i32, y: i32) -> Option<&PanelItem> {
        self.rows
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(item, _)| item)
    }

    /// The action of a click on the point `(x, y)`.
    pub fn action_at(&self, x: i32, y: i32) -> Option<PanelAction> {
        let (index, (item, _)) = self
            .rows
            .iter()
            .enumerate()
            .find(|(_, (_, rect))| rect.contains(x, y))?;
        match item {
            PanelItem::Toggle { key, on, .. } => Some(PanelAction::Toggle {
                key,
                on: !*on,
            }),
            PanelItem::Menu { key, .. } => Some(PanelAction::Menu { key }),
            PanelItem::Settings => Some(PanelAction::Settings),
            PanelItem::Candidate(_) => Some(PanelAction::Candidate { index }),
        }
    }
}

/// Where the panel is opened from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelOrigin {
    /// The mouse rested on the mouth of the glow.
    Glow,
    /// The logo in the corner was clicked.
    Logo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mouth_is_on_the_left_edge_of_the_work_area() {
        let work = Rect::new(0, 0, 1920, 1040);
        let mouth = Mouth::on_the_left_edge(work, 520, 14);
        assert_eq!(mouth.x, 14);
        assert_eq!(mouth.y, 520);
    }

    #[test]
    fn the_mouth_stays_inside_the_work_area() {
        let work = Rect::new(0, 0, 1920, 200);
        let mouth = Mouth::on_the_left_edge(work, 5000, 14);
        assert!(mouth.y <= work.bottom - mouth.radius);
        let top = Mouth::on_the_left_edge(work, -5000, 14);
        assert!(top.y >= work.top + top.radius);
    }

    #[test]
    fn the_mouth_is_hit_only_by_the_lens() {
        let mouth = Mouth {
            x: 20,
            y: 100,
            radius: 12,
            edge: Edge::Left,
        };
        assert!(mouth.contains(20, 100));
        assert!(!mouth.contains(20, 400), "far away is not the mouth");
        assert!(!mouth.contains(400, 100), "far to the right is not the mouth");
    }

    /// The visible part of the mouth is a thin slit that follows the edge it sits on.
    #[test]
    fn the_slit_of_light_follows_the_edge_of_the_mouth() {
        let left = mouth_at(
            Rect::new(0, 0, 1920, 1040),
            MouthPlace {
                edge: Edge::Left,
                offset: 500,
            },
            12,
        );
        let slit = mouth_slit(&left);
        assert!(slit.vertical, "the mouth is on the left edge: the slit runs from top to bottom");
        assert_eq!((slit.x, slit.y), (left.x, left.y));
        assert_eq!(slit.width, SLIT_WIDTH, "a hairline, not a bar");
        assert_eq!(slit.length, SLIT_LENGTH, "a small piece of light");
        let right = mouth_at(
            Rect::new(0, 0, 1920, 1040),
            MouthPlace {
                edge: Edge::Right,
                offset: 500,
            },
            12,
        );
        assert!(mouth_slit(&right).vertical, "the right edge is vertical too");
        let top = mouth_at(
            Rect::new(0, 0, 1920, 1040),
            MouthPlace {
                edge: Edge::Top,
                offset: 700,
            },
            12,
        );
        assert!(
            !mouth_slit(&top).vertical,
            "the top edge: the slit runs from left to right"
        );
    }

    #[test]
    fn a_mouth_on_an_edge_is_never_cut_off_by_it() {
        let work = Rect::new(0, 0, 1920, 1040);
        let left = mouth_at(work, MouthPlace { edge: Edge::Left, offset: 500 }, 15);
        assert_eq!(left.x, 15);
        assert_eq!(left.y, 500);
        assert!(left.x - left.radius >= work.left, "the lens is on the screen");
        let right = mouth_at(work, MouthPlace { edge: Edge::Right, offset: 500 }, 15);
        assert_eq!(right.x, 1920 - 15);
        assert!(right.x + right.radius <= work.right);
        let top = mouth_at(work, MouthPlace { edge: Edge::Top, offset: 700 }, 15);
        assert_eq!(top.y, 15);
        assert_eq!(top.x, 700);
        assert!(top.y - top.radius >= work.top);
    }

    #[test]
    fn a_mouth_that_is_dragged_goes_to_the_nearest_edge() {
        let work = Rect::new(0, 0, 1920, 1040);
        // near the left edge
        assert_eq!(
            place_at(work, 30, 500, 15),
            MouthPlace {
                edge: Edge::Left,
                offset: 500
            }
        );
        // near the right edge
        assert_eq!(
            place_at(work, 1900, 500, 15),
            MouthPlace {
                edge: Edge::Right,
                offset: 500
            }
        );
        // near the top edge
        assert_eq!(
            place_at(work, 700, 20, 15),
            MouthPlace {
                edge: Edge::Top,
                offset: 700
            }
        );
        // the bottom edge is not one of them: the clock of the system is under it, so the mouth goes to the
        // nearest of the three edges that are allowed
        assert_eq!(
            place_at(work, 700, 1035, 15).edge,
            Edge::Left,
            "the mouth does not go to the bottom"
        );
        assert_eq!(
            place_at(work, 1900, 1035, 15).edge,
            Edge::Right,
            "the mouth does not go to the bottom"
        );
    }

    #[test]
    fn a_mouth_that_is_dragged_cannot_be_dragged_out_of_the_screen() {
        let work = Rect::new(0, 0, 1920, 1040);
        // far outside on every side: the mouth stays on the screen
        for (x, y) in [(-500, 500), (5000, 500), (700, -500), (700, 5000)] {
            let place = place_at(work, x, y, 15);
            let mouth = mouth_at(work, place, 15);
            assert!(mouth.x - mouth.radius >= work.left, "{mouth:?} is off the left");
            assert!(mouth.x + mouth.radius <= work.right, "{mouth:?} is off the right");
            assert!(mouth.y - mouth.radius >= work.top, "{mouth:?} is off the top");
            assert!(mouth.y + mouth.radius <= work.bottom, "{mouth:?} is off the bottom");
        }
    }

    #[test]
    fn a_beam_is_over_after_half_a_second() {
        let beam = Beam::new((0, 0), (100, 100), 1_000);
        assert!(!beam.is_finished(1_000));
        assert!(!beam.is_finished(1_000 + BEAM_DURATION_MS - 1));
        assert!(beam.is_finished(1_000 + BEAM_DURATION_MS));
    }

    #[test]
    fn the_head_of_the_beam_arrives_quickly_and_then_stays() {
        let beam = Beam::new((0, 0), (100, 100), 0);
        assert_eq!(beam.progress(0), 0.0);
        assert!(beam.progress(100) > 0.0 && beam.progress(100) < 1.0);
        assert_eq!(beam.progress(BEAM_DURATION_MS), 1.0, "the head stays on the character");
    }

    #[test]
    fn the_light_comes_on_at_once_and_dies_away_at_the_end() {
        let beam = Beam::new((0, 0), (100, 100), 0);
        assert!(beam.strength(0) < 0.2, "the light needs a moment to come on");
        assert!(beam.strength(60) > 0.9, "the light is at its strongest while it travels");
        assert!(beam.strength(BEAM_DURATION_MS - 1) < 0.3, "the light dies away");
        assert_eq!(beam.strength(BEAM_DURATION_MS), 0.0);
    }

    #[test]
    fn a_click_on_a_switch_turns_it_around() {
        let items = vec![PanelItem::Toggle {
            key: "auto_compose",
            label: "自动开组合",
            on: false,
        }];
        let layout = PanelLayout::compute(&items, 30, 8, 260);
        let action = layout.action_at(20, 15).expect("the row is there");
        assert_eq!(
            action,
            PanelAction::Toggle {
                key: "auto_compose",
                on: true
            }
        );
        assert!(layout.action_at(20, 200).is_none(), "below the rows there is nothing");
    }

    #[test]
    fn a_click_on_a_row_opens_its_list() {
        let items = vec![PanelItem::Menu {
            key: "skin",
            label: "外观",
            value: "彩虹".to_string(),
        }];
        let layout = PanelLayout::compute(&items, 30, 8, 260);
        assert_eq!(
            layout.action_at(20, 15),
            Some(PanelAction::Menu { key: "skin" })
        );
    }

    #[test]
    fn every_row_of_the_panel_is_inside_it() {
        let items = vec![
            PanelItem::Menu {
                key: "language",
                label: "语言",
                value: "中文".to_string(),
            },
            PanelItem::Toggle {
                key: "space_confirms",
                label: "空格上屏",
                on: true,
            },
            PanelItem::Settings,
        ];
        let layout = PanelLayout::compute(&items, 30, 8, 260);
        for (_, rect) in &layout.rows {
            assert!(rect.left >= 0 && rect.right <= layout.width);
            assert!(rect.top >= 0 && rect.bottom <= layout.height);
        }
        assert_eq!(layout.rows.len(), 3);
    }
}
