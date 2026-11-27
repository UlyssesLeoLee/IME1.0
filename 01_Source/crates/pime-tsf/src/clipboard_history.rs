//! The clipboard history: the texts the user copied, remembered while the text service was active.
//!
//! The clipboard of Windows is one slot: copy A, copy B, and only B is there. A real history needs a **listener** -
//! a window that hears every change of the clipboard - and this module is that listener: a hidden window that never
//! takes the focus and never shows up in the taskbar, alive exactly as long as the text service is activated.
//!
//! Only text is remembered (`CF_UNICODETEXT`): a picture or a list of files that was copied is not text and never
//! enters the history. The history is the texts the user copied, the most recent first, and the row 「剪贴板」 of the
//! panel offers it under the text that is on the clipboard right now.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HMENU, RegisterClassExW, WINDOW_STYLE,
    WM_CLIPBOARDUPDATE, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::menus::{MenuAction, MenuItem};

/// The class of the window that listens for the changes of the clipboard.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.ClipboardListener");

/// How many texts the history keeps.
///
/// 32 is the default of the product: it holds a working session of copying (the snippets of the day, the texts of the
/// messages) without turning into an archive of the machine. The oldest text falls out when the limit is reached.
pub const HISTORY_LIMIT: usize = 32;

/// How many texts the menu of the panel offers.
///
/// Eight is what the menu shows without a scroll bar, and the texts of the last minutes are the ones the user looks
/// for. The whole history stays in memory: the menu is a view of it, not the history itself.
pub const MENU_LIMIT: usize = 8;

/// The label of the item that writes the text that is on the clipboard right now.
const CURRENT_LABEL: &str = "当前剪贴板";

/// How many characters of a text of the history the label of the menu shows.
const LABEL_CHARS: usize = 40;

/// The texts the user copied, the most recent first.
///
/// The history belongs to the process: the listener of the clipboard and the panel of the laser interface live in
/// the same process (the one that has the focus), so one history serves both.
static HISTORY: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Remember one text of the clipboard.
///
/// The rules of the history: a text that is empty is no text (the rule of the clipboard itself: the NUL at the end
/// of the buffer and the spaces around the text are not part of it); the same text twice in a row is one text (the
/// user copied it twice, not two texts); the history keeps the last [`HISTORY_LIMIT`] texts, the most recent first.
pub fn record(text: &str) {
    let Some(text) = crate::clipboard::clean(text) else {
        return; // an empty text is no text
    };
    let Ok(mut history) = HISTORY.lock() else {
        return; // the history is busy: the text is skipped rather than waited for
    };
    if history.front().is_some_and(|last| *last == text) {
        return; // the same text again: it is one text, not two
    }
    history.push_front(text);
    while history.len() > HISTORY_LIMIT {
        history.pop_back(); // the oldest text falls out
    }
}

/// The texts the user copied, the most recent first.
pub fn entries() -> Vec<String> {
    let Ok(history) = HISTORY.lock() else {
        return Vec::new();
    };
    history.iter().cloned().collect()
}

/// The texts the menu of the panel offers: the most recent ones.
pub fn menu_entries() -> Vec<String> {
    entries().into_iter().take(MENU_LIMIT).collect()
}

/// Forget the texts of the history.
pub fn clear() {
    if let Ok(mut history) = HISTORY.lock() {
        history.clear();
    }
}

/// The items of the menu of the row 「剪贴板」 of the panel.
///
/// The first item is the text that is on the clipboard right now - the row used to write that and nothing else - and
/// the ones after it are the texts that were copied before it, the most recent first. A history that is empty leaves
/// the menu with that one item: a menu with nothing in it is no menu.
///
/// What an item writes into the document is the text itself: no quotes, no number, no time.
pub fn menu_items() -> Vec<MenuItem> {
    let current = crate::clipboard::text().unwrap_or_default();
    let mut items = vec![MenuItem::new(CURRENT_LABEL, MenuAction::InsertText(current))];
    for text in menu_entries() {
        let label: String = text.chars().take(LABEL_CHARS).collect();
        items.push(MenuItem::new(&label, MenuAction::InsertText(text)));
    }
    items
}

