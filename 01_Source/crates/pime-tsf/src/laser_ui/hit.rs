//! The hit tests of the laser interface: what is under the mouse.
//!
//! Three questions are asked here: is the point of the screen on the **mouth** of light (the lens itself, or the
//! slit of light on the edge of the screen), and which **row** of the panel is under the mouse (the row that is lit,
//! and the action of a click on it).

use crate::laser::{Mouth, PanelAction, PanelItem, PanelLayout, mouth_hit};
use crate::layout::Rect;

/// Whether the point of the screen is on the **lens** of the mouth.
///
/// The lens is the mouth itself (the round opening), not the whole band of light: the mouth is the handle of the
/// panel, and a drag of it moves it.
pub(super) fn on_the_lens(mouth: &Mouth, screen_x: i32, screen_y: i32) -> bool {
    mouth.contains(screen_x, screen_y)
}

/// Whether the point of the screen is on the **light** of the mouth (the slit on the edge of the screen).
pub(super) fn on_the_light(mouth: &Mouth, screen_x: i32, screen_y: i32) -> bool {
    mouth_hit(mouth, screen_x, screen_y)
}

/// The row of the panel that is under the point `(x, y)` (the coordinates of the window of the panel).
pub(super) fn row_under(layout: &PanelLayout, x: i32, y: i32) -> Option<usize> {
    layout.rows.iter().position(|(_, rect)| rect.contains(x, y))
}

/// The action of a click on the point `(x, y)` of the panel.
pub(super) fn action_under(layout: &PanelLayout, x: i32, y: i32) -> Option<PanelAction> {
    layout.action_at(x, y)
}

/// The rectangle of the row with the key `key`, in screen coordinates.
///
/// The rows of the panel are the ones of the settings, and the menu of a row opens under it: the owner of the
/// interface asks where the row is.
pub(super) fn row_rect(layout: &PanelLayout, origin: Rect, key: &str) -> Option<Rect> {
    layout.rows.iter().find_map(|(item, rect)| {
        let matches = match item {
            PanelItem::Toggle { key: k, .. } => *k == key,
            PanelItem::Menu { key: k, .. } => *k == key,
            PanelItem::Demo => false,
            PanelItem::Settings => key == "settings",
            PanelItem::Candidate(_) => false,
        };
        if !matches {
            return None;
        }
        Some(Rect::new(
            origin.left + rect.left,
            origin.top + rect.top,
            origin.left + rect.right,
            origin.top + rect.bottom,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laser::{Edge, mouth_slit};

    /// A mouth on the left edge of the work area, in the middle of the screen.
    fn mouth() -> Mouth {
        Mouth {
            x: 20,
            y: 300,
            radius: 12,
            edge: Edge::Left,
        }
    }

    /// The panel with three rows: a switch, a menu and the row of the settings.
    fn layout() -> PanelLayout {
        let items = vec![
            PanelItem::Toggle {
                key: "appearance",
                label: "appearance",
                on: false,
            },
            PanelItem::Menu {
                key: "effort",
                label: "effort",
                value: "medium".to_string(),
            },
            PanelItem::Settings,
        ];
        PanelLayout::compute(&items, 30, 4, 300)
    }

    /// The lens is the round opening of the mouth; the light is the slit **on the edge of the screen**, and the
    /// hit area follows the light (the user asked for the light against the edge on 2026-11-12, D-103).
    #[test]
    fn the_lens_is_the_opening_and_the_light_is_the_slit_on_the_edge() {
        let m = mouth();
        // the centre of the mouth is the lens
        assert!(on_the_lens(&m, 20, 300), "the centre of the mouth is the lens");
        // and it is not the light: the light is one radius away, on the edge of the screen
        assert!(!on_the_light(&m, 20, 300), "the lens is not the light");
        // the slit sits on the edge itself and it is the light the user sees
        let slit = mouth_slit(&m);
        assert_eq!(slit.x, 8, "the slit sits on the edge of the screen");
        assert!(on_the_light(&m, slit.x, 300), "the slit is the light");
        // far away from both
        assert!(!on_the_lens(&m, 200, 300), "far away is not the lens");
        assert!(!on_the_light(&m, 200, 300), "far away is not the light");
    }

    /// The row under the mouse is the one whose rectangle holds the point.
    #[test]
    fn the_row_under_the_mouse_is_the_one_that_holds_the_point() {
        let layout = layout();
        assert_eq!(row_under(&layout, 10, 10), Some(0), "the first row");
        assert_eq!(row_under(&layout, 10, 40), Some(1), "the second row");
        assert_eq!(row_under(&layout, 10, 70), Some(2), "the third row");
        assert_eq!(row_under(&layout, 10, 200), None, "below the panel");
        assert_eq!(row_under(&layout, 2, 10), None, "left of the rows");
    }

    /// The action of a click is the action of that row: a switch that is off is turned on.
    #[test]
    fn the_action_under_the_mouse_is_the_action_of_that_row() {
        let layout = layout();
        assert_eq!(
            action_under(&layout, 10, 10),
            Some(PanelAction::Toggle {
                key: "appearance",
                on: true
            }),
            "the switch is off, the click turns it on"
        );
        assert_eq!(
            action_under(&layout, 10, 40),
            Some(PanelAction::Menu { key: "effort" }),
            "the second row opens the effort menu"
        );
        assert_eq!(
            action_under(&layout, 10, 70),
            Some(PanelAction::Settings),
            "the third row opens the settings"
        );
        assert_eq!(action_under(&layout, 10, 200), None, "below the panel");
    }

    /// The rectangle of a row is in the coordinates of the **screen**, not of the panel: the menu of a row opens
    /// under it, and the panel can be anywhere on the screen.
    #[test]
    fn the_rectangle_of_a_row_is_in_the_coordinates_of_the_screen() {
        let layout = layout();
        let origin = Rect::new(100, 200, 400, 300);
        let row = row_rect(&layout, origin, "appearance").expect("the row of the switch");
        assert_eq!(
            (row.left, row.top, row.right, row.bottom),
            (104, 204, 396, 234),
            "the rectangle of the row is the row moved by the origin of the panel"
        );
        let settings = row_rect(&layout, origin, "settings").expect("the row of the settings");
        assert_eq!(settings.top, 264, "the third row is lower");
        assert!(
            row_rect(&layout, origin, "no-such-row").is_none(),
            "a key that is not in the panel has no row"
        );
    }
}
