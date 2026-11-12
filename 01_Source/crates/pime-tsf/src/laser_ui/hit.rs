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
