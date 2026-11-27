//! The text input processor (TIP): the COM glue between TSF and [`crate::controller`]. It is deliberately thin: it
//! forwards callbacks, computes what only Windows can tell (the produced character, the modifier state, the caret
//! rectangle) and performs the host edits the controller asks for in edit sessions. Every decision lives in the
//! controller, which has the tests.
//!
//! STATUS: compiled, never executed against a real TSF host (nothing here may be registered or loaded outside a
//! disposable VM). The behaviours that only a host can show are listed in the work item's report.
//!
//! Rules kept in every callback: no waiting, no I/O, no network; every COM method runs inside [`guard`] so that a panic
//! becomes "key not eaten" instead of an unwind into the host; shared state is borrowed with `try_borrow_mut` because
//! TSF may re-enter a sink while one of our edit sessions runs (a missed borrow is remembered and replayed).
//!
//! This file is the shell: the class factory, the interfaces that TSF calls and the state they share. The work
//! itself lives in the submodules:
//!
//! - `lifecycle`: the activation, the deactivation, the change of the active profile and the switch of the input
//!   method.
//! - `keys`: the key path: the key events of TSF and what only Windows can tell about them.
//! - `panel`: the laser panel: its rows, the menus and the demonstration.
//! - `bridge`: the AI bridge, on the side of the input method.
//! - `settings`: the settings of the input method and the window that shows them.

mod bridge;
mod keys;
mod lifecycle;
mod panel;
mod settings;

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::{BOOL, E_FAIL, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::TextServices::{
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_INPUTSCOPE, IS_PASSWORD,
    ITfActiveLanguageProfileNotifySink, ITfActiveLanguageProfileNotifySink_Impl, ITfCompartmentMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputScope, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfRange, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, InputScope, TF_AE_NONE, TF_ANCHOR_END, TF_DEFAULT_SELECTION,
    TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::core::{GUID, IUnknown, IUnknownImpl, Interface, Result, implement};

use crate::agent_bridge::Bridge;
use crate::controller::{HostError, HostText, TipController};
use crate::imesettings::LanguageSettings;
use crate::laser_ui::{LaserEvent, LaserUi};
use crate::menus::{MenuAction, MenuSink, MenuWindow};
use crate::register::debug_line;
use crate::settings_window::{SettingsSink, SettingsWindow};
use crate::ui::{Anchor, CandidateUi, CandidateView};

use self::keys::KeyKind;

/// Whether two COM interface pointers point at the same object.
///
/// One object can be reached through several interfaces (a context is also an `ITfContextComposition`, for
/// example), and the pointer of the interface we happen to hold is not stable across them. The identity of the
/// object is its `IUnknown` pointer, which every interface of that object shares.
fn same_object<T: Interface, U: Interface>(a: &T, b: &U) -> bool {
    let (Ok(a), Ok(b)) = (a.cast::<IUnknown>(), b.cast::<IUnknown>()) else {
        return false;
    };
    Interface::as_raw(&a) == Interface::as_raw(&b)
}

/// Debugger output (DebugView) for the rare host behaviours the VM test has to see: codes and words only, never text.
fn trace(message: &str) {
    debug_line(&format!("pime-tsf tip: {message}"));
}

/// Run a COM entry point so that a panic cannot unwind into the host: it answers `fallback` instead. Defined in `ui`
/// (plain `std`) because the popup's window procedure needs the same containment and is also compiled on its own by
/// `examples/popup_demo.rs`, which has no `tip` module.
pub(crate) use crate::ui::guard;

fn refused<E>(_: E) -> HostError {
    HostError::Refused
}

/// The candidate window is injected as a trait object (the production list is `laser_candidates::LaserCandidates`).
struct BoxedUi(Box<dyn CandidateUi>);

impl CandidateUi for BoxedUi {
    fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool {
        self.0.show(view, anchor)
    }

    fn hide(&mut self) {
        self.0.hide();
    }

    fn is_visible(&self) -> bool {
        self.0.is_visible()
    }
}

/// One edit session running a closure with the edit cookie. The sessions that matter for a key are synchronous
/// ([`run_session`]), so the closure can hand its result back through a shared cell; the only asynchronous one is the
/// retry of a refused end of a composition ([`request_async`]), which has no result to hand back.
#[implement(ITfEditSession)]
struct EditSession {
    work: RefCell<Option<SessionWork>>,
    /// Counts this object in the DLL's live-object counter while it exists: TSF calls `DoEditSession` on it, and
    /// that call must not reach an unloaded DLL.
    _live: crate::LiveGuard,
}

type SessionWork = Box<dyn FnOnce(u32)>;

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || {
            let work = self.work.borrow_mut().take();
            if let Some(work) = work {
                work(ec);
            }
            Ok(())
        })
    }
}

/// Ask TSF for an edit session on `ctx` and return what `work` produced. Fails with [`HostError::Refused`] when TSF
/// refuses the session (no lock, read-only document, no synchronous lock available in this callback).
fn run_session<T: 'static>(
    ctx: &ITfContext,
    client_id: u32,
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    work: impl FnOnce(u32) -> std::result::Result<T, HostError> + 'static,
) -> std::result::Result<T, HostError> {
    let out: Rc<RefCell<Option<std::result::Result<T, HostError>>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&out);
    let mut guard = crate::LiveGuard::new();
    guard.count();
    let session: ITfEditSession = EditSession {
        work: RefCell::new(Some(Box::new(move |ec| {
            *sink.borrow_mut() = Some(work(ec));
        }))),
        _live: guard,
    }
    .into();
    // SAFETY: `ctx` and `session` are live COM objects; the session runs (or is refused) before the call returns because
    // the flags request a synchronous session.
    let requested = unsafe { ctx.RequestEditSession(client_id, &session, flags) };
    match requested {
        Ok(hr) if hr.is_ok() => out.borrow_mut().take().unwrap_or(Err(HostError::Refused)),
        _ => Err(HostError::Refused),
    }
}

