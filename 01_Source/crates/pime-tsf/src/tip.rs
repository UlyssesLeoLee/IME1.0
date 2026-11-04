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
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_INPUTSCOPE, IS_PASSWORD, ITfCompartmentMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputScope, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfSource, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, InputScope, TF_AE_NONE, TF_ANCHOR_END, TF_DEFAULT_SELECTION,
    TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
    TF_TMAE_SECUREMODE,
};
use windows::core::{GUID, IUnknown, IUnknownImpl, Interface, Result, implement};

use crate::controller::{HostError, HostText, KeyInput, TipConfig, TipController};
use crate::floating::{
    BarAction, BarMenu, BarSink, BarState, FloatingBar, button_of_menu, skin_colors,
};
use crate::imesettings::{self, ImeSettings, LanguageSettings};
use crate::keymap::{Modifiers, vk};
use crate::laser::PanelItem;
use crate::laser_ui::{LaserEvent, LaserUi};
use crate::layout::Rect;
use crate::menus::{MenuAction, MenuItem, MenuSink, MenuWindow};
use crate::popup::{PopupWindow, work_area_of};
use crate::register::debug_line;
use crate::settings_window::{SettingsEvent, SettingsSink, SettingsWindow};
use crate::ui::{Anchor, CandidateUi, CandidateView};
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

fn refused<E>(_: E) -> HostError {
    HostError::Refused
}

/// The candidate window is injected as a trait object (the production window is `popup::PopupWindow`).
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

/// Everything that exists between `Activate` and `Deactivate`.
struct Inner {
    thread_mgr: ITfThreadMgr,
    client_id: u32,
    thread_sink_cookie: u32,
    controller: TipController<BoxedUi, TsfHost>,
    started: Instant,
    /// Actions the user or the host asked for while the text service was busy: they are applied now. A host that
    /// answers an update by calling back into the element produces one more action, so the rounds are bounded.
    pending_actions: Rc<RefCell<Vec<Pending>>>,
    /// The floating bar of this thread (the small strip with the mode and the settings button).
    bar: FloatingBar,
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
    /// The laser interface of this thread: the glow on the left of the screen, the beam, the panel and the logo.
    laser: Rc<RefCell<LaserUi>>,
}

