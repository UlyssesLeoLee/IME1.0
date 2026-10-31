//! The TSF UI-element protocol, provider side, without any Windows type: what the text service tells the host about its
//! candidate list, and when. [`crate::uielement_com`] is the thin COM layer over it.
//!
//! Why it exists: a host that draws its own candidate UI (Store and other immersive apps, accessibility tools) only
//! learns about a TIP's list when the TIP announces it through `ITfUIElementMgr` (`BeginUIElement`, `UpdateUIElement`,
//! `EndUIElement`) and answers `ITfCandidateListUIElement` queries. `BeginUIElement` also returns whether the TIP may
//! draw: when the host answers "no" the native popup must stay hidden, otherwise two lists would show.
//!
//! Three parts, none of them knows COM:
//! * [`ListSnapshot`]: the answers of `ITfCandidateListUIElement`, derived from the [`CandidateView`] the controller
//!   already builds (no second copy of the paging or preview logic: the text of a row is `layout::one_line`).
//! * [`ElementUi`]: a [`CandidateUi`] decorator over the native popup. The controller keeps calling `show` and `hide`
//!   as before; every route that ends a list (hide, commit, cancel, focus loss, deactivate, composition loss, failure)
//!   already ends in `hide`, so Begin/End pairing needs no change in the controller.
//! * [`Shared`]: the state the COM object reads and the host may change (`ITfUIElement::Show`). It never holds a borrow
//!   while a host call runs, because TSF may call back into the element from inside `BeginUIElement`.
//!
//! What the element describes: the page the TIP is showing, as one page (count = rows of this page, selection = the
//! highlighted row, page index list `[0]`, current page 0). A [`CandidateView`] holds exactly one page, so this is the
//! only description in which every answer is real data; the TIP keeps its own paging (PageUp/PageDown reach the key
//! sink) and every page turn is an `UpdateUIElement` with new strings. A host-drawn list therefore has no "2/3"
//! indicator. The host cannot change the paging (`SetPageIndex` is refused) and cannot commit (the behaviour
//! interface is not implemented).
//!
//! STATUS: pure logic, tested here with a fake host; never run under a real TSF.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use crate::layout::one_line;
use crate::ui::{Anchor, CandidateRow, CandidateUi, CandidateView};

/// Identity of the element (`ITfUIElement::GetGUID`): stable, one per kind of element, not per list.
pub const ELEMENT_GUID: u128 = 0x7c1e4a52_9d83_4f06_b2a7_5e3d90c8f114;

/// `ITfUIElement::GetDescription`. Constant: the host may show it as the name of the list.
pub const DESCRIPTION: &str = "Programming IME candidate list";

/// `TF_CLUIE_*`: which answers of the element changed (the glue test pins these to the `windows` constants).
pub mod updated {
    pub const DOCUMENT_MGR: u32 = 0x01;
    pub const COUNT: u32 = 0x02;
    pub const SELECTION: u32 = 0x04;
    pub const STRING: u32 = 0x08;
    pub const PAGE_INDEX: u32 = 0x10;
    pub const CURRENT_PAGE: u32 = 0x20;
    /// What a first snapshot reports: everything is new.
    pub const ALL: u32 = DOCUMENT_MGR | COUNT | SELECTION | STRING | PAGE_INDEX | CURRENT_PAGE;
}

/// The one page the element reports: it starts at candidate 0.
const PAGE_STARTS: [u32; 1] = [0];

/// What `ITfCandidateListUIElement::GetPageIndex` must answer, decided without touching any buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageIndexReply<'a> {
    /// Always written to `puPageCnt`.
    pub page_count: u32,
    /// Written to the caller's buffer; empty when the caller passed no buffer (it only asked for the count).
    pub starts: &'a [u32],
}

/// A parameter combination `GetPageIndex` answers with `E_INVALIDARG`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidArgs;

/// One row as the host shows it: the one-line text and, in words, source and validation (the accessibility rule of
/// `ui.rs`: state is never carried by colour or position alone).
fn row_string(row: &CandidateRow) -> String {
    format!("{} ({})", one_line(&row.text), one_line(&row.detail))
}

/// The answers of `ITfCandidateListUIElement` for one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListSnapshot {
    strings: Vec<String>,
    selection: u32,
    /// The TIP's own page and page count. Not an answer of the element (see the module comment); only kept so that a
    /// page turn is always an update, even when two pages happen to read alike.
    tip_page: (u32, u32),
}

impl ListSnapshot {
    pub fn from_view(view: &CandidateView) -> ListSnapshot {
        let last = view.rows.len().saturating_sub(1);
        ListSnapshot {
            strings: view.rows.iter().map(row_string).collect(),
            selection: view.selected_in_page.min(last) as u32,
            tip_page: (view.page as u32, view.page_count as u32),
        }
    }

    pub fn description(&self) -> &'static str {
        DESCRIPTION
    }

    /// `GetCount`.
    pub fn count(&self) -> u32 {
        self.strings.len() as u32
    }

    /// `GetSelection`: index of the highlighted candidate.
    pub fn selection(&self) -> u32 {
        self.selection
    }

    /// `GetString`: None for an index that is not a candidate (the glue answers `E_INVALIDARG`).
    pub fn string(&self, index: u32) -> Option<&str> {
        self.strings.get(index as usize).map(String::as_str)
    }

    /// `GetPageIndex` list: the start index of every page of the element.
    pub fn page_starts(&self) -> &'static [u32] {
        &PAGE_STARTS
    }

    /// `GetCurrentPage`.
    pub fn current_page(&self) -> u32 {
        0
    }

    /// Candidates per page of the element: the whole list.
    pub fn page_size(&self) -> u32 {
        self.count()
    }

    /// The TIP's own zero-based page and page count (not exposed to the host).
    pub fn tip_page(&self) -> (u32, u32) {
        self.tip_page
    }

    /// `GetUpdatedFlags` between `previous` (None: the host has seen nothing yet) and `self`. A bit is set when the
    /// matching answer changed; a change of the TIP's own page sets both page bits so that the host re-reads the list
    /// (an over-report only costs a query, an under-report would leave a stale list).
    pub fn updated_flags(&self, previous: Option<&ListSnapshot>) -> u32 {
        let Some(old) = previous else {
            return updated::ALL;
        };
        let mut flags = 0;
        if old.count() != self.count() {
            flags |= updated::COUNT;
        }
        if old.selection != self.selection {
            flags |= updated::SELECTION;
        }
        if old.strings != self.strings {
            flags |= updated::STRING;
        }
        if old.tip_page != self.tip_page {
            flags |= updated::PAGE_INDEX | updated::CURRENT_PAGE;
        }
        flags
    }

    /// `GetPageIndex(pIndex, uSize, puPageCnt)`: `buffer` is the capacity of `pIndex` (None when it is null) and
    /// `count_out` says whether `puPageCnt` is non-null. A null `puPageCnt` or a buffer that cannot hold every page is
    /// `E_INVALIDARG`; nothing is ever written beyond the capacity.
    pub fn page_index(
        &self,
        buffer: Option<u32>,
        count_out: bool,
    ) -> Result<PageIndexReply<'static>, InvalidArgs> {
        let page_count = PAGE_STARTS.len() as u32;
        if !count_out {
            return Err(InvalidArgs);
        }
        match buffer {
            None => Ok(PageIndexReply {
                page_count,
                starts: &[],
            }),
            Some(capacity) if capacity >= page_count => Ok(PageIndexReply {
                page_count,
                starts: &PAGE_STARTS,
            }),
            Some(_) => Err(InvalidArgs),
        }
    }
}