/// Queue an edit session that nothing waits for. With `TF_ES_ASYNCDONTCARE` TSF runs it at once when the lock is free
/// and otherwise when it is granted (`TF_S_ASYNC`): that is what makes it fit for a retry after a refused synchronous
/// session. `work` owns what it needs; TSF drops it with the session. Ok when TSF accepted the request (`S_OK` or
/// `TF_S_ASYNC`), which says nothing about the work itself.
fn request_async(
    ctx: &ITfContext,
    client_id: u32,
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    work: impl FnOnce(u32) + 'static,
) -> std::result::Result<(), HostError> {
    let mut guard = crate::LiveGuard::new();
    guard.count();
    let session: ITfEditSession = EditSession {
        work: RefCell::new(Some(Box::new(work))),
        _live: guard,
    }
    .into();
    // SAFETY: `ctx` and `session` are live COM objects; TSF keeps its own reference to the session while it is queued.
    let requested = unsafe { ctx.RequestEditSession(client_id, &session, flags) };
    match requested {
        Ok(hr) if hr.is_ok() => Ok(()),
        _ => Err(HostError::Refused),
    }
}

const SYNC_READ: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS =
    windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
const SYNC_WRITE: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS =
    windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS(
        TF_ES_SYNC.0 | TF_ES_READWRITE.0,
    );
/// Read-write, synchronous when the lock is free and queued otherwise (`TF_ES_ASYNCDONTCARE` is 0 in the headers).
const ASYNC_WRITE: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS =
    windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS(
        TF_ES_ASYNCDONTCARE.0 | TF_ES_READWRITE.0,
    );

fn selection_of(range: ITfRange) -> TF_SELECTION {
    TF_SELECTION {
        range: ManuallyDrop::new(Some(range)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    }
}

/// The caret as a (possibly empty) range. A non-empty selection is not ours to replace.
///
/// # Safety
/// `ec` must be the edit cookie of the running edit session on `ctx`.
unsafe fn caret_range(ctx: &ITfContext, ec: u32) -> Result<ITfRange> {
    let mut selection = [selection_of_none()];
    let mut fetched = 0u32;
    // SAFETY: `ec` is valid per the contract; the array and counter outlive the call.
    unsafe { ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)? };
    let [first] = selection;
    // the field holds an owned reference filled in by TSF (or None): taking it out hands the release to us
    let range = ManuallyDrop::into_inner(first.range);
    match range {
        Some(range) if fetched == 1 => Ok(range),
        _ => Err(E_FAIL.into()),
    }
}

fn selection_of_none() -> TF_SELECTION {
    TF_SELECTION {
        range: ManuallyDrop::new(None),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    }
}

/// The range covering exactly `typed`, which must be the text just before the caret: verified by reading it back, so
/// the write that follows can never land on text the user did not type (no blind ranges, no simulated Backspace).
///
/// # Safety
/// `ec` must be the edit cookie of the running edit session on `ctx`.
unsafe fn typed_range(
    ctx: &ITfContext,
    ec: u32,
    typed: &str,
) -> std::result::Result<ITfRange, HostError> {
    // SAFETY: forwarded contract.
    let caret = unsafe { caret_range(ctx, ec) }.map_err(refused)?;
    // SAFETY: `ec` is valid; all ranges are live COM objects.
    unsafe {
        if !caret.IsEmpty(ec).map_err(refused)?.as_bool() {
            return Err(HostError::Mismatch);
        }
        let range = caret.Clone().map_err(refused)?;
        let want: Vec<u16> = typed.encode_utf16().collect();
        if want.is_empty() {
            return Ok(range);
        }
        let wanted = -(want.len() as i32);
        let mut start = 0i32;
        range
            .ShiftStart(ec, wanted, &mut start, std::ptr::null())
            .map_err(refused)?;
        // `start` is the **absolute** position of the new start of the range, not the number of units it moved:
        // comparing it with the shift failed for every range, so the verification below never ran, every write was
        // refused, the input method degraded and the keys of the candidate fell through to the application (the digit
        // of a candidate landed in the document instead of the candidate). The text that follows is the proof that the
        // range covers exactly what the user typed: nothing is written over text the user did not type.
        if start < 0 {
            return Err(HostError::Mismatch);
        }
        let mut buffer = vec![0u16; want.len() + 1];
        let mut got = 0u32;
        range
            .GetText(ec, 0, &mut buffer, &mut got)
            .map_err(refused)?;
        if buffer[..(got as usize).min(buffer.len())] != want[..] {
            return Err(HostError::Mismatch);
        }
        Ok(range)
    }
}

/// Open a composition over the caret and hand it to the sink.
///
/// The composition is opened over the **caret**, not over the text the user typed: the range of that text cannot be
/// found by reading the document (a host that does not sync its text to the text service reads empty - measured on
/// a plain edit control on 2026-11-21), and the text service does not need it: it writes the typed text into its own
/// composition and replaces it with the candidate.
///
/// # Safety
/// `ctx`, `client_id` and `sink` must be live; the edit cookie is taken from the session this function opens.
fn open_composition(
    ctx: &ITfContext,
    client_id: u32,
    sink: &ITfCompositionSink,
) -> std::result::Result<ITfComposition, HostError> {
    let (ctx2, sink) = (ctx.clone(), sink.clone());
    run_session(ctx, client_id, SYNC_WRITE, move |ec| {
        // SAFETY: `ec` belongs to the running session on `ctx2`.
        unsafe {
            let caret = caret_range(&ctx2, ec).map_err(refused)?;
            let owner: ITfContextComposition = ctx2.cast().map_err(refused)?;
            // SAFETY: live COM objects and a valid edit cookie.
            let composition = owner.StartComposition(ec, &caret, &sink).map_err(refused)?;
            Ok(composition)
        }
    })
}

/// True when the input scope of the context says "password".
///
/// # Safety
/// `ec` must be the edit cookie of the running (read) edit session on `ctx`.
unsafe fn scope_is_password(ctx: &ITfContext, ec: u32) -> Result<bool> {
    // SAFETY: `ec` is valid; every pointer handed out is checked and the CoTaskMem array is freed exactly once.
    unsafe {
        // the input scope is an application property: the text store supplies it (ITfContext::GetAppProperty)
        let property = ctx.GetAppProperty(&GUID_PROP_INPUTSCOPE)?;
        let start = ctx.GetStart(ec)?;
        let value = property.GetValue(ec, &start)?;
        let Ok(unknown) = IUnknown::try_from(&value) else {
            return Ok(false); // no input scope on this range: an ordinary field
        };
        let scope: ITfInputScope = unknown.cast()?;
        let mut scopes: *mut InputScope = std::ptr::null_mut();
        let mut count = 0u32;
        scope.GetInputScopes(&mut scopes, &mut count)?;
        if scopes.is_null() {
            return Ok(false);
        }
        let password = std::slice::from_raw_parts(scopes, count as usize).contains(&IS_PASSWORD);
        CoTaskMemFree(Some(scopes as *const c_void));
        Ok(password)
    }
}