/// An action that arrived while the text service was busy.
enum Pending {
    /// The user clicked in the candidate window, or a host that draws the list itself selected something.
    List(crate::ui::ListAction),
    /// The user clicked in the floating bar.
    Bar(BarAction),
    /// The user chose something in a menu of the floating bar.
    Menu(MenuAction),
    /// The settings window was saved: the settings are read again.
    ReloadSettings,
    /// The user used the laser panel (the glow, the beam or the logo).
    Laser(LaserEvent),
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
                    Pending::Bar(action) => self.apply_bar_action(action),
                    Pending::Menu(action) => self.apply_menu_action(action),
                    Pending::ReloadSettings => self.reload_settings(),
                    Pending::Laser(event) => self.apply_laser_event(event),
                }
            }
        }
    }

    /// What the user asked for in the floating bar.
    fn apply_bar_action(&mut self, action: BarAction) {
        match action {
            BarAction::ToggleInputMode => {
                // The same thing Alt+F1 does.
                let on = !self.controller.input_mode();
                self.controller.set_input_mode(on);
                self.sync_bar();
            }
            BarAction::OpenMenu(which) => self.open_menu(which),
        }
    }

    /// Open one of the menus of the bar under (or above) its button.
    fn open_menu(&mut self, which: BarMenu) {
        if which == BarMenu::Settings {
            self.open_settings();
            return;
        }
        let Some(rect) = self.bar.button_rect(button_of_menu(which)) else {
            return;
        };
        self.open_menu_at(which, rect);
    }

    /// Open one of the menus under the rectangle the user clicked (the button of the bar or a row of the panel).
    fn open_menu_at(&mut self, which: BarMenu, rect: Rect) {
        if which == BarMenu::Settings {
            self.open_settings();
            return;
        }
        // Only one menu at a time: the old one goes away.
        self.menu = None;
        let items = self.menu_items(which);
        let work = work_area_of(rect);
        let sink = self.menu_sink.clone();
        self.menu = MenuWindow::open(
            items,
            which == BarMenu::Language,
            rect,
            work,
            skin_colors(&self.skin),
            sink,
        );
    }

    /// The items of one menu.
    fn menu_items(&self, which: BarMenu) -> Vec<MenuItem> {
        match which {
            BarMenu::Effort => imesettings::EFFORT_LEVELS
                .iter()
                .map(|level| {
                    MenuItem::checked(
                        imesettings::effort_label(level),
                        MenuAction::SetEffort((*level).to_string()),
                        self.effort == *level,
                    )
                })
                .collect(),
            BarMenu::Skin => imesettings::SKINS
                .iter()
                .map(|skin| {
                    MenuItem::checked(
                        imesettings::skin_label(skin),
                        MenuAction::SetSkin((*skin).to_string()),
                        self.skin == *skin,
                    )
                })
                .collect(),
            BarMenu::Convenience => vec![
                MenuItem::checked(
                    "Tab 确认候选上屏（勾选＝只移动选择）",
                    MenuAction::SetOption("tab_cycles".to_string(), !self.settings.tab_cycles),
                    self.settings.tab_cycles,
                ),
                MenuItem::checked(
                    "候选窗显示时 Tab 交给宿主",
                    MenuAction::SetOption("tab_to_host".to_string(), !self.settings.tab_to_host),
                    self.settings.tab_to_host,
                ),
                MenuItem::checked(
                    "空格上屏",
                    MenuAction::SetOption(
                        "space_confirms".to_string(),
                        !self.settings.space_confirms,
                    ),
                    self.settings.space_confirms,
                ),
                MenuItem::checked(
                    "自动开组合",
                    MenuAction::SetOption("auto_compose".to_string(), !self.settings.auto_compose),
                    self.settings.auto_compose,
                ),
                MenuItem::checked(
                    "显示悬浮窗",
                    MenuAction::SetOption("floating_bar".to_string(), !self.settings.floating_bar),
                    self.settings.floating_bar,
                ),
            ],
            BarMenu::Language => imesettings::LANGUAGES
                .iter()
                .map(|language| {
                    MenuItem::new(
                        imesettings::display_name(language),
                        MenuAction::SwitchLanguage((*language).to_string()),
                    )
                })
                .collect(),
            BarMenu::Settings => Vec::new(),
        }
    }

    /// What the user chose in a menu.
    fn apply_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::ToggleInputMode => {
                let on = !self.controller.input_mode();
                self.controller.set_input_mode(on);
                self.sync_bar();
            }
            MenuAction::SetEffort(effort) => {
                self.effort = effort;
                self.save_settings();
            }
            MenuAction::SetSkin(skin) => {
                self.skin = skin;
                self.save_settings();
                self.sync_bar();
            }
            MenuAction::SetOption(name, on) => {
                self.set_option(&name, on);
            }
            MenuAction::SwitchLanguage(language) => {
                self.switch_language(&language);
            }
            MenuAction::OpenSettings => self.open_settings(),
        }
    }

    /// Switch one of the settings of the active language on or off.
    ///
    /// The floating bar, the menus and the laser panel all go through here, so the three of them can never disagree
    /// about what the setting is.
    fn set_option(&mut self, name: &str, on: bool) {
        let mut value = self.settings;
        match name {
            "tab_cycles" => value.tab_cycles = on,
            "tab_to_host" => value.tab_to_host = on,
            "space_confirms" => value.space_confirms = on,
            "auto_compose" => value.auto_compose = on,
            "floating_bar" => value.floating_bar = on,
            "input_mode" => {
                // The input mode is a state of the controller, not a file: it is the same thing Alt+F1 does.
                self.controller.set_input_mode(on);
                self.settings = value;
                self.sync_bar();
                self.sync_laser();
                return;
            }
            _ => return,
        }
        self.settings = value;
        self.controller.apply_settings(&value);
        self.save_settings();
        self.sync_bar();
        self.sync_laser();
    }

    /// What the user asked for in the laser panel.
    fn apply_laser_event(&mut self, event: LaserEvent) {
        match event {
            LaserEvent::Toggle { key, on } => self.set_option(key, on),
            LaserEvent::Menu { key } => self.open_laser_menu(key),
            LaserEvent::Settings => self.open_settings(),
            LaserEvent::Switch => {
                // The mouth of the light is the switch between the input methods.
                crate::ime_switch::switch_now();
            }
            LaserEvent::MouthMoved { edge, offset } => {
                // The mouth was dragged to another place: it is remembered, so it is still there next time.
                self.mouth_place = crate::laser::MouthPlace { edge, offset }.to_text();
                self.save_settings();
            }
        }
    }

    /// Open one of the lists of the laser panel under the row the user clicked.
    fn open_laser_menu(&mut self, key: &str) {
        let which = match key {
            "language" => BarMenu::Language,
            "effort" => BarMenu::Effort,
            "skin" => BarMenu::Skin,
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

    /// Switch the input language: TSF activates the profile of the language, and the settings of that language apply
    /// from then on.
    fn switch_language(&mut self, language: &str) {
        let Some((langid, profile)) = profile_of(language) else {
            return;
        };
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
        self.sync_bar();
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
        vec![
            PanelItem::Toggle {
                key: "input_mode",
                label: "输入模式",
                on: self.controller.input_mode(),
            },
            PanelItem::Toggle {
                key: "space_confirms",
                label: "空格上屏",
                on: self.settings.space_confirms,
            },
            PanelItem::Toggle {
                key: "auto_compose",
                label: "自动开组合",
                on: self.settings.auto_compose,
            },
            PanelItem::Toggle {
                key: "tab_cycles",
                label: "Tab 确认候选",
                on: self.settings.tab_cycles,
            },
            PanelItem::Menu {
                key: "language",
                label: "语言",
                value: imesettings::display_name(&self.language).to_string(),
            },
            PanelItem::Menu {
                key: "effort",
                label: "AI 力度",
                value: imesettings::effort_label(&self.effort).to_string(),
            },
            PanelItem::Menu {
                key: "skin",
                label: "外观",
                value: imesettings::skin_label(&self.skin).to_string(),
            },
            PanelItem::Settings,
        ]
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
        if !laser.is_visible() {
            let work = work_area_of(Rect::new(0, 0, 0, 0));
            laser.show(work);
        }
    }

    /// Show the floating bar at the caret (or hide it, when the settings say so).
    fn sync_bar(&mut self) {        if !self.settings.floating_bar {
            self.bar.hide();
            return;
        }
        let state = BarState {
            language: self.language.clone(),
            input_mode: self.controller.input_mode(),
            skin: self.skin.clone(),
        };
        let Some(anchor) = self.controller.host_mut().caret_anchor() else {
            return; // the host cannot say where the caret is: the bar stays where it is
        };
        self.bar.show(&state, anchor);
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
        let language = self.language.clone();
        let value = settings.get(&language);
        self.settings = value;
        self.effort = settings.effort.clone();
        self.skin = settings.skin.clone();
        self.mouth_place = settings.mouth_place.clone();
        self.controller.apply_settings(&value);
        self.sync_bar();
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

/// The three interfaces of the object that TSF is given to call back.
type Sinks = (ITfKeyEventSink, ITfThreadMgrEventSink, ITfCompositionSink);

/// The two advises `activate` makes and their undo, as a trait so that the rollback is tested without a thread manager.
trait Advises {
    fn advise_keys(&self) -> Result<()>;
    fn unadvise_keys(&self);
    /// The cookie of the thread-manager event sink.
    fn advise_thread(&self) -> Result<u32>;
    fn unadvise_thread(&self, cookie: u32);
}

/// [`Advises`] on the thread manager TSF handed to `Activate`.
struct TsfAdvises<'a> {
    thread_mgr: &'a ITfThreadMgr,
    client_id: u32,
    key_sink: &'a ITfKeyEventSink,
    thread_sink: &'a ITfThreadMgrEventSink,
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
}

/// What has been advised so far; dropped armed, it undoes it in reverse order. Disarmed once `deactivate` owns the advises.
struct Rollback<'a, A: Advises> {
    advises: &'a A,
    keys: bool,
    thread: Option<u32>,
}

impl<A: Advises> Drop for Rollback<'_, A> {
    fn drop(&mut self) {
        if let Some(cookie) = self.thread.take() {
            self.advises.unadvise_thread(cookie);
        }
        if std::mem::take(&mut self.keys) {
            self.advises.unadvise_keys();
        }
    }
}

