//! Overlapped named-pipe I/O with hard deadlines.
//!
//! Every read, write and connect is an overlapped operation that waits on an event with the remaining time. When the
//! wait ends without completion (deadline, stop request, wait failure) the operation is cancelled with `CancelIoEx`
//! and then *waited for* with `GetOverlappedResult(bWait = TRUE)`: the `OVERLAPPED` and the buffer live on the
//! caller's stack, so the kernel must be finished with them before the function returns. A stuck peer therefore can
//! never block a caller beyond its deadline, and never leaves a dangling I/O behind.

use crate::auth::PipeName;
use crate::error::IpcError;
use crate::frame::Transport;
use crate::winsec::{OwnedHandle, SecurityDescriptor, os, win32_code};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BAD_PIPE, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING,
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_SHARE_MODE, FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, SYNCHRONIZE, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientSessionId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// Advisory pipe buffer sizes: one control frame fits, bigger frames simply need the reader to keep up.
const BUFFER_BYTES: u32 = 64 * 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn millis_ceil(d: Duration) -> u32 {
    d.as_micros().div_ceil(1000).min(0xFFFF_FFFE) as u32
}

/// A failure of an I/O call mapped to the crate's vocabulary: a vanished peer is `Closed`, the rest is an OS error.
fn io_error(op: &'static str, e: &windows::core::Error) -> IpcError {
    let code = win32_code(e);
    if [
        ERROR_BROKEN_PIPE.0,
        ERROR_PIPE_NOT_CONNECTED.0,
        ERROR_NO_DATA.0,
        ERROR_BAD_PIPE.0,
        ERROR_OPERATION_ABORTED.0,
    ]
    .contains(&code)
    {
        IpcError::Closed
    } else {
        os(op, e)
    }
}

/// An event that several threads may signal and wait on (the listener's stop request).
struct SharedEvent(OwnedHandle);

// SAFETY: `SetEvent` and the `WaitFor*` calls are thread-safe kernel operations on an event object.
unsafe impl Sync for SharedEvent {}

fn new_event() -> Result<OwnedHandle, IpcError> {
    // SAFETY: no security attributes, no name; creates a manual-reset, initially non-signalled event.
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map(OwnedHandle::new)
        .map_err(|e| os("CreateEventW", &e))
}

/// Wakes a blocked `Listener::accept` from any thread; stays signalled (a stopped listener never accepts again).
#[derive(Clone)]
pub struct StopHandle(Arc<SharedEvent>);

impl StopHandle {
    pub fn new() -> Result<StopHandle, IpcError> {
        Ok(StopHandle(Arc::new(SharedEvent(new_event()?))))
    }

    pub fn signal(&self) {
        // SAFETY: the event handle is valid for the lifetime of the Arc.
        unsafe {
            let _ = SetEvent((self.0).0.raw());
        }
    }

    pub fn is_signalled(&self) -> bool {
        // SAFETY: the event handle is valid for the lifetime of the Arc; a zero timeout only polls.
        unsafe { WaitForSingleObject(self.raw(), 0) == WAIT_OBJECT_0 }
    }

    fn raw(&self) -> HANDLE {
        (self.0).0.raw()
    }
}

/// One end of a named pipe (a server instance or a client connection).
pub struct Pipe {
    handle: OwnedHandle,
    event: OwnedHandle,
}

impl Pipe {
    fn from_handle(handle: HANDLE) -> Result<Pipe, IpcError> {
        let handle = OwnedHandle::new(handle);
        Ok(Pipe {
            handle,
            event: new_event()?,
        })
    }

    pub fn raw(&self) -> HANDLE {
        self.handle.raw()
    }

