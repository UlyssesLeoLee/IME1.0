//! The Win32 side: the production [`Backend`], UAC elevation, message boxes and console handling.
//!
//! Every `unsafe` block is a call into Win32 with buffers owned by the caller for the whole call. None of this code is
//! reached by the automated tests except through a temporary folder (`RealBackend` file operations with the
//! `--no-register --no-arp --no-vscode` plan); the registry, `MoveFileEx` and elevation paths are exercised only by a
//! person on a disposable machine.

use crate::backend::{Backend, RegValue};
use crate::text::Lang;
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, HANDLE, HWND, WIN32_ERROR,
};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{
    MOVE_FILE_FLAGS, MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW,
};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_DWORD, REG_MULTI_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK,
    MB_SETFOREGROUND, MB_YESNO, MESSAGEBOX_STYLE, MessageBoxW, SW_HIDE,
};
use windows::core::{HRESULT, PCWSTR, PWSTR, w};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// regsvr32 and `code` finish in seconds; this only stops a hung child from hanging the installer forever.
const CHILD_TIMEOUT: Duration = Duration::from_secs(300);

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn win_err(e: WIN32_ERROR) -> io::Error {
    io::Error::from_raw_os_error(e.0 as i32)
}

/// Windows virus scanners briefly hold freshly written files; retry the operations that scanners can collide with.
fn retry_in_use<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Err(e) if crate::backend::is_in_use(&e) && attempt < 3 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(150));
            }
            other => return other,
        }
    }
}

pub struct RealBackend {
    allow_reboot_schedule: bool,
}

impl RealBackend {
    pub fn new() -> RealBackend {
        RealBackend {
            allow_reboot_schedule: true,
        }
    }

    /// For tests that work in a temporary folder: a locked file there must never reach the HKLM pending-rename list.
    pub fn without_reboot_schedule() -> RealBackend {
        RealBackend {
            allow_reboot_schedule: false,
        }
    }
}

impl Default for RealBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW/RegCreateKeyExW and is closed exactly once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(
    key: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<Key>, io::Error> {
    let name = wide(OsStr::new(key));
    let mut hkey = HKEY::default();
    // SAFETY: `name` is NUL-terminated and outlives the call; `hkey` is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(name.as_ptr()),
            0,
            access | KEY_WOW64_64KEY,
            &mut hkey,
        )
    };
    match status {
        WIN32_ERROR(0) => Ok(Some(Key(hkey))),
        s if s == ERROR_FILE_NOT_FOUND => Ok(None),
        s => Err(win_err(s)),
    }
}

/// Under HKLM. `MoveFileEx(DELAY_UNTIL_REBOOT)` appends to `PendingFileRenameOperations`; Windows 11 also has a
/// `PendingFileRenameOperations2` (written by servicing), which is read as well. Neither is ever written to here.
const SESSION_MANAGER: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager";
const PENDING_VALUES: [&str; 2] = [
    "PendingFileRenameOperations",
    "PendingFileRenameOperations2",
];

/// The UTF-16 units of a REG_MULTI_SZ value; a missing value (or one of another type) is empty.
fn read_multi_sz(key: &Key, name: &str) -> io::Result<Vec<u16>> {
    let wname = wide(OsStr::new(name));
    let (mut kind, mut len) = (REG_VALUE_TYPE(0), 0u32);
    // SAFETY: `key` is open; `wname` is NUL-terminated; the out pointers refer to live locals (no data buffer: size query).
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(wname.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut len),
        )
    };
    match status {
        WIN32_ERROR(0) => {}
        s if s == ERROR_FILE_NOT_FOUND => return Ok(Vec::new()),
        s => return Err(win_err(s)),
    }
    if kind != REG_MULTI_SZ {
        return Ok(Vec::new());
    }
    let mut data = vec![0u8; len as usize + 2];
    let mut size = data.len() as u32;
    // SAFETY: the buffer is `size` bytes long and `size` is passed in; the other pointers are as above.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(wname.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(win_err(status));
    }
    data.truncate(size as usize);
    Ok(data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect())
}

