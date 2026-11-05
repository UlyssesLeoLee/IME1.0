//! IME1.0 text service (TSF text input processor).
//!
//! STATUS: the DLL builds with the pure-Rust toolchain and exports the four COM entry points. Module ownership (so that
//! parallel work does not collide): `tip` + `keymap` (activation, key sink, composition), `popup` + `layout` (candidate
//! window), `uielement` + `uielement_com` (the TSF UI-element protocol: announcing the candidate list to the host and
//! letting a host that draws its own list keep the native popup hidden), `register` (registry and TSF profile
//! registration), `ui` (the shared contract between them).
//! Not a working input method until the milestones in docs/RUST_MIGRATION_PLAN.md 13.7 are done AND verified in a
//! disposable Windows VM: nothing here may be registered on a real machine before that.
//!
//! Safety rules for this crate: every `unsafe` block is a COM/FFI boundary and carries a reason; a panic must never
//! unwind into the host application. The release profile therefore uses `panic = "unwind"` (an abort would kill the
//! host process), every COM method of the text input processor catches panics and answers "not eaten"
//! (`tip::guard`; so does `tip::create_instance`, which the class factory calls), and there is no `unwrap` on host
//! input. The class factory and the four DLL exports themselves contain no logic beyond null checks, so they have
//! nothing to catch.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::core::{GUID, HRESULT, IUnknown, Interface, Result, implement};

pub mod agent_bridge;
pub mod candidate_skin;
pub mod controller;
pub mod glass;
pub mod imesettings;
pub mod ime_switch;
pub mod keymap;
pub mod laser;
pub mod laser_candidates;
pub mod laser_ui;
pub mod layout;
pub mod menus;
pub mod mouths;
pub mod paint;
pub mod predictor;
pub mod popup;
pub mod register;
pub mod settings_window;
pub mod skin;
pub mod tip;
pub mod ui;
pub mod uielement;
pub mod uielement_com;

/// Prototype GUIDs (the same values as the C++ skeleton in `windows/tsf`); to be replaced by governed product ids.
pub const CLSID_PIME: GUID = GUID::from_u128(0x43f95ed1_95b4_421d_8f5a_837128c2d854);
/// Language profiles: zh-CN (0x0804), en-US (0x0409), ja-JP (0x0411).
pub const PROFILE_ZH: GUID = GUID::from_u128(0x223fd919_f8cb_476c_a852_351e7381c801);
pub const PROFILE_EN: GUID = GUID::from_u128(0x68ed92de_77cf_43b1_a7b2_d3be2686e2a0);
pub const PROFILE_JA: GUID = GUID::from_u128(0x22e54b52_5082_42d3_9e41_1c2e0ce74dfb);

/// The COM objects this DLL currently owns (the text service object).
///
/// COM asks the DLL whether it may unload through [`DllCanUnloadNow`]; the answer depends on how many objects are
/// still alive and how many `LockServer` calls are outstanding. The counter is atomic because COM may release an
/// object on a different thread than the one that created it.
static LIVE_OBJECTS: AtomicU32 = AtomicU32::new(0);
/// The outstanding `IClassFactory::LockServer` calls.
static LOCKED_SERVERS: AtomicI32 = AtomicI32::new(0);

/// Count one live object. Called right after the object exists and before it can be released.
pub(crate) fn object_created() {
    LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
}

/// Count one object that is gone. Never underflows: a stray release would otherwise turn into a huge number and
/// keep the DLL from ever unloading.
pub(crate) fn object_destroyed() {
    let _ = LIVE_OBJECTS.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_sub(1))
    });
}

/// The live object count, for the tests.
#[cfg(test)]
pub(crate) fn live_objects() -> u32 {
    LIVE_OBJECTS.load(Ordering::Relaxed)
}

/// Whether the DLL may be unloaded: no object of ours is alive and nobody holds a server lock.
fn unload_allowed(live: u32, locked: i32) -> bool {
    live == 0 && locked == 0
}

