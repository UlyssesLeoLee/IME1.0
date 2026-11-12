//! The life of the text service: the activation, the deactivation, the change of the active profile and the switch
//! of the input method.
//!
//! Everything that exists between `Activate` and `Deactivate` is built here: the advises of the four sinks of TSF,
//! the controller, the laser interface, the settings window and the bridge window of the AI agents. The rollback of
//! an activation that fails is here as well: TSF refuses a second key-event sink for the same client id, so the retry
//! has to start clean.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::{BOOL, E_FAIL, E_INVALIDARG};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::TextServices::{
    ITfActiveLanguageProfileNotifySink, ITfKeyEventSink, ITfKeystrokeMgr, ITfSource, ITfThreadMgr,
    ITfThreadMgrEventSink, TF_TMAE_SECUREMODE,
};
use windows::core::{GUID, Interface, Result};

use pime_core::agent_proto::{Error as BridgeError, Reply as BridgeReply};

use crate::agent_bridge::Bridge;
use crate::controller::{TipConfig, TipController};
use crate::imesettings;
use crate::laser_candidates::LaserCandidates;
use crate::laser_ui::{LaserEvent, LaserUi};
use crate::menus::MenuSink;
use crate::settings_window::{SettingsEvent, SettingsSink};
use crate::uielement_com::candidate_ui;

use super::bridge::answer_agent;
use super::settings::{apply_plugins, load_settings};
use super::{BoxedUi, Inner, Pending, Sinks, TsfHost, Tip, settle, trace};

/// `ActivateEx` flag: TSF runs on a secure desktop (logon, UAC): collect nothing there.
fn is_secure_mode(flags: u32) -> bool {
    flags & TF_TMAE_SECUREMODE != 0
}

/// The language whose settings are used when the active profile cannot be told apart.
const DEFAULT_LANGUAGE: &str = "zh-CN";

impl Tip {
    pub(super) fn activate(
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

    pub(super) fn deactivate(&self) -> Result<()> {
        // The text service is going away. The flag says so even when the state is busy: TSF re-enters us from inside
        // our own callbacks (the switch of the input method happens in the callback of the mouth), and the state
        // cannot be taken then. The work happens as soon as the callback returns.
        self.flags.deactivated.set(true);
        self.settle();
        Ok(())
    }

    /// TSF tells us that the active language profile changed.
    ///
    /// Another text service took over, or one of ours is not the active one any more: nothing of ours may stay on
    /// the screen. When IME1.0 is the active one again, the interface comes back with it.
    pub(super) fn profile_activated(&self, clsid: GUID, profile: GUID, activated: bool) {
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

impl Inner {
    /// Switch to the next language of the product (Alt+F2).
    ///
    /// The three languages of the product (zh-CN, en-US and ja-JP) are walked through in that order; the decision
    /// itself is pure and lives in the core.
    pub(super) fn switch_to_next_language(&mut self) {
        let next = pime_core::imeswitch::next_language(&self.language);
        self.switch_language(next);
    }

    /// Switch the input language: TSF activates the profile of the language, and the settings of that language apply
    /// from then on.
    pub(super) fn switch_language(&mut self, language: &str) {
        let Some((langid, profile)) = profile_of(language) else {
            return;
        };
        // The windows that belong to the document go off the screen before the switch: TSF changes the profile
        // inside that very call, and the state is borrowed at that moment.
        self.hide_caret();
        // The panel and the list go away as well (the user reported it on 2026-11-12): the candidates of the old
        // language must not stay on the screen. The mouth of the light stays: the input method is still ours.
        if let Ok(mut laser) = self.laser.try_borrow_mut() {
            laser.close_panel_and_list();
        }
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

    /// Take every floating window of this thread off the screen.
    ///
    /// The laser interface (the glow, the beam and the panel), the menus and the settings window belong to
    /// the input method: when the user switches to another one, or to another language, nothing of it may stay on the
    /// screen.
    pub(super) fn hide_floating(&mut self) {
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
    pub(super) fn hide_caret(&mut self) {
        self.menu = None;
        self.settings_window = None;
    }

    /// Take the text service apart: the windows go off the screen, the composition ends and the advises are undone.
    pub(super) fn shutdown(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::guard;
    use std::cell::Cell;

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
}
