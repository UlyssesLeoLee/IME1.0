//! The menus of the floating bar: a small popup list (the effort of the AI, the skin, the programming switches) and a
//! searchable list (the language switcher).
//!
//! A menu is a window of its own: it is the only way to show a list that can be searched, that can open above the
//! button when there is no room below, and that can be operated with the keyboard. It takes the focus while it is open
//! (the user is typing in it) and gives it back to the editor when it closes.
//!
//! The filtering, the placement and the hit test are pure functions ([`filter`], [`placement`], [`hit_test`]) and are
//! tested without Windows; the Win32 part only draws what those functions decided.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteObject,
    ETO_CLIPPED, EndPaint, ExtTextOutW, FillRect, HDC, HFONT, HGDIOBJ, InvalidateRect, PAINTSTRUCT,
    SRCCOPY, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EN_CHANGE, ES_AUTOHSCROLL, GWLP_USERDATA,
    GetDlgItem, GetForegroundWindow, GetWindowLongPtrW, HMENU, HWND_TOPMOST, IDC_ARROW,
    LoadCursorW, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WINDOW_STYLE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_NCDESTROY, WM_PAINT, WM_SYSKEYDOWN, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::skin::{SkinColors, skin_colors};
use crate::layout::{Rect, TextMetrics};
use crate::popup::{
    GdiMetrics, GdiObject, MemDc, Selection, message_font, module_instance, to_rect,
};
use crate::ui::guard;

/// The window class is process-wide: a distinctive name, registered once per process.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.Menu");

/// `EM_GETLINELENGTH` (not bound by the `windows` crate): the length of the text of an edit box, in characters.
const EM_GETLINELENGTH: u32 = 0x00C2;
/// `EM_GETLINE` (not bound by the `windows` crate): copies one line of an edit box into a buffer.
const EM_GETLINE: u32 = 0x00C4;

/// The control id of the search box.
const ID_SEARCH: i32 = 1;

/// What a menu item asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Switch between direct mode and input mode.
    ToggleInputMode,
    /// The AI features run with this effort ("auto", "low", "medium", "high").
    SetEffort(String),
    /// The floating bar wears this skin ("system", "light", "dark").
    SetSkin(String),
    /// A programming convenience switch: the name of the setting and whether it is on.
    SetOption(String, bool),
    /// The scheme of the keys (the programmer habit, Microsoft, Sogou, ATOK).
    SetKeymapPreset(crate::imesettings::KeymapPreset),
    /// Switch the input language to this profile ("zh-CN", "en-US", "ja-JP").
    SwitchLanguage(String),
    /// Open the settings window.
    OpenSettings,
}

/// One item of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    /// Whether the item carries a check mark (the setting it changes is on).
    pub checked: bool,
}

impl MenuItem {
    /// A plain item.
    pub fn new(label: &str, action: MenuAction) -> MenuItem {
        MenuItem {
            label: label.to_string(),
            action,
            checked: false,
        }
    }

    /// An item that shows whether its setting is on.
    pub fn checked(label: &str, action: MenuAction, checked: bool) -> MenuItem {
        MenuItem {
            label: label.to_string(),
            action,
            checked,
        }
    }
}

/// How a menu hands its choice to the text service.
pub type MenuSink = Rc<dyn Fn(MenuAction)>;

/// A sink that drops every choice.
pub fn no_menu_sink() -> MenuSink {
    Rc::new(|_| {})
}

/// The items whose label contains the query (case-insensitive). An empty query keeps every item, so that the list is
/// complete before the user types anything.
pub fn filter(items: &[MenuItem], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return (0..items.len()).collect();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.label.to_lowercase().contains(&needle))
        .map(|(index, _)| index)
        .collect()
}

/// Where the menu goes: below the button when there is room, above it otherwise. The user asked for a menu that can
/// open upward - a bar near the bottom of the screen has no room below it.
pub fn placement(
    anchor_top: i32,
    anchor_bottom: i32,
    menu_height: i32,
    work_top: i32,
    work_bottom: i32,
) -> MenuPlacement {
    if anchor_bottom + menu_height <= work_bottom {
        return MenuPlacement::Below;
    }
    if anchor_top - menu_height >= work_top {
        return MenuPlacement::Above;
    }
    // Neither side fits: the side with more room wins.
    if (work_bottom - anchor_bottom) >= (anchor_top - work_top) {
        MenuPlacement::Below
    } else {
        MenuPlacement::Above
    }
}

/// Where the menu is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPlacement {
    Below,
    Above,
}

