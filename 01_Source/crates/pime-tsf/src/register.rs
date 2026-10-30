//! Registration of the COM server and the TSF profile: `DllRegisterServer` / `DllUnregisterServer`.
//!
//! OWNER: the "REGISTRATION" work item. Stub: refuses (E_NOTIMPL). NOTHING in the automated tests may write to the real
//! registry or call the real TSF registration APIs: the work is expressed as a pure plan (`Vec<RegistryOp>` plus profile
//! and category lists) that tests inspect, and a backend trait with an in-memory fake.

use windows::Win32::Foundation::E_NOTIMPL;
use windows::core::HRESULT;

pub(crate) fn register_server() -> HRESULT {
    E_NOTIMPL
}

pub(crate) fn unregister_server() -> HRESULT {
    E_NOTIMPL
}
