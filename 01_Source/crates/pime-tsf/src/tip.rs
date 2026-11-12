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

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::{BOOL, E_FAIL, E_INVALIDARG, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, GetKeyboardLayout, ToUnicodeEx};
use windows::Win32::UI::TextServices::{
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_INPUTSCOPE, IS_PASSWORD,
    ITfActiveLanguageProfileNotifySink, ITfActiveLanguageProfileNotifySink_Impl, ITfCompartmentMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputScope, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfSource, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, InputScope, TF_AE_NONE, TF_ANCHOR_END, TF_DEFAULT_SELECTION,
    TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
    TF_TMAE_SECUREMODE,
};
use windows::core::{GUID, IUnknown, IUnknownImpl, Interface, Result, implement};

use pime_core::agent_proto::{Error as BridgeError, Op as BridgeOp, Reply as BridgeReply, Request as BridgeRequest};

use crate::agent_bridge::Bridge;
use crate::controller::{HostError, HostText, KeyInput, TipConfig, TipController};
use crate::skin::skin_colors;
use crate::imesettings::{self, ImeSettings, LanguageSettings};
use crate::keymap::{Modifiers, vk};
use crate::laser::PanelItem;
use crate::laser_candidates::LaserCandidates;
use crate::laser_ui::{LaserEvent, LaserUi};
use crate::layout::Rect;
use crate::menus::{MenuAction, MenuItem, MenuSink, MenuWindow};
use crate::popup::work_area_of;
use crate::register::debug_line;
use crate::settings_window::{SettingsEvent, SettingsSink, SettingsWindow};
use crate::ui::{Anchor, CandidateUi, CandidateView, ListAction};
use crate::uielement_com::candidate_ui;

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

/// `ActivateEx` flag: TSF runs on a secure desktop (logon, UAC): collect nothing there.
fn is_secure_mode(flags: u32) -> bool {
    flags & TF_TMAE_SECUREMODE != 0
}

/// The language whose settings are used when the active profile cannot be told apart.
const DEFAULT_LANGUAGE: &str = "zh-CN";

/// The largest selection that is read for an AI agent (in UTF-16 units). A selection of a megabyte is not something to
/// hand to an agent: it is far more likely a mistake than an intention.
const MAX_SELECTION_CHARS: usize = 1 << 20;

/// How much of the text before the caret is read for the demonstration (in UTF-16 units).
///
/// The demonstration asks whether the document holds a domain header of its own: only the piece it writes
/// into matters, and a document may be long.
const BEFORE_CARET_CHARS: usize = 4096;

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
        let mut moved = 0i32;
        range
            .ShiftStart(ec, wanted, &mut moved, std::ptr::null())
            .map_err(refused)?;
        if moved != wanted {
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
}

impl TsfHost {
    fn new(client_id: u32, sink: ITfCompositionSink) -> TsfHost {
        TsfHost {
            client_id,
            sink,
            context: None,
            composition: None,
            stranded: None,
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

    fn begin_composition(&mut self, typed: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        if self.composition.is_some() {
            return Ok(());
        }
        let (ctx2, sink, typed) = (ctx.clone(), self.sink.clone(), typed.to_string());
        let composition = run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            let range = unsafe { typed_range(&ctx2, ec, &typed) }?;
            let owner: ITfContextComposition = ctx2.cast().map_err(refused)?;
            // SAFETY: live COM objects and a valid edit cookie.
            unsafe { owner.StartComposition(ec, &range, &sink) }.map_err(refused)
        })?;
        self.composition = Some((ctx, composition));
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

    fn commit(&mut self, typed: &str, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        let (ctx2, typed, wide) = (
            ctx.clone(),
            typed.to_string(),
            text.encode_utf16().collect::<Vec<u16>>(),
        );
        run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
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

/// What an AI agent asks the input method to do, answered on the thread of the host.
///
/// The agent does not touch the document itself: it sends a request to the bridge window of this process, and the
/// window procedure runs here, on the UI thread of the application that has the focus, where the text service and the
/// TSF context live.
impl TsfHost {
    /// The text that is selected in the document (empty when nothing is selected).
    pub fn selection_text(&mut self) -> Option<String> {
        let ctx = self.context.clone()?;
        let ctx2 = ctx.clone();
        run_session(&ctx, self.client_id, SYNC_READ, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = caret_range(&ctx2, ec).map_err(refused)?;
                if range.IsEmpty(ec).map_err(refused)?.as_bool() {
                    return Ok(String::new());
                }
                // The interface does not say how long the text is: the buffer grows until everything fits.
                let mut buffer = vec![0u16; 1024];
                let got;
                loop {
                    let mut copied = 0u32;
                    range.GetText(ec, 0, &mut buffer, &mut copied).map_err(refused)?;
                    let count = (copied as usize).min(buffer.len());
                    if count < buffer.len() || buffer.len() >= MAX_SELECTION_CHARS {
                        got = count;
                        break;
                    }
                    buffer.resize(buffer.len() * 2, 0);
                }
                if got >= MAX_SELECTION_CHARS {
                    return Err(HostError::Refused); // a selection of a megabyte is not something to hand to an agent
                }
                buffer.truncate(got);
                Ok(String::from_utf16_lossy(&buffer))
            }
        })
        .ok()
    }

    /// Replace what is selected with the text. An empty selection means "insert at the caret".
    pub fn replace_selection(&mut self, text: &str) -> std::result::Result<(), HostError> {
        let ctx = self.context.clone().ok_or(HostError::Refused)?;
        let (ctx2, wide) = (ctx.clone(), text.encode_utf16().collect::<Vec<u16>>());
        run_session(&ctx, self.client_id, SYNC_WRITE, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let range = caret_range(&ctx2, ec).map_err(refused)?;
                range.SetText(ec, 0, &wide).map_err(refused)?;
                range.Collapse(ec, TF_ANCHOR_END).map_err(refused)?;
                let selection = selection_of(range);
                let set = ctx2.SetSelection(ec, std::slice::from_ref(&selection));
                drop(ManuallyDrop::into_inner(selection.range));
                set.map_err(refused)
            }
        })
    }

    /// The text that is already in the document, just before the caret.
    ///
    /// The demonstration asks it whether the document holds a domain header of its own: only the last window
    /// before the caret is read, because the question is about the piece the demonstration writes into, and
    /// a document may be long.
    pub fn text_before_caret(&mut self) -> Option<String> {
        let ctx = self.context.clone()?;
        let ctx2 = ctx.clone();
        run_session(&ctx, self.client_id, SYNC_READ, move |ec| {
            // SAFETY: `ec` belongs to the running session on `ctx2`.
            unsafe {
                let caret = caret_range(&ctx2, ec).map_err(refused)?;
                let range = caret.Clone().map_err(refused)?;
                // the window before the caret: the shift stops at the beginning of the document
                let wanted = -(BEFORE_CARET_CHARS as i32);
                let mut moved = 0i32;
                range
                    .ShiftStart(ec, wanted, &mut moved, std::ptr::null())
                    .map_err(refused)?;
                let count = moved.unsigned_abs() as usize;
                if count == 0 {
                    return Ok(String::new());
                }
                let mut buffer = vec![0u16; count];
                let mut got = 0u32;
                range.GetText(ec, 0, &mut buffer, &mut got).map_err(refused)?;
                let got = (got as usize).min(count);
                buffer.truncate(got);
                Ok(String::from_utf16_lossy(&buffer))
            }
        })
        .ok()
    }
}