/// `GUID_COMPARTMENT_KEYBOARD_DISABLED` on the context: the application switched text services off for this field.
fn keyboard_disabled(ctx: &ITfContext) -> bool {
    let Ok(compartments) = ctx.cast::<ITfCompartmentMgr>() else {
        return false;
    };
    // SAFETY: plain COM calls on live interfaces; no edit cookie is needed for compartments.
    unsafe {
        compartments
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)
            .and_then(|c| c.GetValue())
            .ok()
            .and_then(|v| i32::try_from(&v).ok())
            .is_some_and(|v| v != 0)
    }
}

/// [`HostText`] over TSF: one current context (the one the last key event named) and at most one open composition.
struct TsfHost {
    client_id: u32,
    sink: ITfCompositionSink,
    context: Option<ITfContext>,
    composition: Option<(ITfContext, ITfComposition)>,
    /// A composition whose synchronous end TSF refused. It waits here, apart from `composition` (so that the next
    /// `begin_composition` never mistakes it for an open one), for the single asynchronous attempt of
    /// `end_composition_later`; the controller makes that attempt right after the refusal.
    stranded: Option<(ITfContext, ITfComposition)>,
    /// The range the text service wrote the typed text into: the range of the composition, or a plain range when the
    /// host refused the composition.
    ///
    /// The candidate replaces the typed text **there**, so the write-back never reads the document: a host that does
    /// not sync its text to the text service reads empty, and a write that depends on reading never lands (the
    /// candidate landed after the pinyin: "wo我").
    owned: Option<ITfRange>,
}

impl TsfHost {
    fn new(client_id: u32, sink: ITfCompositionSink) -> TsfHost {
        TsfHost {
            client_id,
            sink,
            context: None,
            composition: None,
            stranded: None,
            owned: None,
        }
    }

    /// The context a key event arrived for.
    fn set_context(&mut self, pic: Option<&ITfContext>) {
        self.context = pic.cloned();
    }

    /// Forget the document the user left. An open composition stays recorded: the controller closes it with
    /// `end_composition`, which must reach the context that owns it.
    fn forget_context(&mut self) {
        self.context = None;
    }

    /// Whether `ctx` is the context the last key event named.
    fn is_current_context(&self, ctx: &ITfContext) -> bool {
        self.context
            .as_ref()
            .is_some_and(|current| same_object(current, ctx))
    }

    /// Whether the context we type into belongs to `pdim`.
    ///
    /// A host may have several document managers alive at once (one per document, for example); only the one that
    /// owns our context matters when that manager is torn down.
    fn is_in_document(&self, pdim: &ITfDocumentMgr) -> bool {
        let Some(ctx) = self.context.as_ref() else {
            return false;
        };
        // SAFETY: `ctx` is a live context; a host that refuses the call leaves us with "not ours", which is the
        // safe answer: we then keep the context and let the focus change clean it up.
        let Ok(owner) = (unsafe { ctx.GetDocumentMgr() }) else {
            return false;
        };
        same_object(&owner, pdim)
    }

    fn probe_secure(&self, ctx: &ITfContext) -> Option<bool> {
        if keyboard_disabled(ctx) {
            return Some(true);
        }
        let ctx2 = ctx.clone();
        // SAFETY: the closure only runs inside the edit session it is handed to (see `run_session`).
        run_session(ctx, self.client_id, SYNC_READ, move |ec| {
            unsafe { scope_is_password(&ctx2, ec) }.map_err(refused)
        })
        .ok()
    }

    /// The range of the open composition: the position of the typed text, when the composition holds it.
    ///
    /// The composition owns the text that was typed into it (the text service wrote it there, or the host
    /// inserted it into the composition), so this range is where the typed text is - without reading the
    /// document.
    fn composition_range(&self) -> Option<ITfRange> {
        let (_, composition) = self.composition.as_ref()?;
        // SAFETY: the composition is a live COM object.
        unsafe { composition.GetRange() }.ok()
    }

    /// Write the candidate over `range` and put the caret after it.
    fn write_over(
        &self,
        ctx: &ITfContext,
        range: ITfRange,
        text: &str,
    ) -> std::result::Result<(), HostError> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let ctx2 = ctx.clone();
        run_session(ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                range.SetText(ec, 0, &wide).map_err(refused)?;
                // the caret goes after the candidate
                let caret = range.Clone().map_err(refused)?;
                caret.Collapse(ec, TF_ANCHOR_END).map_err(refused)?;
                let selection = selection_of(caret);
                let set = ctx2.SetSelection(ec, std::slice::from_ref(&selection));
                drop(ManuallyDrop::into_inner(selection.range));
                set.map_err(refused)
            }
        })
    }

    /// Replace the typed text that the document holds before the caret with the candidate.
    ///
    /// The typed text is read back and verified against what was typed, so the write never lands on text the
    /// user did not type. A document that cannot be read back (a host that does not sync its text to the text
    /// service) makes this fail: the typed text cannot be located, and the candidate must not be written next to
    /// it.
    fn write_over_typed(
        &self,
        ctx: &ITfContext,
        typed: &str,
        text: &str,
    ) -> std::result::Result<(), HostError> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let (ctx2, typed) = (ctx.clone(), typed.to_string());
        run_session(ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = typed_range(&ctx2, ec, &typed)?;
                range.SetText(ec, 0, &wide).map_err(refused)?;
                // the caret goes after the inserted text
                range.Collapse(ec, TF_ANCHOR_END).map_err(refused)?;
                let selection = selection_of(range);
                let set = ctx2.SetSelection(ec, std::slice::from_ref(&selection));
                drop(ManuallyDrop::into_inner(selection.range));
                set.map_err(refused)
            }
        })
    }
}

/// What a commit does: where the candidate replaces the typed text.
///
/// The typed text is in exactly one place and the candidate replaces it there. When it cannot be found at all, the
/// plan is [`CommitPlan::Fail`] and **nothing is written**: the candidate is never written at the caret next to
/// the typed text - that is exactly how "pinyin + candidate" reaches the document (the report of the user on
/// 2026-11-24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitPlan {
    /// The candidate replaces the typed text in the range the text service wrote it into.
    Owned,
    /// The candidate replaces the typed text in the range of the open composition: the typed text is in it
    /// (the text service wrote it there, or the host inserted it into the composition).
    Composition,
    /// The candidate replaces the typed text that the document holds before the caret: that text is read back and
    /// verified against what was typed.
    Document,
    /// The typed text cannot be located: nothing is written and the commit fails.
    Fail,
}

