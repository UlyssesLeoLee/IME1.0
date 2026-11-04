//! The installation wizard: the designed window the person sees when they start the setup.
//!
//! Everything is drawn by hand (GDI) so that the window looks the same on every Windows edition: a brand band with
//! the product name, a rainbow stripe (the colours of the logo), the page, and a footer with the buttons and the logo
//! in the bottom-right corner.
//!
//! Rules that the wizard follows:
//! * it never changes anything by itself: it collects the choices and hands them back to the installer (`cli`), which
//!   owns the plan and the rollback;
//! * the install runs on a worker thread, so the window stays alive and shows what is happening;
//! * the cancel button is only offered before the install starts: an install that is half done has to be able to roll
//!   itself back, and that is the installer's job, not the window's.

use std::cell::RefCell;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DeleteObject, EndPaint,
    ExcludeClipRect, FF_DONTCARE, FW_BOLD, FW_NORMAL, GetDeviceCaps, GetStockObject,
    GetTextExtentPoint32W, HBRUSH, HDC, HFONT, HGDIOBJ, HORZRES, InvalidateRect,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_NULL, RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW,
    REDRAW_WINDOW_FLAGS, RedrawWindow, RoundRect, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_RETURN};
use windows::Win32::UI::Shell::{
    BROWSEINFOW, FILEOPENDIALOGOPTIONS, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
    FileOpenDialog, IFileDialog, IShellItem, SHBrowseForFolderW, SHCreateItemFromParsingName,
    SHGetPathFromIDListW, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, ACCEL_VIRT_FLAGS, CreateAcceleratorTableW, CreateWindowExW, DI_NORMAL, DefWindowProcW,
    DestroyAcceleratorTable, DestroyIcon, DestroyWindow, DispatchMessageW, DrawIconEx, EN_CHANGE,
    ES_AUTOHSCROLL, ES_LEFT, FVIRTKEY, GWLP_USERDATA, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HACCEL, HICON, HMENU, HWND_TOP,
    ICON_BIG, ICON_SMALL, IDC_ARROW, IMAGE_ICON, IsWindow, KillTimer, LR_LOADFROMFILE,
    LWA_ALPHA, LoadCursorW, LoadImageW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SPI_GETWORKAREA, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, SystemParametersInfoW, TranslateAcceleratorW, TranslateMessage, WINDOW_STYLE,
    WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN,
    WM_NCDESTROY, WM_PAINT, WM_SETFONT, WM_SETICON, WM_TIMER, WNDCLASSEXW, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_EX_CONTROLPARENT, WS_EX_LAYERED, WS_OVERLAPPEDWINDOW,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{IUnknown, PCWSTR, w};

use crate::paint::{self, ColorRef, Rgb, Surface};

use crate::text::{Lang, Msg, wizard_text};

thread_local! {
    /// The brush behind the text field.
    ///
    /// It lives outside the state of the window on purpose: the control asks the parent for its brush **while the
    /// parent is painting**, and at that moment the state is borrowed for the paint. A brush that cannot be found
    /// there would be a null brush, and a control that gets a null brush erases nothing and paints no text.
    static PATH_BRUSH: RefCell<HBRUSH> = RefCell::new(HBRUSH::default());
}

/// The logo of the product, embedded in the program (the multi-resolution Windows icon of the design).
const LOGO_ICO: &[u8] = include_bytes!("../assets/ime1.0.ico");

/// The window class of the wizard.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.Wizard");

/// The message the worker thread posts when it wrote a line.
const WM_APP_LINE: u32 = WM_APP + 1;

/// How opaque the window is: **80%**, the same material as every panel of the product (see `LaserTheme::MATERIAL_ALPHA`).
///
/// The value is the one Windows takes for a layered window: 0 is invisible, 255 is opaque. 80 % of 255 is 204.
const MATERIAL_ALPHA: u8 = 204;

/// The timer that redraws the window while the install runs.
const TIMER_PROGRESS: usize = 1;

/// The colours of the wizard.
mod colour {
    use pime_core::style::{BRAND_RAINBOW, LaserTheme};
    use windows::Win32::Foundation::COLORREF;
    /// The rainbow of the brand: the seven soft colours of the logo. They are light enough for dark text, and they
    /// are the colours the whole window is built from.
    ///
    /// They come from the core, so the wizard and the input method show the **same** rainbow (the logo of the
    /// distribution and the light of the input method are one thing).
    pub const RAINBOW: [COLORREF; 7] = [
        COLORREF(BRAND_RAINBOW[0].to_bgr()),
        COLORREF(BRAND_RAINBOW[1].to_bgr()),
        COLORREF(BRAND_RAINBOW[2].to_bgr()),
        COLORREF(BRAND_RAINBOW[3].to_bgr()),
        COLORREF(BRAND_RAINBOW[4].to_bgr()),
        COLORREF(BRAND_RAINBOW[5].to_bgr()),
        COLORREF(BRAND_RAINBOW[6].to_bgr()),
    ];
    /// The text on the rainbow (the band and the primary button): a deep ink that reads on every one of the seven.
    pub const INK: COLORREF = COLORREF(LaserTheme::ON_LIGHT.to_bgr());
    /// The accent of the window (the check marks of the options, the primary button): the laser cyan.
    pub const ACCENT: COLORREF = COLORREF(LaserTheme::ACCENT.to_bgr());
    /// The pale blue of the laser beam: **translucent light**, not a painted block of colour.
    ///
    /// It is what the filled accent surfaces are made of (the primary button, the tick of a checked box), so that
    /// they read as the light of the input method instead of as a slab of saturated blue.
    pub const LASER: COLORREF = COLORREF(LaserTheme::BEAM_HEAD.to_bgr());
    /// The even paler core of the laser light: the top of a lit surface.
    pub const LASER_CORE: COLORREF = COLORREF(LaserTheme::BEAM_CORE.to_bgr());
    /// The halo of the laser light: the light a lit surface throws into the air around it.
    pub const LASER_HALO: COLORREF = COLORREF(LaserTheme::GLOW_MID.to_bgr());
    /// The fill of a quiet button (the one that opens the folder picker).
    pub const TINT: COLORREF = COLORREF(LaserTheme::PANEL_LIT.to_bgr());
    /// The page background: the deep space the light lives in.
    pub const PAGE: COLORREF = COLORREF(LaserTheme::PAGE.to_bgr());
    /// The panel (the folder, the options): the glass over the page.
    pub const PANEL: COLORREF = COLORREF(LaserTheme::PANEL.to_bgr());
    /// The line around a panel: the rim of the light.
    pub const BORDER: COLORREF = COLORREF(LaserTheme::RIM.to_bgr());
    /// The headline of the page.
    pub const TITLE: COLORREF = COLORREF(LaserTheme::TEXT.to_bgr());
    /// The text of the page.
    pub const BODY: COLORREF = COLORREF(LaserTheme::TEXT.to_bgr());
    /// The dimmed text (the version, the hint).
    pub const DIM: COLORREF = COLORREF(LaserTheme::DIM.to_bgr());
    /// The frame of a button.
    pub const BUTTON: COLORREF = COLORREF(LaserTheme::RIM.to_bgr());
    /// The text of a secondary button.
    pub const BUTTON_TEXT: COLORREF = COLORREF(LaserTheme::TEXT.to_bgr());
    /// The shadow under a tile, a panel or a button (it is drawn with a soft edge, not as a grey box).
    pub const SHADOW: COLORREF = COLORREF(LaserTheme::PAGE.to_bgr());
}

/// What the person chose in the wizard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Install the program into the folder that was chosen.
    Install(Install),
    /// Put the install that is already there back in order: the files are written again and the text service is
    /// registered again. The folder and the options of the earlier install are the ones that are used.
    Repair(Install),
    /// Remove the install that is already there and install the program again.
    Reinstall(Install),
    /// Remove the install that is already there.
    Uninstall,
}

/// The folder and the options of an install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// The folder the program is installed in.
    pub prefix: PathBuf,
    /// Register the text service with Windows.
    pub register: bool,
    /// Add the entry to "Apps & Features".
    pub arp: bool,
    /// Install the VS Code extension.
    pub vscode: bool,
}

/// The page the wizard shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Welcome,
    Progress,
    Done,
}

/// The job the window is doing. The same window installs, repairs, installs again and removes: only the pages, the
/// buttons and the texts differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Install,
    Repair,
    Reinstall,
    Uninstall,
}

/// One button of the wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Browse,
    Cancel,
    Install,
    Repair,
    Reinstall,
    Uninstall,
    Close,
}

/// Where a button is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hit {
    rect: RECT,
    button: Button,
}

/// The fonts of the wizard.
struct Fonts {
    title: HFONT,
    big: HFONT,
    body: HFONT,
    small: HFONT,
}

impl Fonts {
    /// The fonts of the wizard, scaled to the DPI of the system (96 is 100 %).
    fn new(dpi: i32) -> Fonts {
        Fonts {
            title: ui_font(-26 * dpi / 96, FW_BOLD.0 as i32),
            big: ui_font(-15 * dpi / 96, FW_BOLD.0 as i32),
            body: ui_font(-13 * dpi / 96, FW_NORMAL.0 as i32),
            small: ui_font(-12 * dpi / 96, FW_NORMAL.0 as i32),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.title, self.big, self.body, self.small] {
            // SAFETY: every font was created by `ui_font` and is not selected anywhere any more.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            };
        }
    }
}

/// A font of the system UI family (it has the Chinese characters the wizard needs).
fn ui_font(height: i32, weight: i32) -> HFONT {
    // SAFETY: the face name is a static string.
    let font = unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            // The text is anti-aliased: the default quality draws the glyphs with hard edges, which does not go with
            // a window whose shapes have smooth ones.
            ANTIALIASED_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            w!("Microsoft YaHei UI"),
        )
    };
    if font.is_invalid() {
        // SAFETY: a stock object; it must not be deleted.
        let stock = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        HFONT(stock.0)
    } else {
        font
    }
}

/// The three options of the welcome page. They come from the command line (`--no-register` and friends), so the
/// window starts with the state the person asked for on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub register: bool,
    pub arp: bool,
    pub vscode: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            register: true,
            arp: true,
            vscode: true,
        }
    }
}

/// The state the window procedure needs.
struct State {
    page: Page,
    /// The job the window is doing (the texts of the pages and the buttons follow it).
    job: Job,
    lang: Lang,
    version: String,
    prefix: PathBuf,
    /// The text of the welcome page in the uninstall mode (what is going to be removed).
    summary: String,
    register: bool,
    arp: bool,
    vscode: bool,
    /// The lines of the install that are shown while it runs.
    log: Vec<String>,
    /// The text of the last page.
    outcome: String,
    /// Whether the install succeeded.
    ok: bool,
    /// The buttons of the current page (drawn by the paint handler).
    buttons: Vec<Hit>,
    /// The rectangles of the option rows (they are clickable).
    options: Vec<RECT>,
    /// The panel with the installation folder: clicking it opens the folder picker.
    folder_panel: Option<RECT>,
    /// The field where the installation folder is typed (a real edit control of Windows, so it takes an IME).
    path_edit: Option<HWND>,
    /// The table of the keys of the window: Enter starts the install, also while the field has the focus.
    accelerator: HACCEL,
    /// Where the field of the folder is in the client area of the window.
    ///
    /// The page is drawn into a picture and copied over the window afterwards, and the field is a window of its own.
    /// The place of the field is cut out of that copy, or the page would paint over the text of the field.
    path_field: Option<RECT>,
    /// Whether the field has to paint itself again (it was moved).
    path_dirty: bool,
    /// Why the typed folder cannot be used (it is written under the field).
    path_error: Option<String>,
    /// The folder the program really goes to. It is not always the folder that was typed: the product folder is
    /// appended to it, so the program never lands in the root of a drive. It is `None` while the field says
    /// something that cannot be used at all.
    install_dir: Option<PathBuf>,
    /// The folder of an installation from before the rename, if it is still on the machine (it has to be removed
    /// first: the old and the new version would otherwise be in each other's way).
    legacy_hint: Option<String>,
    /// The install that is already on the machine, if there is one. When it is there, the first page is not the page
    /// of a fresh install: it offers to repair that install, to install it again or to remove it.
    existing: Option<crate::detect::Existing>,
    /// What the uninstall of that install would remove. The page of the uninstall shows it, so the person sees what
    /// is going to disappear before it disappears. It is empty when the uninstall cannot be planned.
    uninstall_summary: Option<String>,
    /// The paths of the system (the rules of a usable install folder are checked against them).
    system: crate::plan::SystemPaths,
    /// The button the person clicked (the loops look at it).
    pending: Option<Button>,
    /// Whether the window was closed.
    closed: bool,
    /// The logo of the product, loaded at the sizes it is drawn at (the size of the picture, the handle).
    logos: Vec<(i32, HICON)>,
    /// The file the icon was loaded from (it is deleted when the wizard closes).
    logo_file: Option<PathBuf>,
    /// The fonts of the wizard.
    fonts: Fonts,
    /// The DPI scale (1.0 is 96 dpi).
    scale: i32,
}