/// Answer a request of an AI agent that came through the bridge window.
///
/// The rules are the ones of the product: nothing is ever written into a password field, and a write that the host
/// refuses is reported as refused instead of being retried behind the user's back.
fn answer_agent(inner: &mut Inner, request: BridgeRequest) -> BridgeReply {
    let mut reply = answer_the_request(inner, &request);
    // The answer says who the request was from: the agent learns that its own mouth is the one that wrote.
    reply.source = request.source.clone();
    reply
}

/// The work of the bridge: what the request of the agent asks for.
fn answer_the_request(inner: &mut Inner, request: &BridgeRequest) -> BridgeReply {
    match request.op {
        BridgeOp::Insert => {
            if request.text.is_empty() {
                return BridgeReply::failed(BridgeError::Malformed, "there is no text to write");
            }
            if let Err(error) = crate::agent_bridge::may_write(inner.controller.host_mut().is_secure_field()) {
                return BridgeReply::failed(
                    error,
                    "the field is a password field: nothing is written into it",
                );
            }
            let text = request.text.clone();
            // Where the text lands: the caret before the write is the place the character appears at.
            let anchor = inner.controller.host_mut().caret_anchor();
            let written = if request.replace {
                inner.controller.host_mut().replace_selection(&text)
            } else {
                inner.controller.host_mut().commit("", &text)
            };
            match written {
                Ok(()) => {
                    // The light of the agent: the beam comes out of the mouth of that agent and has its colour, so
                    // that the user sees at once which agent wrote the character.
                    if let Some(anchor) = anchor
                        && let Ok(mut laser) = inner.laser.try_borrow_mut()
                    {
                        laser.fire_for(
                            &request.source.key(),
                            request.source.display_name(),
                            anchor,
                        );
                    }
                    let mut reply = BridgeReply::ok();
                    reply.inserted = text.chars().count();
                    reply.language = Some(inner.language.clone());
                    reply
                }
                Err(HostError::Mismatch) => BridgeReply::failed(
                    BridgeError::Refused,
                    "there is a selection: the text would have replaced it",
                ),
                Err(HostError::Refused) => {
                    BridgeReply::failed(BridgeError::Refused, "the host refused the write")
                }
            }
        }
        BridgeOp::Read => match inner.controller.host_mut().selection_text() {
            Some(text) => {
                let mut reply = BridgeReply::ok();
                reply.text = text;
                reply
            }
            None => BridgeReply::failed(BridgeError::NoContext, "there is no document to read"),
        },
        BridgeOp::Language => {
            let Some(language) = request.language.clone() else {
                return BridgeReply::failed(BridgeError::Malformed, "no language was named");
            };
            if !matches!(language.as_str(), "zh-CN" | "en-US" | "ja-JP") {
                return BridgeReply::failed(
                    BridgeError::Malformed,
                    "the language is not one of the three of the product",
                );
            }
            inner.switch_language(&language);
            let mut reply = BridgeReply::ok();
            reply.language = Some(inner.language.clone());
            reply
        }
        BridgeOp::Status => {
            let mut reply = BridgeReply::ok();
            reply.language = Some(inner.language.clone());
            reply.message = if inner.controller.input_mode() {
                "the input mode is on".to_string()
            } else {
                "the direct mode is on".to_string()
            };
            reply
        }
        // The two operations of the word bank need the storage of the word bank, which is not written yet.
        BridgeOp::Misses => BridgeReply::failed(
            BridgeError::Unsupported,
            "the words the user did not find are not collected in this build",
        ),
        BridgeOp::Curate => BridgeReply::failed(
            BridgeError::Unsupported,
            "the word bank of the user is not written in this build",
        ),
    }
}

/// The date and the time of the machine, as they are written into the document.
///
/// The local time of Windows, not the clock of the machine in UTC: what the user reads is what is written.
fn now_text() -> String {
    // SAFETY: the call fills the structure it returns; nothing is passed in.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}

/// The menus of the product: the ones the panel of the laser interface opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuKind {
    /// The language of the input method.
    Language,
    /// The effort of the AI features.
    Effort,
    /// The skin of the product.
    Skin,
    /// The scheme of the keys (the programmer habit, Microsoft, Sogou, ATOK).
    Keymap,
    /// The symbol table: the symbols of the core, written into the document by a click.
    Symbols,
    /// The text that is on the clipboard.
    Clipboard,
    /// The date and the time of the machine.
    DateTime,
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
    /// How many times the demonstration of the laser interface has run (the seed of the places of its mouths).
    demo_runs: u32,
}

/// The document the demonstration writes into: the text service of this thread.
///
/// It is the only place that talks to the host: the demonstration itself is pure (in `demo`), so that it can
/// be tested without a document and without a screen.
struct DemoHost<'a> {
    /// The text service.
    inner: &'a mut Inner,
}

impl crate::demo::DemoHost for DemoHost<'_> {
    /// A password field is never written into: not one character.
    fn is_secure_field(&mut self) -> bool {
        self.inner.controller.host_mut().is_secure_field()
    }

    /// The text that is already in the document, just before the caret.
    fn text_at_caret(&mut self) -> Option<String> {
        self.inner.controller.host_mut().text_before_caret()
    }

    /// The caret in screen coordinates: the beams fly to it.
    fn caret_anchor(&mut self) -> Option<Anchor> {
        self.inner.controller.host_mut().caret_anchor()
    }

    /// Write the text at the caret and leave the caret after it.
    fn write(&mut self, text: &str) -> std::result::Result<(), crate::demo::DemoError> {
        match self.inner.controller.host_mut().commit("", text) {
            Ok(()) => Ok(()),
            Err(HostError::Mismatch) => Err(crate::demo::DemoError::Refused(
                "there is a selection: the text would have replaced it".to_string(),
            )),
            Err(HostError::Refused) => Err(crate::demo::DemoError::Refused(
                "the host refused the write".to_string(),
            )),
        }
    }

    /// The light of the agent: the beam comes out of its mouth and has its colour.
    fn fire(&mut self, key: &str, label: &str, anchor: Anchor, mouth: (i32, i32)) {
        if let Ok(mut laser) = self.inner.laser.try_borrow_mut() {
            laser.fire_for_at(key, label, anchor, mouth.0, mouth.1);
        }
    }
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

