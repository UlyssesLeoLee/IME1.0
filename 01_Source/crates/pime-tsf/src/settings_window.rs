//! The settings window: the pages of the settings, the way Sogou splits its own settings.
//!
//! The window is an ordinary top-level window (it takes the focus while it is open, so that the user can use the
//! keyboard) that is created on the thread of the host application and destroyed when it is closed. The controls are
//! standard Windows controls (a strip of pages, a group of check boxes and a drop-down list); their geometry is
//! computed by the pure [`compute_layout`], so that "what is where" is testable without a screen.
//!
//! The shape follows the settings dialog of Sogou: a strip of pages (**常规 / 按键 / 外观 / AI / 关于**) and, above it,
//! the language the page belongs to. Every page shows the settings of that language; the pages 外观 and AI show the
//! ones that are the same for every language.
//!
//! What the window does with the values is deliberately small: it reads the controls, writes the file and tells the
//! text service that something changed. The text service then re-reads the file and applies it (see `tip`).

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GWLP_USERDATA, GetForegroundWindow, GetWindowLongPtrW, HMENU, HWND_TOPMOST,
    IDC_ARROW, LoadCursorW, RegisterClassExW, SW_HIDE, SW_SHOW, SWP_NOSIZE, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_NCCREATE, WM_NOTIFY, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_EX_APPWINDOW,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::imesettings::{self, ImeSettings, LanguageSettings};
use crate::layout::Rect;
use crate::popup::module_instance;
use crate::ui::guard;

// The strip of the pages is a real tab control (`SysTabControl32`). The `windows` crate does not bind the tab
// control (its `Win32_UI_Controls` module is enormous and rustc runs out of stack on it), so the few messages and the
// two structures are declared here, exactly as `menus.rs` does it for `EM_GETLINE`.
//
// The values are the ones of the Windows headers (commctrl.h):
//   TCM_FIRST = 0x1300, TCM_GETCURSEL = 0x130B, TCM_INSERTITEMW = 0x133E, TCIF_TEXT = 0x0001,
//   TCN_SELCHANGE = -551.
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_INSERTITEMW: u32 = 0x133E;
const TCIF_TEXT: u32 = 0x0001;
const TCN_SELCHANGE: u32 = 0xFFFF_FDD9; // -551 as an unsigned number

/// One page of the tab control (`TCITEMW`).
#[repr(C)]
struct TabItem {
    /// Which fields of the item are valid (`TCIF_TEXT` here).
    mask: u32,
    /// The state of the item.
    dw_state: u32,
    /// The state that is changed.
    dw_state_mask: u32,
    /// The text of the page.
    psz_text: *mut u16,
    /// How many characters the text has.
    cch_text_max: i32,
    /// The image of the page (-1: none).
    i_image: i32,
    /// The value of the page.
    l_param: isize,
}

/// The header of a notification (`NMHDR`).
#[repr(C)]
struct NmHdr {
    /// The window that sent the notification (the tab control).
    hwnd_from: HWND,
    /// The id of the control that sent it.
    id_from: usize,
    /// What happened.
    code: u32,
}

/// The parameters of a window that is being created (`CREATESTRUCTW`).
///
/// The window procedure receives it with `WM_NCCREATE`, **before** the window exists. Its first field is the value
/// that was handed over to `CreateWindowExW` as `lpParam`: that is where the state of the window travels.
#[repr(C)]
struct CreateStruct {
    /// The value that was given to `CreateWindowExW` as `lpParam`.
    lp_create_params: *mut c_void,
    /// The module of the class.
    h_instance: HINSTANCE,
    /// The menu of the window.
    h_menu: HMENU,
    /// The parent of the window.
    hwnd_parent: HWND,
    /// The height of the client area.
    cy: i32,
    /// The width of the client area.
    cx: i32,
    /// The y of the window.
    y: i32,
    /// The x of the window.
    x: i32,
    /// The style of the window.
    style: u32,
    /// The title of the window.
    name: *mut u16,
    /// The class of the window.
    class: *mut u16,
    /// The extended style of the window.
    ex_style: u32,
}

/// The window class is process-wide: a distinctive name, registered once per process.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.SettingsWindow");

/// The check box of a button: the value `BM_GETCHECK` answers for a checked box.
const BST_CHECKED: i32 = 1;

/// The control id of the save button.
const ID_SAVE: usize = 9001;
/// The control id of the cancel button.
const ID_CANCEL: usize = 9002;
/// The control id of the "back to the defaults" button.
const ID_DEFAULTS: usize = 9003;
/// The control id of the strip of the pages.
const ID_TABS: usize = 9004;

/// The buttons at the bottom of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsButton {
    /// Write the settings and close the window.
    Save,
    /// Close the window without writing anything.
    Cancel,
    /// Put the controls back to the values the product ships with.
    Defaults,
}

/// The pages of the window (the way Sogou splits its own settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    /// The everyday options: the page size, the mode, the composition, the laser interface.
    General,
    /// The keys: the scheme and what the Tab key does.
    Keys,
    /// The look: the skin and the place of the mouth of the light.
    Appearance,
    /// The plugin languages that are on (PL/SQL, T-SQL, Cypher).
    Plugins,
    /// The effort of the AI features.
    Ai,
    /// The version and the files.
    About,
}

/// The pages, in the order the strip shows them.
pub const PAGES: [SettingsPage; 6] = [
    SettingsPage::General,
    SettingsPage::Keys,
    SettingsPage::Appearance,
    SettingsPage::Plugins,
    SettingsPage::Ai,
    SettingsPage::About,
];

/// The title of a page.
pub fn page_title(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::General => "常规",
        SettingsPage::Keys => "按键",
        SettingsPage::Appearance => "外观",
        SettingsPage::Plugins => "插件",
        SettingsPage::Ai => "AI",
        SettingsPage::About => "关于",
    }
}