/// The answer of `ITfUIElementMgr::BeginUIElement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Begun {
    pub id: u32,
    /// `*pbShow`: false means the host draws the list and the TIP must not draw its own.
    pub show: bool,
}

/// The host offers no UI-element manager, or refused this element: the TIP draws its own window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostUnavailable;

/// `ITfUIElementMgr`, as the driver needs it. The driver publishes the snapshot into [`Shared`] before every call, so
/// an implementation (and the host behind it) reads the current list from the element while the call runs.
pub trait UiElementHost {
    fn begin(&mut self) -> Result<Begun, HostUnavailable>;
    fn update(&mut self, id: u32);
    fn end(&mut self, id: u32);
}

/// Who draws the list that is open right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drawer {
    Tip,
    Host,
}

struct State {
    /// The id the host knows (between Begin and End).
    open: Option<u32>,
    drawer: Drawer,
    /// The last view the controller asked for: what `Show(TRUE)` brings back.
    view: Option<(CandidateView, Anchor)>,
    /// The native popup was asked to show and has not been hidden since.
    native_visible: bool,
    /// Begin failed for the list now showing: do not ask again for every update.
    begin_failed: bool,
}

struct Published {
    list: ListSnapshot,
    flags: u32,
}

/// State shared between the driver, the COM element and (through the element) the host. Invariant: no `RefCell` here is
/// borrowed while a host call or a native-window call that could re-enter runs, except `native` itself while the native
/// window is being drawn (the host cannot reach the native window).
pub struct Shared {
    published: RefCell<Option<Published>>,
    state: RefCell<State>,
    native: RefCell<Option<Box<dyn CandidateUi>>>,
    /// A host call is running: `Show` requests are only recorded.
    in_call: Cell<bool>,
    pending: Cell<Option<bool>>,
}

impl Shared {
    pub fn new(native: Box<dyn CandidateUi>) -> Rc<Shared> {
        Rc::new(Shared {
            published: RefCell::new(None),
            state: RefCell::new(State {
                open: None,
                drawer: Drawer::Tip,
                view: None,
                native_visible: false,
                begin_failed: false,
            }),
            native: RefCell::new(Some(native)),
            in_call: Cell::new(false),
            pending: Cell::new(None),
        })
    }

    /// Read the list the host may ask about, with the updated flags; None when no list is published. `f` must not call
    /// out (it only formats plain data).
    pub fn with_published<R>(&self, f: impl FnOnce(&ListSnapshot, u32) -> R) -> Option<R> {
        let published = self.published.borrow();
        published.as_ref().map(|p| f(&p.list, p.flags))
    }

    fn publish(&self, list: ListSnapshot, flags: u32) {
        *self.published.borrow_mut() = Some(Published { list, flags });
    }

    fn current_list(&self) -> Option<ListSnapshot> {
        self.published.borrow().as_ref().map(|p| p.list.clone())
    }

    fn clear_published(&self) {
        *self.published.borrow_mut() = None;
    }

    /// `ITfUIElement::IsShown`: true only while the TIP's own window is drawn for the open element.
    pub fn is_shown(&self) -> bool {
        let state = self.state.borrow();
        state.open.is_some() && state.drawer == Drawer::Tip && state.native_visible
    }

    /// `ITfUIElement::Show` from the host: TRUE the TIP draws its own window, FALSE it hides it. Inside a host call
    /// (TSF re-enters) the request is recorded and applied when the call returns; otherwise it takes effect at once.
    pub fn host_show(&self, show: bool) {
        if self.in_call.get() {
            self.pending.set(Some(show));
            return;
        }
        self.apply_show(show);
        self.sync_native(None);
    }

    fn apply_show(&self, show: bool) {
        match self.state.try_borrow_mut() {
            Ok(mut state) => {
                if state.open.is_some() {
                    state.drawer = if show { Drawer::Tip } else { Drawer::Host };
                }
            }
            Err(_) => self.pending.set(Some(show)),
        }
    }

    /// A `Show` that arrived during a host call takes effect now that the call (and its answer) is recorded.
    fn apply_pending(&self) {
        if let Some(show) = self.pending.take() {
            self.apply_show(show);
        }
    }

    /// Make the native popup match the decision: shown with the latest view while the TIP draws (or no element is open),
    /// hidden while the host draws. `now` is the view the caller has at hand, else the stored one is used. Returns
    /// whether the native popup is on screen afterwards (what the popup answered, not what was asked of it).
    fn sync_native(&self, now: Option<(&CandidateView, Anchor)>) -> bool {
        let (tip_draws, visible, stored) = {
            let state = self.state.borrow();
            let stored = if now.is_none() {
                state.view.clone()
            } else {
                None
            };
            (
                state.open.is_none() || state.drawer == Drawer::Tip,
                state.native_visible,
                stored,
            )
        };
        let Ok(mut native) = self.native.try_borrow_mut() else {
            return false; // the window is being drawn right now; the next show or hide settles it (fail closed: not visible)
        };
        let Some(native) = native.as_mut() else {
            return false;
        };
        let target = match (&stored, now) {
            (_, Some((view, anchor))) => Some((view, anchor)),
            (Some((view, anchor)), None) => Some((view, *anchor)),
            (None, None) => None,
        };
        match target {
            Some((view, anchor)) if tip_draws => {
                // a popup that could not be drawn has hidden itself: `native_visible` follows its answer
                let shown = native.show(view, anchor);
                self.state.borrow_mut().native_visible = shown;
                shown
            }
            _ if visible => {
                native.hide();
                self.state.borrow_mut().native_visible = false;
                false
            }
            _ => false, // not drawn by the TIP and not visible before either (`visible` is false here)
        }
    }