/// The wizard of one thread.
pub struct Wizard {
    hwnd: HWND,
    state: Rc<RefCell<State>>,
}

impl Wizard {
    /// Create the window for an install (nothing is shown yet).
    ///
    /// `existing` is the install that is already on the machine, if there is one. When it is there, the first page is
    /// not the page of a fresh install: it says what is installed and offers to repair it, to install it again or to
    /// remove it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lang: Lang,
        version: &str,
        default_prefix: &Path,
        options: Options,
        system: &crate::plan::SystemPaths,
        existing: Option<crate::detect::Existing>,
        uninstall_summary: Option<String>,
    ) -> Option<Wizard> {
        Wizard::create(
            lang,
            version,
            default_prefix,
            options,
            Job::Install,
            String::new(),
            system,
            existing,
            uninstall_summary,
        )
    }

    /// Create the window for an uninstall. `summary` says what is going to be removed.
    pub fn new_uninstall(
        lang: Lang,
        version: &str,
        prefix: &Path,
        summary: String,
        system: &crate::plan::SystemPaths,
    ) -> Option<Wizard> {
        Wizard::create(
            lang,
            version,
            prefix,
            Options::default(),
            Job::Uninstall,
            summary,
            system,
            None,
            None,
        )
    }

    /// Create the window (nothing is shown yet).
    #[allow(clippy::too_many_arguments)]
    fn create(
        lang: Lang,
        version: &str,
        default_prefix: &Path,
        options: Options,
        job: Job,
        summary: String,
        system: &crate::plan::SystemPaths,
        existing: Option<crate::detect::Existing>,
        uninstall_summary: Option<String>,
    ) -> Option<Wizard> {
        // When the program is already on the machine, the page is about that install: the folder it is in is the
        // folder the window talks about, and it is not changed by the person.
        let (default_prefix, existing) = match existing {
            Some(existing) => (existing.prefix.clone(), Some(existing)),
            None => (default_prefix.to_path_buf(), None),
        };
        let default_prefix = default_prefix.as_path();
        let instance = module_instance()?;
        register_class(instance).ok()?;
        // The logo is written to a private file; the pictures are loaded from it when they are drawn (at the size
        // they are drawn at, so that they are never enlarged - that is what makes a logo blurry).
        let logo_file = write_logo_file();
        // The scale of the window: it follows the DPI of the system (96 is 100 %). The process is made per-monitor
        // aware before the window exists, so Windows scales the frame of the window as well.
        // SAFETY: a process-wide setting, made before any window of this process exists.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let scale = {
            // SAFETY: a plain query of the system DPI.
            let dpi = unsafe { GetDpiForSystem() };
            if dpi == 0 { 96 } else { dpi as i32 }
        };
        // The brush behind the text field (its background is the colour of the panel). It lives outside the state of
        // the window on purpose: the control asks the parent for its brush while the parent is painting, and at that
        // moment the state is borrowed for the paint. A brush that cannot be found there would be a null brush, and a
        // control that gets a null brush erases nothing and paints no text.
        // SAFETY: a plain call; the brush lives as long as the window does.
        let path_brush = unsafe { CreateSolidBrush(colour::PANEL) };
        PATH_BRUSH.with(|cell| *cell.borrow_mut() = path_brush);
        // Enter starts the install, also while the folder field has the focus. The table of the keys is the way of
        // Windows to say that: the key is read before it reaches the control that has the focus.
        // SAFETY: a plain call; the table lives as long as the state does.
        let accelerator = unsafe {
            CreateAcceleratorTableW(&[ACCEL {
                fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0),
                key: VK_RETURN.0,
                cmd: ACCEL_INSTALL,
            }])
        }
        .ok();
        let state = Rc::new(RefCell::new(State {
            page: Page::Welcome,
            job,
            lang,
            version: version.to_string(),
            prefix: default_prefix.to_path_buf(),
            summary,
            register: options.register,
            arp: options.arp,
            vscode: options.vscode,
            log: Vec::new(),
            outcome: String::new(),
            ok: true,
            buttons: Vec::new(),
            options: Vec::new(),
            folder_panel: None,
            path_edit: None,
            accelerator: accelerator.unwrap_or_default(),
            path_field: None,
            path_dirty: false,
            path_error: None,
            install_dir: None,
            legacy_hint: legacy_hint(job, lang, system),
            existing,
            uninstall_summary,
            system: system.clone(),
            pending: None,
            closed: false,
            logos: Vec::new(),
            logo_file,
            fonts: Fonts::new(scale),
            scale,
        }));
        let raw = Rc::into_raw(Rc::clone(&state));
        // The folder the program goes to is worked out once, so that the page says it from the first moment and
        // not only after the person typed something.
        refresh_path(&mut state.borrow_mut());
        // The window is centred on the work area. It must not always open in the same corner: two wizards of the
        // setup that lie exactly on top of each other turn one click into the wrong answer (a click meant for the
        // "Install" of an old window would hit the "Uninstall" of the new one).
        let width = 760 * scale / 96;
        // The window is a little taller than the first version: the page now says where the program really goes
        // to, and those lines need a room of their own.
        let height = 580 * scale / 96;
        let (x, y) = centred_origin(width, height);
        // SAFETY: the class is registered; the only pointer is the one we hand over in USERDATA.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_LAYERED,
                CLASS_NAME,
                w!("IME1.0"),
                WS_OVERLAPPEDWINDOW | WS_SYSMENU | WS_CLIPCHILDREN,
                x,
                y,
                width,
                height,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
        }
        .ok()?;
        // The window is made of the same material as the panels of the product: **80% opaque**, so the desktop shows
        // through it a little. The colour key is not used (0 with LWA_ALPHA means "no key"), only the alpha.
        //
        // SAFETY: our own window; the call only sets how the window is composited.
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), MATERIAL_ALPHA, LWA_ALPHA);
        };
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        // The icon of the window itself (the title bar, the taskbar, Alt+Tab): the logo of the product.
        {
            let mut state = state.borrow_mut();
            let big = logo_icon(&mut state, 32);
            let small = logo_icon(&mut state, 16);
            if let Some(icon) = big {
                // SAFETY: our own window and our own icon; the icon lives as long as the state does.
                unsafe {
                    let _ = SendMessageW(
                        hwnd,
                        WM_SETICON,
                        WPARAM(ICON_BIG as usize),
                        LPARAM(icon.0 as isize),
                    );
                };
            }
            if let Some(icon) = small {
                // SAFETY: our own window and our own icon; the icon lives as long as the state does.
                unsafe {
                    let _ = SendMessageW(
                        hwnd,
                        WM_SETICON,
                        WPARAM(ICON_SMALL as usize),
                        LPARAM(icon.0 as isize),
                    );
                };
            }
        }
        // The field where the install folder is typed. It is a real edit control of Windows: it has a caret, a
        // selection and it takes the input of an IME (which matters for a folder name that is written in Chinese).
        {
            let (prefix, font) = {
                let state = state.borrow();
                (state.prefix.clone(), state.fonts.body)
            };
            // The size the field will have: the width of the panel of the folder without its frame and without the
            // button that opens the picker, and the height of the line of the field.
            let mut client = RECT::default();
            // SAFETY: a plain query of our own window.
            unsafe {
                let _ = GetClientRect(hwnd, &mut client);
            }
            let unit = |v: i32| v * scale / 96;
            // The field is made with the place it will have: an edit control lays its text out for the width it is
            // made with, and it only lays it out again when the width really changes.
            let field = path_field_rect(&unit, client.right, 0);
            let edit = create_path_edit(
                hwnd,
                instance,
                &prefix,
                font,
                field.right - field.left,
                field.bottom - field.top,
            );
            state.borrow_mut().path_edit = edit;
        }
        Some(Wizard { hwnd, state })
    }

    /// The welcome page: the person chooses the folder and the options and starts the install.
    pub fn welcome(&mut self) -> Option<Choice> {
        {
            let mut state = self.state.borrow_mut();
            state.page = Page::Welcome;
            state.buttons = Vec::new();
            state.options = Vec::new();
            state.pending = None;
        }
        self.show();
        // The field of the folder has the focus: the person can type the place right away, and the text that is
        // there is selected, so typing replaces it.
        {
            let edit = self.state.borrow().path_edit;
            if let Some(edit) = edit {
                // SAFETY: our own control.
                unsafe {
                    let _ = SetFocus(edit);
                    let _ = SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
                }
            }
        }
        loop {
            if !self.pump() {
                return None; // the window was closed
            }
            let pending = self.state.borrow_mut().pending.take();
            if pending == Some(Button::Cancel) {
                // The person changed their mind on the page of the uninstall: back to the page of the install that is
                // already there.
                let back = {
                    let mut state = self.state.borrow_mut();
                    if state.job == Job::Uninstall && state.existing.is_some() {
                        state.job = Job::Install;
                        state.summary = String::new();
                        true
                    } else {
                        false
                    }
                };
                if back {
                    continue;
                }
                return None;
            }
            // The button that was pressed says what the person wants to do with the program.
            let repair = pending == Some(Button::Repair);
            let reinstall = pending == Some(Button::Reinstall);
            let fresh = pending == Some(Button::Install);
            if pending == Some(Button::Uninstall) {
                // The first press shows what is going to be removed (the page of the uninstall); the second one does
                // it. A setup that removes something without saying what it removes is a setup that surprises.
                let confirmed = {
                    let mut state = self.state.borrow_mut();
                    if state.job == Job::Uninstall {
                        true
                    } else {
                        state.job = Job::Uninstall;
                        state.summary = state.uninstall_summary.clone().unwrap_or_default();
                        false
                    }
                };
                if confirmed {
                    return Some(Choice::Uninstall);
                }
                // SAFETY: our own window.
                unsafe {
                    let _ = InvalidateRect(self.hwnd, None, false);
                };
                continue;
            }
            if !(repair || reinstall || fresh) {
                continue;
            }
            {
                // The folder that is typed has to be one that the installer can use. When it is not, the window
                // says so and stays on the page: the person can correct it there and there.
                let (prefix, register, arp, vscode, problem) = {
                    let state = self.state.borrow();
                    if repair || reinstall {
                        // The folder of the install that is already there is the folder that is used: it is not worked
                        // out again (the product folder must not be put into it a second time).
                        (
                            state.prefix.clone(),
                            state.register,
                            state.arp,
                            state.vscode,
                            None::<String>,
                        )
                    } else {
                        match checked_prefix(&state.prefix, &state.system) {
                            Ok(prefix) => (
                                prefix,
                                state.register,
                                state.arp,
                                state.vscode,
                                None::<String>,
                            ),
                            Err(e) => (
                                state.prefix.clone(),
                                state.register,
                                state.arp,
                                state.vscode,
                                Some(e),
                            ),
                        }
                    }
                };
                if let Some(problem) = problem {
                    if let Ok(mut state) = self.state.try_borrow_mut() {
                        state.path_error = Some(problem);
                    }
                    // SAFETY: our own window.
                    unsafe {
                        let _ = InvalidateRect(self.hwnd, None, false);
                    };
                    continue;
                }
                if let Ok(mut state) = self.state.try_borrow_mut() {
                    state.prefix = prefix.clone();
                    state.install_dir = Some(prefix.clone());
                    state.job = if repair {
                        Job::Repair
                    } else if reinstall {
                        Job::Reinstall
                    } else {
                        Job::Install
                    };
                }
                let install = Install {
                    prefix,
                    register,
                    arp,
                    vscode,
                };
                return Some(if repair {
                    Choice::Repair(install)
                } else if reinstall {
                    Choice::Reinstall(install)
                } else {
                    Choice::Install(install)
                });
            }
        }
    }

    /// The progress page: `work` runs on a worker thread and writes its lines into the window.
    pub fn progress<F>(&mut self, work: F)
    where
        F: FnOnce(&mut dyn FnMut(&str)) + Send + 'static,
    {
        {
            let mut state = self.state.borrow_mut();
            state.page = Page::Progress;
            state.log.clear();
            state.buttons = Vec::new();
            state.options = Vec::new();
            state.pending = None;
        }
        self.show();
        // The window redraws itself every 100 ms while the install runs.
        // SAFETY: our own window.
        unsafe {
            let _ = SetTimer(self.hwnd, TIMER_PROGRESS, 100, None);
        };
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let worker_lines = Arc::clone(&lines);
        // A window handle is a raw pointer and cannot be sent to another thread: it travels as its address and is
        // turned back into a handle in the worker.
        let hwnd_raw = self.hwnd.0 as isize;
        let worker = std::thread::spawn(move || {
            let mut report = move |line: &str| {
                if let Ok(mut guard) = worker_lines.lock() {
                    guard.push(line.to_string());
                }
                // SAFETY: our own window; a destroyed handle only makes it a no-op.
                let hwnd = HWND(hwnd_raw as *mut c_void);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_LINE, WPARAM(0), LPARAM(0));
                };
            };
            work(&mut report);
        });
        // The window keeps answering until the worker is done.
        loop {
            if !self.pump() {
                break;
            }
            {
                let snapshot = lines.lock().map(|guard| guard.clone()).unwrap_or_default();
                let mut state = self.state.borrow_mut();
                state.log = snapshot;
            }
            if worker.is_finished() {
                break;
            }
        }
        let _ = worker.join();
        {
            let snapshot = lines.lock().map(|guard| guard.clone()).unwrap_or_default();
            let mut state = self.state.borrow_mut();
            state.log = snapshot;
        }
        // SAFETY: our own window.
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_PROGRESS);
        };
    }

    /// The last page: the result of the install.
    pub fn done(&mut self, ok: bool, message: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.page = Page::Done;
            state.ok = ok;
            state.outcome = message.to_string();
            state.buttons = Vec::new();
            state.options = Vec::new();
            state.pending = None;
        }
        self.show();
        loop {
            if !self.pump() {
                return;
            }
            let pending = self.state.borrow_mut().pending.take();
            if pending == Some(Button::Close) {
                return;
            }
        }
    }

    /// Show the window.
    fn show(&mut self) {
        // SAFETY: plain calls on our own window.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                windows::Win32::UI::WindowsAndMessaging::HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    /// One turn of the message loop. Returns false when the window was closed.
    fn pump(&mut self) -> bool {
        let mut message = MSG::default();
        // SAFETY: `message` is a live local.
        unsafe {
            if GetMessageW(&mut message, HWND::default(), 0, 0).0 <= 0 {
                return false;
            }
            // The keys of the window are read before the message reaches the control that has the focus: Enter in the
            // folder field starts the install.
            let accelerator = self.state.borrow().accelerator;
            if !accelerator.is_invalid() {
                let translated = TranslateAcceleratorW(self.hwnd, accelerator, &message);
                if translated != 0 {
                    return !self.state.borrow().closed;
                }
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        !self.state.borrow().closed
    }
}

impl Drop for Wizard {
    fn drop(&mut self) {
        // The logo and the file it was loaded from are taken out of the state before the window goes away.
        let (logos, logo_file) = {
            let state = self.state.borrow();
            (state.logos.clone(), state.logo_file.clone())
        };
        // SAFETY: the window may already be gone (the person closed it); the handle is then stale and must not be
        // used again.
        if unsafe { IsWindow(self.hwnd) }.as_bool() {
            // SAFETY: our own window; the window procedure gives the shared state back.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            };
        }
        for (_, icon) in logos {
            if !icon.is_invalid() {
                // SAFETY: every icon was loaded by `logo_icon` and is not used any more.
                unsafe {
                    let _ = DestroyIcon(icon);
                };
            }
        }
        if let Some(path) = logo_file {
            let _ = std::fs::remove_file(path);
        }
        // the brush behind the text field (it lives outside the state of the window, see `PATH_BRUSH`)
        PATH_BRUSH.with(|cell| {
            let mut slot = cell.borrow_mut();
            if !slot.is_invalid() {
                // SAFETY: the brush is ours and it is not selected in any device context any more.
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(slot.0));
                }
            }
            *slot = HBRUSH::default();
        });
        // the table of the keys of the window
        let accelerator = self.state.borrow().accelerator;
        if !accelerator.is_invalid() {
            // SAFETY: the table is ours and the window is gone.
            unsafe {
                let _ = DestroyAcceleratorTable(accelerator);
            }
        }
    }
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `Wizard::new`.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<State>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<State>;
    // SAFETY: null or the live pointer described above.
    unsafe { ptr.as_ref() }
}