/// The listener of the clipboard: a hidden window that hears every change of the clipboard.
///
/// The window is created when the text service is activated and destroyed when it is deactivated: no window of ours
/// stays in the process when the input method is not running.
pub struct ClipboardListener {
    hwnd: HWND,
}

impl ClipboardListener {
    /// Start listening: the window is created and asks Windows for the changes of the clipboard.
    ///
    /// `None` when the window could not be created or could not be registered as a listener: the input method then
    /// works as before (the row 「剪贴板」 offers the text that is on the clipboard now, and no history).
    pub fn start() -> Option<ClipboardListener> {
        register_class()?;
        let instance = crate::popup::module_instance()?;
        // SAFETY: the class is registered; the window has no style at all (it is never shown), it never takes the
        // focus and it is not shown in the taskbar.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                instance,
                None,
            )
        }
        .ok()?;
        // The window has to ask Windows for the changes of the clipboard: without this it hears nothing.
        // SAFETY: our own window.
        if unsafe { AddClipboardFormatListener(hwnd) }.is_err() {
            // SAFETY: our own window.
            unsafe { let _ = DestroyWindow(hwnd); }
            return None;
        }
        Some(ClipboardListener { hwnd })
    }

    /// Stop listening: the window leaves the changes of the clipboard and is destroyed.
    ///
    /// The window must be destroyed on the thread that created it: the text service is activated and deactivated on
    /// the thread of the host, and the listener lives exactly as long as it does.
    pub fn stop(&mut self) {
        if self.hwnd.0.is_null() {
            return;
        }
        // SAFETY: the window is ours; the messages of the clipboard are not wanted any more.
        unsafe {
            let _ = RemoveClipboardFormatListener(self.hwnd);
            let _ = DestroyWindow(self.hwnd);
        }
        self.hwnd = HWND(std::ptr::null_mut());
    }

    /// The window of the listener (for the tests).
    #[cfg(test)]
    fn handle(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for ClipboardListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The window procedure of the listener.
///
/// Everything runs inside a panic guard: a panic that unwound out of this `extern "system"` frame would abort the
/// host process.
unsafe extern "system" fn listener_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        // The clipboard changed: the text that is on it now is remembered. A clipboard that another program holds
        // open cannot be read: the change is then skipped (it is not waited for and not retried).
        crate::ui::guard(LRESULT(0), || {
            if let Some(text) = crate::clipboard::text() {
                record(&text);
            }
            LRESULT(0)
        });
        return LRESULT(0);
    }
    // SAFETY: forwarding the message unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Register the class of the listener window once per process.
