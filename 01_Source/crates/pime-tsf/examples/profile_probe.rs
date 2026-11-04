//! A throwaway probe: what does the real msctf answer when a profile GUID is registered a second time?
//!
//! The registration step of the installer calls `ITfInputProcessorProfileMgr::RegisterProfile` for our three
//! language profiles. If the profile is already registered (a second install over a registered one), does msctf
//! accept it or refuse it? This probe asks the real msctf and prints the HRESULT.
//!
//!   cargo run --example profile_probe
//!
//! It only reads the state of the text service and makes one registration call for our own profile; it never
//! activates a profile, never changes the default input method and never touches another product's registration.

use pime_tsf::{CLSID_PIME, PROFILE_EN, PROFILE_JA, PROFILE_ZH};
use windows::Win32::Foundation::{BOOL, E_FAIL, S_FALSE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr,
};

fn main() -> std::process::ExitCode {
    // SAFETY: plain COM initialisation of this thread; the matching CoUninitialize is below.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    println!("CoInitializeEx -> {hr:?}");
    // SAFETY: the CLSID is a constant and the interface is the one the class implements.
    let manager: ITfInputProcessorProfileMgr = match unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        )
    } {
        Ok(manager) => manager,
        Err(e) => {
            println!("CoCreateInstance(profile mgr) failed: {e:?}");
            unsafe { CoUninitialize() };
            return std::process::ExitCode::FAILURE;
        }
    };
    println!("the profile manager is alive");

    for (name, langid, guid) in [
        ("zh-CN", 0x0804u16, PROFILE_ZH),
        ("en-US", 0x0409u16, PROFILE_EN),
        ("ja-JP", 0x0411u16, PROFILE_JA),
    ] {
        let description: Vec<u16> = name.encode_utf16().collect();
        // SAFETY: the GUID references and the slice live for the call; the slice carries its length.
        let result = unsafe {
            manager.RegisterProfile(
                &CLSID_PIME,
                langid,
                &guid,
                &description,
                &[],
                0,
                HKL::default(),
                0,
                BOOL::from(true),
                0,
            )
        };
        match result {
            Ok(()) => println!("RegisterProfile({name}) -> S_OK"),
            Err(e) => {
                let code = e.code();
                let known = if code == S_FALSE {
                    "S_FALSE"
                } else if code == E_FAIL {
                    "E_FAIL"
                } else {
                    "other"
                };
                println!(
                    "RegisterProfile({name}) -> {e:?}  code={:#010X}  ({known})  win32={}",
                    code.0 as u32,
                    code.0 as u32
                );
            }
        }
    }
    // SAFETY: the COM apartment was initialised above.
    unsafe { CoUninitialize() };
    std::process::ExitCode::SUCCESS
}
