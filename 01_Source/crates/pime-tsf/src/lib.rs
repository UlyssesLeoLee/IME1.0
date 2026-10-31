//! Programming IME text service (TSF text input processor).
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

use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, S_FALSE,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::core::{GUID, HRESULT, IUnknown, Interface, Result, implement};

pub mod controller;
pub mod keymap;
pub mod layout;
pub mod popup;
pub mod register;
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

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        // SAFETY: `iid` and `object` were checked for null; COM requires them to be valid for the call.
        unsafe { tip::create_instance(iid, object) }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
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
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked for null above; COM passes pointers that are valid for the call.
    unsafe {
        *ppv = std::ptr::null_mut();
        if *rclsid != CLSID_PIME {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    }
}

/// Never unload while any object could be alive (no object counting yet).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
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

    #[test]
    fn the_clsid_matches_the_documented_prototype_value() {
        assert_eq!(
            format!("{:?}", CLSID_PIME).to_lowercase(),
            "43f95ed1-95b4-421d-8f5a-837128c2d854"
        );
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
        assert_eq!(DllCanUnloadNow(), S_FALSE);
        assert_eq!(S_OK, HRESULT(0));
    }
}