/// The measurements the layout of a menu needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuMetrics {
    /// The height of one line.
    pub line_height: i32,
    /// The width of the widest item.
    pub item_width: i32,
    /// The margin around the items.
    pub padding: i32,
    /// The height of the search box (zero when the menu has none).
    pub search_height: i32,
}

/// The geometry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuLayout {
    pub width: i32,
    pub height: i32,
    /// The search box, when the menu has one.
    pub search: Option<Rect>,
    /// The row of every visible item, in the order they are drawn.
    pub rows: Vec<Rect>,
}

/// The geometry of a menu for the visible items.
pub fn compute_layout(visible: &[usize], metrics: &MenuMetrics, searchable: bool) -> MenuLayout {
    let mut rows = Vec::with_capacity(visible.len());
    let mut y = metrics.padding;
    if searchable {
        y += metrics.search_height + metrics.padding;
    }
    for _ in visible {
        rows.push(Rect::new(
            metrics.padding,
            y,
            metrics.padding + metrics.item_width,
            y + metrics.line_height,
        ));
        y += metrics.line_height;
    }
    let height = y + metrics.padding;
    MenuLayout {
        width: metrics.padding * 2 + metrics.item_width,
        height,
        search: if searchable {
            Some(Rect::new(
                metrics.padding,
                metrics.padding,
                metrics.padding + metrics.item_width,
                metrics.padding + metrics.search_height,
            ))
        } else {
            None
        },
        rows,
    }
}

/// The item under a client point, if any.
pub fn hit_test(layout: &MenuLayout, x: i32, y: i32) -> Option<usize> {
    layout.rows.iter().position(|row| row.contains(x, y))
}

/// The state the window procedure needs.
struct MenuState {
    items: Vec<MenuItem>,
    /// The indices of the items that are visible (the filter decides).
    visible: Vec<usize>,
    /// What the user typed in the search box.
    query: String,
    /// The highlighted row.
    selected: usize,
    layout: MenuLayout,
    font: HFONT,
    skin: SkinColors,
    sink: MenuSink,
    /// Whether the menu has a search box.
    searchable: bool,
    /// The window that had the focus before the menu opened.
    owner: HWND,
}

/// A menu window of one thread.
pub struct MenuWindow {
    hwnd: HWND,
    /// Kept alive: the window holds the other strong count.
    _shared: Rc<RefCell<MenuState>>,
    /// The message font of the menu; owned here, so that it outlives the painting.
    _font: GdiObject,
}