/// The handle of this program (the wizard is drawn with its own resources).
fn module_instance() -> Option<HINSTANCE> {
    // SAFETY: a null name asks for the handle of the module the program runs in.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.ok()?;
    Some(HINSTANCE(module.0))
}

/// The top-left corner that centres a window of `width` x `height` on the work area of the screen. When the work
/// area cannot be read the window opens in the corner (it is still a usable window).
fn centred_origin(width: i32, height: i32) -> (i32, i32) {
    let mut area = RECT::default();
    // SAFETY: `area` is a live local; the call only writes the work area into it.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut area as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if read.is_err() {
        return (0, 0);
    }
    (
        area.left + (area.right - area.left - width) / 2,
        area.top + (area.bottom - area.top - height) / 2,
    )
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

/// Is this a device context that can be painted into?
fn is_device_context(dc: HDC) -> bool {
    if dc.is_invalid() {
        return false;
    }
    // SAFETY: a plain query of the device context.
    unsafe { GetDeviceCaps(dc, HORZRES) > 0 }
}

/// The window procedure. Every message runs inside a panic guard: a panic that unwound out of this `extern "system"`
/// frame would abort the setup.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        window_message(hwnd, msg, wparam, lparam)
    }));
    match result {
        Ok(value) => value,
        Err(_) => {
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            on_click(hwnd, lparam);
            LRESULT(0)
        }
        WM_COMMAND => {
            // the field of the folder: what the person typed is the folder of the install
            let id = wparam.0 & 0xFFFF;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if id == PATH_EDIT_ID && code == EN_CHANGE {
                on_path_changed(hwnd);
            }
            if id == ACCEL_INSTALL as usize && (code == 0 || code == 1) {
                // Enter: the install starts, also while the folder field has the focus.
                // (The table of the keys sends its command with the note 1 in the high part of the word.)
                let cell = unsafe { shared_of(hwnd) };
                let wanted = cell
                    .and_then(|cell| cell.try_borrow().ok())
                    .map(|state| state.page == Page::Welcome && state.job != Job::Uninstall)
                    .unwrap_or(false);
                if wanted {
                    if let Some(cell) = cell
                        && let Ok(mut state) = cell.try_borrow_mut()
                    {
                        // Enter does what the primary button does: a repair when the program is already installed,
                        // an install when it is not.
                        state.pending = Some(if state.existing.is_some() {
                            Button::Repair
                        } else {
                            Button::Install
                        });
                    }
                    // SAFETY: our own window.
                    unsafe {
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            // The field has the colours of the page: the text of the page on the panel of the page.
            //
            // The brush comes from outside the state of the window: the control asks for it while the parent is
            // painting, and a null brush makes the control erase nothing and paint no text.
            let brush = PATH_BRUSH.with(|cell| *cell.borrow());
            // The device context of the control is in `wparam`; `lparam` carries the handle of the control itself.
            let dc = HDC(wparam.0 as *mut c_void);
            if is_device_context(dc) {
                // SAFETY: the message carries the device context of the control.
                unsafe {
                    let _ = SetTextColor(dc, colour::BODY);
                    let _ = SetBkColor(dc, colour::PANEL);
                }
            }
            LRESULT(brush.0 as isize)
        }
        WM_APP_LINE | WM_TIMER => {
            // SAFETY: our own window.
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            };
            LRESULT(0)
        }
        WM_CLOSE => {
            on_close(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: the last message of the thread's queue for this window.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: the last message of this window; takes back the count given in `new`.
            unsafe {
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *const RefCell<State>;
                if !ptr.is_null() {
                    drop(Rc::from_raw(ptr));
                }
            }
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => {
            let _ = wparam;
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// The person closed the window: that is a cancel.
fn on_close(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    if let Ok(mut state) = cell.try_borrow_mut() {
        state.closed = true;
    }
    // SAFETY: our own window.
    unsafe {
        let _ = DestroyWindow(hwnd);
    };
}

/// A click in the window: a button or an option row.
fn on_click(hwnd: HWND, lparam: LPARAM) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    let (buttons, options, page, folder_panel) = {
        let Ok(state) = cell.try_borrow() else {
            return;
        };
        (
            state.buttons.clone(),
            state.options.clone(),
            state.page,
            state.folder_panel,
        )
    };
    for hit in &buttons {
        if hit.rect.left <= x && x < hit.rect.right && hit.rect.top <= y && y < hit.rect.bottom {
            if hit.button == Button::Browse {
                browse_for_folder(hwnd);
                return;
            }
            if let Ok(mut state) = cell.try_borrow_mut() {
                state.pending = Some(hit.button);
            }
            // SAFETY: our own window.
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            };
            return;
        }
    }
    // clicking the path itself opens the folder picker: the folder is the thing the person wants to change
    if let Some(panel) = folder_panel
        && panel.left <= x
        && x < panel.right
        && panel.top <= y
        && y < panel.bottom
    {
        browse_for_folder(hwnd);
        return;
    }
    if page == Page::Welcome {
        for (index, rect) in options.iter().enumerate() {
            if rect.left <= x && x < rect.right && rect.top <= y && y < rect.bottom {
                if let Ok(mut state) = cell.try_borrow_mut() {
                    match index {
                        0 => state.register = !state.register,
                        1 => state.arp = !state.arp,
                        2 => state.vscode = !state.vscode,
                        _ => {}
                    }
                }
                // SAFETY: our own window.
                unsafe {
                    let _ = InvalidateRect(hwnd, None, false);
                };
                return;
            }
        }
    }
}

/// The folder picker: the person chooses where the program goes.
fn browse_for_folder(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    // The picker is a modal window of its own: the state must not be borrowed while it is open.
    let current = {
        let Ok(state) = cell.try_borrow() else {
            return;
        };
        state.prefix.clone()
    };
    let Some(folder) = pick_folder(hwnd, &current) else {
        return;
    };
    if let Ok(mut state) = cell.try_borrow_mut() {
        // the field shows the place that was chosen; the line under it shows where the program goes from there
        state.prefix = folder.clone();
        refresh_path(&mut state);
        // The field is the one the person types in: it is not filled from the state while it is being edited, or
        // clearing it would put the old text back. The picker is the one place where the field is written.
        if let Some(edit) = state.path_edit {
            let wide: Vec<u16> = folder
                .display()
                .to_string()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: our own control; the text is a live local with its terminator.
            unsafe {
                let _ = SetWindowTextW(edit, PCWSTR(wide.as_ptr()));
            }
        }
    }
    // SAFETY: our own window.
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// The folder picker of Windows: the modern dialog first, the old shell dialog as the fallback.
fn pick_folder(hwnd: HWND, current: &Path) -> Option<PathBuf> {
    if let Some(folder) = pick_folder_modern(hwnd, current) {
        return Some(folder);
    }
    pick_folder_legacy(hwnd)
}

/// The modern folder picker (`IFileDialog` in the "pick folders" mode). It needs a COM apartment; when the thread
/// cannot have one, the old shell dialog is used instead.
fn pick_folder_modern(hwnd: HWND, current: &Path) -> Option<PathBuf> {
    // SAFETY: a plain call; the apartment is released again at the end of the function.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        // The thread already has an apartment of another kind: the modern dialog cannot be used here.
        return None;
    }
    // SAFETY: the apartment belongs to this thread.
    let folder = unsafe { modern_folder_dialog(hwnd, current) };
    // SAFETY: every successful CoInitializeEx has to be balanced.
    unsafe { CoUninitialize() };
    folder
}

/// The modern folder picker. The caller must have a COM apartment.
unsafe fn modern_folder_dialog(hwnd: HWND, current: &Path) -> Option<PathBuf> {
    let options =
        FILEOPENDIALOGOPTIONS(FOS_PICKFOLDERS.0 | FOS_PATHMUSTEXIST.0 | FOS_FORCEFILESYSTEM.0);
    // SAFETY: the class is the file dialog of the shell; the object is released when it goes out of scope.
    let dialog: IFileDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .ok()?;
    // SAFETY: the object is a file dialog.
    unsafe {
        let _ = dialog.SetOptions(options);
    };
    // the picker starts in the folder that is shown now
    let wide: Vec<u16> = current
        .display()
        .to_string()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a live local with its terminator.
    let start: Result<IShellItem, windows::core::Error> =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>) };
    if let Ok(item) = start {
        let _ = unsafe { dialog.SetFolder(&item) };
    }
    // the person presses Cancel: the dialog answers with an error
    unsafe { dialog.Show(hwnd) }.ok()?;
    // SAFETY: the dialog answered, so there is a result.
    let item = unsafe { dialog.GetResult() }.ok()?;
    // SAFETY: the shell gave us a string that must be released.
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
    // SAFETY: the string is a live wide string.
    let text = unsafe { name.to_string() }.ok();
    // SAFETY: the string was allocated by the shell.
    unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
    let path = PathBuf::from(text?);
    path.is_dir().then_some(path)
}

/// The old shell dialog (`SHBrowseForFolder`): the fallback when the modern dialog cannot be used.
fn pick_folder_legacy(hwnd: HWND) -> Option<PathBuf> {
    let title: Vec<u16> = "选择安装目录"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let info = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        ..Default::default()
    };
    // SAFETY: the info and the strings live until the call returns.
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    // SAFETY: the shell gave us a PIDL that must be released.
    unsafe {
        let mut buffer = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, &mut buffer).as_bool();
        CoTaskMemFree(Some(pidl as *const c_void));
        if !ok {
            return None;
        }
        let folder = String::from_utf16_lossy(&buffer);
        if folder.is_empty() {
            return None;
        }
        let path = PathBuf::from(folder);
        path.is_dir().then_some(path)
    }
}