/// The rows of the laser panel: the settings that are used most often, in the order they are drawn.
///
/// The panel is the quick way into the settings (the Sogou way): the switches that are turned on and off while
/// typing, the scheme of the keys, the language, the effort of the AI and the way the input method looks. The last
/// row opens the settings window itself.
///
/// The function is free of the state of the text service, so that "which rows are there" can be tested without a
/// host: the window of the panel is sized for [`crate::laser_ui::PANEL_ROWS`] rows.
fn laser_items(
    language: &str,
    settings: &LanguageSettings,
    effort: &str,
    skin: &str,
    input_mode: bool,
) -> Vec<PanelItem> {
    vec![
        PanelItem::Toggle {
            key: "input_mode",
            label: "输入模式",
            on: input_mode,
        },
        PanelItem::Toggle {
            key: "space_confirms",
            label: "空格上屏",
            on: settings.space_confirms,
        },
        PanelItem::Toggle {
            key: "auto_compose",
            label: "自动开组合",
            on: settings.auto_compose,
        },
        PanelItem::Menu {
            key: "keymap",
            label: "按键方案",
            value: settings.keymap_preset.label().to_string(),
        },
        PanelItem::Menu {
            key: "language",
            label: "语言",
            value: imesettings::display_name(language).to_string(),
        },
        PanelItem::Menu {
            key: "effort",
            label: "AI 力度",
            value: imesettings::effort_label(effort).to_string(),
        },
        PanelItem::Menu {
            key: "skin",
            label: "外观",
            value: imesettings::skin_label(skin).to_string(),
        },
        // the three local tools of the Sogou-style bar: the symbol table, the date and the time, the clipboard
        PanelItem::Menu {
            key: "symbols",
            label: "符号表",
            value: String::new(),
        },
        PanelItem::Menu {
            key: "datetime",
            label: "日期时间",
            value: String::new(),
        },
        PanelItem::Menu {
            key: "clipboard",
            label: "剪贴板",
            value: String::new(),
        },
        PanelItem::Demo,
        PanelItem::Settings,
    ]
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

    /// Open one of the menus under the rectangle the user clicked (a row of the panel).
    fn open_menu_at(&mut self, which: MenuKind, rect: Rect) {
        // Only one menu at a time: the old one goes away.
        self.menu = None;
        let items = self.menu_items(which);
        let work = work_area_of(rect);
        let sink = self.menu_sink.clone();
        self.menu = MenuWindow::open(
            items,
            which == MenuKind::Language,
            rect,
            work,
            skin_colors(&self.skin),
            sink,
        );
    }

    /// The items of one menu.
    fn menu_items(&self, which: MenuKind) -> Vec<MenuItem> {
        match which {
            MenuKind::Effort => imesettings::EFFORT_LEVELS
                .iter()
                .map(|level| {
                    MenuItem::checked(
                        imesettings::effort_label(level),
                        MenuAction::SetEffort((*level).to_string()),
                        self.effort == *level,
                    )
                })
                .collect(),
            MenuKind::Skin => imesettings::SKINS
                .iter()
                .map(|skin| {
                    MenuItem::checked(
                        imesettings::skin_label(skin),
                        MenuAction::SetSkin((*skin).to_string()),
                        self.skin == *skin,
                    )
                })
                .collect(),
            MenuKind::Language => imesettings::LANGUAGES
                .iter()
                .map(|language| {
                    MenuItem::new(
                        imesettings::display_name(language),
                        MenuAction::SwitchLanguage((*language).to_string()),
                    )
                })
                .collect(),
            MenuKind::Symbols => pime_core::symbol_table::symbol_pairs()
                .into_iter()
                .map(|(text, name)| {
                    MenuItem::new(
                        &format!("{text}  {name}"),
                        MenuAction::InsertText(text.to_string()),
                    )
                })
                .collect(),
            MenuKind::Clipboard => crate::clipboard::entries()
                .into_iter()
                .map(|text| {
                    let label: String = text.chars().take(40).collect();
                    MenuItem::new(&label, MenuAction::InsertText(text))
                })
                .collect(),
            MenuKind::DateTime => {
                let now = now_text();
                vec![MenuItem::new(&now, MenuAction::InsertText(now.clone()))]
            }
            MenuKind::Keymap => imesettings::KeymapPreset::ALL
                .iter()
                .map(|preset| {
                    MenuItem::checked(
                        preset.label(),
                        MenuAction::SetKeymapPreset(*preset),
                        self.settings.keymap_preset == *preset,
                    )
                })
                .collect(),
        }
    }

    /// What the user chose in a menu.
    fn apply_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::ToggleInputMode => {
                let on = !self.controller.input_mode();
                self.controller.set_input_mode(on);
                self.sync_laser();
            }
            MenuAction::SetEffort(effort) => {
                self.effort = effort;
                self.save_settings();
            }
            MenuAction::SetSkin(skin) => {
                self.skin = skin;
                self.save_settings();
                self.sync_laser();
            }
            MenuAction::SetOption(name, on) => {
                self.set_option(&name, on);
            }
            MenuAction::SwitchLanguage(language) => {
                self.switch_language(&language);
            }
            MenuAction::InsertText(text) => {
                // The text goes into the document at the caret, exactly as the agent's write does: the same path,
                // the same safety rules (a password field takes nothing).
                self.insert_text(&text);
            }
            MenuAction::SetKeymapPreset(preset) => {
                // The scheme changes the keys at once: that is the point of the schemes.
                let mut value = self.settings;
                if value.apply_preset(preset) {
                    self.settings = value;
                    self.controller.apply_settings(&value);
                    self.save_settings();
                    self.sync_laser();
                }
            }
            MenuAction::OpenSettings => self.open_settings(),
        }
    }

    /// Switch one of the settings of the active language on or off.
    ///
    /// The menus and the laser panel all go through here, so that they can never disagree about what the setting is.
    fn set_option(&mut self, name: &str, on: bool) {
        let mut value = self.settings;
        match name {
            "tab_cycles" => value.tab_cycles = on,
            "tab_to_host" => value.tab_to_host = on,
            "space_confirms" => value.space_confirms = on,
            "auto_compose" => value.auto_compose = on,
            "punct_paging" => value.punct_paging = on,
            "laser_ui" => value.laser_ui = on,
            "input_mode" => {
                // The input mode is a state of the controller, not a file: it is the same thing Alt+F1 does.
                self.controller.set_input_mode(on);
                self.settings = value;
                self.sync_laser();
                return;
            }
            _ => return,
        }
        // A key that the user turns by hand is no longer the one of the scheme: the scheme becomes "custom".
        if !matches!(name, "laser_ui") {
            value.keymap_preset = imesettings::KeymapPreset::Custom;
        }
        self.settings = value;
        self.controller.apply_settings(&value);
        self.save_settings();
        self.sync_laser();
    }

    /// What the user asked for in the laser panel.
    fn apply_laser_event(&mut self, event: LaserEvent) {
        match event {
            LaserEvent::Toggle { key, on } => self.set_option(key, on),
            LaserEvent::Menu { key } => self.open_laser_menu(key),
            LaserEvent::Candidate { index } => {
                // A candidate of the list was clicked in the panel: it is taken as if the user had pressed
                // the key of it.
                self.controller.apply_list_action(ListAction::Take(index));
            }
            LaserEvent::Settings => self.open_settings(),
            LaserEvent::Demo => self.run_demo(),
            LaserEvent::Switch => {
                // The mouth of the light is the switch between the input methods.
                //
                // The windows go off the screen before the switch: TSF deactivates the text service inside that very
                // call, and the state is borrowed at that moment, so nothing could be hidden after the switch.
                self.hide_floating();
                crate::ime_switch::switch_now();
            }
            LaserEvent::MouthMoved { edge, offset } => {
                // The mouth was dragged to another place: it is remembered, so it is still there next time.
                self.mouth_place = crate::laser::MouthPlace { edge, offset }.to_text();
                self.save_settings();
            }
        }
    }

    /// Run the demonstration of the laser interface: ten agents write their own domain, one after the other.
    ///
    /// The demonstration is a one-off: it runs when the user clicks its row of the panel, and it never
    /// changes the way the ordinary typing works. A password field gets nothing at all.
    fn run_demo(&mut self) {
        // The seed is the number of the run: the same seed gives the same picture of the mouths, and the
        // clock of the machine is never read (the rules of the repository).
        self.demo_runs += 1;
        let seed = u64::from(self.demo_runs);
        let work = self
            .laser
            .try_borrow()
            .map(|laser| laser.work_area())
            .unwrap_or(Rect::new(0, 0, 0, 0));
        let report = {
            let mut host = DemoHost { inner: self };
            crate::demo::run(&mut host, seed, work)
        };
        match &report.error {
            Some(error) => trace(&format!("the demonstration stopped: {error}")),
            None => trace(&format!(
                "the demonstration wrote {} agents",
                report.agents.len()
            )),
        }
        // The panel says what happened: the user must not be left with the idea that something was written
        // when nothing was.
        let status = report.status();
        if let Ok(mut laser) = self.laser.try_borrow_mut() {
            laser.set_status(Some(&status));
            laser.open_panel();
        }
    }

    /// Write a text into the document at the caret: a symbol of the table, the text of the clipboard, the date and
    /// the time.
    ///
    /// It is the same path as the write of an agent, with the same safety rules: a password field takes nothing,
    /// and the panel says why.
    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if crate::agent_bridge::may_write(self.controller.host_mut().is_secure_field()).is_err() {
            if let Ok(mut laser) = self.laser.try_borrow_mut() {
                laser.set_status(Some("密码框：一个字符也没有写入"));
            }
            return;
        }
        // Where the text lands: the caret before the write is the place the character appears at.
        let anchor = self.controller.host_mut().caret_anchor();
        if self.controller.host_mut().commit("", text).is_ok()
            && let Some(anchor) = anchor
            && let Ok(mut laser) = self.laser.try_borrow_mut()
        {
            laser.fire_for(crate::mouths::USER_KEY, crate::mouths::USER_LABEL, anchor);
        }
    }

    /// Open one of the lists of the laser panel under the row the user clicked.
    fn open_laser_menu(&mut self, key: &str) {        let which = match key {
            "language" => MenuKind::Language,
            "effort" => MenuKind::Effort,
            "skin" => MenuKind::Skin,
            "keymap" => MenuKind::Keymap,
            "symbols" => MenuKind::Symbols,
            "clipboard" => MenuKind::Clipboard,
            "datetime" => MenuKind::DateTime,
            _ => return,
        };
        let anchor = self
            .laser
            .try_borrow()
            .ok()
            .and_then(|laser| laser.row_rect(key));
        let Some(anchor) = anchor else {
            return;
        };
        self.open_menu_at(which, anchor);
    }

    /// Switch to the next language of the product (Alt+F2).
    ///
    /// The three languages of the product (zh-CN, en-US and ja-JP) are walked through in that order; the decision
    /// itself is pure and lives in the core.
    fn switch_to_next_language(&mut self) {
        let next = pime_core::imeswitch::next_language(&self.language);
        self.switch_language(next);
    }

    /// Switch the input language: TSF activates the profile of the language, and the settings of that language apply
    /// from then on.
    fn switch_language(&mut self, language: &str) {
        let Some((langid, profile)) = profile_of(language) else {
            return;
        };
        // The windows that belong to the document go off the screen before the switch: TSF changes the profile
        // inside that very call, and the state is borrowed at that moment.
        self.hide_caret();
        if !activate_profile(langid, &profile) {
            trace(&format!("the profile of {language} could not be activated"));
            return;
        }
        // The profile of the thread changes; the settings of the new language apply right away (the text service is
        // activated again by TSF, but the bar must show the new language at once).
        self.language = language.to_string();
        let settings = load_settings();
        let value = settings.get(language);
        self.settings = value;
        self.controller.apply_settings(&value);
        self.sync_laser();
    }

    /// Take the settings of another language (the profile of that language became the active one).
    fn set_language(&mut self, language: &str) {
        if self.language == language {
            return;
        }
        self.language = language.to_string();
        let settings = load_settings();
        let value = settings.get(language);
        self.settings = value;
        self.controller.apply_settings(&value);
        // The engine follows the language: the dictionaries of the core take over.
        self.controller
            .set_profile(pime_core::imeswitch::profile_of(language));
    }

    /// Write the settings back to the file of the user.
    fn save_settings(&mut self) {
        let Some(path) = imesettings::settings_path() else {
            return;
        };
        let mut settings = ImeSettings::load(&path);
        settings.effort = self.effort.clone();
        settings.skin = self.skin.clone();
        settings.mouth_place = self.mouth_place.clone();
        settings.set(&self.language, self.settings);
        if let Err(e) = settings.save(&path) {
            trace(&format!("the settings could not be saved: {e}"));
        }
    }

    /// The rows of the laser panel: the settings that are used most often, in the order they are drawn.
    ///
    /// The panel is the quick way into the settings (the Sogou way): the switches that are turned on and off while
    /// typing, the language, the effort of the AI and the way the input method looks.
    fn laser_items(&self) -> Vec<PanelItem> {
        laser_items(
            &self.language,
            &self.settings,
            &self.effort,
            &self.skin,
            self.controller.input_mode(),
        )
    }

    /// Put the laser interface on the screen and give it the rows it shows.
    fn sync_laser(&mut self) {
        let items = self.laser_items();
        let Ok(mut laser) = self.laser.try_borrow_mut() else {
            return;
        };
        laser.set_items(items);
        laser.set_place(
            crate::laser::MouthPlace::from_text(&self.mouth_place),
        );
        // The interface is put on the screen every time: the switch of the input method can hide it while the state
        // says nothing about it (the hide happened in a callback of one of its own windows).
        let work = work_area_of(Rect::new(0, 0, 0, 0));
        laser.show(work);
    }

    /// Open the settings window (once; a second click does nothing while it is open).
    fn open_settings(&mut self) {
        if self.settings_window.is_some() {
            return;
        }
        let settings = load_settings();
        let sink = self.settings_sink.clone();
        self.settings_window = SettingsWindow::open(settings, sink);
    }

    /// Read the settings file again and apply what it says.
    fn reload_settings(&mut self) {
        let settings = load_settings();
        apply_plugins(&settings.plugins);
        let language = self.language.clone();
        let value = settings.get(&language);
        self.settings = value;
        self.effort = settings.effort.clone();
        self.skin = settings.skin.clone();
        self.mouth_place = settings.mouth_place.clone();
        self.controller.apply_settings(&value);
        // The vertical writing mode is a global setting: the file says whether the text stands as a column.
        // Ctrl+Alt+V switches it for the session only; saving the settings makes the file the truth again.
        self.controller.set_vertical_mode(settings.vertical);
        self.sync_laser();
    }

    /// Take every floating window of this thread off the screen.
    ///
    /// The laser interface (the glow, the beam and the panel), the menus and the settings window belong to
    /// the input method: when the user switches to another one, or to another language, nothing of it may stay on the
    /// screen.
    fn hide_floating(&mut self) {
        self.menu = None;
        self.settings_window = None;
        // The laser interface: it cannot be asked to hide itself while one of its own windows is calling back (that
        // is exactly what happens when the user switches the input method from the mouth of the light), so the flag
        // is set and the windows go off the screen as soon as the callback returns.
        if let Ok(mut laser) = self.laser.try_borrow_mut() {
            laser.hide();
        } else {
            self.laser_hide.set(true);
        }
    }

    /// Take the windows that belong to the document off the screen (the menus and the settings window).
    ///
    /// The laser interface belongs to the input method and not to the document: it stays, because the input method is
    /// still the one the user is in.
    fn hide_caret(&mut self) {
        self.menu = None;
        self.settings_window = None;
    }

    /// Take the text service apart: the windows go off the screen, the composition ends and the advises are undone.
    fn shutdown(&mut self) {
        self.controller.deactivate();
        self.hide_floating();
        // The bridge window of the AI agents goes with the text service: an agent that looks for it finds nothing and
        // says so. The window is destroyed here, on the thread that created it.
        self.bridge = None;
        // SAFETY: the advises made in `activate`; failures here cannot be acted on (the thread is going away).
        unsafe {
            if let Ok(keystrokes) = self.thread_mgr.cast::<ITfKeystrokeMgr>() {
                let _ = keystrokes.UnadviseKeyEventSink(self.client_id);
            }
            if let Ok(source) = self.thread_mgr.cast::<ITfSource>() {
                let _ = source.UnadviseSink(self.thread_sink_cookie);
                let _ = source.UnadviseSink(self.language_sink_cookie);
            }
        }
    }
}

