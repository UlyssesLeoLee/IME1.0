//! COM glue of the UI-element protocol: the candidate-list element object the host queries, and the adapter over
//! `ITfUIElementMgr` that the lifecycle driver in [`crate::uielement`] calls. No decision lives here; every answer comes
//! from the pure snapshot.
//!
//! STATUS: compiled and called in-process by the tests below, never run under a real TSF host (nothing may be
//! registered or loaded outside a disposable VM). What only a host can show is listed in docs/TSF_VM_TEST_PLAN.md.
//!
//! Rules kept in every entry point: it runs inside [`guard`] (a panic becomes an error code, never an unwind into the
//! host); nothing here borrows a `RefCell` while a call into the host runs (the element only reads published data in
//! [`Shared::with_published`], and `Show` only records a request while a host call is running); the trace lines carry
//! ids, the show answer and error codes, never candidate text.

use std::rc::Rc;

use windows::Win32::Foundation::{BOOL, E_FAIL, E_INVALIDARG, E_NOTIMPL};
use windows::Win32::UI::TextServices::{
    ITfCandidateListUIElement, ITfCandidateListUIElement_Impl, ITfDocumentMgr, ITfThreadMgr,
    ITfUIElement, ITfUIElement_Impl, ITfUIElementMgr,
};
use windows::core::{BSTR, Error, GUID, Interface, Result, implement};

use crate::register::debug_line;
use crate::tip::guard;
use crate::ui::CandidateUi;
use crate::uielement::{
    Begun, DESCRIPTION, ELEMENT_GUID, ElementUi, HostUnavailable, Shared, UiElementHost,
};

fn trace(message: &str) {
    debug_line(&format!("pime-tsf uielement: {message}"));
}

fn error(code: windows::core::HRESULT) -> Error {
    Error::from(code)
}

/// The object handed to `ITfUIElementMgr::BeginUIElement`. TSF holds it from Begin to End; it answers from the list the
/// driver published just before the call.
#[implement(ITfCandidateListUIElement)]
struct CandidateListElement {
    shared: Rc<Shared>,
    /// For `GetDocumentMgr`: the document the user is typing into is the thread manager's focus.
    thread_mgr: Option<ITfThreadMgr>,
}

impl CandidateListElement_Impl {
    /// Read the published list; `E_FAIL` when no element is open.
    fn read<R>(&self, f: impl FnOnce(&crate::uielement::ListSnapshot, u32) -> R) -> Result<R> {
        self.shared.with_published(f).ok_or_else(|| error(E_FAIL))
    }
}

impl ITfUIElement_Impl for CandidateListElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        guard(Err(error(E_FAIL)), || Ok(BSTR::from(DESCRIPTION)))
    }

    fn GetGUID(&self) -> Result<GUID> {
        guard(Err(error(E_FAIL)), || Ok(GUID::from_u128(ELEMENT_GUID)))
    }

    /// The host decides who draws: TRUE the TIP draws its own window, FALSE it hides it.
    fn Show(&self, bshow: BOOL) -> Result<()> {
        guard(Err(error(E_FAIL)), || {
            self.shared.host_show(bshow.as_bool());
            Ok(())
        })
    }

    fn IsShown(&self) -> Result<BOOL> {
        guard(Err(error(E_FAIL)), || Ok(self.shared.is_shown().into()))
    }
}