/// The check boxes of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The space key takes the highlighted candidate.
    SpaceConfirms,
    /// A composition is opened as soon as candidates appear.
    AutoCompose,
    /// The input method starts in input mode.
    InputMode,
    /// The laser interface is shown.
    LaserUi,
    /// `=`, `.`, `]` page the list forward and `-`, `,`, `[` page it back.
    PunctPaging,
}

/// The drop-down lists of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combo {
    /// The number of candidates on a page.
    PageSize,
    /// The scheme of the keys (the programmer habit, Microsoft, Sogou, ATOK).
    KeymapPreset,
    /// What the Tab key does while the candidate list is on the screen.
    TabBehaviour,
    /// The skin of the panels.
    Skin,
    /// The effort of the AI features.
    Effort,
    /// The edge of the screen the mouth of the light sits on.
    MouthEdge,
}

/// The kind of one control of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// The drop-down list with the language the pages edit.
    LanguageCombo,
    /// The strip of the pages.
    TabStrip,
    /// A text (a label or a hint).
    Label(&'static str),
    /// One check box.
    Check(Toggle),
    /// One drop-down list.
    Combo(Combo),
    /// The check box of one plugin language (the id is the one of the core).
    PluginCheck(&'static str),
}

/// One control of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSlot {
    pub kind: ControlKind,
    /// The rectangle, relative to the client area of the window.
    pub rect: Rect,
    /// The page the control belongs to (the strip of the pages and the language are on every page).
    pub page: SettingsPage,
}

/// The geometry of the settings window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLayout {
    /// The size of the client area.
    pub width: i32,
    pub height: i32,
    /// The controls, in the order they are created.
    pub controls: Vec<ControlSlot>,
    /// The buttons at the bottom.
    pub buttons: Vec<(Rect, SettingsButton)>,
}

/// The margin around the content.
const MARGIN: i32 = 14;
/// The height of one row of controls.
const ROW: i32 = 24;
/// The height of the strip of the pages.
const TAB_HEIGHT: i32 = 28;
/// The width of the label in front of a drop-down list.
const LABEL_WIDTH: i32 = 120;
/// The width of the drop-down list.
const COMBO_WIDTH: i32 = 170;
/// The height of the button row.
const BUTTON_ROW: i32 = 34;
/// The width of the client area.
const WIDTH: i32 = 520;

/// The width of the text of a label (the box of the label).
const LABEL_TEXT_WIDTH: i32 = WIDTH - MARGIN - (MARGIN + 12);

/// The width of a character of the system font, in pixels.
///
/// The layout reserves the space for a text before the text is drawn, so it has to guess how wide the text will be.
/// The guess is deliberately too big: a box that is a little too wide only wastes a few pixels, a box that is too
/// narrow **cuts the text off** and the user cannot read what the page says.
const ASCII_CHAR_WIDTH: i32 = 9;
/// The width of a character that is not ASCII (the Chinese ones are twice as wide as the Latin ones).
const WIDE_CHAR_WIDTH: i32 = 15;

/// How many lines a text needs in a box of `width` pixels.
///
/// A text that does not fit on one line is wrapped by the control itself; the layout has to know how many lines it
/// will be, or the last ones are cut off.
pub fn lines_of(text: &str, width: i32) -> i32 {
    let used: i32 = text
        .chars()
        .map(|c| {
            if c.is_ascii() {
                ASCII_CHAR_WIDTH
            } else {
                WIDE_CHAR_WIDTH
            }
        })
        .sum();
    if used <= width { 1 } else { 2 }
}

/// The rows of every page: the controls of the page, in the order they are drawn.
fn rows_of(page: SettingsPage) -> Vec<ControlKind> {
    match page {
        SettingsPage::General => vec![
            ControlKind::Label("候选页大小"),
            ControlKind::Combo(Combo::PageSize),
            ControlKind::Check(Toggle::InputMode),
            ControlKind::Check(Toggle::SpaceConfirms),
            ControlKind::Check(Toggle::AutoCompose),
            ControlKind::Check(Toggle::PunctPaging),
            ControlKind::Check(Toggle::LaserUi),
        ],
        SettingsPage::Keys => vec![
            ControlKind::Label("按键方案"),
            ControlKind::Combo(Combo::KeymapPreset),
            ControlKind::Label("Tab 键"),
            ControlKind::Combo(Combo::TabBehaviour),
            ControlKind::Label("Tab 只在候选列表出现时被输入法接管；没有候选时它属于你的编辑器。"),
            ControlKind::Label("Enter、空格、数字键：上屏候选"),
        ],
        SettingsPage::Appearance => vec![
            ControlKind::Label("皮肤"),
            ControlKind::Combo(Combo::Skin),
            ControlKind::Label("发光口位置"),
            ControlKind::Combo(Combo::MouthEdge),
            ControlKind::Label("发光口可以拖到屏幕左边、上边或右边，位置会被记住。"),
        ],
        SettingsPage::Plugins => {
            let mut rows = vec![ControlKind::Label("插件语言")];
            for (id, name) in imesettings::PLUGINS {
                rows.push(ControlKind::PluginCheck(id));
                let _ = name;
            }
            rows.push(ControlKind::Label(
                "插件是一整门语言（自己的规则、方言与校验）。PL/SQL 与 T-SQL 共用 .sql 扩展名，同一时间只能开启其中一个。",
            ));
            rows
        }
        SettingsPage::Ai => vec![
            ControlKind::Label("AI 力度"),
            ControlKind::Combo(Combo::Effort),
            ControlKind::Label("力度决定 AI 功能（解释、注释、生成）付出的代价。"),
        ],
        SettingsPage::About => vec![
            ControlKind::Label("编程输入法 IME1.0"),
            ControlKind::Label("版本 0.2.2"),
            ControlKind::Label("设置文件：%LOCALAPPDATA%\\Programming IME\\settings.json"),
            ControlKind::Label("快捷键与按键方案：01_Source/docs/HOTKEYS.md"),
        ],
    }
}

