//! The laser panel: the rows it shows, what the user does in it, the menus that open from it and the demonstration
//! that writes with it.
//!
//! The panel is the quick way into the settings (the Sogou way): the switches that are turned on and off while
//! typing, the scheme of the keys, the language, the effort of the AI and the way the input method looks. The last
//! row opens the settings window itself.

use crate::controller::{HostError, HostText};
use crate::imesettings::{self, LanguageSettings};
use crate::laser::PanelItem;
use crate::laser_ui::LaserEvent;
use crate::layout::Rect;
use crate::menus::{MenuAction, MenuItem, MenuWindow};
use crate::popup::work_area_of;
use crate::skin::skin_colors;
use crate::ui::{Anchor, ListAction};

use super::{Inner, trace};

/// The menus of the product: the ones the panel of the laser interface opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuKind {
    /// The language of the input method.
    Language,
    /// The effort of the AI features.
    Effort,
    /// The skin of the product.
    Skin,
    /// The scheme of the keys (the programmer habit, Microsoft, Sogou, ATOK).
    Keymap,
    /// The symbol table: the symbols of the core, written into the document by a click.
    Symbols,
    /// The text that is on the clipboard.
    Clipboard,
    /// The date and the time of the machine.
    DateTime,
}

/// The date and the time of the machine, as they are written into the document.
///
/// The local time of Windows, not the clock of the machine in UTC: what the user reads is what is written.
fn now_text() -> String {
    // SAFETY: the call fills the structure it returns; nothing is passed in.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}

/// The rows of the laser panel: the settings that are used most often, in the order they are drawn.
///
/// The panel is the quick way into the settings (the Sogou way): the switches that are turned on and off while
/// typing, the scheme of the keys, the language, the effort of the AI and the way the input method looks. The last
/// row opens the settings window itself.
///
/// The function is free of the state of the text service, so that "which rows are there" can be tested without a
/// host: the window of the panel is sized for [`crate::laser_ui::PANEL_ROWS`] rows.
fn laser_items(
    language: &str,
    settings: &LanguageSettings,
    effort: &str,
    skin: &str,
    input_mode: bool,
) -> Vec<PanelItem> {
    vec![
        PanelItem::Toggle {
            key: "input_mode",
            label: "输入模式",
            on: input_mode,
        },
        PanelItem::Toggle {
            key: "space_confirms",
            label: "空格上屏",
            on: settings.space_confirms,
        },
        PanelItem::Toggle {
            key: "auto_compose",
            label: "自动开组合",
            on: settings.auto_compose,
        },
        PanelItem::Menu {
            key: "keymap",
            label: "按键方案",
            value: settings.keymap_preset.label().to_string(),
        },
        PanelItem::Menu {
            key: "language",
            label: "语言",
            value: imesettings::display_name(language).to_string(),
        },
        PanelItem::Menu {
            key: "effort",
            label: "AI 力度",
            value: imesettings::effort_label(effort).to_string(),
        },
        PanelItem::Menu {
            key: "skin",
            label: "外观",
            value: imesettings::skin_label(skin).to_string(),
        },
        // the three local tools of the Sogou-style bar: the symbol table, the date and the time, the clipboard
        PanelItem::Menu {
            key: "symbols",
            label: "符号表",
            value: String::new(),
        },
        PanelItem::Menu {
            key: "datetime",
            label: "日期时间",
            value: String::new(),
        },
        PanelItem::Menu {
            key: "clipboard",
            label: "剪贴板",
            value: String::new(),
        },
        PanelItem::Demo,
        PanelItem::Settings,
    ]
}

impl Inner {
    /// Open one of the menus under the rectangle the user clicked (a row of the panel).
    fn open_menu_at(&mut self, which: MenuKind, rect: Rect) {
        // Only one menu at a time: the old one goes away.
        self.menu = None;
        let items = self.menu_items(which);
        let work = work_area_of(rect);
        let sink = self.menu_sink.clone();
        self.menu = MenuWindow::open(
            items,
            which == MenuKind::Language,
            rect,
            work,
            skin_colors(&self.skin),
            sink,
        );
    }

    /// The items of one menu.
    fn menu_items(&self, which: MenuKind) -> Vec<MenuItem> {
        match which {
            MenuKind::Effort => imesettings::EFFORT_LEVELS
                .iter()
                .map(|level| {
                    MenuItem::checked(
                        imesettings::effort_label(level),
                        MenuAction::SetEffort((*level).to_string()),
                        self.effort == *level,
                    )
                })
                .collect(),
            MenuKind::Skin => imesettings::SKINS
                .iter()
                .map(|skin| {
                    MenuItem::checked(
                        imesettings::skin_label(skin),
                        MenuAction::SetSkin((*skin).to_string()),
                        self.skin == *skin,
                    )
                })
                .collect(),
            MenuKind::Language => imesettings::LANGUAGES
                .iter()
                .map(|language| {
                    MenuItem::new(
                        imesettings::display_name(language),
                        MenuAction::SwitchLanguage((*language).to_string()),
                    )
                })
                .collect(),
            MenuKind::Symbols => pime_core::symbol_table::symbol_pairs()
                .into_iter()
                .map(|(text, name)| {
                    MenuItem::new(
                        &format!("{text}  {name}"),
                        MenuAction::InsertText(text.to_string()),
                    )
                })
                .collect(),
            MenuKind::Clipboard => crate::clipboard::entries()
                .into_iter()
                .map(|text| {
                    let label: String = text.chars().take(40).collect();
                    MenuItem::new(&label, MenuAction::InsertText(text))
                })
                .collect(),
            MenuKind::DateTime => {
                let now = now_text();
                vec![MenuItem::new(&now, MenuAction::InsertText(now.clone()))]
            }
            MenuKind::Keymap => imesettings::KeymapPreset::ALL
                .iter()
                .map(|preset| {
                    MenuItem::checked(
                        preset.label(),
                        MenuAction::SetKeymapPreset(*preset),
                        self.settings.keymap_preset == *preset,
                    )
                })
                .collect(),
        }
    }

    /// What the user chose in a menu.
    pub(super) fn apply_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::ToggleInputMode => {
                let on = !self.controller.input_mode();
                self.controller.set_input_mode(on);
                self.sync_laser();
            }
            MenuAction::SetEffort(effort) => {
                self.effort = effort;
                self.save_settings();
            }
            MenuAction::SetSkin(skin) => {
                self.skin = skin;
                self.save_settings();
                self.sync_laser();
            }
            MenuAction::SetOption(name, on) => {
                self.set_option(&name, on);
            }
            MenuAction::SwitchLanguage(language) => {
                self.switch_language(&language);
            }
            MenuAction::InsertText(text) => {
                // The text goes into the document at the caret, exactly as the agent's write does: the same path,
                // the same safety rules (a password field takes nothing).
                self.insert_text(&text);
            }
            MenuAction::SetKeymapPreset(preset) => {
                // The scheme changes the keys at once: that is the point of the schemes.
                let mut value = self.settings;
                if value.apply_preset(preset) {
                    self.settings = value;
                    self.controller.apply_settings(&value);
                    self.save_settings();
                    self.sync_laser();
                }
            }
            MenuAction::OpenSettings => self.open_settings(),
        }
    }

    /// Switch one of the settings of the active language on or off.
    ///
    /// The menus and the laser panel all go through here, so that they can never disagree about what the setting is.
    fn set_option(&mut self, name: &str, on: bool) {
        let mut value = self.settings;
        match name {
            "tab_cycles" => value.tab_cycles = on,
            "tab_to_host" => value.tab_to_host = on,
            "space_confirms" => value.space_confirms = on,
            "auto_compose" => value.auto_compose = on,
            "punct_paging" => value.punct_paging = on,
            "laser_ui" => value.laser_ui = on,
            "input_mode" => {
                // The input mode is a state of the controller, not a file: it is the same thing Alt+F1 does.
                self.controller.set_input_mode(on);
                self.settings = value;
                self.sync_laser();
                return;
            }
            _ => return,
        }
        // A key that the user turns by hand is no longer the one of the scheme: the scheme becomes "custom".
        if !matches!(name, "laser_ui") {
            value.keymap_preset = imesettings::KeymapPreset::Custom;
        }
        self.settings = value;
        self.controller.apply_settings(&value);
        self.save_settings();
        self.sync_laser();
    }

    /// What the user asked for in the laser panel.
    pub(super) fn apply_laser_event(&mut self, event: LaserEvent) {
        match event {
            LaserEvent::Toggle { key, on } => self.set_option(key, on),
            LaserEvent::Menu { key } => self.open_laser_menu(key),
            LaserEvent::Candidate { index } => {
                // A candidate of the list was clicked in the panel: it is taken as if the user had pressed
                // the key of it.
                self.controller.apply_list_action(ListAction::Take(index));
            }
            LaserEvent::Settings => self.open_settings(),
            LaserEvent::Demo => self.run_demo(),
            LaserEvent::Switch => {
                // The mouth of the light is the switch between the input methods.
                //
                // The windows go off the screen before the switch: TSF deactivates the text service inside that very
                // call, and the state is borrowed at that moment, so nothing could be hidden after the switch.
                self.hide_floating();
                crate::ime_switch::switch_now();
            }
            LaserEvent::MouthMoved { edge, offset } => {
                // The mouth was dragged to another place: it is remembered, so it is still there next time.
                self.mouth_place = crate::laser::MouthPlace { edge, offset }.to_text();
                self.save_settings();
            }
        }
    }

    /// Run the demonstration of the laser interface: ten agents write their own domain, one after the other.
    ///
    /// The demonstration is a one-off: it runs when the user clicks its row of the panel, and it never
    /// changes the way the ordinary typing works. A password field gets nothing at all.
    fn run_demo(&mut self) {
        // The seed is the number of the run: the same seed gives the same picture of the mouths, and the
        // clock of the machine is never read (the rules of the repository).
        self.demo_runs += 1;
        let seed = u64::from(self.demo_runs);
        let work = self
            .laser
            .try_borrow()
            .map(|laser| laser.work_area())
            .unwrap_or(Rect::new(0, 0, 0, 0));
        let report = {
            let mut host = DemoHost { inner: self };
            crate::demo::run(&mut host, seed, work)
        };
        match &report.error {
            Some(error) => trace(&format!("the demonstration stopped: {error}")),
            None => trace(&format!(
                "the demonstration wrote {} agents",
                report.agents.len()
            )),
        }
        // The panel says what happened: the user must not be left with the idea that something was written
        // when nothing was.
        let status = report.status();
        if let Ok(mut laser) = self.laser.try_borrow_mut() {
            laser.set_status(Some(&status));
            laser.open_panel();
        }
    }

    /// Write a text into the document at the caret: a symbol of the table, the text of the clipboard, the date and
    /// the time.
    ///
    /// It is the same path as the write of an agent, with the same safety rules: a password field takes nothing,
    /// and the panel says why.
    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if crate::agent_bridge::may_write(self.controller.host_mut().is_secure_field()).is_err() {
            if let Ok(mut laser) = self.laser.try_borrow_mut() {
                laser.set_status(Some("密码框：一个字符也没有写入"));
            }
            return;
        }
        // Where the text lands: the caret before the write is the place the character appears at.
        let anchor = self.controller.host_mut().caret_anchor();
        if self.controller.host_mut().commit("", text).is_ok()
            && let Some(anchor) = anchor
            && let Ok(mut laser) = self.laser.try_borrow_mut()
        {
            laser.fire_for(crate::mouths::USER_KEY, crate::mouths::USER_LABEL, anchor);
        }
    }

    /// Open one of the lists of the laser panel under the row the user clicked.
    fn open_laser_menu(&mut self, key: &str) {        let which = match key {
            "language" => MenuKind::Language,
            "effort" => MenuKind::Effort,
            "skin" => MenuKind::Skin,
            "keymap" => MenuKind::Keymap,
            "symbols" => MenuKind::Symbols,
            "clipboard" => MenuKind::Clipboard,
            "datetime" => MenuKind::DateTime,
            _ => return,
        };
        let anchor = self
            .laser
            .try_borrow()
            .ok()
            .and_then(|laser| laser.row_rect(key));
        let Some(anchor) = anchor else {
            return;
        };
        self.open_menu_at(which, anchor);
    }

    /// The rows of the laser panel: the settings that are used most often, in the order they are drawn.
    ///
    /// The panel is the quick way into the settings (the Sogou way): the switches that are turned on and off while
    /// typing, the language, the effort of the AI and the way the input method looks.
    fn laser_items(&self) -> Vec<PanelItem> {
        laser_items(
            &self.language,
            &self.settings,
            &self.effort,
            &self.skin,
            self.controller.input_mode(),
        )
    }

    /// Put the laser interface on the screen and give it the rows it shows.
    pub(super) fn sync_laser(&mut self) {
        let items = self.laser_items();
        let Ok(mut laser) = self.laser.try_borrow_mut() else {
            return;
        };
        laser.set_items(items);
        laser.set_place(
            crate::laser::MouthPlace::from_text(&self.mouth_place),
        );
        // The interface is put on the screen every time: the switch of the input method can hide it while the state
        // says nothing about it (the hide happened in a callback of one of its own windows).
        let work = work_area_of(Rect::new(0, 0, 0, 0));
        laser.show(work);
    }
}