impl ITfCandidateListUIElement_Impl for CandidateListElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        guard(Err(error(E_FAIL)), || self.read(|_, flags| flags))
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        guard(Err(error(E_FAIL)), || {
            let thread_mgr = self.thread_mgr.as_ref().ok_or_else(|| error(E_FAIL))?;
            // SAFETY: a plain COM call on the thread manager TSF gave us; no borrow of ours is held.
            unsafe { thread_mgr.GetFocus() }
        })
    }

    fn GetCount(&self) -> Result<u32> {
        guard(Err(error(E_FAIL)), || self.read(|list, _| list.count()))
    }

    fn GetSelection(&self) -> Result<u32> {
        guard(Err(error(E_FAIL)), || self.read(|list, _| list.selection()))
    }

    fn GetString(&self, uindex: u32) -> Result<BSTR> {
        guard(Err(error(E_FAIL)), || {
            self.read(|list, _| list.string(uindex).map(BSTR::from))?
                .ok_or_else(|| error(E_INVALIDARG))
        })
    }

    fn GetPageIndex(&self, pindex: *mut u32, usize: u32, pupagecnt: *mut u32) -> Result<()> {
        guard(Err(error(E_FAIL)), || {
            let capacity = (!pindex.is_null()).then_some(usize);
            let count_out = !pupagecnt.is_null();
            let reply = self
                .read(|list, _| list.page_index(capacity, count_out))?
                .map_err(|_| error(E_INVALIDARG))?;
            // SAFETY: the model only hands out `starts` when `pindex` is non-null and has room for all of them (the caller
            // promises `usize` writable entries); `pupagecnt` was checked for null by the model.
            unsafe {
                if !pindex.is_null() {
                    std::ptr::copy_nonoverlapping(
                        reply.starts.as_ptr(),
                        pindex,
                        reply.starts.len(),
                    );
                }
                *pupagecnt = reply.page_count;
            }
            Ok(())
        })
    }

    /// The paging belongs to the TIP (its keys and its candidate machine): the host cannot re-page the list.
    fn SetPageIndex(&self, pindex: *const u32, upagecnt: u32) -> Result<()> {
        guard(Err(error(E_FAIL)), || {
            if pindex.is_null() || upagecnt == 0 {
                Err(error(E_INVALIDARG))
            } else {
                Err(error(E_NOTIMPL))
            }
        })
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        guard(Err(error(E_FAIL)), || {
            self.read(|list, _| list.current_page())
        })
    }
}

/// `ITfUIElementMgr` for the driver. Without a manager (the thread manager does not offer one, or the call fails) the
/// driver draws the native popup, so a failure here is never fatal.
pub(crate) struct TsfUiElementHost {
    target: Option<(ITfUIElementMgr, ITfUIElement)>,
}

impl UiElementHost for TsfUiElementHost {
    fn begin(&mut self) -> std::result::Result<Begun, HostUnavailable> {
        guard(Err(HostUnavailable), || {
            let (mgr, element) = self.target.as_ref().ok_or(HostUnavailable)?;
            let mut show = BOOL(1); // TRUE unless the host says it draws
            let mut id = 0u32;
            // SAFETY: live COM objects; `show` and `id` are locals that outlive the call.
            let began = unsafe { mgr.BeginUIElement(element, &mut show, &mut id) };
            match began {
                Ok(()) => {
                    trace(&format!("begin id={id} show={}", u8::from(show.as_bool())));
                    Ok(Begun {
                        id,
                        show: show.as_bool(),
                    })
                }
                Err(e) => {
                    trace(&format!("begin failed code={:#010x}", e.code().0));
                    Err(HostUnavailable)
                }
            }
        })
    }

    fn update(&mut self, id: u32) {
        guard((), || {
            if let Some((mgr, _)) = &self.target {
                // SAFETY: a live COM object; the id came from `begin`.
                if let Err(e) = unsafe { mgr.UpdateUIElement(id) } {
                    trace(&format!("update id={id} failed code={:#010x}", e.code().0));
                }
            }
        });
    }

    fn end(&mut self, id: u32) {
        guard((), || {
            if let Some((mgr, _)) = &self.target {
                // SAFETY: a live COM object; the id came from `begin` and is ended exactly once.
                let ended = unsafe { mgr.EndUIElement(id) };
                match ended {
                    Ok(()) => trace(&format!("end id={id}")),
                    Err(e) => trace(&format!("end id={id} failed code={:#010x}", e.code().0)),
                }
            }
        });
    }
}

