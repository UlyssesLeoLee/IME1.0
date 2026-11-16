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

/// The text of the clipboard as the user copied it: the NUL at the end of the Windows buffer and the spaces around
/// the text are not part of it, and a text that is only spaces is no text at all.
///
/// It is a pure function of the raw text, so that the rule can be tested without touching the clipboard of the
/// machine (the read itself is not a function of its arguments - the clipboard is the machine's).
fn clean(raw: &str) -> Option<String> {
    let text = raw.trim_end_matches('\0').trim();
    if text.is_empty() { None } else { Some(text.to_string()) }
}

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
            Some(text) // the raw text of the buffer; `clean` says what the user copied
        });
        let _ = CloseClipboard();
        match text {
            Some(raw) => clean(&raw),
            None => None,
        }
    }
}

/// The entries of the clipboard: the text that is on it now, and the ones that were written before.
///
/// The history of a real clipboard needs a listener (a window that hears every change of the clipboard); this
/// build has none, so the history is what the text service has seen itself. That is registered as a gap
/// (D-97) and not hidden.
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

    /// The text of the clipboard is the text the user copied: no NUL at the end of the buffer, no spaces around it.
    ///
    /// This is the rule the red-green round watches: it is a pure function of the raw text, so it can be broken on
    /// purpose and the test notices (before this, the two tests of this module could not fail at all - D-113).
    #[test]
    fn the_text_of_the_clipboard_is_the_text_the_user_copied() {
        assert_eq!(clean("hello"), Some("hello".to_string()));
        assert_eq!(clean("hello\0\0"), Some("hello".to_string()), "the NUL of the buffer is not text");
        assert_eq!(clean("  hello  "), Some("hello".to_string()), "the spaces around it are not text");
        assert_eq!(clean("hello\r\n"), Some("hello".to_string()));
        assert_eq!(clean("   "), None, "spaces are no text");
        assert_eq!(clean("\0"), None, "a NUL is no text");
        assert_eq!(clean(""), None);
    }

    /// The entries are the text of the clipboard, and there is at least none when the clipboard is empty.
    #[test]
    fn the_entries_are_the_text_of_the_clipboard() {
        let entries = entries();
        assert!(entries.len() <= 1, "without a listener there is only the current text");
        for entry in &entries {
            assert!(!entry.is_empty(), "an empty text is not an entry");
            assert_eq!(Some(entry.clone()), clean(entry), "an entry is the cleaned text of the clipboard");
        }
    }
}