impl MenuWindow {
    /// Open a menu below (or above) the rectangle of its button.
    ///
    /// `items` are the choices, `searchable` adds a search box at the top of the list. The menu takes the focus; when it
    /// closes, the focus goes back to the window that had it.
    pub fn open(
        items: Vec<MenuItem>,
        searchable: bool,
        anchor: Rect,
        work: Rect,
        skin: SkinColors,
        sink: MenuSink,
    ) -> Option<MenuWindow> {
        let instance = module_instance()?;
        register_class(instance).ok()?;
        let visible = filter(&items, "");
        let font = message_font(96)?;
        let metrics = measure(font.as_font(), &items, &visible, searchable);
        let layout = compute_layout(&visible, &metrics, searchable);
        let (menu_width, menu_height) = (layout.width, layout.height);
        let height = menu_height;
        let where_ = placement(anchor.top, anchor.bottom, height, work.top, work.bottom);
        let y = match where_ {
            MenuPlacement::Below => anchor.bottom,
            MenuPlacement::Above => anchor.top - height,
        };
        // SAFETY: a plain query; the handle is only used to give the focus back later.
        let owner = unsafe { GetForegroundWindow() };
        let shared = Rc::new(RefCell::new(MenuState {
            items,
            visible,
            query: String::new(),
            selected: 0,
            layout,
            font: font.as_font(),
            skin,
            sink,
            searchable,
            owner,
        }));
        let raw = Rc::into_raw(Rc::clone(&shared));
        // SAFETY: the class is registered; the only pointer is the one we hand over in USERDATA.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS_NAME,
                w!(""),
                WS_POPUP | WS_CLIPCHILDREN,
                anchor.left,
                y,
                menu_width,
                menu_height,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
        }
        .ok()?;
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        // SAFETY: plain calls on our own window.
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                anchor.left,
                y,
                menu_width,
                menu_height,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            );
            let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
        }
        let window = MenuWindow {
            hwnd,
            _shared: shared,
            _font: font,
        };
        window.focus_search();
        Some(window)
    }

    /// Give the focus to the search box (or to the menu itself).
    fn focus_search(&self) {
        let searchable = {
            let Some(cell) = (unsafe { shared_of(self.hwnd) }) else {
                return;
            };
            let Ok(shared) = cell.try_borrow() else {
                return;
            };
            shared.searchable
        };
        if searchable {
            // SAFETY: the edit box is our own child window.
            unsafe {
                if let Ok(edit) = GetDlgItem(self.hwnd, ID_SEARCH) {
                    let _ = SetFocus(edit);
                }
            }
        } else {
            // SAFETY: our own window.
            unsafe {
                let _ = SetFocus(self.hwnd);
            }
        }
    }

    /// The window of the menu.
    pub fn handle(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for MenuWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window; on another thread than the creator it fails, which only leaks a window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

/// Measure the texts of the items with the message font.
fn measure(font: HFONT, items: &[MenuItem], visible: &[usize], searchable: bool) -> MenuMetrics {
    let fallback = MenuMetrics {
        line_height: 16,
        item_width: 120,
        padding: 6,
        search_height: 20,
    };
    let Some(metrics) = GdiMetrics::new(font) else {
        return fallback;
    };
    let width = visible
        .iter()
        .map(|index| metrics.text_width(&items[*index].label))
        .max()
        .unwrap_or(60)
        .max(metrics.text_width("输入语言"))
        + 24; // room for the check mark
    MenuMetrics {
        line_height: metrics.line_height(),
        item_width: width,
        padding: 6,
        search_height: if searchable {
            metrics.line_height() + 4
        } else {
            0
        },
    }
}

fn register_class(instance: HINSTANCE) -> Result<(), ()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
    if CLASS_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe { LoadCursorW(HINSTANCE::default(), IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are static.
    let atom = unsafe { RegisterClassExW(&class) };
    if atom != 0
        || unsafe { windows::Win32::Foundation::GetLastError() }
            == windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
    {
        CLASS_REGISTERED.store(true, Ordering::Release);
        Ok(())
    } else {
        Err(())
    }
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `MenuWindow::open`.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<MenuState>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<MenuState>;
    // SAFETY: null or the live pointer described above.
    unsafe { ptr.as_ref() }
}

/// The window procedure. Everything runs inside a panic guard: a panic that unwound out of this `extern "system"` frame
/// would abort the host process.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guard(None, || Some(window_message(hwnd, msg, wparam, lparam))).unwrap_or_else(|| {
        // SAFETY: forwarding the message unchanged.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    })
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            on_create(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            // The search box reports every change: the list is filtered again.
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if code == EN_CHANGE {
                on_query_changed(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            on_key(hwnd, VIRTUAL_KEY(wparam.0 as u16));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            on_click(hwnd, lparam);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            on_destroy(hwnd);
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            // SAFETY: the last message of this window; takes back the count given in `open`.
            unsafe {
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *const RefCell<MenuState>;
                if !ptr.is_null() {
                    drop(Rc::from_raw(ptr));
                }
            }
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => {
            let _ = lparam;
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// Create the search box of the menu (a real edit box, so that it can be typed into with any keyboard and any IME).
fn on_create(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = cell.try_borrow() else {
        return;
    };
    if !shared.searchable {
        return;
    }
    let Some(rect) = shared.layout.search else {
        return;
    };
    let Some(instance) = module_instance() else {
        return;
    };
    // SAFETY: the class is a standard control; the parameters are plain values.
    let edit = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WS_EX_CLIENTEDGE,
            w!("EDIT"),
            w!(""),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            hwnd,
            HMENU(ID_SEARCH as *mut std::ffi::c_void),
            instance,
            None,
        )
    };
    let _ = edit;
}

/// The user typed in the search box: filter the list again.
fn on_query_changed(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return;
    };
    // SAFETY: our own child window.
    let text = unsafe {
        let Ok(edit) = GetDlgItem(hwnd, ID_SEARCH) else {
            return;
        };
        read_window_text(edit)
    };
    shared.query = text;
    shared.visible = filter(&shared.items, &shared.query);
    shared.selected = 0;
    let metrics = measure(
        shared.font,
        &shared.items,
        &shared.visible,
        shared.searchable,
    );
    shared.layout = compute_layout(&shared.visible, &metrics, shared.searchable);
    // SAFETY: our own window.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
            0,
            0,
            shared.layout.width,
            shared.layout.height,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// The text of an edit box.
fn read_window_text(hwnd: HWND) -> String {
    // SAFETY: `hwnd` is our own edit box.
    let length = unsafe { SendMessageW(hwnd, EM_GETLINELENGTH, WPARAM(0), LPARAM(0)) }
        .0
        .max(0) as usize;
    if length == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: `buffer` is a live local with room for the text and its terminator.
    unsafe {
        let copied = SendMessageW(
            hwnd,
            EM_GETLINE,
            WPARAM(0),
            LPARAM(buffer.as_mut_ptr() as isize),
        )
        .0
        .max(0) as usize;
        buffer.truncate(copied);
    }
    String::from_utf16_lossy(&buffer)
}

/// The keyboard of the menu: the arrows move the highlight, Enter takes the row, Esc closes the menu.
fn on_key(hwnd: HWND, key: VIRTUAL_KEY) {
    match key {
        VK_DOWN | VK_UP => {
            let Some(cell) = (unsafe { shared_of(hwnd) }) else {
                return;
            };
            let Ok(mut shared) = cell.try_borrow_mut() else {
                return;
            };
            if shared.visible.is_empty() {
                return;
            }
            let last = shared.visible.len() - 1;
            shared.selected = if key == VK_DOWN {
                (shared.selected + 1).min(last)
            } else {
                shared.selected.checked_sub(1).unwrap_or(last)
            };
            // SAFETY: our own window.
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        VK_RETURN => take_selected(hwnd),
        VK_ESCAPE => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

/// A click takes the row it is on.
fn on_click(hwnd: HWND, lparam: LPARAM) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let row = {
        let Ok(shared) = cell.try_borrow() else {
            return;
        };
        hit_test(&shared.layout, x, y)
    };
    let Some(row) = row else {
        return;
    };
    if let Some(cell) = unsafe { shared_of(hwnd) }
        && let Ok(mut shared) = cell.try_borrow_mut()
    {
        shared.selected = row;
    }
    take_selected(hwnd);
}

/// The highlighted row is chosen: tell the text service and close the menu.
fn take_selected(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let (action, sink) = {
        let Ok(shared) = cell.try_borrow() else {
            return;
        };
        let action = shared
            .visible
            .get(shared.selected)
            .and_then(|index| shared.items.get(*index))
            .map(|item| item.action.clone());
        (action, shared.sink.clone())
    };
    let Some(action) = action else {
        return;
    };
    sink(action);
    // SAFETY: our own window.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// The menu is gone: give the focus back to the window that had it.
fn on_destroy(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = cell.try_borrow() else {
        return;
    };
    let owner = shared.owner;
    if !owner.is_invalid() {
        // SAFETY: the window that had the focus before; it is still alive (it is the host's editor).
        unsafe {
            let _ = SetForegroundWindow(owner);
        }
    }
}

/// Paint the menu.
fn paint(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = cell.try_borrow() else {
        return;
    };
    let layout = shared.layout.clone();
    let items = shared.items.clone();
    let visible = shared.visible.clone();
    let selected = shared.selected;
    let skin = shared.skin;
    let font = shared.font;
    let (w, h) = (layout.width, layout.height);
    if w <= 0 || h <= 0 {
        return;
    }
    // SAFETY: `hwnd` is our own window.
    let mut scope = PaintScope {
        hwnd,
        ps: PAINTSTRUCT::default(),
    };
    // SAFETY: `scope.ps` is a live local that BeginPaint fills in.
    let target = unsafe { BeginPaint(hwnd, &mut scope.ps) };
    if target.is_invalid() {
        return;
    }
    // SAFETY: `target` is the live paint DC.
    let mem = MemDc(unsafe { CreateCompatibleDC(target) });
    if mem.0.is_invalid() {
        return;
    }
    // SAFETY: `target` is the live paint DC.
    let bitmap = unsafe { CreateCompatibleBitmap(target, w, h) };
    if bitmap.is_invalid() {
        return;
    }
    let bitmap = GdiObject(HGDIOBJ(bitmap.0));
    let _bitmap_selected = Selection::new(mem.0, bitmap.0);
    let _font_selected = Selection::new(mem.0, HGDIOBJ(font.0));
    // SAFETY: plain state change on our memory DC.
    let _ = unsafe { SetBkMode(mem.0, TRANSPARENT) };

    fill(mem.0, Rect::new(0, 0, w, h), skin.background);
    for (row, index) in visible.iter().enumerate() {
        let Some(rect) = layout.rows.get(row) else {
            continue;
        };
        let highlighted = row == selected;
        if highlighted {
            fill(mem.0, *rect, skin.highlight);
        }
        let item = &items[*index];
        let mark = if item.checked { "\u{2713} " } else { "" };
        text(
            mem.0,
            &format!("{mark}{}", item.label),
            *rect,
            if highlighted {
                skin.highlight_text
            } else {
                skin.text
            },
        );
    }
    // SAFETY: both DCs are live and the bitmap covers `w` x `h`.
    let _ = unsafe { BitBlt(target, 0, 0, w, h, mem.0, 0, 0, SRCCOPY) };
}

/// Ends the paint cycle even on early return (a WM_PAINT that is not validated repeats forever).
struct PaintScope {
    hwnd: HWND,
    ps: PAINTSTRUCT,
}

impl Drop for PaintScope {
    fn drop(&mut self) {
        // SAFETY: pairs with the BeginPaint in `paint`.
        let _ = unsafe { EndPaint(self.hwnd, &self.ps) };
    }
}

/// Fill a rectangle with one colour (the brush is created and released right away).
fn fill(hdc: HDC, rect: Rect, colour: COLORREF) {
    // SAFETY: a plain call; the brush is ours and is deleted below.
    unsafe {
        let brush = CreateSolidBrush(colour);
        if brush.is_invalid() {
            return;
        }
        let _ = FillRect(hdc, &to_rect(rect), brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

fn text(hdc: HDC, value: &str, rect: Rect, colour: COLORREF) {
    let wide: Vec<u16> = value.encode_utf16().collect();
    let Ok(count) = u32::try_from(wide.len()) else {
        return;
    };
    // SAFETY: `hdc` is live; `wide` holds `count` UTF-16 units and outlives the call; the clip RECT is a local.
    unsafe {
        SetTextColor(hdc, colour);
        let _ = ExtTextOutW(
            hdc,
            rect.left,
            rect.top,
            ETO_CLIPPED,
            Some(&to_rect(rect)),
            PCWSTR(wide.as_ptr()),
            count,
            None,
        );
    }
}

/// The colours of the menus for a skin.
pub fn menu_skin(skin: &str) -> SkinColors {
    skin_colors(skin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<MenuItem> {
        vec![
            MenuItem::new("中文", MenuAction::SwitchLanguage("zh-CN".to_string())),
            MenuItem::new("English", MenuAction::SwitchLanguage("en-US".to_string())),
            MenuItem::new("日本語", MenuAction::SwitchLanguage("ja-JP".to_string())),
        ]
    }

    #[test]
    fn an_empty_query_shows_every_item() {
        assert_eq!(filter(&items(), "").len(), 3);
        assert_eq!(filter(&items(), "   ").len(), 3);
    }

    #[test]
    fn the_query_filters_the_items() {
        let shown = filter(&items(), "en");
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0], 1);
        assert_eq!(filter(&items(), "日").len(), 1);
        assert_eq!(filter(&items(), "zzz").len(), 0);
    }

    #[test]
    fn the_menu_opens_below_when_there_is_room_and_above_when_there_is_none() {
        assert_eq!(
            placement(100, 130, 200, 0, 1000),
            MenuPlacement::Below,
            "below when it fits"
        );
        assert_eq!(
            placement(800, 830, 200, 0, 1000),
            MenuPlacement::Above,
            "above when the screen ends"
        );
        // no room at all: the side with more room wins (here: above)
        assert_eq!(placement(400, 430, 200, 300, 500), MenuPlacement::Above);
    }

    #[test]
    fn the_rows_of_the_layout_do_not_overlap_and_end_at_the_bottom() {
        let metrics = MenuMetrics {
            line_height: 18,
            item_width: 100,
            padding: 6,
            search_height: 22,
        };
        let layout = compute_layout(&[0, 1, 2], &metrics, true);
        assert_eq!(layout.rows.len(), 3);
        for pair in layout.rows.windows(2) {
            assert!(pair[0].bottom <= pair[1].top);
        }
        let last = layout.rows.last().unwrap();
        assert!(last.bottom <= layout.height);
        assert!(layout.search.is_some());
        assert!(layout.rows[0].top >= layout.search.as_ref().unwrap().bottom);
    }

    #[test]
    fn a_menu_without_a_search_box_has_no_search_row() {
        let metrics = MenuMetrics {
            line_height: 18,
            item_width: 100,
            padding: 6,
            search_height: 0,
        };
        let layout = compute_layout(&[0], &metrics, false);
        assert!(layout.search.is_none());
        assert!(layout.rows[0].top <= metrics.padding + metrics.line_height);
    }

    #[test]
    fn a_click_finds_the_row_under_it() {
        let metrics = MenuMetrics {
            line_height: 18,
            item_width: 100,
            padding: 6,
            search_height: 0,
        };
        let layout = compute_layout(&[0, 1], &metrics, false);
        assert_eq!(
            hit_test(&layout, layout.rows[0].left + 2, layout.rows[0].top + 2),
            Some(0)
        );
        assert_eq!(
            hit_test(&layout, layout.rows[1].left + 2, layout.rows[1].top + 2),
            Some(1)
        );
        assert_eq!(hit_test(&layout, 0, 0), None);
    }
}