/// Decide what a commit does, from what the text service knows about the typed text.
///
/// - `owned`: the text service wrote the typed text into a range it owns (the range of the composition, or the
///   range it wrote into when the host refused the composition).
/// - `composition`: a composition is open; its range holds the typed text.
/// - `document`: the typed text may be in the document (the host wrote it there): it is then found before the
///   caret and verified against what was typed.
///
/// The function is pure so that the rule can be broken on purpose by the tests of this file: it is the one place
/// that decides where a candidate goes.
fn plan_commit(owned: bool, composition: bool, document: bool) -> CommitPlan {
    if owned {
        CommitPlan::Owned
    } else if composition {
        CommitPlan::Composition
    } else if document {
        CommitPlan::Document
    } else {
        CommitPlan::Fail
    }
}

impl HostText for TsfHost {
    /// Probed afresh every time it is asked: the controller asks once per key event (never on a key-up), and an answer
    /// kept per context would go stale when a host turns a text field into a password field without a focus event.
    fn is_secure_field(&mut self) -> bool {
        let Some(ctx) = self.context.clone() else {
            return true; // no context to type into or to look at: fail closed
        };
        // could not tell (session refused, no input scope support): fail closed, DD section 3.3
        self.probe_secure(&ctx).unwrap_or(true)
    }

