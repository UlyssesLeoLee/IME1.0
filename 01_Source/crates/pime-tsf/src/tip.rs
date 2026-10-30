//! The text input processor (TIP): activation, key event sink, composition and commit through edit sessions.
//!
//! OWNER: the "TIP" work item. Stub: an object that activates and does nothing (every key stays with the host).

use std::cell::RefCell;
use std::ffi::c_void;

use pime_core::input::{InputMachine, LocalPredictor};
use windows::Win32::UI::TextServices::{
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows::core::{GUID, Interface, Result, implement};

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
        let _ = self.machine.borrow().state();
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        Ok(())
    }
}

/// Called by the class factory: create a TIP and return the requested interface.
///
/// # Safety
/// `iid` and `object` must be valid pointers (the class factory checks them for null).
pub(crate) unsafe fn create_instance(iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
    let tip: ITfTextInputProcessor = Tip::new().into();
    // SAFETY: forwarded from the caller's contract.
    unsafe { tip.query(iid, object).ok() }
}
