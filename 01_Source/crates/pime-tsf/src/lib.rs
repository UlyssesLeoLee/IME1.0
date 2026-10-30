//! Programming IME text service (TSF text input processor).
//!
//! STATUS (milestone M2): the DLL builds with the pure-Rust toolchain, exports the four COM entry points and links
//! `pime-core` in-process. It is NOT a working input method yet: the key sink, the composition, the candidate window and
//! the registration (`DllRegisterServer`) are the next milestones and are deliberately not faked here.
//!
//! Safety rules for this crate: every `unsafe` block is a COM/FFI boundary and carries a reason; a panic must never
//! unwind into the host application (`panic = "abort"` in the release profile, and no `unwrap` on host input).

use std::cell::RefCell;
use std::ffi::c_void;

use pime_core::input::{InputMachine, LocalPredictor};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOTIMPL, E_POINTER, S_FALSE,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows::core::{GUID, HRESULT, IUnknown, Interface, Result, implement};

/// Prototype GUIDs (the same values as the C++ skeleton in `windows/tsf`); to be replaced by governed product ids.
pub const CLSID_PIME: GUID = GUID::from_u128(0x43f95ed1_95b4_421d_8f5a_837128c2d854);

/// The text input processor: one per thread that activates the profile. TSF calls it on the thread's apartment, so
/// interior mutability without locking is enough.
#[implement(ITfTextInputProcessor)]
struct Tip {
    machine: RefCell<InputMachine<LocalPredictor>>,
}

impl Tip {
    fn new() -> Tip {
        Tip {
            machine: RefCell::new(InputMachine::new(LocalPredictor, "rust")),
        }
    }
}

impl ITfTextInputProcessor_Impl for Tip_Impl {
    fn Activate(&self, _ptim: Option<&ITfThreadMgr>, _tid: u32) -> Result<()> {
        // M3: advise the key-event sink and the thread-manager sink here.
        let _ = self.machine.borrow().state();
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        Ok(())
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
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let tip: ITfTextInputProcessor = Tip::new().into();
        // SAFETY: `iid` and `object` were checked for null; COM requires them to be valid for the call.
        unsafe { tip.query(iid, object).ok() }
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

/// Never unload while any object could be alive (no object counting yet: M3).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

/// Registration of the CLSID, the language profiles (zh-CN, en-US, ja-JP) and the TSF categories is milestone M4.
#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    E_NOTIMPL
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
