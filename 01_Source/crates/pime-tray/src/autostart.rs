//! Whether the tray starts with Windows.
//!
//! The user decides: the input method may stay resident, and then the tray has to be there when the machine starts.
//! The switch is a check box in the menu of the tray, and it is the only thing in this program that writes to the
//! registry on its own.
//!
//! It writes **one** value, in the Run key of the **current user**:
//!
//! ```text
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//!     "IME1.0" = "C:\Program Files\IME1.0\pime-tray.exe"
//! ```
//!
//! Why the current user and not the machine: the Run key of the machine needs administrator rights and would start
//! the tray for every account on the machine, while the input method belongs to the person who is using it. Why the
//! path in quotes: a path with a space in it (`C:\Program Files\...`) must be quoted, or Windows cuts it at the space.
//!
//! Nothing else is written and nothing is read from the command: the value is the path of this program, and the
//! program is the only thing that decides what it does when it starts.

use std::path::Path;

/// The key Windows reads the programs of the current user from.
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// The name of our value in that key.
pub const VALUE_NAME: &str = "IME1.0";

/// The command Windows runs to start the tray: the path in quotes, so that a path with a space in it works.
pub fn command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Whether the Run key of the current user already names this program.
pub fn is_enabled() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, RegCloseKey, RegGetValueW, RegOpenKeyExW, RRF_RT_REG_SZ,
    };
    let key: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = VALUE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: the key name is NUL-terminated and the handle is a live out pointer.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut handle,
        )
    };
    if opened != ERROR_SUCCESS {
        return false;
    }
    let mut size = 0u32;
    // SAFETY: the key is live and the value name is NUL-terminated; nothing is read yet.
    let read = unsafe {
        RegGetValueW(
            handle,
            PCWSTR::null(),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
    read == ERROR_SUCCESS && size > 0
}

/// Start the tray with Windows: write the path of this program into the Run key of the current user.
pub fn enable(exe: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegSetValueExW,
    };
    let key: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = VALUE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: the key name is NUL-terminated and the handle is a live out pointer.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut handle,
            None,
        )
    };
    if created != ERROR_SUCCESS {
        return Err("打不开 Windows 的启动项（需要当前用户的权限）".to_string());
    }
    let data: Vec<u8> = command(exe)
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // SAFETY: the key is live, the name is NUL-terminated and the data is the text with its terminator.
    let written = unsafe {
        RegSetValueExW(
            handle,
            PCWSTR(value.as_ptr()),
            0,
            REG_SZ,
            Some(data.as_slice()),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
    if written != ERROR_SUCCESS {
        return Err("写不进 Windows 的启动项".to_string());
    }
    Ok(())
}

/// Do not start the tray with Windows: remove our value from the Run key of the current user.
///
/// A value that is not there is not an error: the state the user wants is the state that is already there.
pub fn disable() -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_SUCCESS, ERROR_FILE_NOT_FOUND};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    };
    let key: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = VALUE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: the key name is NUL-terminated and the handle is a live out pointer.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut handle,
        )
    };
    if opened != ERROR_SUCCESS {
        return Err("打不开 Windows 的启动项（需要当前用户的权限）".to_string());
    }
    // SAFETY: the key is live and the value name is NUL-terminated.
    let deleted = unsafe { RegDeleteValueW(handle, PCWSTR(value.as_ptr())) };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
    if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err("删不掉 Windows 的启动项".to_string())
    }
}

/// The path of this program, as Windows started it.
pub fn own_exe() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_quotes_the_path_so_that_a_space_in_it_works() {
        let command = command(Path::new(r"C:\Program Files\IME1.0\pime-tray.exe"));
        assert_eq!(command, r#""C:\Program Files\IME1.0\pime-tray.exe""#);
    }

    #[test]
    fn the_command_quotes_a_path_without_a_space_too() {
        let command = command(Path::new(r"C:\IME1.0\pime-tray.exe"));
        assert_eq!(command, r#""C:\IME1.0\pime-tray.exe""#);
    }

    #[test]
    fn the_value_name_is_the_name_of_the_product() {
        assert_eq!(VALUE_NAME, "IME1.0");
        assert!(RUN_KEY.ends_with(r"\Run"));
    }
}
