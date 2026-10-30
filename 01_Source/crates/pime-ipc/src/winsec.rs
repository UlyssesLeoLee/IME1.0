//! Windows security helpers: the SID of the process token, the security descriptor of the pipe, the logon session and
//! the system random number generator. Every `unsafe` block is one call with its safety argument next to it.

use crate::auth::{Entropy, sddl_for_sid};
use crate::error::IpcError;
use std::ffi::c_void;
use windows::Win32::Foundation::{BOOL, CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

/// The Win32 error number inside a `windows::core::Error` (`HRESULT_FROM_WIN32` values carry it in the low word).
pub fn win32_code(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr >> 16 == 0x8007 { hr & 0xFFFF } else { hr }
}

pub(crate) fn os(op: &'static str, e: &windows::core::Error) -> IpcError {
    IpcError::Os {
        op,
        code: win32_code(e),
    }
}

/// A kernel handle that is closed exactly once, when the owner is dropped.
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub fn new(h: HANDLE) -> OwnedHandle {
        OwnedHandle(h)
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was valid when wrapped and this is the only owner; a failed close leaves nothing to undo.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// SAFETY: a kernel handle is a process-wide identifier; moving it to another thread is what the API is designed for.
// `Sync` is deliberately not implemented: callers share events only through `Arc` where the type says so.
unsafe impl Send for OwnedHandle {}

/// Memory the system allocated with `LocalAlloc` (SID strings, security descriptors), freed on drop.
struct LocalMem(*mut c_void);

impl Drop for LocalMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Win32 call documented to return LocalAlloc memory, and is freed once.
            unsafe {
                LocalFree(HLOCAL(self.0));
            }
        }
    }
}

/// The user SID of the process token as a string (`S-1-5-21-...`).
pub fn current_user_sid_string() -> Result<String, IpcError> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a valid out pointer; the current-process pseudo handle needs no cleanup.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| os("OpenProcessToken", &e))?;
    let token = OwnedHandle::new(token);

    let mut needed = 0u32;
    // SAFETY: a null buffer of length 0 is the documented size query; it fails with ERROR_INSUFFICIENT_BUFFER on purpose.
    let _ = unsafe { GetTokenInformation(token.raw(), TokenUser, None, 0, &mut needed) };
    if needed == 0 || needed > 4096 {
        return Err(IpcError::Os {
            op: "GetTokenInformation(size)",
            code: needed,
        });
    }
    // u64 elements: the buffer is 8-aligned, as TOKEN_USER (it holds pointers) requires
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes and is suitably aligned.
    unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| os("GetTokenInformation", &e))?;
    // SAFETY: the call above filled `buf` with a TOKEN_USER whose SID pointer targets the same buffer, still alive here.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    let mut wide = PWSTR::null();
    // SAFETY: `sid` is a valid SID (see above); `wide` is a valid out pointer that receives LocalAlloc memory.
    unsafe { ConvertSidToStringSidW(sid, &mut wide) }
        .map_err(|e| os("ConvertSidToStringSidW", &e))?;
    let _free = LocalMem(wide.0.cast());
    // SAFETY: the call succeeded, so `wide` is a NUL-terminated UTF-16 string that `_free` keeps alive until the end.
    unsafe { wide.to_string() }.map_err(|_| IpcError::InvalidArgument("SID string is not UTF-16"))
}

/// The security descriptor of the pipe, built from an SDDL string. Freed on drop.
pub struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    pub fn from_sddl(sddl: &str) -> Result<SecurityDescriptor, IpcError> {
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut ptr = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `ptr` is a valid out pointer.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut ptr,
                None,
            )
        }
        .map_err(|e| os("ConvertStringSecurityDescriptorToSecurityDescriptorW", &e))?;
        Ok(SecurityDescriptor { ptr })
    }

    /// The descriptor that lets only the current user's SID use the pipe (`D:P(A;;mask;;;SID)`).
    pub fn for_current_user() -> Result<SecurityDescriptor, IpcError> {
        SecurityDescriptor::from_sddl(&sddl_for_sid(&current_user_sid_string()?)?)
    }

    /// `SECURITY_ATTRIBUTES` pointing at this descriptor (not inheritable). Valid while `self` lives.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.ptr.0,
            bInheritHandle: BOOL(0),
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        drop(LocalMem(self.ptr.0));
    }
}