/// Advise both sinks, then run `build` with the thread-sink cookie. When an advise fails, or `build` panics (the unwind
/// goes on to the caller's `guard`), every advise made so far is undone, so a retry starts clean: TSF refuses a second
/// key-event sink for the same client id. After a normal return the advises stay, to be undone by `deactivate`.
fn advised<A: Advises, T>(advises: &A, build: impl FnOnce(u32) -> T) -> Result<T> {
    let mut undo = Rollback {
        advises,
        keys: false,
        thread: None,
    };
    advises.advise_keys()?;
    undo.keys = true;
    let cookie = advises.advise_thread()?;
    undo.thread = Some(cookie);
    let built = build(cookie);
    undo.keys = false;
    undo.thread = None;
    Ok(built)
}

const PENDING_FOCUS: u8 = 1;
const PENDING_COMPOSITION_ENDED: u8 = 2;

/// The text input processor object: one per thread that activates the profile (TSF calls it on that thread's
/// apartment, so interior mutability without locking is enough).
#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink
)]
struct Tip {
    inner: Rc<RefCell<Option<Inner>>>,
    /// Events that arrived while `inner` was borrowed (TSF re-entered us from inside one of our edit sessions).
    pending: Cell<u8>,
    /// Counts this object in the DLL's live-object counter while it exists.
    _live: crate::LiveGuard,
}