    /// The host has taken the open list over (`BeginUIElement` answered "I draw", or `Show(FALSE)` later): the user sees
    /// it even though the native popup stays hidden.
    fn host_draws(&self) -> bool {
        let state = self.state.borrow();
        state.open.is_some() && state.drawer == Drawer::Host
    }

    fn hide_native(&self) {
        if let Ok(mut native) = self.native.try_borrow_mut()
            && let Some(native) = native.as_mut()
        {
            native.hide();
        }
        self.state.borrow_mut().native_visible = false;
    }
}

/// The candidate window the controller talks to. It decides who draws (host or native popup) and keeps the host's view
/// of the list in step: Begin when a list appears, Update when something the host can see changed, End when the list
/// goes away, always paired.
pub struct ElementUi<H: UiElementHost> {
    shared: Rc<Shared>,
    host: H,
}

impl<H: UiElementHost> ElementUi<H> {
    pub fn new(shared: Rc<Shared>, host: H) -> ElementUi<H> {
        ElementUi { shared, host }
    }

    /// Announce a new list. An element that is still open is ended first (never two open at once).
    fn begin_list(&mut self, list: ListSnapshot) {
        self.end_open();
        self.shared.publish(list, updated::ALL);
        self.shared.in_call.set(true);
        let answer = self.host.begin();
        self.shared.in_call.set(false);
        let mut state = self.shared.state.borrow_mut();
        match answer {
            Ok(begun) => {
                state.open = Some(begun.id);
                state.drawer = if begun.show {
                    Drawer::Tip
                } else {
                    Drawer::Host
                };
            }
            Err(HostUnavailable) => {
                state.begin_failed = true;
                state.drawer = Drawer::Tip;
                drop(state);
                self.shared.clear_published();
            }
        }
    }

    fn update_list(&mut self, id: u32, list: ListSnapshot) {
        let flags = list.updated_flags(self.shared.current_list().as_ref());
        if flags == 0 {
            return; // nothing the host can see changed
        }
        self.shared.publish(list, flags);
        self.shared.in_call.set(true);
        self.host.update(id);
        self.shared.in_call.set(false);
    }

    /// End the open element, if any (idempotent).
    fn end_open(&mut self) {
        let id = self.shared.state.borrow_mut().open.take();
        if let Some(id) = id {
            self.shared.in_call.set(true);
            self.host.end(id);
            self.shared.in_call.set(false);
        }
        self.shared.clear_published();
        self.shared.pending.set(None);
    }
}

impl<H: UiElementHost> CandidateUi for ElementUi<H> {
    /// Visible when the host draws the list, else exactly when the native popup was drawn (a popup that failed leaves the
    /// element open, but nobody sees the list: the controller then cancels and its `hide` ends the element).
    fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool {
        let list = ListSnapshot::from_view(view);
        let (open, failed) = {
            let mut state = self.shared.state.borrow_mut();
            state.view = Some((view.clone(), anchor));
            (state.open, state.begin_failed)
        };
        match open {
            Some(id) => self.update_list(id, list),
            None if failed => {}
            None => self.begin_list(list),
        }
        // a `Show` that arrived inside the host call counts after the host's answer (the host asked for it later)
        self.shared.apply_pending();
        let native_shown = self.shared.sync_native(Some((view, anchor)));
        native_shown || self.shared.host_draws()
    }

    fn hide(&mut self) {
        // recover from a panic that left a call marked as running (the controller's fail-safe ends here)
        self.shared.in_call.set(false);
        self.shared.hide_native();
        self.end_open();
        let mut state = self.shared.state.borrow_mut();
        state.view = None;
        state.begin_failed = false;
        state.drawer = Drawer::Tip;
    }
}

impl<H: UiElementHost> Drop for ElementUi<H> {
    fn drop(&mut self) {
        // end a list that is still open, then destroy the native window on this (the creating) thread
        let _ = catch_unwind(AssertUnwindSafe(|| CandidateUi::hide(self)));
        let native = self
            .shared
            .native
            .try_borrow_mut()
            .ok()
            .and_then(|mut n| n.take());
        drop(native);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{HostError, HostText, KeyInput, TipConfig, TipController};
    use crate::keymap::{Modifiers, vk};
    use crate::ui::RecordingUi;
    use pime_core::candidate::{Candidate, CandidateSource, Validation};
    use pime_core::input::{CoreFault, PredictPort, PredictRequest};

    const ANCHOR: Anchor = Anchor {
        x: 100,
        y: 200,
        line_height: 18,
    };
    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
    };

    fn row(i: usize) -> CandidateRow {
        CandidateRow {
            label: (i + 1).to_string(),
            text: format!("T{i}"),
            detail: "rule · valid".to_string(),
        }
    }

    fn view(rows: usize, selected: usize, page: usize, page_count: usize) -> CandidateView {
        CandidateView {
            rows: (0..rows).map(row).collect(),
            selected_in_page: selected,
            page,
            page_count,
        }
    }

    // ----- the model -----

    #[test]
    fn a_snapshot_answers_what_the_element_is_asked() {
        let snap = ListSnapshot::from_view(&view(5, 2, 1, 3));
        assert_eq!(snap.description(), "Programming IME candidate list");
        assert_eq!(snap.count(), 5);
        assert_eq!(snap.selection(), 2);
        assert_eq!(snap.string(0), Some("T0 (rule · valid)"));
        assert_eq!(snap.string(4), Some("T4 (rule · valid)"));
        assert_eq!(
            snap.string(5),
            None,
            "one past the last row is not a candidate"
        );
        assert_eq!(snap.page_starts(), &[0]);
        assert_eq!(snap.current_page(), 0);
        assert_eq!(snap.page_size(), 5);
        assert_eq!(
            snap.tip_page(),
            (1, 3),
            "the TIP's own page is kept, not exposed"
        );
    }