///
/// The answer of the registration has to survive the call: the text service is activated and deactivated over and over
/// (every switch of the input method does it), and the listener is started again every time.
fn register_class() -> Option<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = REGISTERED.get_or_init(|| {
        // The class belongs to the module of the text service: it is gone when the module is gone, so its window
        // procedure can never point at code that is not there any more.
        let Some(instance) = crate::popup::module_instance() else {
            return false;
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(listener_wnd_proc),
            hInstance: instance,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        (unsafe { RegisterClassExW(&class) }) != 0
    });
    registered.then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, IsWindow, IsWindowVisible, GWL_EXSTYLE,
    };

    /// The lock of the tests that write into the history: it is one for the whole process, so two tests must not
    /// write into it at the same time.
    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The history keeps the texts the user copied, the most recent first.
    #[test]
    fn the_history_keeps_the_most_recent_text_first() {
        let _guard = test_lock();
        clear();
        record("first");
        record("second");
        record("third");
        assert_eq!(
            entries(),
            vec!["third".to_string(), "second".to_string(), "first".to_string()],
            "the most recent text comes first"
        );
        clear();
    }

    /// The history is bounded: the oldest texts fall out when it is full.
    #[test]
    fn the_history_forgets_the_oldest_text_when_it_is_full() {
        let _guard = test_lock();
        clear();
        for index in 0..HISTORY_LIMIT + 5 {
            record(&format!("text {index}"));
        }
        let texts = entries();
        assert_eq!(
            texts.len(),
            HISTORY_LIMIT,
            "the history keeps {HISTORY_LIMIT} texts and no more"
        );
        assert_eq!(texts[0], "text 36", "the most recent text is the first one");
        assert_eq!(
            texts[HISTORY_LIMIT - 1],
            "text 5",
            "the oldest text that is left is the one that was fifth from the end"
        );
        clear();
    }

    /// The same text twice in a row is one text: the user copied it twice, not two texts.
    #[test]
    fn the_same_text_twice_in_a_row_is_one_text() {
        let _guard = test_lock();
        clear();
        record("same");
        record("same");
        assert_eq!(
            entries(),
            vec!["same".to_string()],
            "the same text twice in a row is one text"
        );
        // a text that comes back later is a text of its own
        record("other");
        record("same");
        assert_eq!(
            entries(),
            vec!["same".to_string(), "other".to_string(), "same".to_string()],
            "only the texts next to each other are the same one"
        );
        clear();
    }

    /// An empty text is no text: it never enters the history.
    #[test]
    fn an_empty_text_is_no_text() {
        let _guard = test_lock();
        clear();
        record("");
        record("   ");
        record("\t");
        assert!(entries().is_empty(), "{:?}", entries());
        clear();
    }

    /// The texts of the history are the texts the user copied: the NUL at the end of the buffer and the spaces
    /// around the text are not part of them (the rule of the clipboard itself).
    #[test]
    fn the_texts_of_the_history_are_the_texts_the_user_copied() {
        let _guard = test_lock();
        clear();
        record("  hello\0");
        assert_eq!(
            entries(),
            vec!["hello".to_string()],
            "the NUL and the spaces around the text are not text"
        );
        clear();
    }

    /// The menu of the row 「剪贴板」 offers the text of the clipboard now and the history behind it.
    #[test]
    fn the_menu_offers_the_current_clipboard_and_the_history() {
        let _guard = test_lock();
        clear();
        record("first");
        record("second");
        let items = menu_items();
        assert_eq!(
            items.len(),
            3,
            "the clipboard of now and the two texts of the history: {items:?}"
        );
        assert_eq!(items[0].label, "当前剪贴板", "the first item is the clipboard of now");
        assert_eq!(items[1].label, "second", "the most recent text comes first");
        assert_eq!(items[2].label, "first");
        // what goes into the document is the text itself: no quotes, no number, no time
        assert_eq!(items[1].action, MenuAction::InsertText("second".to_string()));
        assert_eq!(items[2].action, MenuAction::InsertText("first".to_string()));
        clear();
    }

    /// A history that is empty leaves the menu with the text of the clipboard now: a menu with nothing in it is no
    /// menu.
    #[test]
    fn the_menu_of_the_clipboard_is_not_empty_without_a_history() {
        let _guard = test_lock();
        clear();
        let items = menu_items();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].label, "当前剪贴板");
        clear();
    }

    /// The menu shows the texts of the history and not more than the menu can hold.
    #[test]
    fn the_menu_shows_the_most_recent_texts_of_the_history() {
        let _guard = test_lock();
        clear();
        for index in 0..HISTORY_LIMIT {
            record(&format!("text {index}"));
        }
        let items = menu_items();
        assert_eq!(
            items.len(),
            MENU_LIMIT + 1,
            "the clipboard of now and {MENU_LIMIT} texts of the history: {items:?}"
        );
        assert_eq!(items[1].label, format!("text {}", HISTORY_LIMIT - 1));
        clear();
    }

    /// The listener is a hidden window that never takes the focus and is not in the taskbar, and it is gone when the
    /// text service is deactivated.
    #[test]
    fn the_listener_is_a_hidden_window_that_is_destroyed_with_the_text_service() {
        let _guard = test_lock();
        clear();
        let mut listener = ClipboardListener::start().expect("the window of the listener");
        let hwnd = listener.handle();
        // SAFETY: the window is ours.
        assert!(
            !unsafe { IsWindowVisible(hwnd) }.as_bool(),
            "the window of the listener is never shown"
        );
        // SAFETY: the window is ours.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        assert!(
            style & WS_EX_NOACTIVATE.0 != 0,
            "the window of the listener never takes the focus"
        );
        assert!(
            style & WS_EX_TOOLWINDOW.0 != 0,
            "the window of the listener is not shown in the taskbar"
        );
        listener.stop();
        // SAFETY: a plain query about a window that was ours.
        assert!(
            !unsafe { IsWindow(hwnd) }.as_bool(),
            "the window of the listener is destroyed when the listener stops"
        );
        clear();
    }
}