impl Tip {
    fn new() -> Tip {
        Tip {
            inner: Rc::new(RefCell::new(None)),
            pending: Cell::new(0),
            _live: crate::LiveGuard::new(),
        }
    }

    /// Run `f` on the active state; `fallback` when not activated or when TSF re-entered us (then the event is
    /// remembered if it matters).
    fn with<R>(&self, fallback: R, f: impl FnOnce(&mut Inner) -> R) -> R {
        let Ok(mut slot) = self.inner.try_borrow_mut() else {
            return fallback;
        };
        let Some(inner) = slot.as_mut() else {
            return fallback;
        };
        let pending = self.pending.replace(0);
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
    }

    /// Note an event for the next `with` when the state is busy right now.
    fn remember_if_busy(&self, flag: u8) {
        if self.inner.try_borrow_mut().is_err() {
            self.pending.set(self.pending.get() | flag);
        }
    }
    fn activate(
        &self,
        thread_mgr: Option<&ITfThreadMgr>,
        client_id: u32,
        flags: u32,
        (key_sink, thread_sink, composition_sink): Sinks,
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
        };
        // Everything after the advises runs inside `advised`: an error or a panic there (the popup, the controller, the
        // store below) undoes both advises, so that TSF's retry of `Activate` is not refused as "already advised".
        advised(&tsf, |thread_sink_cookie| {
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
            // The floating bar: the same pattern, for the two buttons of the bar.
            let bar_sink: BarSink = {
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
                            i.apply_bar_action(action);
                        }
                        Err(_) => pending.borrow_mut().push(Pending::Bar(action)),
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
                })
            };
            // The native popup, announced to the host as a candidate-list UI element when the thread manager offers
            // `ITfUIElementMgr`; a host that answers "I draw" keeps the popup hidden (see `uielement`).
            let ui = candidate_ui(
                &thread_mgr,
                Box::new(PopupWindow::with_sink(sink.clone())),
                sink,
            );
            // The language of the active profile and the settings of that language.
            let language = active_language().unwrap_or(DEFAULT_LANGUAGE).to_string();
            let settings = load_settings();
            let values = settings.get(&language);
            let mut config = TipConfig::default();
            config.keymap.page_size = values.page_size().clamp(1, 9);
            config.keymap.tab_cycles = values.tab_cycles;
            config.keymap.tab_to_host = values.tab_to_host;
            config.keymap.space_confirms = values.space_confirms;
            config.auto_compose = values.auto_compose;
            config.input_mode = values.input_mode;
            // The laser interface: the glow on the left of the screen, the beam, the panel and the logo in the
            // corner. What the user does in the panel travels the same way as the actions of the bar.
            let laser: Rc<RefCell<LaserUi>> = {
                let weak = Rc::downgrade(&self.inner);
                let pending = pending_actions.clone();
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
                });
                LaserUi::create(sink)
            };
            let mut controller = TipController::new(
                BoxedUi(Box::new(ui)),
                TsfHost::new(client_id, composition_sink),
                pime_core::input::LocalPredictor,
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
                        laser.fire(anchor);
                    }
                }));
            }
            controller.activate(is_secure_mode(flags));
            *self.inner.borrow_mut() = Some(Inner {
                thread_mgr: thread_mgr.clone(),
                client_id,
                thread_sink_cookie,
                controller,
                started: Instant::now(),
                pending_actions,
                bar: FloatingBar::new(bar_sink),
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
        let Ok(mut slot) = self.inner.try_borrow_mut() else {
            return Ok(());
        };
        let Some(mut inner) = slot.take() else {
            return Ok(());
        };
        inner.controller.deactivate();
        // The floating bar, its menus and the settings window belong to this thread: they go with it.
        inner.bar.hide();
        inner.menu = None;
        inner.settings_window = None;
        // The laser interface (the glow, the beam, the panel and the logo) goes with it as well.
        if let Ok(mut laser) = inner.laser.try_borrow_mut() {
            laser.hide();
        }
        // SAFETY: the advises made in `activate`; failures here cannot be acted on (the thread is going away).
        unsafe {
            if let Ok(keystrokes) = inner.thread_mgr.cast::<ITfKeystrokeMgr>() {
                let _ = keystrokes.UnadviseKeyEventSink(inner.client_id);
            }
            if let Ok(source) = inner.thread_mgr.cast::<ITfSource>() {
                let _ = source.UnadviseSink(inner.thread_sink_cookie);
            }
        }
        self.pending.set(0);
        Ok(())
    }

    /// Forward one key event to the controller; false (not eaten) whenever anything is missing.
    fn key(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM, kind: KeyKind) -> bool {
        self.with(false, |inner| {
            inner.controller.host_mut().set_context(pic);
            let input = key_input(wparam, lparam, inner.started, kind.is_down());
            let eaten = match kind {
                KeyKind::TestDown => inner.controller.on_test_key_down(input),
                KeyKind::Down => inner.controller.on_key_down(input),
                KeyKind::TestUp => inner.controller.on_test_key_up(input),
                KeyKind::Up => inner.controller.on_key_up(input),
            };
            // The floating bar follows the caret: it is shown and moved after every key.
            inner.sync_bar();
            eaten
        })
    }
}