/// The candidate window the controller talks to: the native popup, announced to the host as a candidate-list UI element
/// when the thread manager offers `ITfUIElementMgr`.
pub(crate) fn candidate_ui(
    thread_mgr: &ITfThreadMgr,
    native: Box<dyn CandidateUi>,
) -> ElementUi<TsfUiElementHost> {
    let shared = Shared::new(native);
    let element: ITfCandidateListUIElement = CandidateListElement {
        shared: Rc::clone(&shared),
        thread_mgr: Some(thread_mgr.clone()),
    }
    .into();
    let target = match (
        thread_mgr.cast::<ITfUIElementMgr>(),
        element.cast::<ITfUIElement>(),
    ) {
        (Ok(mgr), Ok(element)) => {
            trace("ITfUIElementMgr available");
            Some((mgr, element))
        }
        _ => {
            trace("no ITfUIElementMgr: the native popup draws");
            None
        }
    };
    ElementUi::new(shared, TsfUiElementHost { target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Anchor, CandidateRow, CandidateView, RecordingUi};
    use windows::Win32::UI::TextServices::{
        ITfCandidateListUIElementBehavior, TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE,
        TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION, TF_CLUIE_STRING,
    };
    use windows::core::IUnknown;

    const ANCHOR: Anchor = Anchor {
        x: 10,
        y: 20,
        line_height: 16,
    };

    fn view(rows: usize, selected: usize) -> CandidateView {
        CandidateView {
            rows: (0..rows)
                .map(|i| CandidateRow {
                    label: (i + 1).to_string(),
                    text: format!("T{i}"),
                    detail: "rule · valid".to_string(),
                })
                .collect(),
            selected_in_page: selected,
            page: 0,
            page_count: 1,
        }
    }

    /// A host that lets the TIP draw and counts nothing: only here to make the driver publish a list.
    struct AnyHost {
        show: bool,
    }

    impl UiElementHost for AnyHost {
        fn begin(&mut self) -> std::result::Result<Begun, HostUnavailable> {
            Ok(Begun {
                id: 7,
                show: self.show,
            })
        }

        fn update(&mut self, _id: u32) {}

        fn end(&mut self, _id: u32) {}
    }

    struct Fixture {
        element: ITfCandidateListUIElement,
        ui: ElementUi<AnyHost>,
        shared: Rc<Shared>,
    }

    fn fixture(show: bool) -> Fixture {
        let shared = Shared::new(Box::new(RecordingUi::default()));
        let element: ITfCandidateListUIElement = CandidateListElement {
            shared: Rc::clone(&shared),
            thread_mgr: None,
        }
        .into();
        Fixture {
            element,
            ui: ElementUi::new(Rc::clone(&shared), AnyHost { show }),
            shared,
        }
    }

    #[test]
    fn the_flag_bits_of_the_model_are_the_windows_constants() {
        use crate::uielement::updated;
        assert_eq!(updated::DOCUMENT_MGR, TF_CLUIE_DOCUMENTMGR);
        assert_eq!(updated::COUNT, TF_CLUIE_COUNT);
        assert_eq!(updated::SELECTION, TF_CLUIE_SELECTION);
        assert_eq!(updated::STRING, TF_CLUIE_STRING);
        assert_eq!(updated::PAGE_INDEX, TF_CLUIE_PAGEINDEX);
        assert_eq!(updated::CURRENT_PAGE, TF_CLUIE_CURRENTPAGE);
    }

    #[test]
    fn the_element_answers_every_interface_a_host_may_ask_for() {
        let f = fixture(true);
        assert!(f.element.cast::<IUnknown>().is_ok());
        let base: ITfUIElement = f.element.cast().expect("ITfUIElement");
        assert!(
            f.element
                .cast::<ITfCandidateListUIElementBehavior>()
                .is_err(),
            "the behaviour interface (host-driven selection) is not implemented and must not be claimed"
        );
        // the same object behind every interface
        let back: ITfCandidateListUIElement = base.cast().expect("ITfCandidateListUIElement");
        assert_eq!(back.as_raw(), f.element.as_raw());
    }

    #[test]
    fn before_any_list_every_query_fails_cleanly_and_the_identity_is_fixed() {
        let f = fixture(true);
        // SAFETY: in-process calls on a live object; the out pointers are locals.
        unsafe {
            assert!(f.element.GetCount().is_err());
            assert!(f.element.GetSelection().is_err());
            assert!(f.element.GetString(0).is_err());
            assert!(f.element.GetUpdatedFlags().is_err());
            assert!(f.element.GetCurrentPage().is_err());
            let mut count = 0u32;
            assert!(f.element.GetPageIndex(&mut [], &mut count).is_err());
            assert!(
                f.element.GetDocumentMgr().is_err(),
                "no thread manager in this fixture"
            );
            assert_eq!(f.element.GetGUID().unwrap(), GUID::from_u128(ELEMENT_GUID));
            assert_eq!(
                f.element.GetDescription().unwrap().to_string(),
                "Programming IME candidate list"
            );
            assert!(!f.element.IsShown().unwrap().as_bool());
        }
    }

    #[test]
    fn the_getters_answer_from_the_published_list() {
        let mut f = fixture(true);
        f.ui.show(&view(5, 2), ANCHOR);
        // SAFETY: in-process calls on a live object; the buffers outlive the calls.
        unsafe {
            assert_eq!(f.element.GetCount().unwrap(), 5);
            assert_eq!(f.element.GetSelection().unwrap(), 2);
            assert_eq!(
                f.element.GetString(0).unwrap().to_string(),
                "T0 (rule · valid)"
            );
            assert_eq!(
                f.element.GetString(4).unwrap().to_string(),
                "T4 (rule · valid)"
            );
            assert_eq!(
                f.element.GetString(5).unwrap_err().code(),
                E_INVALIDARG,
                "an index past the list is an invalid argument"
            );
            assert_eq!(f.element.GetCurrentPage().unwrap(), 0);
            assert_eq!(
                f.element.GetUpdatedFlags().unwrap(),
                TF_CLUIE_DOCUMENTMGR
                    | TF_CLUIE_COUNT
                    | TF_CLUIE_SELECTION
                    | TF_CLUIE_STRING
                    | TF_CLUIE_PAGEINDEX
                    | TF_CLUIE_CURRENTPAGE
            );
        }
        f.ui.show(&view(5, 3), ANCHOR);
        // SAFETY: as above.
        unsafe {
            assert_eq!(f.element.GetSelection().unwrap(), 3);
            assert_eq!(f.element.GetUpdatedFlags().unwrap(), TF_CLUIE_SELECTION);
        }
        crate::ui::CandidateUi::hide(&mut f.ui);
        // SAFETY: as above.
        unsafe {
            assert!(
                f.element.GetCount().is_err(),
                "nothing is published after the end"
            );
        }
    }

    #[test]
    fn get_page_index_follows_the_documented_two_step_protocol_and_never_overruns() {
        let mut f = fixture(true);
        f.ui.show(&view(5, 0), ANCHOR);
        // SAFETY: in-process calls; the buffers are locals and the sizes passed are their real lengths (the slice wrapper
        // of the generated method derives the size from the slice); the canary entries prove that a one-page answer
        // never touches the next entry. The raw vtable call is the only way to pass a null buffer, which is how a host
        // asks for the count first.
        unsafe {
            let vtbl = Interface::vtable(&f.element);
            let raw = Interface::as_raw(&f.element);
            // step 1: a null buffer, ask for the count
            let mut pages = 99u32;
            let hr = (vtbl.GetPageIndex)(raw, std::ptr::null_mut(), 0, &mut pages);
            assert!(hr.is_ok());
            assert_eq!(pages, 1);
            // step 2: a buffer with room
            let mut buffer = [0xDEAD_BEEFu32; 4];
            let mut pages = 0u32;
            f.element.GetPageIndex(&mut buffer, &mut pages).unwrap();
            assert_eq!(pages, 1);
            assert_eq!(buffer, [0, 0xDEAD_BEEF, 0xDEAD_BEEF, 0xDEAD_BEEF]);
            // a null count pointer is refused
            let mut buffer = [0u32; 1];
            let hr = (vtbl.GetPageIndex)(raw, buffer.as_mut_ptr(), 1, std::ptr::null_mut());
            assert_eq!(hr, E_INVALIDARG);
            // a buffer of size 0 cannot hold the page
            let mut pages = 0u32;
            let hr = (vtbl.GetPageIndex)(raw, buffer.as_mut_ptr(), 0, &mut pages);
            assert_eq!(hr, E_INVALIDARG);
            assert_eq!(buffer, [0], "nothing was written");
        }
    }

    #[test]
    fn the_host_cannot_re_page_the_list() {
        let mut f = fixture(true);
        f.ui.show(&view(5, 0), ANCHOR);
        // SAFETY: in-process calls; the buffer is a local.
        unsafe {
            let starts = [0u32, 3];
            assert_eq!(
                f.element.SetPageIndex(&starts).unwrap_err().code(),
                E_NOTIMPL
            );
            let vtbl = Interface::vtable(&f.element);
            let raw = Interface::as_raw(&f.element);
            assert_eq!(
                (vtbl.SetPageIndex)(raw, std::ptr::null(), 2),
                E_INVALIDARG,
                "a null array is an invalid argument, not a crash"
            );
        }
    }

    #[test]
    fn the_hosts_show_call_reaches_the_shared_decision() {
        let mut f = fixture(true);
        f.ui.show(&view(3, 0), ANCHOR);
        // SAFETY: in-process calls on a live object.
        unsafe {
            assert!(
                f.element.IsShown().unwrap().as_bool(),
                "our window is drawn"
            );
            f.element.Show(BOOL(0)).unwrap();
            assert!(!f.element.IsShown().unwrap().as_bool());
            assert!(!f.shared.is_shown());
            f.element.Show(BOOL(1)).unwrap();
            assert!(f.element.IsShown().unwrap().as_bool());
        }
    }

    #[test]
    fn without_a_manager_the_adapter_reports_unavailable_and_ends_nothing() {
        let mut host = TsfUiElementHost { target: None };
        assert_eq!(host.begin(), Err(HostUnavailable));
        host.update(1);
        host.end(1);
    }
}