/// Paint the window.
fn paint(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut state) = cell.try_borrow_mut() else {
        return;
    };
    {
        // SAFETY: `scope.ps` is a live local that BeginPaint fills in.
        let mut scope = PaintScope {
            hwnd,
            ps: PAINTSTRUCT::default(),
        };
        // SAFETY: `scope.ps` is a live local that BeginPaint fills in.
        let target = unsafe { BeginPaint(hwnd, &mut scope.ps) };
        if target.is_invalid() {
            return;
        }
        // SAFETY: a plain query of our own window.
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut client);
        };
        let (w, h) = (client.right, client.bottom);
        // The picture the page is drawn into. It is a buffer of pixels, not a device bitmap, so the shapes can have
        // smooth edges and the gradients can be computed for every single pixel (the text and the logo are still drawn
        // by GDI, the way Windows draws them).
        let Some(surface) = Surface::new(target, w, h) else {
            return;
        };
        let mem = surface.dc();
        // SAFETY: a plain state change on our memory DC.
        unsafe {
            let _ = SetBkMode(mem, TRANSPARENT);
        }
        SURFACE.with(|cell| *cell.borrow_mut() = Some(surface));

        draw(mem, &mut state, w, h);

        // The field of the folder is a window of its own: the page must not be painted over it. Windows keeps the
        // child windows out of the copy of the parent (WS_CLIPCHILDREN), but only for the place they had when the
        // paint began - and the field is moved while the page is drawn. The place it has now is cut out by hand,
        // or the page covers the text of the field.
        if let Some(field) = state.path_field {
            // SAFETY: the device context of our own window; the field lies inside the client area.
            unsafe {
                let _ = ExcludeClipRect(target, field.left, field.top, field.right, field.bottom);
            }
        }

        // The picture is shown with BitBlt, which copies the colours and ignores the alpha byte. Making it opaque
        // first keeps GDI from leaving a transparent background under the text it drew into the picture.
        let surface = SURFACE.with(|cell| cell.borrow_mut().take());
        if let Some(mut surface) = surface {
            surface.make_opaque();
            // SAFETY: both DCs are live and the picture covers `w` x `h`.
            unsafe {
                let _ = BitBlt(
                    target,
                    0,
                    0,
                    w,
                    h,
                    surface.dc(),
                    0,
                    0,
                    windows::Win32::Graphics::Gdi::SRCCOPY,
                );
            }
        }
        // The scope ends here: `EndPaint` is called with it. A child window that paints itself while the parent
        // still holds its `BeginPaint` would draw into a surface that the parent paints over afterwards.
    }
    // The page is on the screen now, and the field of the folder is a window of its own: it is drawn by Windows,
    // not by the picture. It has to paint itself again on top of the page, or its text stays under the page and
    // only the caret of Windows is visible.
    //
    // The repaint is done right here and not left to the message queue: the window is drawn as one picture that is
    // copied over the page, and the field has to come after it.
    if state.path_dirty
        && let Some(edit) = state.path_edit
    {
        state.path_dirty = false;
        // SAFETY: our own control.
        unsafe {
            let _ = RedrawWindow(
                edit,
                None,
                None,
                REDRAW_WINDOW_FLAGS(RDW_INVALIDATE.0 | RDW_ERASE.0 | RDW_UPDATENOW.0),
            );
        }
    }
}

/// Draw the whole page into the memory DC.
fn draw(hdc: HDC, state: &mut State, w: i32, h: i32) {
    let scale = state.scale;
    let unit = |v: i32| v * scale / 96;
    let band = unit(96);
    // the page
    fill(
        hdc,
        RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
        colour::PAGE,
    );
    // the buttons of this paint cycle (the page may add its own)
    state.buttons.clear();
    state.folder_panel = None;
    // The brand band: the rainbow of the logo. It is one smooth ramp (the seven colours flow into each other) with
    // a vertical shading, so the band has depth instead of being seven blocks of colour.
    draw_rainbow(
        hdc,
        RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: band,
        },
        true,
    );
    // The logo in the top-left corner, on a white circle that sits on a soft shadow.
    let tile = unit(56);
    let tile_rect = RECT {
        left: unit(26),
        top: (band - tile) / 2,
        right: unit(26) + tile,
        bottom: (band + tile) / 2,
    };
    fill_shadow(hdc, tile_rect, tile / 2, unit(8), unit(2), colour::SHADOW);
    fill_round(hdc, tile_rect, colour::PANEL, tile);
    draw_logo(hdc, state, tile_rect, unit(38));
    // the product name and the subtitle
    let text_x = tile_rect.right + unit(16);
    draw_text(
        hdc,
        "IME1.0",
        text_x,
        unit(18),
        colour::INK,
        state.fonts.title,
    );
    draw_text(
        hdc,
        wizard_text(state.lang, Msg::WizardProduct),
        text_x,
        unit(56),
        colour::INK,
        state.fonts.body,
    );
    let version = format!("v{}", state.version);
    draw_text(
        hdc,
        &version,
        w - unit(30) - measure(hdc, &version, state.fonts.small),
        unit(60),
        colour::INK,
        state.fonts.small,
    );

    let top = band + unit(34);
    match state.page {
        Page::Welcome => draw_welcome(hdc, state, w, h, top),
        Page::Progress => draw_progress(hdc, state, w, h, top),
        Page::Done => draw_done(hdc, state, w, h, top),
    }
    draw_footer(hdc, state, w, h);
}

/// The control id of the field where the install folder is typed.
const PATH_EDIT_ID: usize = 1001;

/// The command of the key table: Enter starts the install.
const ACCEL_INSTALL: u16 = 1;

/// The message of the edit control that selects a range of its text (`EM_SETSEL`).
const EM_SETSEL: u32 = 0x00B1;

/// The measures of the panel with the folder (they are in the units of the design, at 96 dpi).
///
/// The field of the folder and the button that opens the picker share the panel. They are written down here, so
/// that the place of the field and the width it is made with cannot drift apart.
const PANEL_MARGIN: i32 = 28;
const BUTTON_GAP: i32 = 16;
const BUTTON_WIDTH: i32 = 92;
const FIELD_LEFT: i32 = 38;
const FIELD_RIGHT_GAP: i32 = 14;
const FIELD_TOP: i32 = 26;
const FIELD_BOTTOM: i32 = 50;

/// The width of the field of the folder: the width of the client area without the frame of the panel and without
/// the button that opens the picker.
fn path_field_width(unit: &impl Fn(i32) -> i32, client_width: i32) -> i32 {
    client_width
        - unit(PANEL_MARGIN)
        - unit(BUTTON_GAP)
        - unit(BUTTON_WIDTH)
        - unit(FIELD_RIGHT_GAP)
        - unit(FIELD_LEFT)
}

/// The place of the field of the folder in the client area of the window.
///
/// The field is a real edit control of Windows, and an edit control lays its text out for the width it was made
/// with: it only lays it out again when the width really changes. The field is therefore made with the width it
/// will have here, and it is put in this place when the page is drawn. One rule for both, or the field would show
/// a part of the path only.
fn path_field_rect(unit: &impl Fn(i32) -> i32, client_width: i32, panel_top: i32) -> RECT {
    RECT {
        left: unit(FIELD_LEFT),
        top: panel_top + unit(FIELD_TOP),
        right: unit(FIELD_LEFT) + path_field_width(unit, client_width),
        bottom: panel_top + unit(FIELD_BOTTOM),
    }
}

/// The field where the install folder is typed. It is a real edit control of Windows: it has a caret, a selection and
/// it takes the input of an IME (which matters for a folder name that is written in Chinese).
///
/// The control is made with the width it will have, not with a small one that is corrected later: an edit control
/// lays its text out for the width it was made with, and it only lays it out again when the width really changes.
/// A field that was made ten pixels wide shows ten pixels of the path, no matter how wide the window becomes later.
fn create_path_edit(
    parent: HWND,
    instance: HINSTANCE,
    prefix: &Path,
    font: HFONT,
    width: i32,
    height: i32,
) -> Option<HWND> {
    let wide: Vec<u16> = prefix
        .display()
        .to_string()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: "EDIT" is a class of Windows itself; the parent is our own window.
    let edit = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("EDIT"),
            PCWSTR(wide.as_ptr()),
            WS_CHILD
                | WS_VISIBLE
                | WINDOW_STYLE(ES_LEFT as u32)
                | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
                | WINDOW_STYLE(WS_TABSTOP.0),
            0,
            0,
            width.max(1),
            height.max(1),
            parent,
            HMENU(PATH_EDIT_ID as *mut c_void),
            instance,
            None,
        )
    }
    .ok()?;
    // the font of the window, so that the text looks like the rest of the page
    // SAFETY: our own control.
    unsafe {
        let _ = SendMessageW(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
    Some(edit)
}

/// The text of a control.
fn window_text(hwnd: HWND) -> String {
    // SAFETY: a plain query of the length of the text.
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    // SAFETY: the buffer is big enough for the text and its terminator.
    let written = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if written <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..written as usize])
}

/// The folder of the install after the check: the folder as Windows writes it, or the reason why it cannot be used.
///
/// The folder that is typed is not the folder the program goes to: the product folder is appended to it, so the
/// program never lands in the root of a drive. The window shows the result of this under the field.
fn checked_prefix(prefix: &Path, system: &crate::plan::SystemPaths) -> Result<PathBuf, String> {
    let dir =
        crate::plan::resolve_install_dir(prefix, system).map_err(|e| e.reason().to_string())?;
    crate::plan::validate_prefix(&dir, system).map_err(|e| e.reason().to_string())?;
    Ok(dir)
}