/// The document the demonstration writes into: the text service of this thread.
///
/// It is the only place that talks to the host: the demonstration itself is pure (in `demo`), so that it can
/// be tested without a document and without a screen.
struct DemoHost<'a> {
    /// The text service.
    inner: &'a mut Inner,
}

impl crate::demo::DemoHost for DemoHost<'_> {
    /// A password field is never written into: not one character.
    fn is_secure_field(&mut self) -> bool {
        self.inner.controller.host_mut().is_secure_field()
    }

    /// The text that is already in the document, just before the caret.
    fn text_at_caret(&mut self) -> Option<String> {
        self.inner.controller.host_mut().text_before_caret()
    }

    /// The caret in screen coordinates: the beams fly to it.
    fn caret_anchor(&mut self) -> Option<Anchor> {
        self.inner.controller.host_mut().caret_anchor()
    }

    /// Write the text at the caret and leave the caret after it.
    fn write(&mut self, text: &str) -> std::result::Result<(), crate::demo::DemoError> {
        match self.inner.controller.host_mut().commit("", text) {
            Ok(()) => Ok(()),
            Err(HostError::Mismatch) => Err(crate::demo::DemoError::Refused(
                "there is a selection: the text would have replaced it".to_string(),
            )),
            Err(HostError::Refused) => Err(crate::demo::DemoError::Refused(
                "the host refused the write".to_string(),
            )),
        }
    }

    /// The light of the agent: the beam comes out of its mouth and has its colour.
    fn fire(&mut self, key: &str, label: &str, anchor: Anchor, mouth: (i32, i32)) {
        if let Ok(mut laser) = self.inner.laser.try_borrow_mut() {
            laser.fire_for_at(key, label, anchor, mouth.0, mouth.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel of the laser interface must have exactly the number of rows its window is tall enough for.
    #[test]
    fn the_panel_has_the_number_of_rows_its_window_expects() {
        let items = laser_items(
            "zh-CN",
            &LanguageSettings::default(),
            "auto",
            "laser",
            true,
        );
        assert_eq!(
            items.len(),
            crate::laser_ui::PANEL_ROWS,
            "the panel shows {items:?}"
        );
        // the last row is the one that opens the settings window
        assert_eq!(items.last(), Some(&PanelItem::Settings));
        // and the demonstration is the row just before it (the user asked for it there)
        assert_eq!(items[items.len() - 2], PanelItem::Demo);
        assert!(
            items.iter().any(|item| item.label() == "多 Agent 演示"),
            "{items:?}"
        );
        // and the scheme of the keys is one of the rows (the user asked for it to be reachable from the panel)
        assert!(
            items.iter().any(|item| item.label() == "按键方案"),
            "{items:?}"
        );
    }
}