    /// Creates one server instance. `first` adds `FILE_FLAG_FIRST_PIPE_INSTANCE`: the call then fails if any process
    /// already owns the name, which is how pipe squatting is detected. Byte mode, local clients only, overlapped.
    pub fn create_server_instance(
        name: &PipeName,
        first: bool,
        sd: &SecurityDescriptor,
        max_instances: u32,
    ) -> Result<Pipe, IpcError> {
        let name_w = wide(name.as_str());
        let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let sa = sd.attributes();
        // SAFETY: `name_w` is NUL-terminated; `sa` points at a descriptor that `sd` keeps alive across the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name_w.as_ptr()),
                open,
                mode,
                max_instances,
                BUFFER_BYTES,
                BUFFER_BYTES,
                0,
                Some(&sa),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let e = windows::core::Error::from_win32();
            let code = win32_code(&e);
            return Err(
                if first && (code == ERROR_ACCESS_DENIED.0 || code == ERROR_PIPE_BUSY.0) {
                    IpcError::NameInUse
                } else {
                    os("CreateNamedPipeW", &e)
                },
            );
        }
        Pipe::from_handle(handle)
    }

    /// Opens the pipe as a client. Fails at once when no server exists (`Unavailable`: Direct input must not wait),
    /// waits for a busy instance until `deadline`. The server may not impersonate the client
    /// (`SECURITY_IDENTIFICATION`), and the handle is overlapped.
    pub fn connect_client(name: &PipeName, deadline: Instant) -> Result<Pipe, IpcError> {
        let name_w = wide(name.as_str());
        let access = FILE_READ_DATA.0 | FILE_WRITE_DATA.0 | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0;
        loop {
            // SAFETY: `name_w` is NUL-terminated; no security attributes, no template file.
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(name_w.as_ptr()),
                    access,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    HANDLE::default(),
                )
            };
            match opened {
                Ok(h) => return Pipe::from_handle(h),
                Err(e) => match win32_code(&e) {
                    c if c == ERROR_FILE_NOT_FOUND.0 => return Err(IpcError::Unavailable),
                    // the ACL refused this user (or the pipe belongs to someone else): an identity failure of the OS
                    c if c == ERROR_ACCESS_DENIED.0 => return Err(IpcError::IdentityRejected),
                    c if c == ERROR_PIPE_BUSY.0 => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(IpcError::Timeout);
                        }
                        // SAFETY: `name_w` is NUL-terminated. Bounded wait; the result only says whether to retry now.
                        unsafe {
                            let _ = WaitNamedPipeW(
                                PCWSTR(name_w.as_ptr()),
                                millis_ceil(remaining.min(Duration::from_millis(100))),
                            );
                        }
                    }
                    _ => return Err(os("CreateFileW", &e)),
                },
            }
        }
    }

    /// Waits for a client to connect to this (listening) instance, until `deadline` or until `stop` is signalled.
    /// On timeout or stop the instance stays valid and listening.
    pub fn wait_for_client(
        &mut self,
        deadline: Instant,
        stop: Option<&StopHandle>,
    ) -> Result<(), IpcError> {
        let h = self.handle.raw();
        self.overlapped(
            deadline,
            stop.map(StopHandle::raw),
            "ConnectNamedPipe",
            Some(ERROR_PIPE_CONNECTED.0),
            |ov| {
                // SAFETY: `ov` outlives the operation (see `overlapped`); the handle is a valid overlapped server instance.
                unsafe { ConnectNamedPipe(h, Some(ov)) }
            },
        )
        .map(|_| ())
    }

    /// The terminal-services session of the connected client process.
    pub fn client_session_id(&self) -> Result<u32, IpcError> {
        let mut id = 0u32;
        // SAFETY: valid server pipe handle and a valid out pointer.
        unsafe { GetNamedPipeClientSessionId(self.handle.raw(), &mut id) }
            .map_err(|e| os("GetNamedPipeClientSessionId", &e))?;
        Ok(id)
    }

    /// Runs one overlapped operation to completion or to its end of life. Returns the bytes transferred.
    /// `already_done` is a Win32 error code that means "finished without I/O" (`ERROR_PIPE_CONNECTED`).
    fn overlapped(
        &self,
        deadline: Instant,
        stop: Option<HANDLE>,
        op: &'static str,
        already_done: Option<u32>,
        start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> Result<u32, IpcError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(IpcError::Timeout);
        }
        let handle = self.handle.raw();
        let event = self.event.raw();
        // SAFETY: the event handle is owned by `self` and valid.
        unsafe { ResetEvent(event) }.map_err(|e| os("ResetEvent", &e))?;
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut transferred = 0u32;
        match start(&mut ov) {
            Ok(()) => {}
            Err(e) if Some(win32_code(&e)) == already_done => return Ok(0),
            Err(e) if win32_code(&e) == ERROR_IO_PENDING.0 => {
                let ms = millis_ceil(remaining);
                // SAFETY: both handles are valid events/handles owned by self or by the StopHandle of the caller.
                let waited = unsafe {
                    match stop {
                        Some(s) => WaitForMultipleObjects(&[event, s], false, ms),
                        None => WaitForSingleObject(event, ms),
                    }
                };
                if waited != WAIT_OBJECT_0 {
                    let stopped = stop.is_some() && waited.0 == WAIT_OBJECT_0.0 + 1;
                    // SAFETY: `ov` is the pending operation of `handle`; a failure only means it already finished.
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&ov));
                    }
                    // SAFETY: waiting here is what keeps `ov` and the caller's buffer alive until the kernel is done.
                    let done = unsafe { GetOverlappedResult(handle, &ov, &mut transferred, true) };
                    return match done {
                        // the I/O completed in the race with the cancel: keep the result
                        Ok(()) => Ok(transferred),
                        Err(e) if win32_code(&e) == ERROR_OPERATION_ABORTED.0 => Err(if stopped {
                            IpcError::Stopped
                        } else {
                            IpcError::Timeout
                        }),
                        Err(e) => Err(io_error(op, &e)),
                    };
                }
            }
            Err(e) => return Err(io_error(op, &e)),
        }
        // SAFETY: the operation has completed (synchronously or by the signalled event); `ov` is still valid.
        unsafe { GetOverlappedResult(handle, &ov, &mut transferred, false) }
            .map_err(|e| io_error(op, &e))?;
        Ok(transferred)
    }
}

impl Transport for Pipe {
    fn read_some(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, IpcError> {
        let h = self.handle.raw();
        let n = self.overlapped(deadline, None, "ReadFile", None, |ov| {
            // SAFETY: `buf` and `ov` stay alive until `overlapped` returns, and it never returns before the I/O is done.
            unsafe { ReadFile(h, Some(buf), None, Some(ov)) }
        })?;
        if n == 0 {
            Err(IpcError::Closed)
        } else {
            Ok(n as usize)
        }
    }

    fn write_all(&mut self, data: &[u8], deadline: Instant) -> Result<(), IpcError> {
        let h = self.handle.raw();
        let mut done = 0usize;
        while done < data.len() {
            let chunk = &data[done..];
            let n = self.overlapped(deadline, None, "WriteFile", None, |ov| {
                // SAFETY: as for ReadFile.
                unsafe { WriteFile(h, Some(chunk), None, Some(ov)) }
            })?;
            if n == 0 {
                return Err(IpcError::Closed);
            }
            done += n as usize;
        }
        Ok(())
    }
}