/// The geometry of the settings window.
pub fn compute_layout() -> SettingsLayout {
    let mut controls = Vec::new();
    // The language of the pages: above the strip, on every page.
    controls.push(ControlSlot {
        kind: ControlKind::Label("语言"),
        rect: Rect::new(MARGIN, MARGIN, MARGIN + 50, MARGIN + ROW),
        page: SettingsPage::General,
    });
    controls.push(ControlSlot {
        kind: ControlKind::LanguageCombo,
        rect: Rect::new(MARGIN + 56, MARGIN, MARGIN + 56 + COMBO_WIDTH, MARGIN + ROW),
        page: SettingsPage::General,
    });
    let strip_top = MARGIN + ROW + 6;
    controls.push(ControlSlot {
        kind: ControlKind::TabStrip,
        rect: Rect::new(MARGIN, strip_top, WIDTH - MARGIN, strip_top + TAB_HEIGHT),
        page: SettingsPage::General,
    });
    // The content of the pages starts under the strip.
    let content_top = strip_top + TAB_HEIGHT + 8;
    let mut content_height = 0;
    for page in PAGES {
        let rows = rows_of(page);
        let mut y = content_top;
        let mut page_height = 0;
        for kind in rows {
            // A label whose text does not fit on one line is wrapped: the box of the label must be as tall as the
            // text needs, or the last line of it is cut off.
            let height = match kind {
                ControlKind::Label(text) => ROW * lines_of(text, LABEL_TEXT_WIDTH),
                _ => ROW,
            };
            let rect = match kind {
                ControlKind::Label(_) => Rect::new(MARGIN + 12, y, WIDTH - MARGIN, y + height),
                ControlKind::Combo(_) => Rect::new(
                    MARGIN + 12 + LABEL_WIDTH,
                    y,
                    MARGIN + 12 + LABEL_WIDTH + COMBO_WIDTH,
                    y + ROW,
                ),
                ControlKind::Check(_) | ControlKind::PluginCheck(_) => {
                    Rect::new(MARGIN + 12, y, WIDTH - MARGIN, y + ROW)
                }
                _ => Rect::new(MARGIN, y, WIDTH - MARGIN, y + ROW),
            };
            controls.push(ControlSlot { kind, rect, page });
            y += height;
            page_height += height;
        }
        content_height = content_height.max(page_height);
    }
    let height = content_top + content_height + BUTTON_ROW + MARGIN;
    let mut buttons = Vec::new();
    let mut x = MARGIN;
    for (kind, label_width) in [
        (SettingsButton::Save, 90),
        (SettingsButton::Cancel, 90),
        (SettingsButton::Defaults, 120),
    ] {
        buttons.push((
            Rect::new(x, height - BUTTON_ROW - 2, x + label_width, height - 8),
            kind,
        ));
        x += label_width + 8;
    }
    SettingsLayout {
        width: WIDTH,
        height,
        controls,
        buttons,
    }
}

/// The name of a plugin language, as the settings window shows it.
pub fn plugin_label(id: &str) -> &'static str {
    imesettings::PLUGINS
        .iter()
        .find(|(plugin, _)| *plugin == id)
        .map(|(_, name)| *name)
        .unwrap_or("")
}

/// The label of a check box.
pub fn toggle_label(toggle: Toggle) -> &'static str {
    match toggle {
        Toggle::SpaceConfirms => "空格上屏（否则空格就是空格）",
        Toggle::AutoCompose => "自动开组合（打字即进入组字）",
        Toggle::InputMode => "默认进入输入模式（否则直接输入）",
        Toggle::LaserUi => "显示激光界面",
        Toggle::PunctPaging => "标点翻页（= . ] 翻页，- , [ 后退）",
    }
}

/// The label of the drop-down list of the page size.
pub const PAGE_SIZE_LABEL: &str = "候选页大小";

/// The items of a drop-down list.
fn combo_items(combo: Combo) -> Vec<String> {
    match combo {
        Combo::PageSize => (1..=9).map(|size| size.to_string()).collect(),
        Combo::KeymapPreset => imesettings::KeymapPreset::ALL
            .iter()
            .map(|preset| preset.label().to_string())
            .collect(),
        Combo::TabBehaviour => vec![
            "移动高亮（选取候选，程序员习惯）".to_string(),
            "上屏选中的候选（微软/搜狗/ATOK）".to_string(),
            "交给宿主（编辑器保留 Tab）".to_string(),
        ],
        Combo::Skin => imesettings::SKINS
            .iter()
            .map(|skin| imesettings::skin_label(skin).to_string())
            .collect(),
        Combo::Effort => imesettings::EFFORT_LEVELS
            .iter()
            .map(|level| imesettings::effort_label(level).to_string())
            .collect(),
        Combo::MouthEdge => vec![
            "屏幕左侧".to_string(),
            "屏幕上边".to_string(),
            "屏幕右侧".to_string(),
        ],
    }
}

/// What the drop-down list of the Tab key means.
///
/// The three ways are the three the user asked for: the programmer habit (Tab moves the highlight, it selects),
/// the mainstream behaviour of the other input methods (Tab takes the candidate) and the one that gives the key
/// back to the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabBehaviour {
    /// Tab moves the highlight of the list (it selects, it does not write).
    Cycle,
    /// Tab takes the highlighted candidate.
    Commit,
    /// The key belongs to the host, the list is a preview.
    Host,
}

/// The three ways of the Tab key, in the order of the drop-down list.
pub const TAB_BEHAVIOURS: [TabBehaviour; 3] =
    [TabBehaviour::Cycle, TabBehaviour::Commit, TabBehaviour::Host];

/// The way the Tab key behaves, out of the settings of a language.
pub fn tab_behaviour_of(settings: &LanguageSettings) -> TabBehaviour {
    if settings.tab_to_host {
        TabBehaviour::Host
    } else if settings.tab_cycles {
        TabBehaviour::Cycle
    } else {
        TabBehaviour::Commit
    }
}

