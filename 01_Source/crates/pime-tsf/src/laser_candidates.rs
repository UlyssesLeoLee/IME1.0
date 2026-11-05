//! The candidate list of the input method: the panel of the laser interface.
//!
//! The list is **not** drawn next to the caret any more. The Sogou-style window that floated over the text was in
//! the way of the code the user writes, and it was the last window of the old style; the list is now drawn in the
//! panel that hangs on the mouth of the light, on the left of the screen, together with the rest of the interface.
//!
//! The panel is the list: it opens by itself while there are candidates and it closes when they are gone. The
//! numbers, the arrows and the Tab key take a candidate exactly as they did with the window next to the caret, and a
//! click on a row of the panel takes it as well.

use std::cell::RefCell;
use std::rc::Rc;

use crate::laser::PanelCandidate;
use crate::laser_ui::LaserUi;
use crate::ui::{Anchor, CandidateUi, CandidateView};

/// The list of the candidates, drawn in the panel of the laser interface.
pub struct LaserCandidates {
    /// The interface the panel belongs to.
    laser: Rc<RefCell<LaserUi>>,
    /// Whether the panel is showing the list right now.
    visible: bool,
}

impl LaserCandidates {
    /// A list that is drawn in the panel of the given interface.
    pub fn new(laser: Rc<RefCell<LaserUi>>) -> LaserCandidates {
        LaserCandidates {
            laser,
            visible: false,
        }
    }
}

impl CandidateUi for LaserCandidates {
    fn show(&mut self, view: &CandidateView, _anchor: Anchor) -> bool {
        let rows: Vec<PanelCandidate> = view
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| PanelCandidate {
                label: row.label.clone(),
                text: row.text.clone(),
                detail: row.detail.clone(),
                selected: index == view.selected_in_page,
            })
            .collect();
        // The interface is borrowed while one of its own windows is calling back; then the list cannot be drawn and
        // the caller has to know that nobody sees it.
        let Ok(mut laser) = self.laser.try_borrow_mut() else {
            return false;
        };
        laser.set_candidates(rows);
        self.visible = true;
        true
    }

    fn hide(&mut self) {
        if let Ok(mut laser) = self.laser.try_borrow_mut() {
            laser.clear_candidates();
        }
        self.visible = false;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laser_ui::LaserEvent;
    use crate::ui::CandidateRow;

    fn view(rows: usize, selected: usize) -> CandidateView {
        CandidateView {
            rows: (0..rows)
                .map(|i| CandidateRow {
                    label: (i + 1).to_string(),
                    text: format!("candidate {i}"),
                    detail: "rule".to_string(),
                })
                .collect(),
            selected_in_page: selected,
            page: 0,
            page_count: 1,
        }
    }

    /// The rows of the list become the rows of the panel, and the highlighted one is the one the machine has.
    #[test]
    fn the_rows_of_the_list_become_the_rows_of_the_panel() {
        let laser = LaserUi::create(Rc::new(|_: LaserEvent| {}));
        let mut list = LaserCandidates::new(Rc::clone(&laser));
        assert!(list.show(&view(3, 1), Anchor { x: 0, y: 0, line_height: 0 }));
        let laser = laser.borrow();
        let rows = laser.candidate_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].label, "2");
        assert!(rows[1].selected, "the highlighted row of the list is the highlighted row of the panel");
        assert!(!rows[0].selected);
    }

    /// The list is gone: the panel does not show it any more.
    #[test]
    fn the_list_ends_with_the_panel() {
        let laser = LaserUi::create(Rc::new(|_: LaserEvent| {}));
        let mut list = LaserCandidates::new(Rc::clone(&laser));
        assert!(list.show(&view(2, 0), Anchor { x: 0, y: 0, line_height: 0 }));
        assert!(list.is_visible());
        list.hide();
        assert!(!list.is_visible());
        assert!(laser.borrow().candidate_rows().is_empty());
    }
}