    fn caret_anchor(&mut self) -> Option<Anchor> {
        let ctx = self.context.clone()?;
        let ctx2 = ctx.clone();
        run_session(&ctx, self.client_id, SYNC_READ, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`; the rectangle and flag outlive the call.
            unsafe {
                let caret = caret_range(&ctx2, ec).map_err(refused)?;
                let view = ctx2.GetActiveView().map_err(refused)?;
                let mut rect = RECT::default();
                let mut clipped = BOOL(0);
                view.GetTextExt(ec, &caret, &mut rect, &mut clipped)
                    .map_err(refused)?;
                if rect.bottom <= rect.top {
                    return Err(HostError::Refused); // the host reported an empty rectangle: no usable position
                }
                Ok(Anchor {
                    x: rect.left,
                    y: rect.top,
                    line_height: rect.bottom - rect.top,
                })
            }
        })
        .ok()
    }

    /// Open the composition of the text service over the caret (empty).
    ///
    /// The range of the typed text cannot be found by reading the document (a host that does not sync its text to the
    /// text service reads empty - measured on a plain edit control on 2026-11-21), so the composition is opened over
    /// the caret and the typed text goes into it.
    fn begin_composition(&mut self) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        if self.composition.is_some() {
            return Ok(()); // the composition is open: there is nothing to open
        }
        let composition = open_composition(&ctx, self.client_id, &self.sink)?;
        self.composition = Some((ctx, composition));
        Ok(())
    }

    /// Write the typed text into the composition of the text service, opening that composition over the caret
    /// when it is not open yet.
    ///
    /// The text service owns the range of the composition: it writes the typed text into it and replaces it with the
    /// candidate later. Nothing is read from the document, so this works on a host that does not sync its text to the
    /// text service (measured on a plain edit control on 2026-11-21).
    fn write_composition(&mut self, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        if self.composition.is_none() {
            self.begin_composition()?;
        }
        let (_, composition) = self.composition.clone().ok_or(HostError::Refused)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the composition is a live COM object.
        let range = unsafe { composition.GetRange() }.map_err(refused)?;
        // The text service keeps the range: the candidate replaces the typed text there (the commit never reads the
        // document).
        self.owned = Some(range.clone());
        run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on the composition's context.
            unsafe {
                range.SetText(ec, 0, &wide).map_err(refused)
            }
        })
    }

    /// Write one letter into the document at the caret, and put the caret after it.
    ///
    /// This is the path of the last resort: the host refused the composition, so the text service cannot replace the
    /// typed text later - the candidate lands after it instead.
    fn write_char(&mut self, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // The range the text service owns (the one it wrote the typed text into) or the caret: the typed text grows
        // inside that range, so the candidate can replace it later.
        let owned = self.owned.clone();
        let ctx2 = ctx.clone();
        let range = run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = match owned {
                    Some(range) => range,
                    None => caret_range(&ctx2, ec).map_err(refused)?,
                };
                range.SetText(ec, 0, &wide).map_err(refused)?;
                // the caret goes after the typed text
                let caret = range.Clone().map_err(refused)?;
                caret.Collapse(ec, TF_ANCHOR_END).map_err(refused)?;
                let selection = selection_of(caret);
                let set = ctx2.SetSelection(ec, std::slice::from_ref(&selection));
                drop(ManuallyDrop::into_inner(selection.range));
                set.map_err(refused)?;
                Ok(range)
            }
        })?;
        // The text service keeps the range it wrote into: the candidate replaces the typed text there.
        self.owned = Some(range);
        Ok(())
    }

    fn end_composition(&mut self) -> std::result::Result<(), HostError> {
        let Some((ctx, composition)) = self.composition.take() else {
            return Ok(());
        };
        let ending = composition.clone();
        let ended = run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on the composition's context.
            unsafe { ending.EndComposition(ec) }.map_err(refused)
        });
        if ended.is_err() {
            // refused: do not drop the handle, the composition is still open in that document
            self.stranded = Some((ctx, composition));
        }
        ended
    }

    fn end_composition_later(&mut self) -> std::result::Result<(), HostError> {
        let Some((ctx, composition)) = self.stranded.take() else {
            return Ok(());
        };
        let requested = request_async(&ctx, self.client_id, ASYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on the composition's context. A composition that is already
            // gone (the host ended it meanwhile) only makes this fail, which nobody is waiting for.
            match unsafe { composition.EndComposition(ec) } {
                Ok(()) => trace("asynchronous end ran: composition ended"),
                Err(e) => trace(&format!(
                    "asynchronous end ran and failed code={:#010x}",
                    e.code().0
                )),
            }
        });
        trace(if requested.is_ok() {
            "end of the composition refused, asynchronous end requested"
        } else {
            "end of the composition refused, asynchronous end refused too (composition left to the host)"
        });
        requested
    }

    /// Replace the typed text with the candidate: the document holds the candidate and nothing else.
    ///
    /// The typed text is in exactly one place and the candidate replaces it there:
    ///
    /// - the range the text service wrote it into ([`TsfHost::owned`]), or
    /// - the range of the open composition: the typed text is in it, whether the text service wrote it there or
    ///   the host inserted it into the composition. That range is the position of the typed text by construction:
    ///   nothing is read from the document, so this works on a host that does not sync its text to the text
    ///   service (measured on a plain edit control on 2026-11-21), or
    /// - the document: the host wrote the typed text there (the candidate preview of the code input is exactly
    ///   this case). The text before the caret is read back and verified against what was typed.
    ///
    /// When the typed text cannot be located at all, the commit **fails** and nothing is written. The candidate is
    /// never written at the caret next to the typed text: that is exactly how "pinyin + candidate" reaches the
    /// document (the report of the user on 2026-11-24). A failed commit hands the key back to the host and the
    /// machine degrades: the user sees the pinyin alone, never the pinyin and the candidate together.
    fn commit(&mut self, typed: &str, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        // The plan: where the candidate replaces the typed text. The decision is the pure function
        // `plan_commit`; the code below only wires it to the COM calls.
        let plan = plan_commit(
            self.owned.is_some(),
            self.composition.is_some(),
            // The document is the last chance: the typed text may be there (the host wrote it there).
            true,
        );
        match plan {
            CommitPlan::Owned | CommitPlan::Composition => {
                let range = match plan {
                    CommitPlan::Owned => self.owned.clone(),
                    CommitPlan::Composition => self.composition_range(),
                    _ => None,
                }
                .ok_or(HostError::Refused)?;
                // The candidate replaces the typed text **there**; the caret goes after it. Nothing is read from
                // the document: a host that does not sync its text to the text service reads empty (measured on a
                // plain edit control on 2026-11-21), and a write that depends on reading never lands - the
                // candidate landed after the pinyin ("wo我", the report of the user on 2026-11-23).
                self.write_over(&ctx, range, text)?;
                // The composition ends after the text is written (the order of the platform).
                let _ = self.end_composition();
                self.owned = None;
                Ok(())
            }
            CommitPlan::Document => {
                // The typed text is in the document (the host wrote it there): it is read back, verified against
                // what was typed, and the candidate replaces it there.
                match self.write_over_typed(&ctx, typed, text) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // The document cannot be read back: the typed text is nowhere the text service can reach.
                        // The plan for these facts is `Fail`: **nothing is written**. The key goes back to the host
                        // and the machine degrades - the pinyin stays alone in the document, the candidate is never
                        // written next to it.
                        debug_assert_eq!(plan_commit(false, false, false), CommitPlan::Fail);
                        trace("the typed text could not be located: the candidate is not written");
                        Err(error)
                    }
                }
            }
            CommitPlan::Fail => Err(HostError::Refused),
        }
    }
}

/// Everything that exists between `Activate` and `Deactivate`.
struct Inner {
    thread_mgr: ITfThreadMgr,
    client_id: u32,
    thread_sink_cookie: u32,
    /// The cookie of the sink that hears about the active language profile.
    language_sink_cookie: u32,
    controller: TipController<BoxedUi, TsfHost, crate::predictor::PluginPredictor>,
    started: Instant,
    /// Actions the user or the host asked for while the text service was busy: they are applied now. A host that
    /// answers an update by calling back into the element produces one more action, so the rounds are bounded.
    pending_actions: Rc<RefCell<Vec<Pending>>>,
    /// The menu that is open (at most one).
    menu: Option<MenuWindow>,
    /// The settings window while it is open; `None` when it is closed.
    settings_window: Option<SettingsWindow>,
    /// How the settings window tells this text service that the settings were saved.
    settings_sink: SettingsSink,
    /// How a menu tells this text service what the user chose.
    menu_sink: MenuSink,
    /// The language profile that is active ("zh-CN", "en-US", "ja-JP").
    language: String,
    /// The settings of that language (the ones the floating bar and the controller work with).
    settings: LanguageSettings,
    /// The effort the AI features run with.
    effort: String,
    /// The skin of the floating bar.
    skin: String,
    /// Where the mouth of the light is on the screen (a global setting, not one of a language).
    mouth_place: String,
    /// The laser interface of this thread: the glow on the left of the screen, the beam and the panel.
    laser: Rc<RefCell<LaserUi>>,
    /// That the laser interface has to go off the screen, asked for while it was busy.
    ///
    /// The interface cannot be asked to hide itself while one of its own windows is calling back, and that is
    /// exactly what happens when the user switches the input method from the mouth of the light.
    laser_hide: Rc<Cell<bool>>,
    /// The window an AI agent sends its requests to. It exists exactly while the text service of this process is
    /// active, so an agent that finds it finds a running input method.
    bridge: Option<Bridge>,
    /// The window that hears every change of the clipboard, so that the row 「剪贴板」 of the panel can offer what
    /// the user copied and not only what is on the clipboard now. It exists exactly while the text service of this
    /// process is active.
    clipboard_listener: Option<crate::clipboard_history::ClipboardListener>,
    /// How many times the demonstration of the laser interface has run (the seed of the places of its mouths).
    demo_runs: u32,
}

/// An action that arrived while the text service was busy.
enum Pending {
    /// The user clicked in the candidate window, or a host that draws the list itself selected something.
    List(crate::ui::ListAction),
    /// The user chose something in a menu of the floating bar.
    Menu(MenuAction),
    /// The settings window was saved: the settings are read again.
    ReloadSettings,
    /// The user used the laser panel (the glow or the beam).
    Laser(LaserEvent),
    /// The user pressed Alt+F2: walk to the next language of the product.
    SwitchLanguage,
    /// The input mode changed (Ctrl+F12 or Alt+F1): the panel has to be told.
    ModeChanged,
}

impl Inner {
    fn drain_actions(&mut self) {
        for _ in 0..4 {
            let actions: Vec<Pending> = self.pending_actions.borrow_mut().drain(..).collect();
            if actions.is_empty() {
                return;
            }
            for action in actions {
                match action {
                    Pending::List(action) => self.controller.apply_list_action(action),
                    Pending::Menu(action) => self.apply_menu_action(action),
                    Pending::ReloadSettings => self.reload_settings(),
                    Pending::Laser(event) => self.apply_laser_event(event),
                    Pending::ModeChanged => self.sync_laser(),
                    Pending::SwitchLanguage => self.switch_to_next_language(),
                }
            }
        }
    }
}

const PENDING_FOCUS: u8 = 1;
const PENDING_COMPOSITION_ENDED: u8 = 2;

/// What TSF asked for while the state of the text service was borrowed.
///
/// TSF re-enters us from inside our own callbacks: the switch of the input method happens in the callback of the
/// mouth, and TSF deactivates the text service inside that very call. The state is borrowed at that moment, so the
/// work cannot be done right away: it is remembered here, and done as soon as the callback returns.
#[derive(Default)]
struct Flags {
    /// Events that arrived while the state was borrowed (the focus and the end of a composition).
    pending: Cell<u8>,
    /// That the text service is being deactivated, and the state could not be taken yet.
    deactivated: Cell<bool>,
    /// That the floating windows have to go off the screen, and the state could not be reached yet.
    hide: Cell<bool>,
}

/// The four interfaces of the object that TSF is given to call back.
type Sinks = (
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfActiveLanguageProfileNotifySink,
);

/// The text input processor object: one per thread that activates the profile (TSF calls it on that thread's
/// apartment, so interior mutability without locking is enough).
#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfActiveLanguageProfileNotifySink
)]
struct Tip {
    inner: Rc<RefCell<Option<Inner>>>,
    /// What TSF asked for while the state was borrowed.
    flags: Rc<Flags>,
    /// Counts this object in the DLL's live-object counter while it exists.
    _live: crate::LiveGuard,
}

impl Tip {
    fn new() -> Tip {
        Tip {
            inner: Rc::new(RefCell::new(None)),
            flags: Rc::new(Flags::default()),
            _live: crate::LiveGuard::new(),
        }
    }

    /// Run `f` on the active state; `fallback` when not activated or when TSF re-entered us (then the event is
    /// remembered if it matters).
    fn with<R>(&self, fallback: R, f: impl FnOnce(&mut Inner) -> R) -> R {
        let result = {
            let Ok(mut slot) = self.inner.try_borrow_mut() else {
                return fallback;
            };
            let Some(inner) = slot.as_mut() else {
                return fallback;
            };
            let pending = self.flags.pending.replace(0);
            if pending & PENDING_FOCUS != 0 {
                inner.controller.host_mut().forget_context();
                inner.controller.focus_changed();
            }
            if pending & PENDING_COMPOSITION_ENDED != 0 {
                inner.controller.host_mut().composition = None;
                inner.controller.composition_terminated();
            }
            let result = f(inner);
            inner.drain_actions();
            result
        };
        // The state is free again: the work that had to wait for it can be done now.
        self.settle();
        result
    }

    /// Note an event for the next `with` when the state is busy right now.
    fn remember_if_busy(&self, flag: u8) {
        if self.inner.try_borrow_mut().is_err() {
            self.flags.pending.set(self.flags.pending.get() | flag);
        }
    }

    /// Do the work that had to wait for the state to be free.
    fn settle(&self) {
        settle(&self.inner, &self.flags);
    }

    /// Take every floating window of this thread off the screen.
    ///
    /// The windows belong to the input method: when another one takes over, they have to go, and they have to go
    /// at once. When the state is busy (TSF re-enters us from inside our own callbacks), the work waits for it.
    fn hide_floating(&self) {
        let Ok(mut slot) = self.inner.try_borrow_mut() else {
            self.flags.hide.set(true);
            return;
        };
        let Some(inner) = slot.as_mut() else {
            return;
        };
        inner.hide_floating();
    }
}

/// Do the work that had to wait for the state of the text service to be free.
///
/// TSF re-enters us from inside our own callbacks: the switch of the input method happens in the callback of the
/// mouth, and TSF deactivates the text service inside that very call. The state is borrowed at that moment, so the
/// work waits until the callback returns.
fn settle(inner: &RefCell<Option<Inner>>, flags: &Flags) {
    let Ok(mut slot) = inner.try_borrow_mut() else {
        return; // still busy: the next callback settles it
    };
    if flags.hide.get() {
        flags.hide.set(false);
        if let Some(state) = slot.as_mut() {
            state.hide_floating();
        }
    }
    if flags.deactivated.get() {
        flags.deactivated.set(false);
        let Some(mut state) = slot.take() else {
            return;
        };
        state.shutdown();
    }
}

impl Tip_Impl {
    /// The object's own interfaces, as TSF will hold them.
    fn sinks(&self) -> Sinks {
        let this = self.to_object();
        (
            this.to_interface(),
            this.to_interface(),
            this.to_interface(),
            this.to_interface(),
        )
    }
}

impl ITfTextInputProcessor_Impl for Tip_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || {
            self.activate(ptim, tid, 0, self.sinks())
        })
    }

    fn Deactivate(&self) -> Result<()> {
        guard(Ok(()), || self.deactivate())
    }
}

impl ITfTextInputProcessorEx_Impl for Tip_Impl {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || {
            self.activate(ptim, tid, dwflags, self.sinks())
        })
    }
}

impl ITfKeyEventSink_Impl for Tip_Impl {
    fn OnSetFocus(&self, fforeground: BOOL) -> Result<()> {
        guard(Ok(()), || {
            // the sink is advised as foreground: losing the foreground is losing the document
            if !fforeground.as_bool() {
                self.remember_if_busy(PENDING_FOCUS);
                self.with((), |inner| {
                    inner.controller.host_mut().forget_context();
                    inner.controller.focus_changed();
                    // The windows that belong to the document go off the screen with it.
                    inner.hide_caret();
                });
            }
            Ok(())
        })
    }

    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(Ok(BOOL(0)), || {
            Ok(self.key(pic, wparam, lparam, KeyKind::TestDown).into())
        })
    }

    fn OnTestKeyUp(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(Ok(BOOL(0)), || {
            Ok(self.key(pic, wparam, lparam, KeyKind::TestUp).into())
        })
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(BOOL(0)), || {
            Ok(self.key(pic, wparam, lparam, KeyKind::Down).into())
        })
    }

    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(BOOL(0)), || {
            Ok(self.key(pic, wparam, lparam, KeyKind::Up).into())
        })
    }

    /// No preserved key is registered (Ctrl+Space belongs to the editors, Shift/Win+Space/Ctrl+Shift to Windows; the
    /// pause is the Shift tap), so TSF has nothing to report here: always "not eaten".
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

impl ITfThreadMgrEventSink_Impl for Tip_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        // A new document manager is not our business: the context we type into arrives with the key events.
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        // The document manager is going away. When it owns the context we type into, the host will not send us any
        // more events for it: the composition has to end now and the context has to be forgotten, or we would keep
        // writing into a document that no longer exists.
        guard(Ok(()), || {
            let Some(pdim) = pdim else {
                return Ok(());
            };
            self.with((), |inner| {
                {
                    let host = inner.controller.host_mut();
                    if !host.is_in_document(pdim) {
                        return;
                    }
                    let _ = host.end_composition();
                    host.forget_context();
                }
                inner.controller.focus_changed();
            });
            Ok(())
        })
    }

    fn OnSetFocus(
        &self,
        _pdimfocus: Option<&ITfDocumentMgr>,
        _pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        guard(Ok(()), || {
            self.remember_if_busy(PENDING_FOCUS);
            self.with((), |inner| {
                inner.controller.host_mut().forget_context();
                inner.controller.focus_changed();
                // The windows that belong to the document go off the screen with it.
                inner.hide_caret();
            });
            Ok(())
        })
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        // A context was pushed onto the stack. It is not necessarily the one the user types into (the focused
        // context is), so there is nothing to do here.
        Ok(())
    }

    fn OnPopContext(&self, pic: Option<&ITfContext>) -> Result<()> {
        // The context was popped: the host will not send us any more events for it. When it is the context we type
        // into, the composition has to end now.
        guard(Ok(()), || {
            let Some(pic) = pic else {
                return Ok(());
            };
            self.with((), |inner| {
                {
                    let host = inner.controller.host_mut();
                    if !host.is_current_context(pic) {
                        return;
                    }
                    let _ = host.end_composition();
                    host.forget_context();
                }
                inner.controller.focus_changed();
            });
            Ok(())
        })
    }
}

impl ITfCompositionSink_Impl for Tip_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        guard(Ok(()), || {
            self.remember_if_busy(PENDING_COMPOSITION_ENDED);
            self.with((), |inner| {
                inner.controller.host_mut().composition = None;
                inner.controller.composition_terminated();
            });
            Ok(())
        })
    }
}

/// The text service hears about the active language profile, so that nothing of it stays on the screen when the
/// user switches to another input method or to another language.
impl ITfActiveLanguageProfileNotifySink_Impl for Tip_Impl {
    fn OnActivated(&self, clsid: *const GUID, guidprofile: *const GUID, factivated: BOOL) -> Result<()> {
        guard(Ok(()), || {
            // SAFETY: the two pointers are read-only GUIDs that TSF gave us for the length of the call.
            let clsid = unsafe { clsid.as_ref().copied().unwrap_or_default() };
            let profile = unsafe { guidprofile.as_ref().copied().unwrap_or_default() };
            self.profile_activated(clsid, profile, factivated.as_bool());
            Ok(())
        })
    }
}

/// Called by the class factory: create a TIP and return the requested interface.
///
/// # Safety
/// `iid` and `object` must be valid pointers (the class factory checks them for null).
pub(crate) unsafe fn create_instance(iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
    // SAFETY: valid per the contract. NULL after every failure, including a panic below that is turned into E_FAIL.
    unsafe { *object = std::ptr::null_mut() };
    guard(Err(E_FAIL.into()), || {
        let mut tip = Tip::new();
        // The object counts as live from here on. If the query below fails, the object is dropped right away and the
        // guard gives the count back.
        tip._live.count();
        let tip: ITfTextInputProcessorEx = tip.into();
        // SAFETY: forwarded from the caller's contract.
        unsafe { tip.query(iid, object).ok() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::TextServices::ITfTextInputProcessor;

    #[test]
    fn guard_passes_a_value_through_and_turns_a_panic_into_the_fallback() {        assert_eq!(guard(7, || 42), 42);
        assert!(!guard(false, || panic!("host callback panicked (test)")));
        let got: Result<BOOL> = guard(Ok(BOOL(0)), || -> Result<BOOL> { panic!("again (test)") });
        assert_eq!(
            got.unwrap(),
            BOOL(0),
            "a panic inside a callback means: not eaten"
        );
        // the guard is reusable after a panic
        assert_eq!(guard(1, || 2), 2);
    }

    /// The glue must answer "not eaten" before activation and after deactivation: these calls touch no TSF object.
    #[test]
    fn an_inactive_text_service_eats_nothing_and_survives_every_callback() {
        let tip: ITfTextInputProcessorEx = Tip::new().into();
        let sink: ITfKeyEventSink = tip.cast().unwrap();
        let thread_sink: ITfThreadMgrEventSink = tip.cast().unwrap();
        // SAFETY: the object is a live in-process COM object; no TSF state is touched (never activated).
        unsafe {
            for code in [0x41usize, 0x09, 0x1B, 0x31] {
                assert_eq!(
                    sink.OnTestKeyDown(None, WPARAM(code), LPARAM(1)).unwrap(),
                    BOOL(0)
                );
                assert_eq!(
                    sink.OnKeyDown(None, WPARAM(code), LPARAM(1)).unwrap(),
                    BOOL(0)
                );
                assert_eq!(
                    sink.OnTestKeyUp(None, WPARAM(code), LPARAM(1)).unwrap(),
                    BOOL(0)
                );
                assert_eq!(
                    sink.OnKeyUp(None, WPARAM(code), LPARAM(1)).unwrap(),
                    BOOL(0)
                );
            }
            assert_eq!(
                sink.OnPreservedKey(None, &GUID::from_u128(1)).unwrap(),
                BOOL(0)
            );
            assert!(sink.OnSetFocus(BOOL(1)).is_ok());
            assert!(sink.OnSetFocus(BOOL(0)).is_ok());
            assert!(thread_sink.OnSetFocus(None, None).is_ok());
            // the document and context callbacks answer even without a document manager or a context
            assert!(thread_sink.OnInitDocumentMgr(None).is_ok());
            assert!(thread_sink.OnUninitDocumentMgr(None).is_ok());
            assert!(thread_sink.OnPushContext(None).is_ok());
            assert!(thread_sink.OnPopContext(None).is_ok());
            assert!(tip.Deactivate().is_ok());
            assert!(
                tip.Activate(None, 1).is_err(),
                "a missing thread manager is refused, not dereferenced"
            );
            assert!(tip.ActivateEx(None, 1, 0).is_err());
        }
    }

    #[test]
    fn the_class_factory_helper_answers_every_interface_tsf_may_ask_for() {
        // TSF activates through ITfTextInputProcessor or ITfTextInputProcessorEx; COM identity goes through IUnknown
        for iid in [
            ITfTextInputProcessorEx::IID,
            ITfTextInputProcessor::IID,
            IUnknown::IID,
            ITfKeyEventSink::IID,
        ] {
            let mut out: *mut c_void = std::ptr::null_mut();
            // SAFETY: valid pointers to live locals; the reference handed back is released at once.
            unsafe {
                assert!(create_instance(&iid, &mut out).is_ok(), "{iid:?}");
                assert!(!out.is_null(), "{iid:?}");
                drop(IUnknown::from_raw(out));
            }
        }
        // SAFETY: as above; an interface the object does not implement is an error, not a crash, and the out pointer
        // is NULL afterwards even when it held something before (a sentinel that is never dereferenced).
        unsafe {
            let mut out = std::ptr::NonNull::<c_void>::dangling().as_ptr();
            assert!(create_instance(&GUID::from_u128(7), &mut out).is_err());
            assert!(out.is_null());
        }
    }

    /// A model of the document and of the ranges the text service holds, so that the plan of a commit can be
    /// tested without Windows.
    ///
    /// The spans are the positions of the typed text in the text: the range the text service owns and the range of
    /// the open composition. The model does with the plan what the COM code of `TsfHost::commit` does with it.
    struct Model {
        text: String,
        /// The range the text service wrote the typed text into.
        owned: Option<(usize, usize)>,
        /// The range of the open composition.
        composition: Option<(usize, usize)>,
        /// Whether the document can be read back (a host that syncs its text to the text service).
        readable: bool,
    }

    impl Model {
        fn new(text: &str) -> Model {
            Model {
                text: text.to_string(),
                owned: None,
                composition: None,
                readable: false,
            }
        }

        /// The span of `typed` at the end of the text.
        fn span_of(text: &str, typed: &str) -> (usize, usize) {
            (text.len() - typed.len(), text.len())
        }

        /// Do what the plan of the commit says. `Err` means: nothing was written.
        fn commit(&mut self, typed: &str, candidate: &str) -> std::result::Result<String, ()> {
            match plan_commit(
                self.owned.is_some(),
                self.composition.is_some(),
                self.readable,
            ) {
                CommitPlan::Owned => {
                    let (start, end) = self.owned.expect("the plan says the range is there");
                    self.replace(start, end, candidate);
                    Ok(self.text.clone())
                }
                CommitPlan::Composition => {
                    let (start, end) = self
                        .composition
                        .expect("the plan says the composition is there");
                    self.replace(start, end, candidate);
                    Ok(self.text.clone())
                }
                CommitPlan::Document => {
                    // the typed text is read back and verified against what was typed
                    if !self.readable || !self.text.ends_with(typed) {
                        return Err(()); // the typed text cannot be located
                    }
                    let (start, end) = Model::span_of(&self.text, typed);
                    self.replace(start, end, candidate);
                    Ok(self.text.clone())
                }
                CommitPlan::Fail => Err(()),
            }
        }

        fn replace(&mut self, start: usize, end: usize, text: &str) {
            self.text.replace_range(start..end, text);
        }
    }

    /// The decision of a commit: the range of the text service first, then the composition, then the document.
    ///
    /// There is no plan that writes the candidate at the caret: the pinyin and the candidate can never end up side
    /// by side because of a commit.
    #[test]
    fn the_plan_of_a_commit_has_no_way_to_write_the_candidate_at_the_caret() {
        assert_eq!(plan_commit(true, true, true), CommitPlan::Owned);
        assert_eq!(plan_commit(false, true, true), CommitPlan::Composition);
        assert_eq!(plan_commit(false, false, true), CommitPlan::Document);
        assert_eq!(plan_commit(false, false, false), CommitPlan::Fail);
    }

    /// The pinyin is replaced by the candidate: the document never holds the pinyin and the candidate together.
    #[test]
    fn a_commit_replaces_the_pinyin_instead_of_writing_the_candidate_next_to_it() {
        // The text service wrote the pinyin into the range it owns (the composition holds it).
        let mut model = Model::new("wo");
        model.owned = Some((0, 2));
        model.composition = Some((0, 2));
        let written = model.commit("wo", "我").expect("the commit writes the candidate");
        assert_eq!(written, "我", "the candidate is alone in the document");
        assert!(!model.text.contains("wo"), "the pinyin is gone");
        assert!(
            !model.text.contains("wo我"),
            "the pinyin is never left next to the candidate"
        );
    }

    /// Without a range of its own, the text service replaces the typed text in the range of the composition: the
    /// position of the pinyin is known without reading the document.
    #[test]
    fn a_commit_without_an_owned_range_replaces_the_typed_text_in_the_composition() {
        let mut model = Model::new("wo");
        model.composition = Some((0, 2));
        let written = model.commit("wo", "我").expect("the commit writes the candidate");
        assert_eq!(written, "我");
        assert!(
            !model.text.contains("wo我"),
            "the pinyin is never left next to the candidate"
        );
    }

    /// The typed text cannot be located: nothing is written. The candidate is never written at the caret.
    #[test]
    fn a_commit_that_cannot_locate_the_typed_text_writes_nothing() {
        let mut model = Model::new("wo");
        // no range of our own, no composition, and the document cannot be read back
        assert_eq!(plan_commit(false, false, false), CommitPlan::Fail);
        assert!(
            model.commit("wo", "我").is_err(),
            "the commit fails instead of writing the candidate"
        );
        assert_eq!(
            model.text, "wo",
            "the document is untouched: no candidate next to the pinyin"
        );
    }

    /// The code input: the text service opens no composition, the letters belong to the host (they are in the
    /// document as they were typed) and the machine never asks for a commit - the code goes into the editor as it
    /// was typed.
    #[test]
    fn the_code_input_keeps_its_code_and_writes_no_candidate() {
        let mut model = Model::new("fn main()");
        assert!(model.composition.is_none(), "the code input opens no composition");
        assert!(model.owned.is_none(), "the code input writes no range of its own");
        // The candidate preview of the code input, when the host lets the text service read the document: the
        // code is replaced by the candidate, and it is never left next to it.
        model.readable = true;
        let written = model.commit("fn main()", "函数").expect("the code is replaced");
        assert_eq!(written, "函数");
        // The same, with a host that does not let the text service read the document: the commit fails, the code
        // stays as it was typed, and the candidate is not written next to it.
        let mut model = Model::new("fn main()");
        assert!(
            model.commit("fn main()", "函数").is_err(),
            "the commit fails instead of writing the candidate next to the code"
        );
        assert_eq!(model.text, "fn main()", "the code is as it was typed");
    }
}