/// Splits a REG_MULTI_SZ of pending renames (source, destination, source, destination ...; the destination of a delete
/// is empty) into paths: the `\??\` prefix and the `!` (replace existing) marker are removed, empty strings dropped.
pub fn parse_pending_renames(units: &[u16]) -> Vec<String> {
    units
        .split(|&u| u == 0)
        .map(String::from_utf16_lossy)
        .map(|s| {
            s.trim_start_matches('!')
                .trim_start_matches(r"\??\")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn utf16_value(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

impl Backend for RealBackend {
    fn exists(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn is_dir(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_dir())
    }

    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn dir_is_empty(&self, path: &Path) -> io::Result<bool> {
        Ok(std::fs::read_dir(path)?.next().is_none())
    }

    fn create_dir(&mut self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn write_file(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
        std::fs::write(path, data)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        // Not `std::fs::rename`: on Windows that replaces an existing destination, and an existing destination here is
        // a leftover (an earlier file still waiting for the restart) that must not be overwritten.
        let (from, to) = (wide(from.as_os_str()), wide(to.as_os_str()));
        retry_in_use(|| {
            // SAFETY: both names are NUL-terminated and outlive the call; flags 0 mean "rename, never replace".
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVE_FILE_FLAGS(0),
                )
            }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
        })
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        retry_in_use(|| std::fs::remove_file(path))
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }

    fn schedule_delete_on_reboot(&mut self, path: &Path) -> io::Result<()> {
        if !self.allow_reboot_schedule {
            return Err(io::Error::other(
                "scheduling deletions at restart is disabled for this backend",
            ));
        }
        let name = wide(path.as_os_str());
        // SAFETY: `name` is NUL-terminated and outlives the call; a null destination means "delete".
        unsafe {
            MoveFileExW(
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                MOVEFILE_DELAY_UNTIL_REBOOT,
            )
        }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
    }

    fn pending_file_renames(&self) -> io::Result<Vec<String>> {
        if !self.allow_reboot_schedule {
            return Ok(Vec::new()); // the temp-folder tests never read HKLM
        }
        let Some(k) = open(SESSION_MANAGER, KEY_READ)? else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for value in PENDING_VALUES {
            entries.extend(parse_pending_renames(&read_multi_sz(&k, value)?));
        }
        Ok(entries)
    }

    fn reg_read_key(&self, key: &str) -> io::Result<Option<Vec<(String, RegValue)>>> {
        let Some(k) = open(key, KEY_READ)? else {
            return Ok(None);
        };
        let (mut count, mut max_name, mut max_data) = (0u32, 0u32, 0u32);
        // SAFETY: `k` is an open key; every out pointer refers to a live local.
        let status = unsafe {
            RegQueryInfoKeyW(
                k.0,
                PWSTR::null(),
                None,
                None,
                None,
                None,
                None,
                Some(&mut count),
                Some(&mut max_name),
                Some(&mut max_data),
                None,
                None,
            )
        };
        if status != WIN32_ERROR(0) {
            return Err(win_err(status));
        }
        let mut values = Vec::new();
        for index in 0..count {
            let mut name = vec![0u16; max_name as usize + 1];
            let mut data = vec![0u8; max_data as usize + 2];
            let (mut name_len, mut data_len, mut kind) =
                (name.len() as u32, data.len() as u32, 0u32);
            // SAFETY: the buffers are sized from RegQueryInfoKeyW and the lengths passed in match their real sizes.
            let status = unsafe {
                RegEnumValueW(
                    k.0,
                    index,
                    PWSTR(name.as_mut_ptr()),
                    &mut name_len,
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if status != WIN32_ERROR(0) {
                return Err(win_err(status));
            }
            let name = String::from_utf16_lossy(&name[..name_len as usize]);
            let data = &data[..data_len as usize];
            match REG_VALUE_TYPE(kind) {
                REG_SZ if !data.is_empty() => values.push((name, RegValue::Str(utf16_value(data)))),
                REG_DWORD if data.len() >= 4 => values.push((
                    name,
                    RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
                )),
                _ => {}
            }
        }
        Ok(Some(values))
    }

    fn reg_write_key(&mut self, key: &str, values: &[(String, RegValue)]) -> io::Result<()> {
        let name = wide(OsStr::new(key));
        let mut hkey = HKEY::default();
        // SAFETY: `name` is NUL-terminated and outlives the call; `hkey` is a valid out pointer.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(name.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_WOW64_64KEY,
                None,
                &mut hkey,
                None,
            )
        };
        if status != WIN32_ERROR(0) {
            return Err(win_err(status));
        }
        let k = Key(hkey);
        for (value_name, value) in values {
            let wname = wide(OsStr::new(value_name));
            let (kind, bytes): (REG_VALUE_TYPE, Vec<u8>) = match value {
                RegValue::Str(s) => (
                    REG_SZ,
                    wide(OsStr::new(s))
                        .iter()
                        .flat_map(|u| u.to_le_bytes())
                        .collect(),
                ),
                RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            };
            // SAFETY: `k` is an open key with KEY_SET_VALUE; `wname` is NUL-terminated; `bytes` outlives the call.
            let status =
                unsafe { RegSetValueExW(k.0, PCWSTR(wname.as_ptr()), 0, kind, Some(&bytes)) };
            if status != WIN32_ERROR(0) {
                return Err(win_err(status));
            }
        }
        Ok(())
    }

    fn reg_delete_key(&mut self, key: &str) -> io::Result<()> {
        let name = wide(OsStr::new(key));
        // SAFETY: `name` is NUL-terminated and outlives the call. With a sub-key name the key itself is deleted too.
        let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(name.as_ptr())) };
        match status {
            WIN32_ERROR(0) => Ok(()),
            s if s == ERROR_FILE_NOT_FOUND => Ok(()),
            s => Err(win_err(s)),
        }
    }

    fn run(&mut self, program: &Path, args: &[String]) -> io::Result<i32> {
        let mut child = Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.code().unwrap_or(-1));
            }
            if started.elapsed() > CHILD_TIMEOUT {
                let _ = child.kill();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the program did not finish within 5 minutes and was stopped",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn find_vscode(&self) -> Option<PathBuf> {
        let var = |n: &str| std::env::var_os(n).map(PathBuf::from);
        find_code(
            std::env::var_os("PATH"),
            var("LOCALAPPDATA"),
            var("ProgramFiles"),
            &|p| p.is_file(),
        )
    }
}