/// The language id and the profile GUID of one of the three languages of the product.
fn profile_of(language: &str) -> Option<(u16, GUID)> {
    match language {
        "zh-CN" => Some((0x0804, crate::PROFILE_ZH)),
        "en-US" => Some((0x0409, crate::PROFILE_EN)),
        "ja-JP" => Some((0x0411, crate::PROFILE_JA)),
        _ => None,
    }
}

/// Ask TSF to activate the language profile of our text service (the same thing the language bar does).
fn activate_profile(langid: u16, profile: &GUID) -> bool {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::TextServices::{
        CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfiles,
    };
    // SAFETY: the system object of the text services; it is created in-process and released with the handle.
    let profiles: ITfInputProcessorProfiles = match unsafe {
        CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
    } {
        Ok(profiles) => profiles,
        Err(_) => return false,
    };
    // SAFETY: both pointers are live locals of the right type.
    unsafe { profiles.ActivateLanguageProfile(&crate::CLSID_PIME, langid, profile) }.is_ok()
}

/// The settings of every language, from the file of the user (the defaults when there is none).
/// Tell the core which plugin languages are on.
///
/// The set belongs to the engine: it is the place that decides whether a plugin profile has candidates at all,
/// and the settings window is the place that changes it.
fn apply_plugins(plugins: &[String]) {
    pime_core::predict::engine().set_plugin_languages(plugins);
}