/// Counts one COM object of ours in the DLL's live-object counter while it exists.
///
/// Every object we hand to the host has to be counted, not only the text service: TSF holds the candidate-list UI
/// element from `BeginUIElement` to `EndUIElement` and calls back into it, so the DLL must not be unloaded while the
/// host still holds one.
///
/// The `implement` macro moves the fields of the user's struct into the COM object and drops them when the last
/// reference to that object is released. A field with its own `Drop` therefore runs exactly when the object dies -
/// which is what the counter needs. The counter itself is a plain atomic in this module, because COM may release an
/// object on another thread than the one that created it.
pub(crate) struct LiveGuard {
    /// Whether this object is counted. Only the objects the class factory hands out are counted; the tests build
    /// objects of their own and must not disturb the counter.
    counted: bool,
}

impl LiveGuard {
    /// A guard for an object that is not counted (the tests build such objects).
    pub(crate) fn new() -> LiveGuard {
        LiveGuard { counted: false }
    }

    /// Start counting this object.
    pub(crate) fn count(&mut self) {
        if !self.counted {
            self.counted = true;
            object_created();
        }
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        if self.counted {
            object_destroyed();
        }
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if object.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: `object` was checked for null; COM requires it to be writable. COM also requires it to be NULL after
        // every failure, so it is cleared before any other check can return.
        unsafe { *object = std::ptr::null_mut() };
        if iid.is_null() {
            return Err(E_POINTER.into());
        }
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        // SAFETY: `iid` and `object` were checked for null; COM requires them to be valid for the call.
        unsafe { tip::create_instance(iid, object) }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        // COM asks the DLL to stay loaded while a caller holds a server lock. The count is per call, not per object.
        if lock.as_bool() {
            LOCKED_SERVERS.fetch_add(1, Ordering::Relaxed);
        } else {
            let _ = LOCKED_SERVERS.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            });
        }
        Ok(())
    }
}

/// # Safety
/// Called by COM with valid (or null) pointers; null and unknown classes are answered with an error code.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    // SAFETY: `ppv` was checked for null; COM passes a writable pointer. It is NULL after every failure, whichever
    // check returns.
    unsafe { *ppv = std::ptr::null_mut() };
    if rclsid.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked for null above; COM passes pointers that are valid for the call.
    unsafe {
        if *rclsid != CLSID_PIME {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    }
}

/// Whether the DLL may unload right now: only when no object of ours is alive and no server lock is held.
///
/// COM calls this before it unloads the module; answering `S_FALSE` forever keeps the text service in the host
/// process for as long as the process lives.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if unload_allowed(
        LIVE_OBJECTS.load(Ordering::Relaxed),
        LOCKED_SERVERS.load(Ordering::Relaxed),
    ) {
        S_OK
    } else {
        S_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    register::register_server()
}

