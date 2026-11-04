//! The logo of the input method in the notification area (the corner of the screen).
//!
//! Windows decides where the icon sits, so it can never cover the clock - the thing that happens when a program
//! draws its own logo in the corner of the desktop.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, DestroyMenu, HICON, HMENU, LR_DEFAULTCOLOR,
};

/// The message the icon sends when the user clicks it.
pub const WM_TRAY: u32 = 0x8000 + 20;

/// `NIM_ADD`.
const NIM_ADD: u32 = 0;
/// `NIM_DELETE`.
const NIM_DELETE: u32 = 2;
/// `NIM_SETVERSION`.
const NIM_SETVERSION: u32 = 4;

/// `NIF_MESSAGE`.
const NIF_MESSAGE: u32 = 0x0000_0001;
/// `NIF_ICON`.
const NIF_ICON: u32 = 0x0000_0002;
/// `NIF_TIP`.
const NIF_TIP: u32 = 0x0000_0004;

/// `NOTIFYICON_VERSION_4`.
const NOTIFYICON_VERSION_4: u32 = 4;

/// The data of one icon in the notification area (`NOTIFYICONDATAW`).
///
/// The `windows` crate of this version does not have it, so it is written out here. The layout is the one of
/// `shellapi.h`; the compiler checks that the fields are in the right order and that the structure has the size
/// Windows expects.
#[repr(C)]
#[allow(non_snake_case)]
pub struct NotifyIconData {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: HICON,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uVersion: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: windows::core::GUID,
    pub hBalloonIcon: HICON,
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, pnid: *const NotifyIconData) -> i32;
}

/// The icon of the input method in the notification area.
pub struct Tray {
    data: NotifyIconData,
    icon: HICON,
}

impl Tray {
    /// Put the logo into the notification area.
    ///
    /// None when the area is not there (a session without a shell) or when Windows refuses the icon.
    pub fn add(hwnd: HWND, id: u32, tip: &str, icon_bytes: &[u8]) -> Option<Tray> {
        // SAFETY: the bytes are the picture of an icon, as the resource functions want it.
        let icon = unsafe {
            CreateIconFromResourceEx(
                icon_bytes,
                true,
                0x0003_0000,
                0,
                0,
                LR_DEFAULTCOLOR,
            )
        }
        .ok()?;
        let mut data = NotifyIconData {
            cbSize: std::mem::size_of::<NotifyIconData>() as u32,
            hWnd: hwnd,
            uID: id,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            szTip: [0; 128],
            dwState: 0,
            dwStateMask: 0,
            szInfo: [0; 256],
            uVersion: 0,
            szInfoTitle: [0; 64],
            dwInfoFlags: 0,
            guidItem: windows::core::GUID::zeroed(),
            hBalloonIcon: HICON::default(),
        };
        let tip: Vec<u16> = tip.encode_utf16().take(127).collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
        // SAFETY: the data is a live local and the call only reads it.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        if !added {
            // SAFETY: the icon was created above and is not used any more.
            let _ = unsafe { DestroyIcon(icon) };
            return None;
        }
        data.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: the data is a live local; the version tells Windows which events it sends.
        let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
        Some(Tray { data, icon })
    }

    /// Take the logo out of the notification area.
    pub fn remove(&self) {
        // SAFETY: the data is a live local that describes the icon that is there.
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        // SAFETY: the icon was created by this struct and is not used any more.
        let _ = unsafe { DestroyIcon(self.icon) };
    }
}

/// The place the menu opens at, under the mouse.
pub fn cursor_position() -> POINT {
    let mut point = POINT::default();
    // SAFETY: the point is a live local.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
    }
    point
}

/// Close a menu that was opened for the user.
pub fn close_menu(menu: HMENU) {
    // SAFETY: the menu was created by this program.
    let _ = unsafe { DestroyMenu(menu) };
}