fn load_settings() -> ImeSettings {
    match imesettings::settings_path() {
        Some(path) => ImeSettings::load(&path),
        None => ImeSettings::default(),
    }
}

/// The language of the profile that is active, if it can be told.
///
/// Two sources, in this order: TSF itself (the profile manager knows which of our three profiles is active) and the
/// keyboard layout of the thread (right whenever the layout follows the profile, which is the usual case). `None`
/// means "not one of our languages": the defaults apply then.
fn active_language() -> Option<&'static str> {
    if let Some(langid) = active_profile_language()
        && let Some(language) = imesettings::language_for_id(langid)
    {
        return Some(language);
    }
    // SAFETY: a plain query; the handle is only read for its language id (the low word of the handle).
    let hkl = unsafe { GetKeyboardLayout(0) };
    let langid = (hkl.0 as usize & 0xFFFF) as u16;
    imesettings::language_for_id(langid)
}

/// The language id of the profile of our text service that is active, as TSF sees it.
fn active_profile_language() -> Option<u16> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::TextServices::{
        CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfiles,
    };
    // SAFETY: the system object of the text services; it is created in-process and released with the handle.
    let profiles: ITfInputProcessorProfiles = match unsafe {
        CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
    } {
        Ok(profiles) => profiles,
        Err(_) => return None,
    };
    let mut langid = 0u16;
    let mut profile = GUID::default();
    // SAFETY: both pointers are live locals of the right type.
    match unsafe {
        profiles.GetActiveLanguageProfile(&crate::CLSID_PIME, &mut langid, &mut profile)
    } {
        Ok(()) => Some(langid),
        Err(_) => None,
    }
}

/// The four interfaces of the object that TSF is given to call back.
type Sinks = (
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfActiveLanguageProfileNotifySink,
);

/// The two advises `activate` makes and their undo, as a trait so that the rollback is tested without a thread manager.
trait Advises {
    fn advise_keys(&self) -> Result<()>;
    fn unadvise_keys(&self);
    /// The cookie of the thread-manager event sink.
    fn advise_thread(&self) -> Result<u32>;
    fn unadvise_thread(&self, cookie: u32);
    /// The cookie of the sink that hears about the active language profile.
    fn advise_language(&self) -> Result<u32>;
    fn unadvise_language(&self, cookie: u32);
}

/// [`Advises`] on the thread manager TSF handed to `Activate`.
struct TsfAdvises<'a> {
    thread_mgr: &'a ITfThreadMgr,
    client_id: u32,
    key_sink: &'a ITfKeyEventSink,
    thread_sink: &'a ITfThreadMgrEventSink,
    language_sink: &'a ITfActiveLanguageProfileNotifySink,
}

impl Advises for TsfAdvises<'_> {
    fn advise_keys(&self) -> Result<()> {
        let keystrokes: ITfKeystrokeMgr = self.thread_mgr.cast()?;
        // SAFETY: plain COM call on the thread manager TSF handed us; the sink is a live object of ours.
        unsafe { keystrokes.AdviseKeyEventSink(self.client_id, self.key_sink, BOOL(1)) }
    }

    fn unadvise_keys(&self) {
        if let Ok(keystrokes) = self.thread_mgr.cast::<ITfKeystrokeMgr>() {
            // SAFETY: undoes `advise_keys`; a failure cannot be acted on.
            let _ = unsafe { keystrokes.UnadviseKeyEventSink(self.client_id) };
        }
    }

    fn advise_thread(&self) -> Result<u32> {
        let source: ITfSource = self.thread_mgr.cast()?;
        // SAFETY: plain COM call; the sink is a live object of ours.
        unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, self.thread_sink) }
    }

    fn unadvise_thread(&self, cookie: u32) {
        if let Ok(source) = self.thread_mgr.cast::<ITfSource>() {
            // SAFETY: undoes `advise_thread` with the cookie it returned; a failure cannot be acted on.
            let _ = unsafe { source.UnadviseSink(cookie) };
        }
    }

    fn advise_language(&self) -> Result<u32> {
        let source: ITfSource = self.thread_mgr.cast()?;
        // SAFETY: plain COM call; the sink is a live object of ours. The thread manager tells its sinks about the
        // active language profile, so that we hear when another input method takes over.
        unsafe { source.AdviseSink(&ITfActiveLanguageProfileNotifySink::IID, self.language_sink) }
    }

    fn unadvise_language(&self, cookie: u32) {
        if let Ok(source) = self.thread_mgr.cast::<ITfSource>() {
            // SAFETY: undoes `advise_language` with the cookie it returned; a failure cannot be acted on.
            let _ = unsafe { source.UnadviseSink(cookie) };
        }
    }
}

/// What has been advised so far; dropped armed, it undoes it in reverse order. Disarmed once `deactivate` owns the advises.
struct Rollback<'a, A: Advises> {
    advises: &'a A,
    keys: bool,
    thread: Option<u32>,
    language: Option<u32>,
}

impl<A: Advises> Drop for Rollback<'_, A> {
    fn drop(&mut self) {
        if let Some(cookie) = self.language.take() {
            self.advises.unadvise_language(cookie);
        }
        if let Some(cookie) = self.thread.take() {
            self.advises.unadvise_thread(cookie);
        }
        if std::mem::take(&mut self.keys) {
            self.advises.unadvise_keys();
        }
    }
}