/// The folder the program goes to, and why the typed folder cannot be used. Both are shown while the person types,
/// so the reason is there before the install is started and not only after it was pressed.
fn refresh_path(state: &mut State) {
    // When the program is already installed, the folder it is in is the folder that is used: the product folder is
    // not appended to it again (the install that is there has it already, and a repair has to land in that folder).
    if state.existing.is_some() {
        state.install_dir = Some(state.prefix.clone());
        state.path_error = None;
        return;
    }
    let resolved = crate::plan::resolve_install_dir(&state.prefix, &state.system);
    state.install_dir = None;
    state.path_error = None;
    let dir = match resolved {
        Ok(dir) => dir,
        Err(e) => {
            state.path_error = Some(e.reason().to_string());
            return;
        }
    };
    state.install_dir = Some(dir.clone());
    if let Err(e) = crate::plan::validate_prefix(&dir, &state.system) {
        state.path_error = Some(e.reason().to_string());
    }
}

/// The hint about an installation from before the rename: it has to be removed first, so that the old and the new
/// version do not get in each other's way.
fn legacy_hint(job: Job, lang: Lang, system: &crate::plan::SystemPaths) -> Option<String> {
    if job == Job::Uninstall {
        return None;
    }
    let legacy = system.legacy_prefix();
    legacy.is_dir().then(|| {
        crate::text::text(
            lang,
            Msg::WizardLegacyFolder,
            &[&legacy.display().to_string()],
        )
    })
}