    #[test]
    fn a_row_reads_as_one_line_and_the_selection_never_leaves_the_list() {
        let mut v = view(2, 99, 0, 1);
        v.rows[0].text = "fn main() {\n    \u{202E}body\n}".to_string();
        v.rows[1].detail = "rule · invalid · cannot insert".to_string();
        let snap = ListSnapshot::from_view(&v);
        assert_eq!(
            snap.string(0),
            Some("fn main() { \u{FFFD}body } (rule · valid)")
        );
        assert_eq!(snap.string(1), Some("T1 (rule · invalid · cannot insert)"));
        assert_eq!(snap.selection(), 1, "clamped to the last row");
        let empty = ListSnapshot::from_view(&view(0, 0, 0, 1));
        assert_eq!((empty.count(), empty.selection()), (0, 0));
        assert_eq!(empty.string(0), None);
    }

    #[test]
    fn the_identity_is_a_fixed_guid_and_the_flag_values_are_the_documented_bits() {
        assert_eq!(
            format!("{:032x}", ELEMENT_GUID),
            "7c1e4a529d834f06b2a75e3d90c8f114"
        );
        assert_eq!(updated::DOCUMENT_MGR, 1);
        assert_eq!(updated::COUNT, 2);
        assert_eq!(updated::SELECTION, 4);
        assert_eq!(updated::STRING, 8);
        assert_eq!(updated::PAGE_INDEX, 0x10);
        assert_eq!(updated::CURRENT_PAGE, 0x20);
        assert_eq!(updated::ALL, 0x3F);
    }

    #[test]
    fn updated_flags_name_exactly_what_changed() {
        let base = ListSnapshot::from_view(&view(5, 0, 0, 3));
        assert_eq!(
            base.updated_flags(None),
            updated::ALL,
            "a first snapshot is all new"
        );
        assert_eq!(base.updated_flags(Some(&base)), 0);
        let moved = ListSnapshot::from_view(&view(5, 3, 0, 3));
        assert_eq!(moved.updated_flags(Some(&base)), updated::SELECTION);
        let mut v = view(5, 0, 0, 3);
        v.rows[2].text = "other".into();
        let text = ListSnapshot::from_view(&v);
        assert_eq!(text.updated_flags(Some(&base)), updated::STRING);
        let shorter = ListSnapshot::from_view(&view(3, 0, 0, 3));
        assert_eq!(
            shorter.updated_flags(Some(&base)),
            updated::COUNT | updated::STRING,
            "fewer rows: the count and the strings changed"
        );
        let turned = ListSnapshot::from_view(&view(5, 0, 1, 3));
        assert_eq!(
            turned.updated_flags(Some(&base)),
            updated::PAGE_INDEX | updated::CURRENT_PAGE,
            "a page turn between two pages that read alike still says so"
        );
        let all = ListSnapshot::from_view(&view(2, 1, 2, 4));
        assert_eq!(
            all.updated_flags(Some(&base)),
            updated::COUNT
                | updated::SELECTION
                | updated::STRING
                | updated::PAGE_INDEX
                | updated::CURRENT_PAGE
        );
    }

    #[test]
    fn get_page_index_writes_only_what_the_caller_made_room_for() {
        let snap = ListSnapshot::from_view(&view(5, 0, 0, 3));
        let ask = |buffer, count| snap.page_index(buffer, count);
        assert_eq!(
            ask(None, true),
            Ok(PageIndexReply {
                page_count: 1,
                starts: &[]
            }),
            "no buffer: the count only"
        );
        assert_eq!(
            ask(Some(1), true),
            Ok(PageIndexReply {
                page_count: 1,
                starts: &[0]
            })
        );
        assert_eq!(
            ask(Some(64), true).map(|r| r.starts.len()),
            Ok(1),
            "never more entries than pages, however large the buffer"
        );
        assert_eq!(
            ask(Some(0), true),
            Err(InvalidArgs),
            "a buffer without room"
        );
        assert_eq!(ask(None, false), Err(InvalidArgs), "no place for the count");
        assert_eq!(ask(Some(1), false), Err(InvalidArgs));
    }