/// Advise the sinks, then run `build` with the cookies of the two advise sinks. When an advise fails, or `build`
/// panics (the unwind goes on to the caller's `guard`), every advise made so far is undone, so a retry starts clean:
/// TSF refuses a second key-event sink for the same client id. After a normal return the advises stay, to be undone
/// by `deactivate`.
fn advised<A: Advises, T>(advises: &A, build: impl FnOnce(u32, u32) -> T) -> Result<T> {
    let mut undo = Rollback {
        advises,
        keys: false,
        thread: None,
        language: None,
    };
    advises.advise_keys()?;
    undo.keys = true;
    let cookie = advises.advise_thread()?;
    undo.thread = Some(cookie);
    let language = advises.advise_language()?;
    undo.language = Some(language);
    let built = build(cookie, language);
    undo.keys = false;
    undo.thread = None;
    undo.language = None;
    Ok(built)
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
    fn activate(
        &self,
        thread_mgr: Option<&ITfThreadMgr>,
        client_id: u32,
        flags: u32,
        (key_sink, thread_sink, composition_sink, language_sink): Sinks,
    ) -> Result<()> {
        let thread_mgr = thread_mgr.ok_or(E_INVALIDARG)?.clone();
        if self.inner.try_borrow().map(|s| s.is_some()).unwrap_or(true) {
            return Err(E_FAIL.into()); // activated twice (or re-entered): refuse rather than double-advise
        }
        let tsf = TsfAdvises {
            thread_mgr: &thread_mgr,
            client_id,
            key_sink: &key_sink,
            thread_sink: &thread_sink,
            language_sink: &language_sink,
        };
        // Everything after the advises runs inside `advised`: an error or a panic there (the popup, the controller, the
        // store below) undoes both advises, so that TSF's retry of `Activate` is not refused as "already advised".
        advised(&tsf, |thread_sink_cookie, language_sink_cookie| {
            // The actions of the user (a click in the candidate window, a click in the floating bar) and of a host
            // that draws the candidate list itself travel through this one channel: they are applied at once, or
            // queued when the text service is busy inside a call of ours (then the queue is drained when that call
            // returns).
            let pending_actions: Rc<RefCell<Vec<Pending>>> = Rc::new(RefCell::new(Vec::new()));
            let sink: crate::ui::ActionSink = {
                let weak = Rc::downgrade(&self.inner);
                let pending = pending_actions.clone();
                Rc::new(move |action| {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    match inner.try_borrow_mut() {
                        Ok(mut slot) => {
                            let Some(i) = slot.as_mut() else {
                                return;
                            };
                            i.controller.apply_list_action(action);
                        }
                        Err(_) => pending.borrow_mut().push(Pending::List(action)),
                    }
                })
            };
            // The settings window: when the user saves, the settings are read again and applied.
            let settings_sink: SettingsSink = {
                let weak = Rc::downgrade(&self.inner);
                let pending = pending_actions.clone();
                Rc::new(move |event| {
                    if !matches!(event, SettingsEvent::Saved) {
                        return;
                    }
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    match inner.try_borrow_mut() {
                        Ok(mut slot) => {
                            let Some(i) = slot.as_mut() else {
                                return;
                            };
                            i.reload_settings();
                        }
                        Err(_) => pending.borrow_mut().push(Pending::ReloadSettings),
                    }
                })
            };
            // A menu of the floating bar: the choice travels the same way.
            let menu_sink: MenuSink = {
                let weak = Rc::downgrade(&self.inner);
                let pending = pending_actions.clone();
                let flags = Rc::clone(&self.flags);
                Rc::new(move |action| {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    match inner.try_borrow_mut() {
                        Ok(mut slot) => {
                            let Some(i) = slot.as_mut() else {
                                return;
                            };
                            i.apply_menu_action(action);
                        }
                        Err(_) => pending.borrow_mut().push(Pending::Menu(action)),
                    }
                    // The state is free again: the work that had to wait for it (the switch of the input method
                    // deactivates the text service inside this very call) happens now.
                    settle(&inner, &flags);
                })
            };
            // The language of the active profile and the settings of that language.
            let language = active_language().unwrap_or(DEFAULT_LANGUAGE).to_string();
            let settings = load_settings();
            apply_plugins(&settings.plugins);
            let values = settings.get(&language);
            let mut config = TipConfig::default();
            config.keymap.page_size = values.page_size().clamp(1, 9);
            config.keymap.tab_cycles = values.tab_cycles;
            config.keymap.tab_to_host = values.tab_to_host;
            config.keymap.space_confirms = values.space_confirms;
            config.auto_compose = values.auto_compose;
            config.input_mode = values.input_mode;
            // The vertical writing mode is a global setting: the text stands as a column when it is on.
            config.vertical = settings.vertical;
            // The profile of the engine follows the language: the Chinese and the Japanese are answered by the
            // dictionaries of the core, the English one by the engine of the code.
            config.language = pime_core::imeswitch::profile_of(&language).to_string();
            // The laser interface: the glow on the left of the screen, the beam and the panel. What the user does
            // in the panel travels the same way as the actions of the bar.
            let laser: Rc<RefCell<LaserUi>> = {
                let weak = Rc::downgrade(&self.inner);
                let pending = pending_actions.clone();
                let flags = Rc::clone(&self.flags);
                let sink: Rc<dyn Fn(LaserEvent)> = Rc::new(move |event| {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    match inner.try_borrow_mut() {
                        Ok(mut slot) => {
                            let Some(i) = slot.as_mut() else {
                                return;
                            };
                            i.apply_laser_event(event);
                        }
                        Err(_) => pending.borrow_mut().push(Pending::Laser(event)),
                    }
                    // The state is free again: the work that had to wait for it (the switch of the input method
                    // deactivates the text service inside this very call) happens now.
                    settle(&inner, &flags);
                });
                LaserUi::create(sink)
            };
            // The list of the candidates is drawn in the panel of the laser interface, on the left of the screen.
            // There is no floating window next to the caret any more (the Sogou-style window was in the way of
            // the code the user writes). The list is still announced to the host as a candidate-list UI element
            // when the thread manager offers `ITfUIElementMgr`.
            let ui = candidate_ui(
                &thread_mgr,
                Box::new(LaserCandidates::new(Rc::clone(&laser))),
                sink,
            );
            let laser_hide = laser.borrow().hide_flag();
            let mut controller = TipController::new(
                BoxedUi(Box::new(ui)),
                TsfHost::new(client_id, composition_sink),
                crate::predictor::PluginPredictor::new(),
                config,
            );
            // The light of the input method shines on the character that was written.
            {
                let laser_for_hook = Rc::clone(&laser);
                controller.set_commit_hook(Rc::new(move |anchor| {
                    let Some(anchor) = anchor else {
                        return;
                    };
                    if let Ok(mut laser) = laser_for_hook.try_borrow_mut() {
                        // The human writes with the mouth of the human (the pale blue one).
                        laser.fire(anchor);
                    }
                }));
            }
            // Alt+F2 walks through the three languages of the product. The hook cannot touch the state of the text
            // service (the controller has it borrowed while the key is handled), so the request travels through the
            // queue of the actions and is applied when the key is done.
            {
                let pending = pending_actions.clone();
                controller.set_language_hook(Rc::new(move || {
                    pending.borrow_mut().push(Pending::SwitchLanguage);
                }));
            }
            // The same for the input mode (Ctrl+F12 or Alt+F1): the panel has to learn whether the list of
            // candidates takes the keys (the natural language input) or is only a preview (the code input).
            {
                let pending = pending_actions.clone();
                controller.set_mode_hook(Rc::new(move || {
                    pending.borrow_mut().push(Pending::ModeChanged);
                }));
            }
            controller.activate(is_secure_mode(flags));
            // The bridge window: how an AI agent asks this input method to write into the document of the host. It is
            // created with the text service and dies with it, so an agent that finds it finds a running input method.
            let bridge = {
                let weak = Rc::downgrade(&self.inner);
                Bridge::create(Box::new(move |request| {
                    let Some(inner) = weak.upgrade() else {
                        return BridgeReply::failed(BridgeError::NoIme, "the input method is not active");
                    };
                    match inner.try_borrow_mut() {
                        Ok(mut slot) => match slot.as_mut() {
                            Some(state) => answer_agent(state, request),
                            None => {
                                BridgeReply::failed(BridgeError::NoIme, "the input method is not active")
                            }
                        },
                        Err(_) => BridgeReply::failed(
                            BridgeError::Busy,
                            "the input method is busy: try again in a moment",
                        ),
                    }
                }))
            };
            *self.inner.borrow_mut() = Some(Inner {
                thread_mgr: thread_mgr.clone(),
                client_id,
                thread_sink_cookie,
                language_sink_cookie,
                controller,
                started: Instant::now(),
                pending_actions,
                menu: None,
                settings_window: None,
                settings_sink,
                menu_sink,
                language,
                settings: values,
                effort: settings.effort.clone(),
                skin: settings.skin.clone(),
                mouth_place: settings.mouth_place.clone(),
                laser,
                laser_hide,
                bridge,
                demo_runs: 0,
            });
            // The laser interface comes on the screen with the text service.
            if let Ok(mut slot) = self.inner.try_borrow_mut()
                && let Some(inner) = slot.as_mut()
            {
                inner.sync_laser();
            }
        })
    }

    fn deactivate(&self) -> Result<()> {
        // The text service is going away. The flag says so even when the state is busy: TSF re-enters us from inside
        // our own callbacks (the switch of the input method happens in the callback of the mouth), and the state
        // cannot be taken then. The work happens as soon as the callback returns.
        self.flags.deactivated.set(true);
        self.settle();
        Ok(())
    }

    /// Forward one key event to the controller; false (not eaten) whenever anything is missing.
    fn key(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM, kind: KeyKind) -> bool {
        self.with(false, |inner| {
            inner.controller.host_mut().set_context(pic);
            let input = key_input(wparam, lparam, inner.started, kind.is_down());
            // The user is working in the application again: the panel of the settings has no business on the
            // screen any more (the user asked for that on 2026-11-11). The click somewhere else is not ours to
            // see, but the typing that follows it is.
            if kind == KeyKind::Down
                && input.code != 0
                && input.ch.is_some()
                && let Ok(mut laser) = inner.laser.try_borrow_mut()
            {
                laser.close_panel_unless_list();
            }
            let eaten = match kind {
                KeyKind::TestDown => inner.controller.on_test_key_down(input),
                KeyKind::Down => inner.controller.on_key_down(input),
                KeyKind::TestUp => inner.controller.on_test_key_up(input),
                KeyKind::Up => inner.controller.on_key_up(input),
            };
            eaten
        })
    }

    /// TSF tells us that the active language profile changed.
    ///
    /// Another text service took over, or one of ours is not the active one any more: nothing of ours may stay on
    /// the screen. When IME1.0 is the active one again, the interface comes back with it.
    fn profile_activated(&self, clsid: GUID, profile: GUID, activated: bool) {
        match profile_change(clsid, profile, activated) {
            ProfileChange::Ours(language) => {
                self.with((), |inner| {
                    if let Some(language) = language {
                        inner.set_language(language);
                    }
                    inner.sync_laser();
                });
            }
            ProfileChange::Other | ProfileChange::OursGone => self.hide_floating(),
            ProfileChange::Nothing => {}
        }
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

/// What the notification of the active profile means for the floating windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileChange {
    /// One of our own profiles is the active one: the language of that profile is what the product shows.
    Ours(Option<&'static str>),
    /// Another text service took over: everything floating has to go off the screen.
    Other,
    /// One of our own profiles is not the active one any more: everything floating has to go off the screen.
    OursGone,
    /// Nothing that concerns us.
    Nothing,
}

/// What the notification of the active profile means, as a pure decision.
///
/// The text service of the product has three profiles (zh-CN, en-US and ja-JP). When one of them is the active one,
/// the product is the input method of the user; when another text service is, it is not.
fn profile_change(clsid: GUID, profile: GUID, activated: bool) -> ProfileChange {
    if clsid != crate::CLSID_PIME {
        // Another text service: it matters only when it takes over.
        return if activated {
            ProfileChange::Other
        } else {
            ProfileChange::Nothing
        };
    }
    if activated {
        ProfileChange::Ours(language_of_profile(profile))
    } else {
        ProfileChange::OursGone
    }
}

/// The language of one of the three profiles of the product.
fn language_of_profile(profile: GUID) -> Option<&'static str> {
    if profile == crate::PROFILE_ZH {
        Some("zh-CN")
    } else if profile == crate::PROFILE_EN {
        Some("en-US")
    } else if profile == crate::PROFILE_JA {
        Some("ja-JP")
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    TestDown,
    Down,
    TestUp,
    Up,
}

impl KeyKind {
    fn is_down(self) -> bool {
        matches!(self, KeyKind::TestDown | KeyKind::Down)
    }
}

fn pressed(code: u32) -> bool {
    // SAFETY: GetKeyState has no preconditions; the high bit of the result is the key's down state.
    unsafe { GetKeyState(code as i32) < 0 }
}

/// The character a key press produces with the active layout and the current modifiers, without disturbing the
/// layout's dead-key state (flag 4: Windows 10 1607 and later; earlier versions ignore it).
fn produced_char(code: u32, lparam: isize) -> Option<char> {
    let scan = ((lparam >> 16) & 0xFF) as u32;
    let mut state = [0u8; 256];
    for key in [
        vk::SHIFT,
        vk::CONTROL,
        vk::MENU,
        vk::CAPITAL,
        vk::LSHIFT,
        vk::RSHIFT,
        vk::LCONTROL,
        vk::RCONTROL,
        vk::LMENU,
        vk::RMENU,
    ] {
        // SAFETY: as in `pressed`; the low bit is the toggle state (Caps Lock).
        let raw = unsafe { GetKeyState(key as i32) };
        state[key as usize] = if raw < 0 { 0x80 } else { 0 } | u8::from(raw & 1 != 0);
    }
    let mut buffer = [0u16; 8];
    // SAFETY: the buffers are live for the call; GetKeyboardLayout(0) is the calling thread's layout.
    let written = unsafe { ToUnicodeEx(code, scan, &state, &mut buffer, 4, GetKeyboardLayout(0)) };
    // one UTF-16 unit and a real character: a dead key (-1) or a control code has no text
    match written {
        1 => char::from_u32(u32::from(buffer[0])).filter(|c| !c.is_control()),
        2 => char::decode_utf16(buffer[..2].iter().copied()).next()?.ok(),
        _ => None,
    }
}

fn key_input(wparam: WPARAM, lparam: LPARAM, started: Instant, down: bool) -> KeyInput {
    let code = wparam.0 as u32;
    KeyInput {
        code,
        lparam: lparam.0,
        ch: if down {
            produced_char(code, lparam.0)
        } else {
            None
        },
        mods: Modifiers {
            shift: pressed(vk::SHIFT),
            ctrl: pressed(vk::CONTROL),
            alt: pressed(vk::MENU),
            win: pressed(vk::LWIN) || pressed(vk::RWIN),
        },
        now_ms: started.elapsed().as_millis() as u64,
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

    /// The panel of the laser interface must have exactly the number of rows its window is tall enough for.
    #[test]
    fn the_panel_has_the_number_of_rows_its_window_expects() {
        let items = laser_items(
            "zh-CN",
            &LanguageSettings::default(),
            "auto",
            "laser",
            true,
        );
        assert_eq!(
            items.len(),
            crate::laser_ui::PANEL_ROWS,
            "the panel shows {items:?}"
        );
        // the last row is the one that opens the settings window
        assert_eq!(items.last(), Some(&PanelItem::Settings));
        // and the demonstration is the row just before it (the user asked for it there)
        assert_eq!(items[items.len() - 2], PanelItem::Demo);
        assert!(
            items.iter().any(|item| item.label() == "多 Agent 演示"),
            "{items:?}"
        );
        // and the scheme of the keys is one of the rows (the user asked for it to be reachable from the panel)
        assert!(
            items.iter().any(|item| item.label() == "按键方案"),
            "{items:?}"
        );
    }

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

    /// Advises as TSF keeps them: a second advise while one is outstanding is refused (a key-event sink per client id).
    #[derive(Default)]
    struct FakeAdvises {
        keys: Cell<bool>,
        thread: Cell<Option<u32>>,
        language: Cell<Option<u32>>,
        next_cookie: Cell<u32>,
        fail_thread: Cell<bool>,
        fail_language: Cell<bool>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeAdvises {
        fn outstanding(&self) -> bool {
            self.keys.get() || self.thread.get().is_some() || self.language.get().is_some()
        }
    }

    impl Advises for FakeAdvises {
        fn advise_keys(&self) -> Result<()> {
            self.calls.borrow_mut().push("advise_keys");
            if self.keys.replace(true) {
                return Err(E_FAIL.into()); // "already advised"
            }
            Ok(())
        }

        fn unadvise_keys(&self) {
            self.calls.borrow_mut().push("unadvise_keys");
            self.keys.set(false);
        }

        fn advise_thread(&self) -> Result<u32> {
            self.calls.borrow_mut().push("advise_thread");
            if self.fail_thread.get() || self.thread.get().is_some() {
                return Err(E_FAIL.into());
            }
            self.next_cookie.set(self.next_cookie.get() + 1);
            self.thread.set(Some(self.next_cookie.get()));
            Ok(self.next_cookie.get())
        }

        fn unadvise_thread(&self, cookie: u32) {
            self.calls.borrow_mut().push("unadvise_thread");
            assert_eq!(
                self.thread.get(),
                Some(cookie),
                "the cookie that was advised"
            );
            self.thread.set(None);
        }

        fn advise_language(&self) -> Result<u32> {
            self.calls.borrow_mut().push("advise_language");
            if self.fail_language.get() || self.language.get().is_some() {
                return Err(E_FAIL.into());
            }
            self.next_cookie.set(self.next_cookie.get() + 1);
            self.language.set(Some(self.next_cookie.get()));
            Ok(self.next_cookie.get())
        }

        fn unadvise_language(&self, cookie: u32) {
            self.calls.borrow_mut().push("unadvise_language");
            assert_eq!(
                self.language.get(),
                Some(cookie),
                "the cookie that was advised"
            );
            self.language.set(None);
        }
    }

    #[test]
    fn a_panic_after_the_advises_unadvises_both_sinks_and_the_retry_succeeds() {
        // review finding 13: PopupWindow::new / TipController::new could panic after the advises; the guard answered
        // E_FAIL and left both sinks advised, so TSF's retry of Activate was refused
        let tsf = FakeAdvises::default();
        let got = guard(Err(E_FAIL.into()), || {
            advised(&tsf, |_, _| -> () {
                panic!("popup or controller panicked (test)")
            })
        });
        assert!(got.is_err());
        assert!(!tsf.outstanding(), "{:?}", tsf.calls.borrow());
        assert_eq!(
            *tsf.calls.borrow(),
            [
                "advise_keys",
                "advise_thread",
                "advise_language",
                "unadvise_language",
                "unadvise_thread",
                "unadvise_keys"
            ],
            "undone in reverse order"
        );
        // the retry is not refused, and its advises are the live ones
        let cookie = advised(&tsf, |cookie, _| cookie).expect("the retry succeeds");
        assert_eq!(tsf.thread.get(), Some(cookie));
        assert!(tsf.keys.get());
    }

    #[test]
    fn a_successful_build_keeps_the_advises_for_deactivate() {
        let tsf = FakeAdvises::default();
        let seen = advised(&tsf, |cookie, _| cookie).unwrap();
        assert_eq!(
            tsf.thread.get(),
            Some(seen),
            "build got the cookie of the advise"
        );
        assert!(tsf.keys.get());
        assert_eq!(
            *tsf.calls.borrow(),
            ["advise_keys", "advise_thread", "advise_language"]
        );
        // a second activation without a deactivate is refused by the advise itself, and changes nothing
        assert!(advised(&tsf, |_, _| ()).is_err());
        assert!(
            tsf.keys.get(),
            "the live advises are not undone by the refused second attempt"
        );
    }

    #[test]
    fn a_refused_thread_sink_unadvises_the_key_sink_and_a_refused_key_sink_undoes_nothing() {
        let tsf = FakeAdvises::default();
        tsf.fail_thread.set(true);
        assert!(advised(&tsf, |_, _| ()).is_err());
        assert!(!tsf.outstanding(), "{:?}", tsf.calls.borrow());
        tsf.fail_thread.set(false);
        assert!(advised(&tsf, |_, _| ()).is_ok(), "the retry is not refused");
        // the key sink is refused (already advised): nothing of this attempt exists, so nothing is undone
        let tsf = FakeAdvises::default();
        tsf.keys.set(true);
        assert!(advised(&tsf, |_, _| ()).is_err());
        assert_eq!(*tsf.calls.borrow(), ["advise_keys"]);
        assert!(tsf.keys.get(), "someone else's advise stays");
    }

    /// The refused third sink undoes the two before it: a retry of `Activate` must start clean.
    #[test]
    fn a_refused_language_sink_unadvises_the_two_before_it() {
        let tsf = FakeAdvises::default();
        tsf.fail_language.set(true);
        assert!(advised(&tsf, |_, _| ()).is_err());
        assert!(!tsf.outstanding(), "{:?}", tsf.calls.borrow());
        assert_eq!(
            *tsf.calls.borrow(),
            [
                "advise_keys",
                "advise_thread",
                "advise_language",
                "unadvise_thread",
                "unadvise_keys"
            ],
            "the language sink never existed, so it is not undone"
        );
    }

    /// The notification of the active profile is what tells the product that another input method took over.
    #[test]
    fn another_text_service_that_takes_over_empties_the_screen() {
        let other = GUID::from_u128(0xe3600580_1cf5_4305_a3b4_8f1a0f0a2b5d);
        assert_eq!(
            profile_change(other, GUID::default(), true),
            ProfileChange::Other,
            "another text service became the active one"
        );
        assert_eq!(
            profile_change(other, GUID::default(), false),
            ProfileChange::Nothing,
            "another text service went away: nothing of ours changes"
        );
    }

    #[test]
    fn our_own_profiles_are_the_ones_the_product_shows() {
        assert_eq!(
            profile_change(crate::CLSID_PIME, crate::PROFILE_ZH, true),
            ProfileChange::Ours(Some("zh-CN"))
        );
        assert_eq!(
            profile_change(crate::CLSID_PIME, crate::PROFILE_EN, true),
            ProfileChange::Ours(Some("en-US"))
        );
        assert_eq!(
            profile_change(crate::CLSID_PIME, crate::PROFILE_JA, true),
            ProfileChange::Ours(Some("ja-JP"))
        );
        assert_eq!(
            profile_change(crate::CLSID_PIME, crate::PROFILE_ZH, false),
            ProfileChange::OursGone,
            "our profile is not the active one any more"
        );
        assert_eq!(
            profile_change(crate::CLSID_PIME, GUID::default(), true),
            ProfileChange::Ours(None),
            "a profile we do not know: the product shows what it had"
        );
    }

    #[test]
    fn only_the_secure_mode_flag_makes_the_activation_secure() {
        assert!(is_secure_mode(TF_TMAE_SECUREMODE));
        assert!(is_secure_mode(TF_TMAE_SECUREMODE | 8));
        assert!(!is_secure_mode(0));
        assert!(!is_secure_mode(8 | 64));
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
}