/// The person typed in the field: the folder that is typed is what the person chose, the folder the program goes to
/// is worked out from it.
fn on_path_changed(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let edit = {
        let Ok(state) = cell.try_borrow() else {
            return;
        };
        state.path_edit
    };
    let Some(edit) = edit else {
        return;
    };
    let text = window_text(edit);
    let trimmed = text.trim();
    {
        let Ok(mut state) = cell.try_borrow_mut() else {
            return;
        };
        // An empty field means the default folder (the same rule as on the command line). The field is not filled
        // from the state while it is being edited, or clearing it would put the old text back.
        state.prefix = PathBuf::from(trimmed);
        refresh_path(&mut state);
    }
    // The page says where the program really goes to: it has to be drawn again, or the line under the field would
    // still show the folder of the last letter.
    // SAFETY: our own window.
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// Draw the logo in the middle of a tile. The picture is drawn in the size of the file it comes from, so it is
/// never scaled: a scaled picture is what looks blurry.
fn draw_logo(hdc: HDC, state: &mut State, tile: RECT, target: i32) {
    let room = tile.right - tile.left;
    let size = logo_fit_size(target, room);
    let Some(icon) = logo_icon(state, size) else {
        return;
    };
    // SAFETY: our own icon, drawn into our own DC.
    unsafe {
        let _ = DrawIconEx(
            hdc,
            tile.left + (tile.right - tile.left - size) / 2,
            tile.top + (tile.bottom - tile.top - size) / 2,
            icon,
            size,
            size,
            0,
            None,
            DI_NORMAL,
        );
    }
}

/// The welcome page: what is installed, where, and the options.
fn draw_welcome(hdc: HDC, state: &mut State, w: i32, _h: i32, top: i32) {
    let scale = state.scale;
    let unit = |v: i32| v * scale / 96;
    if state.job == Job::Uninstall {
        draw_uninstall_page(hdc, state, w, top, &unit);
        return;
    }
    if state.existing.is_some() {
        draw_existing_page(hdc, state, w, top, &unit);
        return;
    }
    draw_text(
        hdc,
        wizard_text(state.lang, Msg::WizardWelcome),
        unit(28),
        top,
        colour::TITLE,
        state.fonts.big,
    );
    // the intro (it is broken into lines that fit; the panel below moves down with it)
    let y = draw_paragraph(
        hdc,
        wizard_text(state.lang, Msg::WizardIntro),
        unit(28),
        top + unit(32),
        w - unit(56),
        colour::BODY,
        state.fonts.body,
        unit(21),
    ) + unit(12);
    // the folder
    let panel_bottom = draw_folder_panel(hdc, state, w, y, &unit, true);
    // the options (they move down when the lines under the field need their room)
    state.options = draw_options(hdc, state, w, panel_bottom + unit(22), &unit);
}

/// The three check boxes of the page. They say what the install does besides writing the files.
fn draw_options(
    hdc: HDC,
    state: &mut State,
    w: i32,
    mut y: i32,
    unit: &dyn Fn(i32) -> i32,
) -> Vec<RECT> {
    let options = [
        (
            wizard_text(state.lang, Msg::WizardOptRegister),
            state.register,
        ),
        (wizard_text(state.lang, Msg::WizardOptArp), state.arp),
        (wizard_text(state.lang, Msg::WizardOptVscode), state.vscode),
    ];
    let mut rects = Vec::new();
    let font = state.fonts.body;
    for (label, on) in options {
        // The check box is drawn, not taken from the font: the tick of a system font is a different size on every
        // Windows and it does not match the colours of the page.
        let box_size = unit(18);
        let box_rect = RECT {
            left: unit(34),
            top: y + unit(2),
            right: unit(34) + box_size,
            bottom: y + unit(2) + box_size,
        };
        if on {
            // The tick box is a lit surface: the pale blue of the laser, with the deep ink on it.
            fill_round(hdc, box_rect, colour::LASER, unit(5));
            // the tick: two strokes, one short and one long
            let (bx, by) = (box_rect.left as f64, box_rect.top as f64);
            let size = box_size as f64;
            stroke_segment(
                hdc,
                bx + size * 0.24,
                by + size * 0.52,
                bx + size * 0.42,
                by + size * 0.70,
                unit(2) as f64,
                colour::INK,
            );
            stroke_segment(
                hdc,
                bx + size * 0.42,
                by + size * 0.70,
                bx + size * 0.78,
                by + size * 0.30,
                unit(2) as f64,
                colour::INK,
            );
        } else {
            fill_round(hdc, box_rect, colour::PANEL, unit(5));
            stroke_round(hdc, box_rect, unit(5), colour::BUTTON, 1);
        }
        let row_top = y;
        for line in wrap_text(label, w - unit(90), &|t| measure(hdc, t, font)) {
            draw_text(hdc, &line, unit(62), y, colour::BODY, font);
            y += unit(21);
        }
        rects.push(RECT {
            left: unit(28),
            top: row_top - unit(4),
            right: w - unit(28),
            bottom: y - unit(21) + unit(20),
        });
        y += unit(8);
    }
    rects
}

/// The page of a program that is already on the machine.
///
/// It says what is installed and where, and it offers the three things a setup of another program offers: repair the
/// install, install it again, or remove it. The folder is not changed here: a repair has to land in the folder that is
/// already there.
fn draw_existing_page(
    hdc: HDC,
    state: &mut State,
    w: i32,
    top: i32,
    unit: &dyn Fn(i32) -> i32,
) {
    let Some(existing) = state.existing.clone() else {
        return;
    };
    draw_text(
        hdc,
        wizard_text(state.lang, Msg::WizardExistingTitle),
        unit(28),
        top,
        colour::TITLE,
        state.fonts.big,
    );
    // what is installed, and where
    let version = existing.version.clone().unwrap_or_else(|| "-".to_string());
    let info = crate::text::text(
        state.lang,
        Msg::WizardExistingInfo,
        &[&version, &existing.prefix.display().to_string()],
    );
    let mut y = draw_paragraph(
        hdc,
        &info,
        unit(28),
        top + unit(34),
        w - unit(56),
        colour::BODY,
        state.fonts.body,
        unit(21),
    ) + unit(6);
    // the line that says what the buttons below it do
    y = draw_paragraph(
        hdc,
        wizard_text(state.lang, Msg::WizardExistingHint),
        unit(28),
        y,
        w - unit(56),
        colour::DIM,
        state.fonts.body,
        unit(21),
    ) + unit(6);
    // the folder of that install (it is only shown: it is not changed here)
    let panel_bottom = draw_folder_panel(hdc, state, w, y, &unit, false);
    // the options (a repair and a reinstall use them)
    state.options = draw_options(hdc, state, w, panel_bottom + unit(18), &unit);
}

/// The uninstall page: what is going to be removed, and where it is.
fn draw_uninstall_page(hdc: HDC, state: &mut State, w: i32, top: i32, unit: &dyn Fn(i32) -> i32) {
    draw_text(
        hdc,
        wizard_text(state.lang, Msg::WizardUninstallTitle),
        unit(28),
        top,
        colour::TITLE,
        state.fonts.big,
    );
    // the summary (what is removed, and the warnings of the plan)
    let mut y = top + unit(34);
    let font = state.fonts.body;
    y = draw_paragraph(
        hdc,
        &state.summary,
        unit(28),
        y,
        w - unit(56),
        colour::BODY,
        font,
        unit(21),
    ) + unit(10);
    // the folder it is removed from
    draw_folder_panel(hdc, state, w, y, unit, false);
}

/// The panel with the installation folder. When `editable` is true it carries the button that changes the folder,
/// and the whole panel can be clicked as well.
fn draw_folder_panel(
    hdc: HDC,
    state: &mut State,
    w: i32,
    y: i32,
    unit: &dyn Fn(i32) -> i32,
    editable: bool,
) -> i32 {
    let panel = RECT {
        left: unit(PANEL_MARGIN),
        top: y,
        right: w - unit(PANEL_MARGIN),
        bottom: y + unit(58),
    };
    // The panel is a card: rounded corners, a hairline and a soft shadow, so it sits on the page instead of being a
    // rectangle cut out of it.
    fill_shadow(hdc, panel, unit(10), unit(7), unit(2), colour::SHADOW);
    fill_round(hdc, panel, colour::PANEL, unit(10));
    stroke_round(hdc, panel, unit(10), colour::BORDER, 1);
    draw_text(
        hdc,
        wizard_text(state.lang, Msg::WizardFolder),
        unit(40),
        panel.top + unit(8),
        colour::DIM,
        state.fonts.small,
    );
    // the button that opens the folder picker
    let button_width = unit(BUTTON_WIDTH);
    let button = RECT {
        left: panel.right - unit(BUTTON_GAP) - button_width,
        top: panel.top + unit(12),
        right: panel.right - unit(BUTTON_GAP),
        bottom: panel.top + unit(46),
    };
    if editable {
        // The folder is typed in a real field: it has a caret, a selection and it takes the input of an IME.
        // The place of the field comes from the same rule that made it, or the field would be a different width
        // than the one it lays its text out for.
        let field = path_field_rect(&unit, w, panel.top);
        // The place of the field is known now. The page is drawn into a picture and copied over the window
        // afterwards, and the field is a window of its own: its place is cut out of that copy, or the page would
        // paint over the text of the field.
        let moved = state.path_field != Some(field);
        state.path_field = Some(field);
        if let Some(edit) = state.path_edit {
            // The text of the field is not written from the state here: the field is what the person types in, and
            // filling it from the state would put the old text back while it is being edited.
            // SAFETY: our own control.
            unsafe {
                let _ = SetWindowPos(
                    edit,
                    HWND_TOP,
                    field.left,
                    field.top,
                    field.right - field.left,
                    field.bottom - field.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = ShowWindow(edit, SW_SHOW);
            }
            if moved {
                // The control was moved: it has to paint itself again.
                state.path_dirty = true;
            }
        }
    } else {
        state.path_field = None;
        // the folder is only shown (the uninstall page): it is cut off with an ellipsis when it is very long
        draw_text_clipped(
            hdc,
            &state.prefix.display().to_string(),
            unit(40),
            panel.top + unit(28),
            button.left - unit(52),
            colour::BODY,
            state.fonts.body,
        );
        if let Some(edit) = state.path_edit {
            // SAFETY: our own control.
            unsafe {
                let _ = ShowWindow(edit, SW_HIDE);
            }
        }
    }
    if editable {
        fill_round(hdc, button, colour::TINT, unit(8));
        stroke_round(hdc, button, unit(8), colour::BORDER, 1);
        let label = wizard_text(state.lang, Msg::WizardBrowse);
        let text_width = measure(hdc, label, state.fonts.body);
        draw_text(
            hdc,
            label,
            button.left + (button_width - text_width) / 2,
            button.top + unit(12),
            colour::ACCENT,
            state.fonts.body,
        );
        state.buttons.push(Hit {
            rect: button,
            button: Button::Browse,
        });
        // the panel itself opens the picker too: the path is the thing the person wants to change
        state.folder_panel = Some(panel);
    }
    // the lines under the panel: where the program really goes to, the rule behind it, and the reason why the
    // typed folder cannot be used (it is written under the field, not in a popup)
    let mut bottom = panel.bottom;
    if editable {
        if let Some(dir) = state.install_dir.clone() {
            let line = crate::text::text(
                state.lang,
                Msg::WizardResolvedPath,
                &[&dir.display().to_string()],
            );
            draw_text_clipped(
                hdc,
                &line,
                unit(40),
                bottom + unit(8),
                w - unit(40),
                colour::BODY,
                state.fonts.small,
            );
            bottom += unit(22);
            let rule = crate::text::text(
                state.lang,
                Msg::WizardPathRule,
                &[crate::plan::PRODUCT_FOLDER],
            );
            draw_text(hdc, &rule, unit(40), bottom, colour::DIM, state.fonts.small);
            bottom += unit(18);
        }
        // an installation from before the rename: it has to be removed first
        if let Some(hint) = state.legacy_hint.clone() {
            let lines = wrap_text(&hint, w - unit(80), &|t| measure(hdc, t, state.fonts.small));
            for (index, line) in lines.iter().enumerate() {
                draw_text(
                    hdc,
                    line,
                    unit(40),
                    bottom + index as i32 * unit(16),
                    colour::ACCENT,
                    state.fonts.small,
                );
            }
            bottom += lines.len() as i32 * unit(16);
        }
    }
    if let Some(reason) = state.path_error.clone() {
        let mut y = bottom + unit(8);
        // The sentence is in the language of the window ...
        for line in wrap_text(
            wizard_text(state.lang, Msg::WizardPathInvalid),
            w - unit(80),
            &|t| measure(hdc, t, state.fonts.small),
        ) {
            draw_text(hdc, &line, unit(40), y, colour::ACCENT, state.fonts.small);
            y += unit(16);
        }
        // ... and the reason is a technical detail of its own: a reason in another language inside a sentence
        // would be hard to read.
        for line in wrap_text(&reason, w - unit(80), &|t| {
            measure(hdc, t, state.fonts.small)
        }) {
            draw_text(hdc, &line, unit(40), y, colour::DIM, state.fonts.small);
            y += unit(16);
        }
        bottom = y + unit(4);
    }
    bottom
}

/// The progress page: what the install is doing.
fn draw_progress(hdc: HDC, state: &mut State, w: i32, h: i32, top: i32) {
    let scale = state.scale;
    let unit = |v: i32| v * scale / 96;
    draw_text(
        hdc,
        match state.job {
            Job::Install => wizard_text(state.lang, Msg::WizardWorking),
            Job::Repair => wizard_text(state.lang, Msg::WizardRepairWorking),
            Job::Reinstall => wizard_text(state.lang, Msg::WizardReinstallWorking),
            Job::Uninstall => wizard_text(state.lang, Msg::WizardUninstallWorking),
        },
        unit(28),
        top,
        colour::TITLE,
        state.fonts.big,
    );
    let panel = RECT {
        left: unit(28),
        top: top + unit(40),
        right: w - unit(28),
        bottom: h - unit(108),
    };
    // The report sits in a card, like the folder panel on the first page.
    fill_shadow(hdc, panel, unit(10), unit(7), unit(2), colour::SHADOW);
    fill_round(hdc, panel, colour::PANEL, unit(10));
    stroke_round(hdc, panel, unit(10), colour::BORDER, 1);
    // the last lines of the report (a long line is broken, not cut off)
    let mut y = panel.top + unit(12);
    let lines: Vec<String> = state.log.clone();
    let first = lines.len().saturating_sub(12);
    let font = state.fonts.small;
    for line in lines.iter().skip(first) {
        for part in wrap_text(line, w - unit(80), &|t| measure(hdc, t, font)) {
            draw_text(hdc, &part, unit(40), y, colour::BODY, font);
            y += unit(20);
        }
    }
}

/// The last page: what happened.
fn draw_done(hdc: HDC, state: &mut State, w: i32, _h: i32, top: i32) {
    let unit = |v: i32| v * state.scale / 96;
    let title = match (state.job, state.ok) {
        (Job::Uninstall, true) => wizard_text(state.lang, Msg::WizardUninstallDone),
        (Job::Uninstall, false) => wizard_text(state.lang, Msg::WizardUninstallFailed),
        (Job::Repair, true) => wizard_text(state.lang, Msg::WizardRepairDone),
        (Job::Repair, false) => wizard_text(state.lang, Msg::WizardRepairFailed),
        (Job::Reinstall, true) => wizard_text(state.lang, Msg::WizardReinstallDone),
        (Job::Reinstall, false) => wizard_text(state.lang, Msg::WizardReinstallFailed),
        (Job::Install, true) => wizard_text(state.lang, Msg::WizardDone),
        (Job::Install, false) => wizard_text(state.lang, Msg::WizardFailed),
    };
    draw_text(hdc, title, unit(28), top, colour::TITLE, state.fonts.big);
    draw_paragraph(
        hdc,
        &state.outcome,
        unit(28),
        top + unit(38),
        w - unit(56),
        colour::BODY,
        state.fonts.body,
        unit(21),
    );
}

/// The footer: the buttons on the right and the logo in the corner.
fn draw_footer(hdc: HDC, state: &mut State, w: i32, h: i32) {
    let scale = state.scale;
    let unit = |v: i32| v * scale / 96;
    let footer = h - unit(88);
    // the separator: a thin rainbow, the signature of the brand
    draw_rainbow(
        hdc,
        RECT {
            left: 0,
            top: footer,
            right: w,
            bottom: footer + unit(3),
        },
        false,
    );
    // The logo in the bottom-right corner, on a white circle that sits on a soft shadow.
    let tile = unit(56);
    let tile_rect = RECT {
        left: w - tile - unit(18),
        top: h - tile - unit(16),
        right: w - unit(18),
        bottom: h - unit(16),
    };
    fill_shadow(hdc, tile_rect, tile / 2, unit(8), unit(2), colour::SHADOW);
    fill_round(hdc, tile_rect, colour::PANEL, tile);
    draw_logo(hdc, state, tile_rect, unit(38));
    // the buttons, right-aligned before the logo
    // The program is already on the machine: the first page offers the three things the setup of another program
    // offers - repair the install, install it again, or remove it.
    let existing_page =
        state.page == Page::Welcome && state.job == Job::Install && state.existing.is_some();
    let (labels, actions): (Vec<&str>, Vec<Button>) = if existing_page {
        let mut labels = vec![wizard_text(state.lang, Msg::WizardCancel)];
        let mut actions = vec![Button::Cancel];
        // The uninstall is only offered when it can be planned: without the manifest of the install there is nothing
        // to remove file by file, so the button is not drawn at all.
        if state.uninstall_summary.is_some() {
            labels.push(wizard_text(state.lang, Msg::WizardUninstallButton));
            actions.push(Button::Uninstall);
        }
        labels.push(wizard_text(state.lang, Msg::WizardReinstall));
        actions.push(Button::Reinstall);
        labels.push(wizard_text(state.lang, Msg::WizardRepair));
        actions.push(Button::Repair);
        (labels, actions)
    } else {
        match (state.job, state.page) {
            // The folder is changed on the first page (the panel with the path carries the button).
            (Job::Install, Page::Welcome) => (
                vec![
                    wizard_text(state.lang, Msg::WizardCancel),
                    wizard_text(state.lang, Msg::WizardInstall),
                ],
                vec![Button::Cancel, Button::Install],
            ),
            (Job::Uninstall, Page::Welcome) => (
                vec![
                    wizard_text(state.lang, Msg::WizardCancel),
                    wizard_text(state.lang, Msg::WizardUninstallButton),
                ],
                vec![Button::Cancel, Button::Uninstall],
            ),
            // While the install runs there is no cancel button: an install that is half done has to be able to roll itself
            // back, and that is the installer's job, not the window's.
            (_, Page::Progress) => (Vec::new(), Vec::new()),
            (_, Page::Done) => (
                vec![wizard_text(state.lang, Msg::WizardClose)],
                vec![Button::Close],
            ),
            // A repair or a reinstall is never on the first page: the job is set when the button was pressed, and the
            // window goes to the progress page right away.
            (_, Page::Welcome) => (Vec::new(), Vec::new()),
        }
    };
    let mut x = w - unit(84);
    let mut buttons = Vec::new();
    for (label, action) in labels.iter().zip(actions.iter()).rev() {
        let width = measure(hdc, label, state.fonts.body) + unit(36);
        let rect = RECT {
            left: x - width,
            top: h - unit(62),
            right: x,
            bottom: h - unit(24),
        };
        let primary = match action {
            // The action the person came here for is the lit one. Removing the program is not lit on the page of an
            // install that is already there: it is the button next to the repair, not the thing the page is for.
            Button::Install | Button::Repair | Button::Close => true,
            Button::Uninstall => state.job == Job::Uninstall,
            _ => false,
        };
        if primary {
            // The primary button is the one thing that acts: it gets a shadow and the light of the laser. The
            // blue is the **pale** blue of the beam — translucent light, not a solid block of colour — and the
            // halo around it is the light it throws into the air.
            fill_shadow(hdc, rect, unit(8), unit(6), unit(2), colour::SHADOW);
            fill_halo(hdc, rect, unit(10), unit(14), colour::LASER_HALO);
            fill_gradient_round(hdc, rect, unit(8), colour::LASER_CORE, colour::LASER);
        } else {
            fill_round(hdc, rect, colour::PANEL, unit(8));
            stroke_round(hdc, rect, unit(8), colour::BUTTON, 1);
        }
        let tw = measure(hdc, label, state.fonts.body);
        draw_text(
            hdc,
            label,
            rect.left + (width - tw) / 2,
            rect.top + unit(12),
            if primary {
                // The button is a pale, lit surface: the deep ink reads on it, the white would not.
                colour::INK
            } else {
                colour::BUTTON_TEXT
            },
            state.fonts.body,
        );
        buttons.push(Hit {
            rect,
            button: *action,
        });
        x = rect.left - unit(12);
    }
    buttons.reverse();
    state.buttons.extend(buttons);
}

/// Ends the paint cycle even on early return.
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

// The picture of the paint cycle that is running (there is none outside a paint cycle).
thread_local! {
    static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) };
}

/// Run `f` with the picture of the current paint cycle.
fn with_surface<R>(f: impl FnOnce(&mut Surface) -> R) -> Option<R> {
    SURFACE.with(|cell| cell.borrow_mut().as_mut().map(f))
}