#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    register::unregister_server()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::S_OK;
    use windows::Win32::UI::TextServices::ITfTextInputProcessorEx;

    #[test]
    fn the_clsid_matches_the_documented_prototype_value() {
        assert_eq!(
            format!("{:?}", CLSID_PIME).to_lowercase(),
            "43f95ed1-95b4-421d-8f5a-837128c2d854"
        );
    }

    /// COM requires the out pointer to be NULL after a failure. Every case starts from a non-null sentinel, because a
    /// pointer that is already NULL proves nothing (review finding 13).
    #[test]
    fn every_refused_request_leaves_the_out_pointer_null() {
        let sentinel = std::ptr::NonNull::<c_void>::dangling().as_ptr();
        let iid = IClassFactory::IID;
        let other = GUID::from_u128(1);
        let null_guid: *const GUID = std::ptr::null();
        // SAFETY: valid pointers to live locals; the sentinel is only compared, never dereferenced; the factory is a live
        // in-process object and `this` doubles as the (never dereferenced) outer object of the aggregation case.
        unsafe {
            for (clsid, riid, want) in [
                (null_guid, &iid as *const GUID, E_POINTER),
                (&CLSID_PIME as *const GUID, null_guid, E_POINTER),
                (
                    &other as *const GUID,
                    &iid as *const GUID,
                    CLASS_E_CLASSNOTAVAILABLE,
                ),
            ] {
                let mut out = sentinel;
                assert_eq!(DllGetClassObject(clsid, riid, &mut out), want);
                assert!(out.is_null(), "DllGetClassObject left {out:?} behind");
            }
            assert_eq!(
                DllGetClassObject(&CLSID_PIME, &iid, std::ptr::null_mut()),
                E_POINTER
            );

            let mut raw: *mut c_void = std::ptr::null_mut();
            assert_eq!(DllGetClassObject(&CLSID_PIME, &iid, &mut raw), S_OK);
            let factory = IClassFactory::from_raw(raw);
            let vtbl = Interface::vtable(&factory);
            let this = Interface::as_raw(&factory);

            let mut out = sentinel;
            let hr = (vtbl.CreateInstance)(this, std::ptr::null_mut(), null_guid, &mut out);
            assert_eq!(hr, E_POINTER, "no interface id");
            assert!(out.is_null(), "CreateInstance left {out:?} behind");

            let mut out = sentinel;
            let hr = (vtbl.CreateInstance)(this, this, &IUnknown::IID, &mut out);
            assert_eq!(hr, CLASS_E_NOAGGREGATION);
            assert!(
                out.is_null(),
                "aggregation: CreateInstance left {out:?} behind"
            );

            let mut out = sentinel;
            let hr = (vtbl.CreateInstance)(this, std::ptr::null_mut(), &other, &mut out);
            assert!(hr.is_err(), "an interface the object does not have");
            assert!(out.is_null(), "unknown interface: left {out:?} behind");

            let hr = (vtbl.CreateInstance)(
                this,
                std::ptr::null_mut(),
                &IUnknown::IID,
                std::ptr::null_mut(),
            );
            assert_eq!(hr, E_POINTER, "no place for the answer");

            // and the one that must work still works
            let mut out = sentinel;
            let hr = (vtbl.CreateInstance)(this, std::ptr::null_mut(), &IUnknown::IID, &mut out);
            assert_eq!(hr, S_OK);
            assert!(!out.is_null() && out != sentinel);
            drop(IUnknown::from_raw(out));
        }
    }

    #[test]
    fn unknown_classes_and_null_pointers_are_refused_not_crashed_on() {
        let mut out: *mut c_void = std::ptr::null_mut();
        let other = GUID::from_u128(1);
        let iid = IClassFactory::IID;
        // SAFETY: valid pointers to live locals.
        unsafe {
            assert_eq!(
                DllGetClassObject(&other, &iid, &mut out),
                CLASS_E_CLASSNOTAVAILABLE
            );
            assert_eq!(
                DllGetClassObject(std::ptr::null(), &iid, &mut out),
                E_POINTER
            );
        }
        assert_eq!(S_OK, HRESULT(0));
    }

    #[test]
    fn the_unload_decision_depends_on_the_live_objects_and_the_server_locks() {
        // the pure decision: no object and no lock means the DLL may go
        assert!(unload_allowed(0, 0));
        // one live object keeps it loaded, whatever the locks say
        assert!(!unload_allowed(1, 0));
        assert!(!unload_allowed(0, 1));
        assert!(!unload_allowed(3, -1));
        assert!(unload_allowed(0, 0));
    }

    /// While an object of ours is alive the DLL must stay loaded. The check is written as "at least one object"
    /// because the other tests of this binary may hold objects of their own at the same time.
    #[test]
    fn a_live_object_keeps_the_dll_loaded() {
        let mut raw = std::ptr::null_mut();
        let iid = ITfTextInputProcessorEx::IID;
        // SAFETY: both pointers are valid; `create_instance` writes a live object into `raw`.
        let hr = unsafe { tip::create_instance(&iid as *const GUID, &mut raw) };
        assert!(
            hr.is_ok(),
            "the class factory must be able to create the text service"
        );
        assert!(!raw.is_null());
        assert!(live_objects() >= 1, "the object must be counted as live");
        assert_eq!(
            DllCanUnloadNow(),
            S_FALSE,
            "a live object keeps the DLL loaded"
        );
        // SAFETY: `raw` is a live object with exactly one reference (the one `create_instance` handed over).
        drop(unsafe { ITfTextInputProcessorEx::from_raw(raw) });
    }
}