    // ----- the driver, against a fake host -----

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Begin,
        Update(u32),
        End(u32),
    }

    /// What the host saw through the element while one of its calls ran.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Seen {
        count: u32,
        selection: u32,
        flags: u32,
        shown: bool,
    }

    struct Wire {
        calls: Vec<Call>,
        open: Vec<u32>,
        max_open: usize,
        next_id: u32,
        /// What `BeginUIElement` answers in `*pbShow`.
        answer_show: bool,
        unavailable: bool,
        seen: Vec<Seen>,
        /// Call `ITfUIElement::Show(b)` on the element from inside Begin and Update (TSF re-enters).
        reenter: Option<bool>,
        /// The same, from inside `EndUIElement`.
        reenter_in_end: Option<bool>,
        panic_in_update: bool,
    }

    impl Wire {
        fn new(answer_show: bool) -> Rc<RefCell<Wire>> {
            Rc::new(RefCell::new(Wire {
                calls: Vec::new(),
                open: Vec::new(),
                max_open: 0,
                next_id: 1,
                answer_show,
                unavailable: false,
                seen: Vec::new(),
                reenter: None,
                reenter_in_end: None,
                panic_in_update: false,
            }))
        }
    }

    fn begins(wire: &Rc<RefCell<Wire>>) -> usize {
        let calls = &wire.borrow().calls;
        calls.iter().filter(|c| **c == Call::Begin).count()
    }

    fn ends(wire: &Rc<RefCell<Wire>>) -> usize {
        let calls = &wire.borrow().calls;
        calls.iter().filter(|c| matches!(c, Call::End(_))).count()
    }

    /// The host side of the protocol. It reads the list through the element while each call runs, exactly as the COM
    /// getters do: a borrow the driver still held across the call would panic here.
    struct FakeHost {
        wire: Rc<RefCell<Wire>>,
        shared: Rc<Shared>,
    }

    impl FakeHost {
        fn look(&self) {
            let seen = self
                .shared
                .with_published(|list, flags| (list.count(), list.selection(), flags))
                .map(|(count, selection, flags)| Seen {
                    count,
                    selection,
                    flags,
                    shown: self.shared.is_shown(),
                })
                .expect("the list is published before the host is called");
            let reenter = self.wire.borrow().reenter;
            if let Some(show) = reenter {
                self.shared.host_show(show);
            }
            self.wire.borrow_mut().seen.push(seen);
        }
    }

    impl UiElementHost for FakeHost {
        fn begin(&mut self) -> Result<Begun, HostUnavailable> {
            self.wire.borrow_mut().calls.push(Call::Begin);
            if self.wire.borrow().unavailable {
                return Err(HostUnavailable);
            }
            self.look();
            let mut wire = self.wire.borrow_mut();
            let id = wire.next_id;
            wire.next_id += 1;
            wire.open.push(id);
            wire.max_open = wire.max_open.max(wire.open.len());
            Ok(Begun {
                id,
                show: wire.answer_show,
            })
        }

        fn update(&mut self, id: u32) {
            self.wire.borrow_mut().calls.push(Call::Update(id));
            assert!(
                self.wire.borrow().open.contains(&id),
                "Update of unknown id {id}"
            );
            if self.wire.borrow().panic_in_update {
                panic!("host panicked in update (test)");
            }
            self.look();
        }

        fn end(&mut self, id: u32) {
            let reenter = {
                let mut wire = self.wire.borrow_mut();
                wire.calls.push(Call::End(id));
                let at = wire
                    .open
                    .iter()
                    .position(|open| *open == id)
                    .unwrap_or_else(|| panic!("End of unknown id {id}"));
                wire.open.remove(at);
                wire.reenter_in_end
            };
            if let Some(show) = reenter {
                self.shared.host_show(show);
            }
        }
    }

    /// The native popup as a test can still look at it after it moved into `Shared`.
    #[derive(Clone, Default)]
    struct Spy(Rc<RefCell<RecordingUi>>);

    impl CandidateUi for Spy {
        fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool {
            self.0.borrow_mut().show(view, anchor)
        }

        fn hide(&mut self) {
            self.0.borrow_mut().hide();
        }
    }

    struct Rig {
        ui: ElementUi<FakeHost>,
        wire: Rc<RefCell<Wire>>,
        shared: Rc<Shared>,
        native: Spy,
    }

    impl Rig {
        fn new(answer_show: bool) -> Rig {
            let wire = Wire::new(answer_show);
            let native = Spy::default();
            let shared = Shared::new(Box::new(native.clone()));
            let host = FakeHost {
                wire: Rc::clone(&wire),
                shared: Rc::clone(&shared),
            };
            Rig {
                ui: ElementUi::new(Rc::clone(&shared), host),
                wire,
                shared,
                native,
            }
        }

        fn show(&mut self, v: &CandidateView) {
            self.ui.show(v, ANCHOR);
        }

        fn native_visible(&self) -> bool {
            self.native.0.borrow().visible
        }

        fn native_shows(&self) -> usize {
            self.native.0.borrow().shows
        }

        fn calls(&self) -> Vec<Call> {
            self.wire.borrow().calls.clone()
        }
    }

    #[test]
    fn begin_when_a_list_appears_update_on_every_visible_change_end_when_it_hides() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 0, 0, 3));
        rig.show(&view(5, 1, 0, 3)); // selection
        rig.show(&view(5, 0, 1, 3)); // page
        let mut more = view(5, 0, 1, 3);
        more.rows[4].text = "changed".into();
        rig.show(&more); // candidates
        rig.ui.hide();
        assert_eq!(
            rig.calls(),
            vec![
                Call::Begin,
                Call::Update(1),
                Call::Update(1),
                Call::Update(1),
                Call::End(1)
            ]
        );
        let wire = rig.wire.borrow();
        let flags: Vec<u32> = wire.seen.iter().map(|s| s.flags).collect();
        assert_eq!(
            flags,
            vec![
                updated::ALL,
                updated::SELECTION,
                updated::PAGE_INDEX | updated::CURRENT_PAGE | updated::SELECTION,
                updated::STRING
            ]
        );
        assert!(wire.open.is_empty(), "no id leaked");
        assert_eq!(wire.max_open, 1);
    }

    #[test]
    fn an_unchanged_list_is_not_announced_again_but_the_native_window_still_follows() {
        let mut rig = Rig::new(true);
        rig.show(&view(3, 0, 0, 1));
        rig.show(&view(3, 0, 0, 1));
        assert_eq!(
            rig.calls(),
            vec![Call::Begin],
            "nothing the host can see changed"
        );
        assert_eq!(
            rig.native_shows(),
            2,
            "the caret may have moved: the popup is placed again"
        );
        rig.ui.hide();
        assert_eq!(rig.calls(), vec![Call::Begin, Call::End(1)]);
    }

    #[test]
    fn the_list_is_published_before_every_host_call() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 2, 0, 1));
        rig.show(&view(4, 3, 0, 1));
        let wire = rig.wire.borrow();
        assert_eq!(
            wire.seen[0],
            Seen {
                count: 5,
                selection: 2,
                flags: updated::ALL,
                shown: false
            },
            "the host sees the list inside BeginUIElement; the element is not 'shown' before Begin has answered"
        );
        assert_eq!(
            wire.seen[1],
            Seen {
                count: 4,
                selection: 3,
                flags: updated::COUNT | updated::SELECTION | updated::STRING,
                shown: true
            },
            "and the new one inside UpdateUIElement"
        );
    }

    #[test]
    fn end_is_idempotent_and_leaves_nothing_published() {
        let mut rig = Rig::new(true);
        rig.ui.hide(); // never began: nothing to end
        assert!(rig.calls().is_empty());
        rig.show(&view(2, 0, 0, 1));
        rig.ui.hide();
        rig.ui.hide();
        rig.ui.hide();
        assert_eq!(
            rig.calls(),
            vec![Call::Begin, Call::End(1)],
            "one End for one Begin"
        );
        assert!(rig.shared.with_published(|_, _| ()).is_none());
        assert!(!rig.shared.is_shown());
        assert!(!rig.native_visible());
        rig.show(&view(2, 0, 0, 1));
        assert_eq!(
            rig.calls().last(),
            Some(&Call::Begin),
            "a later list begins again"
        );
        assert_eq!(begins(&rig.wire), 2);
    }

    #[test]
    fn a_second_begin_without_an_end_ends_the_first() {
        let mut rig = Rig::new(true);
        rig.show(&view(2, 0, 0, 1));
        rig.ui
            .begin_list(ListSnapshot::from_view(&view(3, 0, 0, 1)));
        assert_eq!(
            rig.calls(),
            vec![Call::Begin, Call::End(1), Call::Begin],
            "the first element is ended before the second begins"
        );
        assert_eq!(rig.wire.borrow().max_open, 1, "never two open at once");
        rig.ui.hide();
        assert!(rig.wire.borrow().open.is_empty());
    }

    #[test]
    fn show_true_keeps_the_native_popup_drawing() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 0, 0, 1));
        assert!(rig.native_visible());
        assert!(rig.shared.is_shown(), "our window is the one on screen");
        rig.show(&view(5, 1, 0, 1));
        assert_eq!(rig.native_shows(), 2);
        rig.ui.hide();
        assert!(!rig.native_visible());
        assert!(!rig.shared.is_shown());
    }

    #[test]
    fn show_false_keeps_the_native_popup_hidden_for_the_whole_list() {
        let mut rig = Rig::new(false);
        rig.show(&view(5, 0, 0, 3));
        rig.show(&view(5, 1, 0, 3));
        rig.show(&view(5, 0, 1, 3));
        assert_eq!(
            rig.native_shows(),
            0,
            "the host draws: the popup was never shown"
        );
        assert!(!rig.native_visible());
        assert!(
            !rig.shared.is_shown(),
            "IsShown tells the truth: our window is not drawn"
        );
        assert_eq!(
            rig.calls().len(),
            3,
            "the host is still told about every change"
        );
        rig.ui.hide();
        assert_eq!(rig.native_shows(), 0);
        assert_eq!(ends(&rig.wire), 1);
    }

    #[test]
    fn no_manager_means_the_native_popup_and_one_attempt_per_list() {
        let mut rig = Rig::new(true);
        rig.wire.borrow_mut().unavailable = true;
        rig.show(&view(5, 0, 0, 1));
        rig.show(&view(5, 1, 0, 1));
        rig.show(&view(5, 2, 0, 1));
        assert!(
            rig.native_visible(),
            "no host to take over: the popup draws"
        );
        assert_eq!(begins(&rig.wire), 1, "Begin is not retried on every update");
        assert_eq!(rig.native_shows(), 3);
        rig.ui.hide();
        assert_eq!(ends(&rig.wire), 0, "nothing was begun, so nothing is ended");
        assert!(!rig.native_visible());
        rig.show(&view(5, 0, 0, 1));
        assert_eq!(begins(&rig.wire), 2, "the next list asks again");
    }

    #[test]
    fn the_hosts_show_call_toggles_the_same_decision_and_leaves_no_stale_popup() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 2, 0, 1));
        assert!(rig.native_visible() && rig.shared.is_shown());
        // the host takes the list over in the middle of the composition
        rig.shared.host_show(false);
        assert!(!rig.native_visible(), "no stale popup on screen");
        assert!(!rig.shared.is_shown());
        rig.show(&view(5, 3, 0, 1));
        assert!(!rig.native_visible(), "stays hidden while the host draws");
        // and gives it back
        rig.shared.host_show(true);
        assert!(rig.native_visible(), "the popup comes back");
        assert!(rig.shared.is_shown());
        let shown = rig.native.0.borrow().last.clone().unwrap().0;
        assert_eq!(
            shown.selected_in_page, 3,
            "with the latest view, not the first one"
        );
        rig.ui.hide();
        assert!(!rig.native_visible());
        // a Show after the end has no element to act on
        rig.shared.host_show(true);
        assert!(!rig.native_visible());
        assert!(!rig.shared.is_shown());
        assert!(rig.wire.borrow().open.is_empty());
    }

    #[test]
    fn a_show_call_made_inside_begin_or_update_is_applied_after_the_call_returns() {
        // Begin answers show = TRUE, but the host calls Show(FALSE) from inside the same call (TSF re-enters)
        let mut rig = Rig::new(true);
        rig.wire.borrow_mut().reenter = Some(false);
        rig.show(&view(5, 0, 0, 1));
        assert_eq!(
            rig.native_shows(),
            0,
            "the later request wins: the popup stays hidden"
        );
        assert!(!rig.shared.is_shown());
        // Update: the host switches back from inside UpdateUIElement
        rig.wire.borrow_mut().reenter = Some(true);
        rig.show(&view(5, 1, 0, 1));
        assert!(
            rig.native_visible(),
            "Show(TRUE) inside Update is honoured once it returned"
        );
        assert!(rig.shared.is_shown());
        // and the other way round
        rig.wire.borrow_mut().reenter = Some(false);
        rig.show(&view(5, 2, 0, 1));
        assert!(!rig.native_visible());
        rig.wire.borrow_mut().reenter = None;
        rig.ui.hide();
        assert!(rig.wire.borrow().open.is_empty());
        assert!(!rig.native_visible());
    }

    #[test]
    fn a_show_call_made_inside_end_cannot_revive_the_popup() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 0, 0, 1));
        rig.wire.borrow_mut().reenter_in_end = Some(true); // Show(TRUE) from inside EndUIElement
        rig.ui.hide();
        assert!(
            !rig.native_visible(),
            "the list is over: nothing may draw it again"
        );
        assert!(!rig.shared.is_shown());
        assert!(
            rig.shared.pending.get().is_none(),
            "a request recorded during a call never outlives the list"
        );
        rig.wire.borrow_mut().reenter_in_end = None;
        rig.show(&view(5, 0, 0, 1));
        assert!(
            rig.native_visible(),
            "and the next list starts from the host's answer, not from that request"
        );
    }

    #[test]
    fn a_panicking_host_call_ends_in_a_clean_state_after_the_controllers_hide() {
        let mut rig = Rig::new(true);
        rig.show(&view(5, 0, 0, 1));
        rig.wire.borrow_mut().panic_in_update = true;
        let result = catch_unwind(AssertUnwindSafe(|| rig.show(&view(5, 1, 0, 1))));
        assert!(
            result.is_err(),
            "the panic is the host's; the controller contains it"
        );
        assert!(
            rig.shared.in_call.get(),
            "the panic left a call marked as running"
        );
        rig.wire.borrow_mut().panic_in_update = false;
        // the controller's fail-safe: hide
        rig.ui.hide();
        assert!(!rig.shared.in_call.get());
        assert!(rig.wire.borrow().open.is_empty(), "the element was ended");
        assert!(!rig.native_visible());
        rig.show(&view(5, 0, 0, 1));
        assert_eq!(begins(&rig.wire), 2, "and the next list works");
    }

    #[test]
    fn show_answers_whether_the_user_sees_the_list_when_the_tip_draws() {
        let mut rig = Rig::new(true);
        assert!(
            rig.ui.show(&view(5, 0, 0, 1), ANCHOR),
            "the popup drew the list"
        );
        assert!(rig.shared.is_shown());
        // the popup cannot be drawn (window creation, monitor, font, layout): nobody sees the list
        rig.native.0.borrow_mut().refuse = true;
        assert!(!rig.ui.show(&view(5, 1, 0, 1), ANCHOR));
        assert!(!rig.native_visible());
        assert!(
            !rig.shared.is_shown(),
            "IsShown tells the truth: our window is not on screen"
        );
        assert_eq!(
            rig.calls(),
            vec![Call::Begin, Call::Update(1)],
            "the host is still told about the list: the controller's hide ends it"
        );
        rig.ui.hide();
        assert_eq!(rig.calls().last(), Some(&Call::End(1)));
        assert!(rig.wire.borrow().open.is_empty());
        // and the popup works again with the next list
        rig.native.0.borrow_mut().refuse = false;
        assert!(rig.ui.show(&view(5, 0, 0, 1), ANCHOR));
        assert!(rig.shared.is_shown());
    }

    #[test]
    fn show_answers_true_while_the_host_draws_whatever_the_native_popup_could_do() {
        let mut rig = Rig::new(false); // BeginUIElement answers "I draw"
        rig.native.0.borrow_mut().refuse = true;
        assert!(
            rig.ui.show(&view(5, 0, 0, 1), ANCHOR),
            "the host's list is the one the user sees"
        );
        assert!(rig.ui.show(&view(5, 1, 0, 1), ANCHOR));
        assert_eq!(rig.native_shows(), 0, "the popup was never asked");
        // the host takes a TIP-drawn list over in the middle of it
        let mut rig = Rig::new(true);
        assert!(rig.ui.show(&view(5, 0, 0, 1), ANCHOR));
        rig.native.0.borrow_mut().refuse = true;
        rig.shared.host_show(false);
        assert!(rig.ui.show(&view(5, 1, 0, 1), ANCHOR));
        // and gives it back: now the popup's answer counts
        rig.shared.host_show(true);
        assert!(!rig.ui.show(&view(5, 2, 0, 1), ANCHOR));
        assert!(!rig.shared.is_shown());
    }

    #[test]
    fn show_answers_with_the_native_popup_when_there_is_no_manager() {
        let mut rig = Rig::new(true);
        rig.wire.borrow_mut().unavailable = true;
        assert!(rig.ui.show(&view(5, 0, 0, 1), ANCHOR));
        rig.native.0.borrow_mut().refuse = true;
        assert!(
            !rig.ui.show(&view(5, 1, 0, 1), ANCHOR),
            "no host to take over: a failed popup is no list at all"
        );
        assert_eq!(ends(&rig.wire), 0);
    }

    #[test]
    fn dropping_the_window_ends_an_open_list_and_destroys_the_native_window() {
        let mut rig = Rig::new(true);
        rig.show(&view(2, 0, 0, 1));
        let wire = Rc::clone(&rig.wire);
        let shared = Rc::clone(&rig.shared);
        drop(rig.ui);
        assert!(wire.borrow().open.is_empty(), "End reached the host");
        assert!(
            shared.native.borrow().is_none(),
            "the native window was dropped on this thread"
        );
        // the element may outlive the window (the host still holds it): its calls are harmless
        shared.host_show(true);
        assert!(!shared.is_shown());
    }

    // ----- the driver under the real controller -----

    /// 12 candidates (three pages of five) for every prefix.
    struct FakePredictor;

    impl PredictPort for FakePredictor {
        fn predict(&mut self, r: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
            Ok((0..12)
                .map(|i| {
                    let mut c = Candidate::new(&format!("c{i}")).with_source(CandidateSource::Rule);
                    c.validation = Validation::Valid;
                    c.text = Some(format!("T{i}:{}", r.prefix));
                    c
                })
                .collect())
        }
    }

    struct FakeText {
        doc: String,
    }

    impl HostText for FakeText {
        fn is_secure_field(&mut self) -> bool {
            false
        }

        fn caret_anchor(&mut self) -> Option<Anchor> {
            Some(ANCHOR)
        }

        fn begin_composition(&mut self, typed: &str) -> Result<(), HostError> {
            if self.doc.ends_with(typed) {
                Ok(())
            } else {
                Err(HostError::Mismatch)
            }
        }

        fn end_composition(&mut self) -> Result<(), HostError> {
            Ok(())
        }

        fn commit(&mut self, typed: &str, text: &str) -> Result<(), HostError> {
            if !self.doc.ends_with(typed) {
                return Err(HostError::Mismatch);
            }
            self.doc.truncate(self.doc.len() - typed.len());
            self.doc.push_str(text);
            Ok(())
        }
    }

    struct Session {
        ctl: TipController<ElementUi<FakeHost>, FakeText, FakePredictor>,
        wire: Rc<RefCell<Wire>>,
        shared: Rc<Shared>,
        native: Spy,
        t: u64,
    }

    impl Session {
        fn new(answer_show: bool) -> Session {
            let rig = Rig::new(answer_show);
            let mut ctl = TipController::new(
                rig.ui,
                FakeText { doc: String::new() },
                FakePredictor,
                TipConfig {
                    candidate_capture: true,
                    ..TipConfig::default()
                },
            );
            ctl.activate(false);
            Session {
                ctl,
                wire: rig.wire,
                shared: rig.shared,
                native: rig.native,
                t: 0,
            }
        }

        fn press(&mut self, code: u32, ch: Option<char>) -> bool {
            self.t += 1;
            let down = KeyInput {
                code,
                lparam: ((code as isize) << 16) | 1,
                ch,
                mods: NONE,
                now_ms: self.t,
            };
            let eaten = self.ctl.on_test_key_down(down) && self.ctl.on_key_down(down);
            if !eaten && let Some(c) = ch {
                self.ctl.host_mut().doc.push(c);
            }
            self.t += 1;
            let up = KeyInput {
                now_ms: self.t,
                ..down
            };
            self.ctl.on_test_key_up(up);
            self.ctl.on_key_up(up);
            eaten
        }

        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                let code = 0x41 + (c as u32 - 'a' as u32);
                assert!(!self.press(code, Some(c)), "typing {c:?} reaches the host");
            }
        }

        fn balanced(&self) -> bool {
            self.wire.borrow().open.is_empty() && begins(&self.wire) == ends(&self.wire)
        }
    }

    #[test]
    fn typing_paging_and_committing_pairs_every_begin_with_an_end() {
        let mut s = Session::new(true);
        s.type_text("fn");
        assert_eq!(
            s.wire.borrow().open,
            vec![1],
            "the list is announced when it appears"
        );
        assert!(
            s.native.0.borrow().visible,
            "show = TRUE: the popup draws as before"
        );
        assert!(s.press(vk::NEXT, None), "PageDown");
        assert!(s.press(vk::DOWN, None));
        let updates = s
            .wire
            .borrow()
            .calls
            .iter()
            .filter(|c| matches!(c, Call::Update(_)))
            .count();
        assert!(
            updates >= 2,
            "every page turn and selection move is an update ({updates})"
        );
        assert!(s.press(0x31, Some('1')), "digit 1 commits");
        assert!(s.balanced(), "{:?}", s.wire.borrow().calls);
        assert!(!s.native.0.borrow().visible);
        assert_eq!(begins(&s.wire), 1);
    }

    #[test]
    fn typing_one_more_character_is_an_update_not_a_second_begin() {
        let mut s = Session::new(true);
        s.type_text("fn");
        s.type_text("m");
        assert_eq!(begins(&s.wire), 1, "{:?}", s.wire.borrow().calls);
        assert!(
            s.wire
                .borrow()
                .seen
                .iter()
                .skip(1)
                .any(|v| v.flags & updated::STRING != 0)
        );
        s.ctl.focus_changed();
        assert!(s.balanced());
    }

    #[test]
    fn cancel_ends_the_element() {
        let mut s = Session::new(true);
        s.type_text("fn");
        assert!(s.press(vk::ESCAPE, None));
        assert!(s.balanced(), "{:?}", s.wire.borrow().calls);
        assert!(!s.native.0.borrow().visible);
        assert!(!s.shared.is_shown());
    }

    #[test]
    fn focus_loss_ends_the_element_and_the_next_list_begins_again() {
        let mut s = Session::new(true);
        s.type_text("fn");
        s.ctl.focus_changed();
        assert!(s.balanced());
        s.ctl.host_mut().doc.clear();
        s.type_text("fn");
        assert_eq!(begins(&s.wire), 2);
        assert_eq!(s.wire.borrow().open, vec![2], "a new id for the new list");
        s.ctl.composition_terminated();
        assert!(
            s.balanced(),
            "a composition ended by the host also ends the list"
        );
    }

    #[test]
    fn deactivate_ends_the_element() {
        let mut s = Session::new(true);
        s.type_text("fn");
        s.ctl.deactivate();
        assert!(s.balanced());
        assert!(!s.native.0.borrow().visible);
        s.ctl.deactivate();
        assert!(s.balanced(), "deactivating twice ends nothing twice");
    }

    #[test]
    fn a_host_that_draws_the_list_never_sees_the_native_popup_under_the_controller() {
        let mut s = Session::new(false);
        s.type_text("fn");
        assert!(s.press(vk::NEXT, None));
        assert!(s.press(vk::DOWN, None));
        assert!(s.press(0x31, Some('1')));
        assert_eq!(
            s.native.0.borrow().shows,
            0,
            "not once during the whole script"
        );
        assert!(s.balanced());
        assert!(begins(&s.wire) >= 1);
    }

    #[test]
    fn switching_to_the_host_in_the_middle_of_a_script_hides_the_popup_at_once() {
        let mut s = Session::new(true);
        s.type_text("fn");
        assert!(s.native.0.borrow().visible);
        s.shared.host_show(false);
        assert!(!s.native.0.borrow().visible);
        assert!(
            s.press(vk::NEXT, None),
            "the controller still pages: it believes a list is showing"
        );
        assert!(!s.native.0.borrow().visible, "and the popup stays away");
        s.shared.host_show(true);
        assert!(s.native.0.borrow().visible);
        s.ctl.deactivate();
        assert!(s.balanced());
        assert!(!s.native.0.borrow().visible);
    }

    #[test]
    fn a_popup_that_cannot_draw_cancels_the_list_under_the_controller_and_begin_still_pairs_with_end()
     {
        let mut s = Session::new(true);
        s.native.0.borrow_mut().refuse = true;
        s.type_text("fn");
        assert!(!s.shared.is_shown());
        assert!(
            !s.press(vk::NEXT, None),
            "nobody sees a list: PageDown is the host's"
        );
        assert!(!s.press(0x31, Some('1')), "and so is the digit");
        assert_eq!(s.ctl.host().doc, "fn1");
        assert!(s.balanced(), "{:?}", s.wire.borrow().calls);
        // a host that draws makes the same popup failure irrelevant: its list is on screen
        let mut s = Session::new(false);
        s.native.0.borrow_mut().refuse = true;
        s.type_text("fn");
        assert!(s.press(vk::NEXT, None));
        assert!(s.press(0x31, Some('1')));
        assert!(s.balanced());
    }

    #[test]
    fn a_missing_manager_leaves_the_controller_exactly_as_it_was() {
        let mut s = Session::new(true);
        s.wire.borrow_mut().unavailable = true;
        s.type_text("fn");
        assert!(s.native.0.borrow().visible);
        assert!(s.press(vk::ESCAPE, None));
        assert!(!s.native.0.borrow().visible);
        assert_eq!(ends(&s.wire), 0);
        assert_eq!(s.wire.borrow().max_open, 0);
    }
}