#[derive(Clone, Copy)]
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
    fn guard_passes_a_value_through_and_turns_a_panic_into_the_fallback() {
        assert_eq!(guard(7, || 42), 42);
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
        next_cookie: Cell<u32>,
        fail_thread: Cell<bool>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeAdvises {
        fn outstanding(&self) -> bool {
            self.keys.get() || self.thread.get().is_some()
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
    }

    #[test]
    fn a_panic_after_the_advises_unadvises_both_sinks_and_the_retry_succeeds() {
        // review finding 13: PopupWindow::new / TipController::new could panic after the advises; the guard answered
        // E_FAIL and left both sinks advised, so TSF's retry of Activate was refused
        let tsf = FakeAdvises::default();
        let got = guard(Err(E_FAIL.into()), || {
            advised(&tsf, |_| -> () {
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
                "unadvise_thread",
                "unadvise_keys"
            ],
            "undone in reverse order"
        );
        // the retry is not refused, and its advises are the live ones
        let cookie = advised(&tsf, |cookie| cookie).expect("the retry succeeds");
        assert_eq!(tsf.thread.get(), Some(cookie));
        assert!(tsf.keys.get());
    }

    #[test]
    fn a_successful_build_keeps_the_advises_for_deactivate() {
        let tsf = FakeAdvises::default();
        let seen = advised(&tsf, |cookie| cookie).unwrap();
        assert_eq!(
            tsf.thread.get(),
            Some(seen),
            "build got the cookie of the advise"
        );
        assert!(tsf.keys.get());
        assert_eq!(*tsf.calls.borrow(), ["advise_keys", "advise_thread"]);
        // a second activation without a deactivate is refused by the advise itself, and changes nothing
        assert!(advised(&tsf, |_| ()).is_err());
        assert!(
            tsf.keys.get(),
            "the live advises are not undone by the refused second attempt"
        );
    }

    #[test]
    fn a_refused_thread_sink_unadvises_the_key_sink_and_a_refused_key_sink_undoes_nothing() {
        let tsf = FakeAdvises::default();
        tsf.fail_thread.set(true);
        assert!(advised(&tsf, |_| ()).is_err());
        assert!(!tsf.outstanding(), "{:?}", tsf.calls.borrow());
        tsf.fail_thread.set(false);
        assert!(advised(&tsf, |_| ()).is_ok(), "the retry is not refused");
        // the key sink is refused (already advised): nothing of this attempt exists, so nothing is undone
        let tsf = FakeAdvises::default();
        tsf.keys.set(true);
        assert!(advised(&tsf, |_| ()).is_err());
        assert_eq!(*tsf.calls.borrow(), ["advise_keys"]);
        assert!(tsf.keys.get(), "someone else's advise stays");
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