/// VS Code's launcher is `code.cmd` (a batch file), which `Command` does not find by the bare name `code`, so it is
/// looked up explicitly: first on PATH, then in the two default install folders.
pub fn find_code(
    path_var: Option<OsString>,
    local_app_data: Option<PathBuf>,
    program_files: Option<PathBuf>,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let on_path = path_var
        .iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join("code.cmd"));
    let defaults = [
        local_app_data.map(|d| d.join(r"Programs\Microsoft VS Code\bin\code.cmd")),
        program_files.map(|d| d.join(r"Microsoft VS Code\bin\code.cmd")),
    ];
    on_path
        .chain(defaults.into_iter().flatten())
        .find(|p| is_file(p))
}

pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle from GetCurrentProcess needs no closing; `token` is a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the buffer is a TOKEN_ELEVATION and its size is passed; the token handle is closed exactly once.
    let ok = unsafe {
        let r = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        r.is_ok()
    };
    ok && elevation.TokenIsElevated != 0
}

/// Quotes one argument for a Windows command line (the rules of `CommandLineToArgvW`).
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

pub fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

pub enum ElevationError {
    /// The user said no at the UAC prompt.
    Declined,
    Failed(String),
}

/// Starts `exe args` elevated ("runas": the UAC prompt), hidden, and waits for it. Returns its exit code.
pub fn run_elevated(exe: &Path, args: &[String]) -> Result<u32, ElevationError> {
    let file = wide(exe.as_os_str());
    let params = wide(OsStr::new(&command_line(args)));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised, its string pointers outlive the call, and `hProcess` (returned because of
    // SEE_MASK_NOCLOSEPROCESS) is waited on and closed exactly once below.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                ElevationError::Declined
            } else {
                ElevationError::Failed(e.message())
            });
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let got = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        got.map(|()| code)
            .map_err(|e| ElevationError::Failed(e.message()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Question,
    Info,
    Warning,
    Error,
}

/// A message box. With `ask` it has Yes/No (default No) and returns whether the user chose Yes.
pub fn message_box(title: &str, text: &str, icon: Icon, ask: bool) -> bool {
    let icon_flag: MESSAGEBOX_STYLE = match icon {
        Icon::Question => MB_ICONQUESTION,
        Icon::Info => MB_ICONINFORMATION,
        Icon::Warning => MB_ICONWARNING,
        Icon::Error => MB_ICONERROR,
    };
    let style = icon_flag | MB_SETFOREGROUND | if ask { MB_YESNO | MB_DEFBUTTON2 } else { MB_OK };
    let (text, title) = (wide(OsStr::new(text)), wide(OsStr::new(title)));
    // SAFETY: both strings are NUL-terminated and outlive the call; a null owner window is allowed.
    let answer = unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
        )
    };
    answer == IDYES
}