/// Fill a rectangle with one colour.
fn fill(hdc: HDC, rect: RECT, value: COLORREF) {
    let colour = Rgb::from_colorref(value);
    if with_surface(|surface| surface.fill_rect(rect, colour)).is_some() {
        return;
    }
    // SAFETY: a plain call; the brush is ours and is deleted below.
    unsafe {
        let brush = CreateSolidBrush(value);
        if brush.is_invalid() {
            return;
        }
        let _ = windows::Win32::Graphics::Gdi::FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// Draw the frame of a rectangle (one pixel).
fn frame(hdc: HDC, rect: RECT, value: COLORREF) {
    // SAFETY: a plain call; the brush is ours and is deleted below.
    unsafe {
        let brush = CreateSolidBrush(value);
        if brush.is_invalid() {
            return;
        }
        let _ = windows::Win32::Graphics::Gdi::FrameRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// Fill a rectangle with rounded corners.
///
/// The edge is **smooth**: GDI's own `RoundRect` is stair-stepped, so the shape is drawn into the picture instead,
/// pixel by pixel, with the share of every pixel that lies inside the shape as its coverage.
fn fill_round(hdc: HDC, rect: RECT, value: COLORREF, radius: i32) {
    let colour = Rgb::from_colorref(value);
    if with_surface(|surface| surface.fill_rounded(rect, radius, colour)).is_some() {
        return;
    }
    // SAFETY: the brush and the pen are ours; they are selected for the call and deleted below.
    unsafe {
        let brush = CreateSolidBrush(value);
        if brush.is_invalid() {
            return;
        }
        // The shape is filled, not outlined. The pen that would draw the outline is put aside, otherwise every
        // rounded shape gets a black line around it (the logo tile on the rainbow looked like a dirty ring).
        let pen = CreatePen(PS_NULL, 0, COLORREF(0));
        let previous_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let previous_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let _ = RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        let _ = SelectObject(hdc, previous_brush);
        let _ = SelectObject(hdc, previous_pen);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// Draw the line around a rounded rectangle, `width` pixels wide, just inside its edge.
fn stroke_round(hdc: HDC, rect: RECT, radius: i32, value: COLORREF, width: i32) {
    let colour = Rgb::from_colorref(value);
    if with_surface(|surface| surface.stroke_rounded(rect, radius, colour, width)).is_some() {
        return;
    }
    frame(hdc, rect, value);
}

/// The shadow under a shape: the shape, a little down and to the right, with a soft edge.
fn fill_shadow(_hdc: HDC, rect: RECT, radius: i32, blur: i32, offset: i32, value: COLORREF) {
    let colour = Rgb::from_colorref(value);
    with_surface(|surface| {
        surface.fill_shadow(rect, radius, colour, paint::Shadow::soft(blur, offset))
    });
}

/// The halo of light around a shape: the shape itself, not moved, with a soft edge that fades away all around.
///
/// It is the light a lit surface throws into the air. A shadow is the same shape with an offset; a halo has none,
/// so it needs its own call into the picture.
fn fill_halo(_hdc: HDC, rect: RECT, radius: i32, blur: i32, value: COLORREF) {
    let colour = Rgb::from_colorref(value);
    with_surface(|surface| {
        surface.fill_shadow(
            rect,
            radius,
            colour,
            paint::Shadow {
                blur,
                offset_x: 0,
                offset_y: 0,
                strength: 0.55,
            },
        )
    });
}

/// Draw a line with the given width (the edge is smooth).
fn stroke_segment(_hdc: HDC, ax: f64, ay: f64, bx: f64, by: f64, width: f64, value: COLORREF) {
    let colour = Rgb::from_colorref(value);
    with_surface(|surface| surface.stroke_segment(ax, ay, bx, by, width, colour));
}

/// Fill a rectangle with the rainbow of the brand: **one smooth ramp** through the seven colours.
///
/// The seven colours are control points of a curve, so the ramp runs through every one of them without a corner
/// between them, and the vertical shading (in linear light) gives the band depth instead of leaving it flat.
fn draw_rainbow(hdc: HDC, rect: RECT, gradient: bool) {
    let stops: Vec<Rgb> = colour::RAINBOW
        .iter()
        .map(|value| Rgb::from_colorref(*value))
        .collect();
    let (top, bottom) = if gradient { (1.0, 0.86) } else { (1.0, 1.0) };
    if with_surface(|surface| surface.fill_rainbow(rect, &stops, top, bottom)).is_some() {
        return;
    }
    // The GDI fallback (only outside a paint cycle): one flat colour per segment.
    let count = colour::RAINBOW.len() as i32;
    let width = rect.right - rect.left;
    let segment = (width + count - 1) / count;
    for (index, value) in colour::RAINBOW.iter().enumerate() {
        let left = rect.left + index as i32 * segment;
        let right = (rect.left + (index as i32 + 1) * segment).min(rect.right);
        if left >= right {
            continue;
        }
        let part = RECT {
            left,
            top: rect.top,
            right,
            bottom: rect.bottom,
        };
        if gradient {
            fill_gradient(hdc, part, *value, darken(*value, 0.16));
        } else {
            fill(hdc, part, *value);
        }
    }
}

/// Fill a rectangle with a vertical gradient from `top` to `bottom`, with rounded corners.
///
/// Every button of the window is round: a hard-edged rectangle would be the one square corner in a window of
/// round ones.
fn fill_gradient_round(hdc: HDC, rect: RECT, radius: i32, top: COLORREF, bottom: COLORREF) {
    let (top, bottom) = (Rgb::from_colorref(top), Rgb::from_colorref(bottom));
    if with_surface(|surface| surface.fill_gradient_rounded(rect, radius, top, bottom)).is_some() {
        return;
    }
    // The GDI fallback (only outside a paint cycle): a flat band, still with the rounded corners.
    fill_round(hdc, rect, top.to_colorref(), radius);
}

/// Fill a rectangle with a vertical gradient from `top` to `bottom`.
///
/// The gradient is computed for every pixel in linear light, so there is no step in the colour (GDI's own gradient
/// is a stack of flat bands, and the steps are visible).
fn fill_gradient(hdc: HDC, rect: RECT, top: COLORREF, bottom: COLORREF) {
    let (top, bottom) = (Rgb::from_colorref(top), Rgb::from_colorref(bottom));
    if with_surface(|surface| surface.fill_gradient_v(rect, top, bottom)).is_some() {
        return;
    }
    // The GDI fallback (only outside a paint cycle): bands of flat colour.
    let height = rect.bottom - rect.top;
    if height <= 0 {
        return;
    }
    let steps = 24;
    for step in 0..steps {
        let y0 = rect.top + height * step / steps;
        let y1 = rect.top + height * (step + 1) / steps;
        let t = step as f64 / (steps - 1) as f64;
        fill(
            hdc,
            RECT {
                left: rect.left,
                top: y0,
                right: rect.right,
                bottom: y1,
            },
            mix(top.to_colorref(), bottom.to_colorref(), t),
        );
    }
}

/// Mix two colours (`t` = 0 gives `a`, 1 gives `b`).
///
/// The mix happens in linear light, so the middle of two colours is the colour the eye expects in the middle and
/// not the arithmetic middle of two numbers (which looks darker and muddier than it should).
fn mix(a: COLORREF, b: COLORREF, t: f64) -> COLORREF {
    paint::mix(Rgb::from_colorref(a), Rgb::from_colorref(b), t).to_colorref()
}

/// A colour that is `amount` darker (0 = the same colour, 1 = black).
///
/// The factor is applied to the linear light, so the colour keeps its hue instead of going grey.
fn darken(value: COLORREF, amount: f64) -> COLORREF {
    paint::darken(Rgb::from_colorref(value), amount).to_colorref()
}

/// Draw a line of text.
fn draw_text(hdc: HDC, value: &str, x: i32, y: i32, colour: COLORREF, font: HFONT) {
    let wide: Vec<u16> = value.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    // SAFETY: `hdc` is live; `wide` outlives the call.
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(font.0));
        SetTextColor(hdc, colour);
        let _ = windows::Win32::Graphics::Gdi::TextOutW(hdc, x, y, &wide);
        let _ = SelectObject(hdc, previous);
    }
}

/// Draw a line of text that is cut off with an ellipsis when it does not fit into `max_width`.
fn draw_text_clipped(
    hdc: HDC,
    value: &str,
    x: i32,
    y: i32,
    max_width: i32,
    colour: COLORREF,
    font: HFONT,
) {
    if max_width <= 0 {
        return;
    }
    if measure(hdc, value, font) <= max_width {
        draw_text(hdc, value, x, y, colour, font);
        return;
    }
    let mut cut = value.to_string();
    while !cut.is_empty() && measure(hdc, &format!("{cut}…"), font) > max_width {
        cut.pop();
    }
    draw_text(hdc, &format!("{cut}…"), x, y, colour, font);
}

/// Breaks one line of text into lines that are not wider than `max_width`.
///
/// Latin text breaks at a space, CJK text breaks between two characters (there are no spaces to break at). A word that
/// is longer than the whole line is broken in the middle, so that nothing is ever cut off.
fn wrap_line(line: &str, max_width: i32, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    // The place where the line may be broken: after a space.
    let mut break_at: Option<usize> = None;
    for ch in line.chars() {
        let candidate = format!("{current}{ch}");
        if !current.is_empty() && measure(&candidate) > max_width {
            match break_at {
                Some(at) => {
                    // break at the space: the space is eaten, the part of the word that was already read starts the
                    // next line
                    let head = current[..at].trim_end().to_string();
                    let tail = current[at + 1..].to_string();
                    out.push(head);
                    current = format!("{tail}{ch}");
                }
                None => {
                    out.push(std::mem::take(&mut current));
                    current.push(ch);
                }
            }
            break_at = None;
        } else {
            current = candidate;
        }
        if ch == ' ' {
            break_at = Some(current.len() - 1);
        } else if !ch.is_ascii() {
            // A CJK character is its own break opportunity.
            break_at = None;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Breaks a text into the lines that fit into `max_width`. A line break in the text is kept.
fn wrap_text(text: &str, max_width: i32, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        out.extend(wrap_line(line, max_width, measure));
    }
    out
}

/// Draws a text that may be longer than the window: it is broken into lines that fit. Returns the y of the line below
/// the text.
#[allow(clippy::too_many_arguments)]
fn draw_paragraph(
    hdc: HDC,
    value: &str,
    x: i32,
    mut y: i32,
    max_width: i32,
    colour: COLORREF,
    font: HFONT,
    line_height: i32,
) -> i32 {
    let measure = |text: &str| measure(hdc, text, font);
    for line in wrap_text(value, max_width, &measure) {
        draw_text(hdc, &line, x, y, colour, font);
        y += line_height;
    }
    y
}

/// The width of a line of text.
fn measure(hdc: HDC, value: &str, font: HFONT) -> i32 {
    let wide: Vec<u16> = value.encode_utf16().collect();
    let mut size = SIZE::default();
    // SAFETY: `hdc` is live; `wide` and `size` are live locals.
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
        let _ = SelectObject(hdc, previous);
    }
    size.cx
}

/// The sizes the embedded logo has (the file carries one picture per size).
const LOGO_SIZES: [i32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// The size of the picture to use for a target size: the smallest one that is not smaller than the target, so the
/// logo is never enlarged. Enlarging a small picture is what makes it blurry.
fn logo_source_size(target: i32) -> i32 {
    LOGO_SIZES
        .iter()
        .copied()
        .find(|size| *size >= target)
        .unwrap_or(*LOGO_SIZES.last().unwrap_or(&256))
}

/// The size of the picture to draw for a wanted size, limited by the room that is free. The result is always one of
/// the sizes of the file, so the picture can be drawn in its own size and stays sharp.
fn logo_fit_size(target: i32, room: i32) -> i32 {
    let wanted = logo_source_size(target);
    // the biggest picture of the file that still fits into the room
    let fits = LOGO_SIZES
        .iter()
        .copied()
        .rev()
        .find(|size| *size <= room)
        .unwrap_or(*LOGO_SIZES.first().unwrap_or(&16));
    wanted.min(fits)
}

/// Write the embedded logo to a private file (the icon loader of Windows only reads files).
fn write_logo_file() -> Option<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("ime1.0-logo-{}.ico", std::process::id()));
    if std::fs::write(&path, LOGO_ICO).is_err() {
        return None;
    }
    Some(path)
}

/// The logo at the size it is drawn at (`size` in pixels). The picture is loaded at the smallest size of the file
/// that is not smaller than the wanted one, so it is never blown up.
fn logo_icon(state: &mut State, size: i32) -> Option<HICON> {
    let source = logo_source_size(size);
    if let Some((_, icon)) = state.logos.iter().find(|(s, _)| *s == source) {
        return Some(*icon);
    }
    let path = state.logo_file.as_ref()?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: the file exists and the string is a live local with its terminator.
    let icon = unsafe {
        LoadImageW(
            HINSTANCE::default(),
            PCWSTR(wide.as_ptr()),
            IMAGE_ICON,
            source,
            source,
            LR_LOADFROMFILE,
        )
    };
    match icon {
        Ok(handle) => {
            let handle = HICON(handle.0);
            state.logos.push((source, handle));
            Some(handle)
        }
        Err(_) => None,
    }
}

/// The texts of the wizard (the console mode uses the same language).
pub fn wizard_title(lang: Lang) -> String {
    crate::text::text(lang, Msg::Title, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measure that says: every character is one unit wide.
    fn width(text: &str) -> i32 {
        text.chars().count() as i32
    }

    /// The paths of a machine, for the checks of a folder (they do not touch the machine they run on).
    fn system_paths() -> crate::plan::SystemPaths {
        crate::plan::SystemPaths {
            system_root: PathBuf::from(r"C:\Windows"),
            system_dir: PathBuf::from(r"C:\Windows\System32"),
            program_files: PathBuf::from(r"C:\Program Files"),
            program_files_x86: Some(PathBuf::from(r"C:\Program Files (x86)")),
            program_data: Some(PathBuf::from(r"C:\ProgramData")),
            user_profile: Some(PathBuf::from(r"C:\Users\me")),
            local_time: crate::plan::LocalTime {
                year: 2026,
                month: 11,
                day: 2,
                hour: 9,
                minute: 5,
                second: 3,
            },
        }
    }

    #[test]
    fn the_field_of_the_folder_is_made_with_the_width_it_is_drawn_with() {
        // An edit control of Windows lays its text out for the width it was made with, and it only lays it out
        // again when the width really changes. A field that is made narrow and made wider afterwards shows a part
        // of the path only. The field is therefore made with the width it has later, and both come from one rule.
        for scale in [96, 120, 144, 192] {
            let unit = |v: i32| v * scale / 96;
            for client in [520, 744, 1000] {
                let field = path_field_rect(&unit, client, 0);
                let width = field.right - field.left;
                assert!(width > 0, "the field has no room at {scale} dpi");
                // the field ends where the button begins, with the gap that is designed
                let panel_right = client - unit(PANEL_MARGIN);
                let button_left = panel_right - unit(BUTTON_GAP) - unit(BUTTON_WIDTH);
                assert_eq!(field.right, button_left - unit(FIELD_RIGHT_GAP));
                assert!(
                    field.left > unit(PANEL_MARGIN),
                    "the field is outside the panel"
                );
                // the width does not depend on where the panel stands
                let lower = path_field_rect(&unit, client, 200);
                assert_eq!(lower.right - lower.left, width);
                // the field is a line of text
                assert!(field.bottom - field.top >= unit(20));
            }
        }
    }

    #[test]
    fn the_window_accepts_a_usable_folder_and_refuses_the_others_with_a_reason() {
        let system = system_paths();
        // a folder that the installer can use is taken as it is
        let ok = checked_prefix(Path::new(r"C:\Tools\IME1.0"), &system);
        assert_eq!(ok.as_deref().ok(), Some(Path::new(r"C:\Tools\IME1.0")));
        // a place that is chosen gets the product folder, so the program never lands in the root of a drive
        let appended = checked_prefix(Path::new(r"D:\"), &system);
        assert_eq!(appended.as_deref().ok(), Some(Path::new(r"D:\IME1.0")));
        // nothing typed means the default folder
        let empty = checked_prefix(Path::new(""), &system);
        assert_eq!(
            empty.as_deref().ok(),
            Some(Path::new(r"C:\Program Files\IME1.0"))
        );
        // the ones it cannot use are refused, and the window always has a reason to write down
        for raw in [
            "not a path",
            r"C:\Windows\PIME",
            r"C:\Program Files\X\..\Y",
            r"\\server\share\PIME",
        ] {
            let refused = checked_prefix(Path::new(raw), &system);
            assert!(refused.is_err(), "the window accepted {raw:?}");
            let reason = refused.unwrap_err();
            assert!(
                !reason.trim().is_empty(),
                "the window has no reason for {raw:?}"
            );
        }
    }

    /// A state of the window, for the checks of the folder (nothing is shown and nothing is drawn).
    fn state_with(prefix: &str) -> State {
        State {
            page: Page::Welcome,
            job: Job::Install,
            lang: Lang::En,
            version: "0.2.0".to_string(),
            prefix: PathBuf::from(prefix),
            summary: String::new(),
            register: true,
            arp: true,
            vscode: true,
            log: Vec::new(),
            outcome: String::new(),
            ok: true,
            buttons: Vec::new(),
            options: Vec::new(),
            folder_panel: None,
            path_edit: None,
            accelerator: HACCEL::default(),
            path_field: None,
            path_dirty: false,
            path_error: None,
            install_dir: None,
            legacy_hint: None,
            existing: None,
            uninstall_summary: None,
            system: system_paths(),
            pending: None,
            closed: false,
            logos: Vec::new(),
            logo_file: None,
            fonts: Fonts::new(96),
            scale: 96,
        }
    }

    /// The folder of an install that is already there is not worked out again: a repair has to land in that very
    /// folder, and the product folder must not be put into it a second time.
    #[test]
    fn the_folder_of_an_install_that_is_already_there_is_not_worked_out_again() {
        let mut state = state_with(r"C:\Program Files\Programming IME");
        state.existing = Some(crate::detect::Existing {
            prefix: PathBuf::from(r"C:\Program Files\Programming IME"),
            version: Some("0.1.0".into()),
            found: crate::detect::Found::Manifest,
            manifest: None,
        });
        refresh_path(&mut state);
        assert_eq!(
            state.install_dir.as_deref(),
            Some(Path::new(r"C:\Program Files\Programming IME"))
        );
        assert!(state.path_error.is_none(), "{:?}", state.path_error);
    }

    /// The window says where the program really goes to, while the person types: the field keeps what was typed,
    /// the line under it shows the folder the program is installed in.
    #[test]
    fn the_window_shows_where_the_program_goes_while_the_person_types() {
        let mut state = state_with(r"D:\");
        refresh_path(&mut state);
        assert_eq!(state.install_dir.as_deref(), Some(Path::new(r"D:\IME1.0")));
        assert!(state.path_error.is_none(), "{:?}", state.path_error);
        // a folder that cannot be used is reported with a reason, and nothing is promised
        let mut state = state_with(r"C:\Windows\Tools");
        refresh_path(&mut state);
        assert!(state.install_dir.is_none());
        // the reason is the technical detail; the sentence that says it cannot be used is in the language of the
        // window and is written above the reason
        let reason = state.path_error.clone().unwrap();
        assert!(reason.contains("Windows"), "{reason}");
        let headline = wizard_text(state.lang, Msg::WizardPathInvalid);
        assert!(headline.contains("cannot be used"), "{headline}");
        // and the rule is the same everywhere: the product folder is the last part of the folder
        let mut state = state_with(r"E:\Tools");
        refresh_path(&mut state);
        assert_eq!(
            state.install_dir.as_deref(),
            Some(Path::new(r"E:\Tools\IME1.0"))
        );
    }

    #[test]
    fn an_empty_field_means_the_default_folder() {
        // The field is what the person types in: clearing it means "the default folder", the same rule as on the
        // command line. It does not keep the folder that was there before.
        let mut state = state_with("");
        refresh_path(&mut state);
        assert_eq!(
            state.install_dir.as_deref(),
            Some(Path::new(r"C:\Program Files\IME1.0"))
        );
        assert!(state.path_error.is_none(), "{:?}", state.path_error);
    }

    #[test]
    fn a_short_text_stays_one_line() {
        assert_eq!(wrap_line("short", 80, &width), vec!["short".to_string()]);
    }

    #[test]
    fn a_long_latin_text_breaks_at_a_space() {
        let text = "IME1.0 is a programming input method and it is not code signed";
        let lines = wrap_line(text, 20, &width);
        assert!(lines.len() > 1, "the text must be broken: {lines:?}");
        for line in &lines {
            assert!(width(line) <= 20, "the line is too wide: {line:?}");
        }
        // nothing is lost: the words are still there
        let joined: String = lines.join(" ");
        assert_eq!(
            joined.split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn cjk_text_breaks_between_two_characters() {
        // there is no space to break at: the line breaks between two characters
        let text = "これはプログラミング用の入力方式です";
        let lines = wrap_line(text, 10, &width);
        assert!(lines.len() > 1, "the text must be broken: {lines:?}");
        for line in &lines {
            assert!(width(line) <= 10, "the line is too wide: {line:?}");
        }
        assert_eq!(lines.concat(), text, "no character is lost");
    }

    #[test]
    fn a_word_that_is_longer_than_the_line_is_broken_in_the_middle() {
        let text = "C:\\Program Files\\Programming IME\\pime_tsf.dll";
        let lines = wrap_line(text, 12, &width);
        for line in &lines {
            assert!(width(line) <= 12, "the line is too wide: {line:?}");
        }
        // the spaces at a break are eaten, every other character is kept
        assert_eq!(lines.concat().replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn a_line_break_in_the_text_is_kept() {
        let lines = wrap_text("one\ntwo", 80, &width);
        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn the_logo_is_never_enlarged_that_is_what_makes_it_blurry() {
        for target in [1, 16, 17, 32, 38, 48, 49, 64, 100, 200, 256] {
            let source = logo_source_size(target);
            assert!(
                source >= target,
                "the logo would be enlarged: {target} -> {source}"
            );
            assert!(
                LOGO_SIZES.contains(&source),
                "the logo file has no picture of that size: {source}"
            );
        }
        // a size that the file carries is used as it is (no scaling at all)
        for size in LOGO_SIZES {
            assert_eq!(logo_source_size(size), size);
        }
        // and it is the smallest picture that is big enough (a bigger one would be scaled down for nothing)
        assert_eq!(logo_source_size(1), 16);
        assert_eq!(logo_source_size(17), 24);
        assert_eq!(logo_source_size(33), 48);
        // a target bigger than every picture falls back to the biggest one
        assert_eq!(logo_source_size(512), *LOGO_SIZES.last().unwrap());
    }

    #[test]
    fn the_logo_is_drawn_in_the_size_of_the_file_it_comes_from() {
        // the picture that is drawn is always one of the sizes of the file, so it is never scaled: a scaled picture
        // is what looks blurry
        for target in [1, 16, 17, 32, 38, 48, 49, 64, 76, 100, 200, 256] {
            for room in [16, 24, 32, 46, 56, 64, 112, 256] {
                let size = logo_fit_size(target, room);
                assert!(
                    LOGO_SIZES.contains(&size),
                    "the logo file has no picture of that size: {size}"
                );
                assert!(
                    size <= room,
                    "the logo does not fit into its tile: {size} > {room}"
                );
            }
        }
        // a room that fits the wanted picture keeps it: the wizard draws the 48 pixel picture in a 56 pixel tile
        assert_eq!(logo_fit_size(38, 56), 48);
        // a room that is too small keeps the biggest picture that fits: it stays sharp and it stays in its tile
        assert_eq!(logo_fit_size(38, 46), 32);
        assert_eq!(logo_fit_size(38, 32), 32);
        assert_eq!(logo_fit_size(38, 16), 16);
        // on a high dpi screen the room is big and the picture is still one of the file
        assert_eq!(logo_fit_size(76, 112), 64);
    }

    #[test]
    fn the_colours_of_the_rainbow_are_mixed_and_darkened_as_expected() {
        let black = COLORREF(0x00000000);
        let white = COLORREF(0x00FFFFFF);
        assert_eq!(mix(black, white, 0.0).0, black.0);
        assert_eq!(mix(black, white, 1.0).0, white.0);
        // The mix happens in linear light, so the middle of black and white is the grey the eye expects in the
        // middle (0xBC) and not the arithmetic middle (0x80), which looks too dark.
        assert_eq!(mix(black, white, 0.5).0, 0x00BCBCBC);
        // a darker colour has smaller channels, and it never becomes brighter
        let colour = colour::RAINBOW[0];
        let darker = darken(colour, 0.5);
        assert!(darker.0 < colour.0, "the colour did not get darker");
        assert_eq!(darken(colour, 0.0).0, colour.0);
    }

    #[test]
    fn an_empty_text_is_one_empty_line() {
        assert_eq!(wrap_text("", 80, &width), vec![String::new()]);
    }
}