// SAFETY: the descriptor is immutable after construction and only read by the system while a pipe is created.
unsafe impl Send for SecurityDescriptor {}

/// The DACL of a kernel object as an SDDL string; used to verify that the pipe carries the ACL we asked for.
pub fn dacl_sddl_of_handle(handle: HANDLE) -> Result<String, IpcError> {
    let mut psd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `handle` is a valid kernel handle with READ_CONTROL (the pipe creator has it); `psd` is a valid out pointer.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut psd),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(IpcError::Os {
            op: "GetSecurityInfo",
            code: status.0,
        });
    }
    let _free_sd = LocalMem(psd.0);
    let mut wide = PWSTR::null();
    // SAFETY: `psd` is the valid descriptor returned above; `wide` is a valid out pointer.
    unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            psd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut wide,
            None,
        )
    }
    .map_err(|e| os("ConvertSecurityDescriptorToStringSecurityDescriptorW", &e))?;
    let _free_str = LocalMem(wide.0.cast());
    // SAFETY: the call succeeded, so `wide` is a NUL-terminated UTF-16 string kept alive by `_free_str`.
    unsafe { wide.to_string() }.map_err(|_| IpcError::InvalidArgument("SDDL is not UTF-16"))
}

/// The terminal-services session of this process (DD 2.1: the pipe also refuses other logon sessions).
pub fn current_session_id() -> Result<u32, IpcError> {
    let mut id = 0u32;
    // SAFETY: `id` is a valid out pointer; the current process id is always valid for the query.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) }
        .map_err(|e| os("ProcessIdToSessionId", &e))?;
    Ok(id)
}

/// The system-preferred CSPRNG (`BCryptGenRandom`): the only entropy source of the crate.
pub struct SystemEntropy;

impl Entropy for SystemEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), IpcError> {
        // SAFETY: `buf` is a valid writable slice; with BCRYPT_USE_SYSTEM_PREFERRED_RNG the algorithm handle must be null.
        let status = unsafe {
            BCryptGenRandom(
                BCRYPT_ALG_HANDLE::default(),
                buf,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status.0 < 0 {
            // never fall back to a weaker source: no nonce is better than a predictable one
            return Err(IpcError::Os {
                op: "BCryptGenRandom",
                code: status.0 as u32,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ut_ipc_current_user_sid_is_a_plain_sid_string() {
        let sid = current_user_sid_string().expect("process token SID");
        crate::auth::validate_sid(&sid).expect("a real SID passes the strict validator");
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(sid, current_user_sid_string().unwrap());
    }

    #[test]
    fn ut_ipc_sddl_of_the_current_user_converts_to_a_descriptor_and_back() {
        let sid = current_user_sid_string().unwrap();
        let sddl = sddl_for_sid(&sid).unwrap();
        let sd =
            SecurityDescriptor::from_sddl(&sddl).expect("the built SDDL is accepted by Windows");
        assert_eq!(sd.attributes().bInheritHandle, BOOL(0));
        assert!(!sd.attributes().lpSecurityDescriptor.is_null());
        // a hostile string is refused by Windows too (defence in depth: the builder already refuses it)
        assert!(SecurityDescriptor::from_sddl("D:P(A;;GA;;;").is_err());
    }

    #[test]
    fn ut_ipc_system_entropy_is_not_constant_and_fills_every_byte() {
        let mut e = SystemEntropy;
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        e.fill(&mut a).unwrap();
        e.fill(&mut b).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
        e.fill(&mut []).unwrap();
    }

    #[test]
    fn ut_ipc_session_id_is_available() {
        // session 0 is services, 1.. are interactive sessions; on a dev machine/CI runner any value is valid
        current_session_id().expect("ProcessIdToSessionId");
    }
}