/// The way the Tab key behaves, written back into the settings of a language.
pub fn apply_tab_behaviour(settings: &mut LanguageSettings, behaviour: TabBehaviour) {
    match behaviour {
        TabBehaviour::Cycle => {
            settings.tab_cycles = true;
            settings.tab_to_host = false;
        }
        TabBehaviour::Commit => {
            settings.tab_cycles = false;
            settings.tab_to_host = false;
        }
        TabBehaviour::Host => {
            settings.tab_to_host = true;
        }
    }
}

/// What happened in the settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    /// The user saved the settings: the file was written.
    Saved,
    /// The window was closed without saving.
    Cancelled,
}

/// How the settings window tells the text service what happened.
pub type SettingsSink = Rc<dyn Fn(SettingsEvent)>;

/// The state the window procedure needs.
struct SettingsState {
    settings: ImeSettings,
    layout: SettingsLayout,
    /// The window of every control, in the order of the layout.
    handles: Vec<HWND>,
    /// The strip of the pages.
    tabs: HWND,
    sink: SettingsSink,
    /// Whether the user saved the settings (then the window does not report a cancellation).
    saved: bool,
    /// The window that had the focus before the settings window opened.
    owner: HWND,
    /// The page that is shown.
    page: SettingsPage,
    /// The language the pages edit.
    language: usize,
}

impl SettingsState {
    /// The settings of the language the pages edit.
    fn language_settings(&self) -> LanguageSettings {
        self.settings.get(imesettings::LANGUAGES[self.language])
    }

    /// The index of the control of the kind, if there is one.
    fn slot_of(&self, kind_of: impl Fn(&ControlKind) -> bool) -> Option<usize> {
        self.layout
            .controls
            .iter()
            .position(|slot| kind_of(&slot.kind))
    }

    /// The window of the control of the kind.
    fn handle_of(&self, kind_of: impl Fn(&ControlKind) -> bool) -> Option<HWND> {
        self.slot_of(kind_of).map(|index| self.handles[index])
    }

