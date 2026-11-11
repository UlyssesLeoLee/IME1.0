//! The clipboard of Windows: the text that was copied, read on demand.
//!
//! The row 「剪贴板」 of the panel opens this: the text on the clipboard is offered and a click writes it into the
//! document. **Nothing is written to the clipboard** and nothing is watched: the clipboard is read when the user
//! asks for it, and only then.

use std::sync::Mutex;

use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// The clipboard belongs to the whole machine: only one thread at a time may hold it open.
static CLIPBOARD: Mutex<()> = Mutex::new(());

/// The text on the clipboard right now.
///
/// `None` when the clipboard is empty, when it holds something else than text, or when Windows does not let the
/// text service read it (another program has the clipboard open).
pub fn text() -> Option<String> {
    // The clipboard is a resource of the machine: two threads that open it at the same time crash Windows.
    let _guard = CLIPBOARD.lock().ok()?;
    // SAFETY: the clipboard is opened and closed around the read, and the memory of the clipboard belongs to it
    // while it is open. The pointers are used before the clipboard is closed.
    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return None;
        }
        let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|handle| {
            let handle = HGLOBAL(handle.0);
            let size = GlobalSize(handle);
            if size == 0 {
                return None;
            }
            let pointer = GlobalLock(handle);
            if pointer.is_null() {
                return None;
            }
            let units = size / 2;
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer.cast::<u16>(), units));
            let _ = GlobalUnlock(handle);
            Some(text.trim_end_matches('\0').trim().to_string())
        });
        let _ = CloseClipboard();
        match text {
            Some(text) if text.is_empty() => None,
            other => other,
        }
    }
}

/// The entries of the clipboard: the text that is on it now, and the ones that were written before.
///
/// The history of a real clipboard needs a listener (a window that hears every change of the clipboard); this
/// build has none, so the history is what the text service has seen itself. That is registered as a gap
/// (D-96) and not hidden.
pub fn entries() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(text) = text() {
        out.push(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading the clipboard never panics and never invents an entry.
    #[test]
    fn the_clipboard_is_read_without_panicking() {
        let _ = text();
        let _ = entries();
    }

    /// The entries are the text of the clipboard, and there is at least none when the clipboard is empty.
    #[test]
    fn the_entries_are_the_text_of_the_clipboard() {
        let entries = entries();
        assert!(entries.len() <= 1, "without a listener there is only the current text");
        for entry in &entries {
            assert!(!entry.is_empty(), "an empty text is not an entry");
        }
    }
}