pub fn ui_language() -> Lang {
    // SAFETY: no arguments, no pointers.
    let id = unsafe { GetUserDefaultUILanguage() };
    match id & 0x3ff {
        0x04 => Lang::ZhCn,
        0x11 => Lang::Ja,
        _ => Lang::En,
    }
}

/// When the setup was started by a double-click it owns a console window nobody asked for: close it. When it was
/// started from a shell (two processes attached) the console stays, so output and exit codes work.
pub fn free_console_if_alone() {
    let mut list = [0u32; 2];
    // SAFETY: the slice is valid for the call.
    let attached = unsafe { GetConsoleProcessList(&mut list) };
    if attached == 1 {
        // SAFETY: no arguments; failing just leaves the console open.
        let _ = unsafe { FreeConsole() };
    }
}

pub fn current_exe_bytes() -> io::Result<(PathBuf, Vec<u8>)> {
    let exe = std::env::current_exe()?;
    let bytes = std::fs::read(&exe)?;
    Ok((exe, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ut_061_arguments_are_quoted_for_the_elevated_child() {
        assert_eq!(quote_arg("--silent"), "--silent");
        assert_eq!(
            quote_arg(r"C:\Program Files\Programming IME"),
            r#""C:\Program Files\Programming IME""#
        );
        assert_eq!(quote_arg(""), r#""""#);
        // a trailing backslash inside quotes must be doubled or it would swallow the closing quote
        assert_eq!(quote_arg(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(
            command_line(&["--prefix".into(), r"D:\日本語 フォルダ".into()]),
            r#"--prefix "D:\日本語 フォルダ""#
        );
    }

    fn multi_sz(items: &[&str]) -> Vec<u16> {
        let mut v: Vec<u16> = items
            .iter()
            .flat_map(|s| s.encode_utf16().chain(Some(0)))
            .collect();
        v.push(0); // the empty string that ends a REG_MULTI_SZ
        v
    }

    #[test]
    fn st_035_review_1_the_pending_rename_list_is_read_as_source_and_destination_paths() {
        // a delete is (source, empty destination); a replacing rename is (source, "!" + destination)
        let raw = multi_sz(&[
            r"\??\C:\Program Files\Programming IME\uninstall.exe",
            "",
            r"\??\C:\a\old.dll",
            r"!\??\C:\a\new.dll",
            r"\??\D:\日本語\x.dll",
            "",
        ]);
        assert_eq!(
            parse_pending_renames(&raw),
            [
                r"C:\Program Files\Programming IME\uninstall.exe",
                r"C:\a\old.dll",
                r"C:\a\new.dll",
                r"D:\日本語\x.dll"
            ]
        );
        assert!(parse_pending_renames(&[]).is_empty());
        assert!(parse_pending_renames(&[0, 0]).is_empty());
    }

    #[test]
    fn st_035_code_is_found_as_code_cmd_on_path_then_in_the_default_folders() {
        let exists = |p: &Path| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(r"C:\Tools\bin\code.cmd")
        };
        let path = std::env::join_paths([r"C:\Windows", r"C:\Tools\bin"]).unwrap();
        assert_eq!(
            find_code(Some(path), None, None, &exists),
            Some(PathBuf::from(r"C:\Tools\bin\code.cmd"))
        );

        let default_user = |p: &Path| p.ends_with(r"Programs\Microsoft VS Code\bin\code.cmd");
        let found = find_code(
            None,
            Some(PathBuf::from(r"C:\Users\me\AppData\Local")),
            Some(PathBuf::from(r"C:\Program Files")),
            &default_user,
        );
        assert_eq!(
            found,
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\bin\code.cmd"
            ))
        );
        assert_eq!(find_code(None, None, None, &|_| false), None);
    }
}