    /// Read the controls into the settings.
    fn collect(&self) -> ImeSettings {
        let mut settings = self.settings.clone();
        let mut language = self.language;
        if let Some(handle) = self.handle_of(|kind| *kind == ControlKind::LanguageCombo) {
            // SAFETY: a plain call on our own combo box.
            let index = unsafe { SendMessageW(handle, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
            if index >= 0 {
                language = (index as usize).min(imesettings::LANGUAGES.len() - 1);
            }
        }
        let mut value = settings.get(imesettings::LANGUAGES[language]);
        for (slot, handle) in self.layout.controls.iter().zip(self.handles.iter()) {
            match slot.kind {
                ControlKind::Combo(Combo::PageSize) => {
                    // SAFETY: a plain call on our own combo box.
                    let index =
                        unsafe { SendMessageW(*handle, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
                    if index >= 0 {
                        value.page_size = (index as usize + 1).clamp(1, 9);
                    }
                }
                ControlKind::Combo(Combo::KeymapPreset) => {
                    let index = self.selected_index(*handle);
                    if let Some(preset) = imesettings::KeymapPreset::ALL.get(index) {
                        value.keymap_preset = *preset;
                    }
                }
                ControlKind::Combo(Combo::TabBehaviour) => {
                    let index = self.selected_index(*handle);
                    if let Some(behaviour) = TAB_BEHAVIOURS.get(index) {
                        apply_tab_behaviour(&mut value, *behaviour);
                    }
                }
                ControlKind::Combo(Combo::Skin) => {
                    let index = self.selected_index(*handle);
                    if let Some(skin) = imesettings::SKINS.get(index) {
                        settings.skin = (*skin).to_string();
                    }
                }
                ControlKind::Combo(Combo::Effort) => {
                    let index = self.selected_index(*handle);
                    if let Some(level) = imesettings::EFFORT_LEVELS.get(index) {
                        settings.effort = (*level).to_string();
                    }
                }
                ControlKind::Combo(Combo::MouthEdge) => {
                    let index = self.selected_index(*handle);
                    let edge = match index {
                        0 => "left",
                        1 => "top",
                        2 => "right",
                        _ => continue,
                    };
                    let offset = crate::laser::MouthPlace::from_text(&settings.mouth_place)
                        .map(|place| place.offset)
                        .unwrap_or(0);
                    settings.mouth_place = format!("{edge}:{offset}");
                }
                ControlKind::Check(toggle) => {
                    // SAFETY: a plain call on our own check box.
                    let checked =
                        unsafe { SendMessageW(*handle, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0
                            == BST_CHECKED as isize;
                    match toggle {
                        Toggle::SpaceConfirms => value.space_confirms = checked,
                        Toggle::AutoCompose => value.auto_compose = checked,
                        Toggle::InputMode => value.input_mode = checked,
                        Toggle::LaserUi => value.laser_ui = checked,
                        Toggle::PunctPaging => value.punct_paging = checked,
                    }
                }
                ControlKind::PluginCheck(id) => {
                    // SAFETY: a plain call on our own check box.
                    let checked =
                        unsafe { SendMessageW(*handle, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0
                            == BST_CHECKED as isize;
                    settings.set_plugin(id, checked);
                }
                _ => {}
            }
        }
        settings.set(imesettings::LANGUAGES[language], value);
        settings
    }

    /// The index the drop-down list has selected.
    fn selected_index(&self, handle: HWND) -> usize {
        // SAFETY: a plain call on our own combo box.
        let index = unsafe { SendMessageW(handle, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
        if index < 0 { 0 } else { index as usize }
    }

    /// Put the controls back to the values the product ships with.
    fn show_defaults(&mut self) {
        self.settings = ImeSettings::default();
        self.refresh();
    }

    /// Put every control to the value of the settings of the language the pages edit.
    fn refresh(&mut self) {
        let value = self.language_settings();
        for (index, slot) in self.layout.controls.iter().enumerate() {
            let handle = self.handles[index];
            match slot.kind {
                ControlKind::Combo(combo) => {
                    let items = combo_items(combo);
                    let selected = match combo {
                        Combo::PageSize => value.page_size().clamp(1, 9) - 1,
                        Combo::KeymapPreset => imesettings::KeymapPreset::ALL
                            .iter()
                            .position(|preset| *preset == value.keymap_preset)
                            .unwrap_or(0),
                        Combo::TabBehaviour => TAB_BEHAVIOURS
                            .iter()
                            .position(|behaviour| *behaviour == tab_behaviour_of(&value))
                            .unwrap_or(0),
                        Combo::Skin => imesettings::SKINS
                            .iter()
                            .position(|skin| *skin == self.settings.skin)
                            .unwrap_or(0),
                        Combo::Effort => imesettings::EFFORT_LEVELS
                            .iter()
                            .position(|level| *level == self.settings.effort)
                            .unwrap_or(0),
                        Combo::MouthEdge => match crate::laser::MouthPlace::from_text(
                            &self.settings.mouth_place,
                        ) {
                            Some(place) => match place.edge {
                                crate::laser::Edge::Left => 0,
                                crate::laser::Edge::Top => 1,
                                crate::laser::Edge::Right => 2,
                            },
                            None => 0,
                        },
                    };
                    // SAFETY: a plain call on our own combo box.
                    unsafe {
                        let _ = SendMessageW(
                            handle,
                            CB_SETCURSEL,
                            WPARAM(selected.min(items.len().saturating_sub(1))),
                            LPARAM(0),
                        );
                    }
                }
                ControlKind::Check(toggle) => {
                    let checked = match toggle {
                        Toggle::SpaceConfirms => value.space_confirms,
                        Toggle::AutoCompose => value.auto_compose,
                        Toggle::InputMode => value.input_mode,
                        Toggle::LaserUi => value.laser_ui,
                        Toggle::PunctPaging => value.punct_paging,
                    };
                    // SAFETY: a plain call on our own check box.
                    unsafe {
                        let _ = SendMessageW(
                            handle,
                            BM_SETCHECK,
                            WPARAM(i32::from(checked) as usize),
                            LPARAM(0),
                        );
                    }
                }
                ControlKind::PluginCheck(id) => {
                    let checked = self.settings.plugin_is_on(id);
                    // SAFETY: a plain call on our own check box.
                    unsafe {
                        let _ = SendMessageW(
                            handle,
                            BM_SETCHECK,
                            WPARAM(i32::from(checked) as usize),
                            LPARAM(0),
                        );
                    }
                }
                _ => {}
            }
        }
        // The language of the pages.
        if let Some(handle) = self.handle_of(|kind| *kind == ControlKind::LanguageCombo) {
            // SAFETY: a plain call on our own combo box.
            unsafe {
                let _ = SendMessageW(
                    handle,
                    CB_SETCURSEL,
                    WPARAM(self.language.min(imesettings::LANGUAGES.len() - 1)),
                    LPARAM(0),
                );
            }
        }
    }

    /// Show the page and hide the other ones.
    fn show_page(&mut self, page: SettingsPage) {
        self.page = page;
        for (index, slot) in self.layout.controls.iter().enumerate() {
            // The strip of the pages and the language are always there.
            let always = matches!(
                slot.kind,
                ControlKind::LanguageCombo | ControlKind::TabStrip
            );
            if always {
                continue;
            }
            let handle = self.handles[index];
            // SAFETY: our own child window.
            unsafe {
                let _ = ShowWindow(handle, if slot.page == page { SW_SHOW } else { SW_HIDE });
            }
        }
    }
}

/// The settings window of one thread.
///
/// The window is created when the user asks for it (the settings row of the panel of the laser interface, or the
/// menu of the tray) and destroyed when it is closed. It takes the focus while it is open.
pub struct SettingsWindow {
    hwnd: HWND,
    /// Kept alive: the window holds the other strong count.
    _shared: Rc<RefCell<SettingsState>>,
}

impl SettingsWindow {
    /// Open the window with the settings the user has right now. `None` when the window could not be created.
    pub fn open(settings: ImeSettings, sink: SettingsSink) -> Option<SettingsWindow> {
        // The tab control needs its library: without it the window cannot be created at all.
        ensure_common_controls();
        let instance = match module_instance() {
            Some(instance) => instance,
            None => {
                            return None;
            }
        };
        if let Err(()) = register_class(instance) {
                    return None;
        }
        let layout = compute_layout();
        let (window_width, window_height) = (layout.width + 40, layout.height + 60);
        // SAFETY: a plain query; the handle is only used to give the focus back later.
        let owner = unsafe { GetForegroundWindow() };
        let shared = Rc::new(RefCell::new(SettingsState {
            settings,
            layout,
            handles: Vec::new(),
            tabs: HWND::default(),
            sink,
            saved: false,
            owner,
            page: SettingsPage::General,
            language: 0,
        }));
        // The state of the window travels with the creation: `WM_NCCREATE` carries it in
        // `CREATESTRUCT.lpCreateParams`. The pointer is only borrowed: the window keeps the state alive (the
        // `SettingsWindow` holds the `Rc`), so there is no count to give back and nothing to free by hand.
        let raw = Rc::as_ptr(&shared) as *const c_void;
        // SAFETY: the class is registered; the only pointer is the one we hand over in USERDATA.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW,
                CLASS_NAME,
                w!("IME1.0 设置"),
                // A fixed size: the geometry of the controls is computed once, so the window must not be resizable.
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
                0,
                0,
                window_width,
                window_height,
                HWND::default(),
                HMENU::default(),
                instance,
                Some(raw),
            )
        };
        let Ok(hwnd) = hwnd else {
            return None;
        };
            // The state of the window is already in the user data: it was set when the window was created.
        // The window is put in the middle of the work area: a window that always opens at (0, 0) lands on the task
        // bar of the screen of the left, and two of them land on each other.
        let (x, y) = centered_on(work_area(), window_width, window_height);
        // SAFETY: plain calls on our own window.
        unsafe {
            // The window is **moved** to the middle of the work area and put on top: `SWP_NOMOVE` would leave it where
            // it was created (the corner of the screen, under the window of the user) and `SWP_NOZORDER` would leave it
            // behind the other windows, so a settings window that was opened would not be seen at all.
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE);
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        Some(SettingsWindow {
            hwnd,
            _shared: shared,
        })
    }

    /// The window of the settings.
    pub fn handle(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window; on another thread than the creator it fails, which only leaks a window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

/// The work area of the screen the window opens on.
fn work_area() -> Rect {
    crate::popup::work_area_of(Rect::new(0, 0, 0, 0))
}

/// The place of a window of the given size in the middle of an area.
fn centered_on(area: Rect, width: i32, height: i32) -> (i32, i32) {
    let x = area.left + (area.width() - width) / 2;
    let y = area.top + (area.height() - height) / 2;
    (x.max(area.left), y.max(area.top))
}

fn register_class(instance: HINSTANCE) -> Result<(), ()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
    if CLASS_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    ensure_common_controls();
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe { LoadCursorW(HINSTANCE::default(), IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        // Without a brush the client area is never painted: the window would show the holes of the screen
        // through it. The colour of the system (white) is the one a settings page of a native program uses.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
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

/// Load the library of the common controls, once.
///
/// The strip of the pages is a real tab control (`SysTabControl32`), and that class is registered by
/// `comctl32.dll` when the library is loaded. The process does not link it (the text service is a DLL that must
/// not drag libraries into the host), so the class was simply **not there**: the creation of the control failed,
/// the window procedure gave up, and the settings window did not open at all.
fn ensure_common_controls() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: loading a system library that stays loaded for the life of the process; the handle is not freed
        // on purpose (the classes must stay registered).
        unsafe {
            let _ = windows::Win32::System::LibraryLoader::LoadLibraryW(w!("comctl32.dll"));
        }
    });
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `SettingsWindow::open`.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<SettingsState>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<SettingsState>;
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
        WM_NCCREATE => {
            // The window is being created and does not exist yet: `USERDATA` cannot be set before it, so the state
            // travels with the creation itself, in `CREATESTRUCT.lpCreateParams`.
            //
            // **This is the message that carries it.** `WM_CREATE`, which comes next, does not carry the pointer
            // (its `lParam` is not the one that was given to `CreateWindowExW` on every system), and the user data
            // cannot be written before the window exists either — which is why the window never opened at all:
            // the procedure did not find its state and gave up.
            //
            // SAFETY: the message carries a live `CREATESTRUCT`.
            let params = unsafe { &*(lparam.0 as *const CreateStruct) };
            // SAFETY: the window is ours; the user data is ours alone.
            unsafe {
                let _ = SetWindowLongPtrW(
                    hwnd,
                    GWLP_USERDATA,
                    params.lp_create_params as isize,
                );
            }
            // TRUE: the creation goes on.
            LRESULT(1)
        }
        WM_CREATE => {
            // The state of the window was put in the user data by `WM_NCCREATE`, before the window existed.
            if !on_create(hwnd) {
                return LRESULT(-1);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_NOTIFY => {
            on_notify(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            on_destroy(hwnd);
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

/// A null-terminated UTF-16 string, ready for `CreateWindowExW`.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the controls of the window. Returns whether they are all there.
fn on_create(hwnd: HWND) -> bool {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return false;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return false;
    };
    let Some(instance) = module_instance() else {
        return false;
    };
    let layout = shared.layout.clone();
    let mut handles: Vec<HWND> = Vec::with_capacity(layout.controls.len());
    let mut tabs = HWND::default();
    for (index, slot) in layout.controls.iter().enumerate() {
        let (class, text, style): (PCWSTR, Vec<u16>, WINDOW_STYLE) = match slot.kind {
            ControlKind::Label(label) => (
                w!("Static"),
                wide(label),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            ),
            ControlKind::LanguageCombo => (
                w!("ComboBox"),
                wide(""),
                WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
            ControlKind::Combo(_) => (
                w!("ComboBox"),
                wide(""),
                WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
            ControlKind::Check(toggle) => (
                w!("Button"),
                wide(toggle_label(toggle)),
                WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
            ControlKind::PluginCheck(id) => (
                w!("Button"),
                wide(plugin_label(id)),
                WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
            ControlKind::TabStrip => (
                w!("SysTabControl32"),
                wide(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
        };
        let id = if matches!(slot.kind, ControlKind::TabStrip) {
            ID_TABS
        } else {
            index
        };
        // SAFETY: the class is a standard control; the parameters are plain values.
        let control = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_CONTROLPARENT,
                class,
                PCWSTR(text.as_ptr()),
                style,
                slot.rect.left,
                slot.rect.top,
                slot.rect.width(),
                slot.rect.height(),
                hwnd,
                HMENU(id as *mut c_void),
                instance,
                None,
            )
        };
        let Ok(control) = control else {
            return false;
        };
        if matches!(slot.kind, ControlKind::TabStrip) {
            tabs = control;
        }
        handles.push(control);
    }
    shared.handles = handles;
    shared.tabs = tabs;
    // The strip of the pages gets its pages.
    //
    // Every page is inserted at **its own index**, and not at the end of the strip (the "append" idiom, the index
    // -1): the control refuses that one on this machine - the insert fails, the strip stays empty and the user cannot
    // reach any page (not even the page of the plugins). Inserting at the index of the page works everywhere.
    for (index, page) in PAGES.iter().enumerate() {
        let mut text = wide(page_title(*page));
        let mut item = TabItem {
            mask: TCIF_TEXT,
            dw_state: 0,
            dw_state_mask: 0,
            psz_text: text.as_mut_ptr(),
            cch_text_max: text.len() as i32,
            i_image: -1,
            l_param: 0,
        };
        // SAFETY: the item points at a live buffer with its terminator.
        let inserted = unsafe {
            SendMessageW(
                tabs,
                TCM_INSERTITEMW,
                WPARAM(index),
                LPARAM(&mut item as *mut TabItem as isize),
            )
        };
        if inserted.0 < 0 {
            // The window is still opened (the user must be able to see what is wrong), but the title says it: a
            // window whose pages cannot be reached is not a window that works.
            set_title(
                hwnd,
                "IME1.0 设置 - 页签创建失败，只有第一页能用",
            );
            break;
        }
    }
    // The drop-down lists get their items.
    for (index, slot) in layout.controls.iter().enumerate() {
        let handle = shared.handles[index];
        match slot.kind {
            ControlKind::LanguageCombo => {
                for language in imesettings::LANGUAGES {
                    let text = wide(imesettings::display_name(language));
                    // SAFETY: a plain call on our own combo box; the string outlives the call.
                    unsafe {
                        let _ = SendMessageW(
                            handle,
                            CB_ADDSTRING,
                            WPARAM(0),
                            LPARAM(text.as_ptr() as isize),
                        );
                    }
                }
            }
            ControlKind::Combo(combo) => {
                for item in combo_items(combo) {
                    let text = wide(&item);
                    // SAFETY: a plain call on our own combo box; the string outlives the call.
                    unsafe {
                        let _ = SendMessageW(
                            handle,
                            CB_ADDSTRING,
                            WPARAM(0),
                            LPARAM(text.as_ptr() as isize),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    // The values of the controls.
    shared.refresh();
    shared.show_page(SettingsPage::General);
    // The buttons at the bottom.
    for (rect, kind) in layout.buttons.iter() {
        let id = match kind {
            SettingsButton::Save => ID_SAVE,
            SettingsButton::Cancel => ID_CANCEL,
            SettingsButton::Defaults => ID_DEFAULTS,
        };
        let label = match kind {
            SettingsButton::Save => "保存",
            SettingsButton::Cancel => "取消",
            SettingsButton::Defaults => "恢复默认",
        };
        let base = if matches!(kind, SettingsButton::Save) {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let text = wide(label);
        // SAFETY: the class is a standard control; the parameters are plain values.
        let button = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_CONTROLPARENT,
                w!("Button"),
                PCWSTR(text.as_ptr()),
                WINDOW_STYLE(base as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                hwnd,
                HMENU(id as *mut c_void),
                instance,
                None,
            )
        };
        if button.is_err() {
            return false;
        }
    }
    true
}

/// A click on one of the buttons, or a change of one of the drop-down lists.
fn on_command(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let id = wparam.0 & 0xFFFF;
    let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
    // The drop-down lists report their change with CBN_SELCHANGE, the buttons with BN_CLICKED.
    if code == BN_CLICKED {
        match id {
            ID_SAVE => save_and_close(hwnd),
            ID_CANCEL => {
                // SAFETY: our own window.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            ID_DEFAULTS => {
                let Some(cell) = (unsafe { shared_of(hwnd) }) else {
                    return;
                };
                let Ok(mut shared) = cell.try_borrow_mut() else {
                    return;
                };
                shared.show_defaults();
            }
            _ => {}
        }
        return;
    }
    // A drop-down list: the language changes the page, the scheme changes the keys.
    if lparam.0 == 0 {
        return;
    }
    // The message carries the handle of the control that sent it.
    let source = windows::Win32::Foundation::HWND(lparam.0 as *mut c_void);
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return;
    };
    let Some(index) = shared
        .handles
        .iter()
        .position(|handle| *handle == source)
    else {
        return;
    };
    match shared.layout.controls[index].kind {
        ControlKind::LanguageCombo => {
            // SAFETY: a plain call on our own combo box.
            let selected = unsafe { SendMessageW(source, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
            if selected >= 0 {
                shared.language = (selected as usize).min(imesettings::LANGUAGES.len() - 1);
                shared.refresh();
            }
        }
        ControlKind::Combo(Combo::KeymapPreset) => {
            // The scheme changes the keys at once: that is the point of the schemes.
            // SAFETY: a plain call on our own combo box.
            let selected = unsafe { SendMessageW(source, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
            if selected < 0 {
                return;
            }
            let Some(preset) = imesettings::KeymapPreset::ALL.get(selected as usize) else {
                return;
            };
            let language = imesettings::LANGUAGES[shared.language];
            let mut value = shared.settings.get(language);
            if value.apply_preset(*preset) {
                shared.settings.set(language, value);
                shared.refresh();
            }
        }
        _ => {}
    }
}

/// The strip of the pages reports the page the user picked.
fn on_notify(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let _ = wparam;
    // SAFETY: the message carries the header of the notification.
    let header = unsafe { &*(lparam.0 as *const NmHdr) };
    if header.code != TCN_SELCHANGE {
        return;
    }
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return;
    };
    if header.hwnd_from != shared.tabs {
        return;
    }
    // SAFETY: a plain call on our own tab control.
    let index = unsafe { SendMessageW(shared.tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
    if index < 0 {
        return;
    }
    let Some(page) = PAGES.get(index as usize).copied() else {
        return;
    };
    shared.show_page(page);
}

/// Write the settings and close the window. A file that cannot be written is reported in the title bar of the window
/// (there is no other place to say it): the window stays open.
fn save_and_close(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return;
    };
    let settings = shared.collect();
    let Some(path) = imesettings::settings_path() else {
        set_title(hwnd, "IME1.0 设置 - 没有 LOCALAPPDATA，设置无法保存");
        return;
    };
    if let Err(e) = settings.save(&path) {
        set_title(hwnd, &format!("IME1.0 设置 - 保存失败：{e}"));
        return;
    }
    shared.settings = settings;
    shared.saved = true;
    let sink = shared.sink.clone();
    sink(SettingsEvent::Saved);
    // SAFETY: our own window.
    unsafe {
        let _ = DestroyWindow(hwnd);
    };
}

/// The window is gone: tell the text service whether the settings were saved, and give the focus back to the editor.
fn on_destroy(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = cell.try_borrow() else {
        return;
    };
    // SAFETY: the last message of this window; the state is owned by the `SettingsWindow`, not by the user data.
    unsafe {
        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    let saved = shared.saved;
    let sink = shared.sink.clone();
    let owner = shared.owner;
    if !saved {
        sink(SettingsEvent::Cancelled);
    }
    if !owner.is_invalid() {
        // SAFETY: the window that had the focus before; it is still alive (it is the host's editor).
        unsafe {
            let _ = SetForegroundWindow(owner);
        }
    }
}

fn set_title(hwnd: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: the string is a live local with its terminator.
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_title_and_its_own_controls() {
        for page in PAGES {
            assert!(!page_title(page).is_empty(), "{page:?}");
            let rows = rows_of(page);
            assert!(!rows.is_empty(), "{page:?}");
        }
    }

    #[test]
    fn the_controls_of_a_page_do_not_overlap_each_other() {
        let layout = compute_layout();
        for page in PAGES {
            let rects: Vec<Rect> = layout
                .controls
                .iter()
                .filter(|slot| slot.page == page)
                .map(|slot| slot.rect)
                .collect();
            for (i, a) in rects.iter().enumerate() {
                for b in rects.iter().skip(i + 1) {
                    let overlap =
                        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
                    assert!(!overlap, "{page:?}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    /// Every text of the settings window has a box that is big enough for it: a text that is cut off is a sentence the
    /// user cannot read (the hint of the page of the plugins was cut in half).
    #[test]
    fn every_text_of_the_settings_window_has_a_box_that_is_big_enough() {
        let layout = compute_layout();
        for slot in &layout.controls {
            let ControlKind::Label(text) = slot.kind else {
                continue;
            };
            let lines = lines_of(text, LABEL_TEXT_WIDTH);
            assert!(
                slot.rect.height() >= lines * ROW,
                "the text {:?} needs {lines} lines, but its box is {} pixels tall",
                text,
                slot.rect.height()
            );
        }
    }

    /// A text that does not fit on one line gets two rows, and a short one stays on one.
    #[test]
    fn a_text_that_does_not_fit_gets_two_rows() {
        assert_eq!(lines_of("短", LABEL_TEXT_WIDTH), 1, "a short text is one line");
        assert_eq!(
            lines_of(&"长".repeat(60), LABEL_TEXT_WIDTH),
            2,
            "a long text is wrapped and needs two rows"
        );
    }

    #[test]
    fn the_window_is_big_enough_for_its_content() {
        let layout = compute_layout();
        for slot in &layout.controls {
            assert!(slot.rect.right <= layout.width, "{slot:?}");
            assert!(slot.rect.bottom <= layout.height, "{slot:?}");
        }
        assert!(layout.height > 200, "the pages need room");
    }

    #[test]
    fn the_buttons_are_at_the_bottom_and_next_to_each_other() {
        let layout = compute_layout();
        assert_eq!(layout.buttons.len(), 3);
        let kinds: Vec<SettingsButton> = layout.buttons.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            vec![
                SettingsButton::Save,
                SettingsButton::Cancel,
                SettingsButton::Defaults
            ]
        );
        for (rect, _) in &layout.buttons {
            assert!(rect.bottom <= layout.height);
            assert!(rect.top > 0);
        }
        for pair in layout.buttons.windows(2) {
            assert!(pair[0].0.right <= pair[1].0.left);
        }
    }

    #[test]
    fn every_toggle_has_a_label() {
        for toggle in [
            Toggle::SpaceConfirms,
            Toggle::AutoCompose,
            Toggle::InputMode,
            Toggle::LaserUi,
            Toggle::PunctPaging,
        ] {
            assert!(!toggle_label(toggle).is_empty());
        }
    }

    #[test]
    fn the_three_ways_of_the_tab_key_are_the_ones_the_user_asked_for() {
        let mut settings = LanguageSettings::default();
        // the programmer habit: Tab moves the highlight
        assert_eq!(tab_behaviour_of(&settings), TabBehaviour::Cycle);
        apply_tab_behaviour(&mut settings, TabBehaviour::Commit);
        assert_eq!(tab_behaviour_of(&settings), TabBehaviour::Commit);
        assert!(!settings.tab_cycles && !settings.tab_to_host);
        apply_tab_behaviour(&mut settings, TabBehaviour::Host);
        assert_eq!(tab_behaviour_of(&settings), TabBehaviour::Host);
        assert!(settings.tab_to_host);
        apply_tab_behaviour(&mut settings, TabBehaviour::Cycle);
        assert_eq!(tab_behaviour_of(&settings), TabBehaviour::Cycle);
        assert!(!settings.tab_to_host, "the key comes back from the host");
    }

    #[test]
    fn the_plugins_page_has_one_check_box_for_every_plugin() {
        let rows = rows_of(SettingsPage::Plugins);
        let checks: Vec<&str> = rows
            .iter()
            .filter_map(|kind| match kind {
                ControlKind::PluginCheck(id) => Some(*id),
                _ => None,
            })
            .collect();
        let expected: Vec<&str> = imesettings::PLUGINS.iter().map(|(id, _)| *id).collect();
        assert_eq!(checks, expected, "one check box for every plugin");
    }

    #[test]
    fn every_plugin_has_a_name_to_show() {
        for (id, _) in imesettings::PLUGINS {
            assert!(!plugin_label(id).is_empty(), "{id}");
        }
    }

}
