//! The settings of the input method: the file of the user, and the window that shows them.
//!
//! The settings are read when the text service is activated and written when the user turns something on or off.
//! The settings window is the place where the whole set is shown; when the user saves there, the file is read again
//! and applied, which is why the reading and the writing live in the same module.

use crate::imesettings::{self, ImeSettings};
use crate::settings_window::SettingsWindow;

use super::{Inner, trace};

/// The settings of every language, from the file of the user (the defaults when there is none).
pub(super) fn load_settings() -> ImeSettings {
    match imesettings::settings_path() {
        Some(path) => ImeSettings::load(&path),
        None => ImeSettings::default(),
    }
}

/// Tell the core which plugin languages are on.
///
/// The set belongs to the engine: it is the place that decides whether a plugin profile has candidates at all,
/// and the settings window is the place that changes it.
pub(super) fn apply_plugins(plugins: &[String]) {
    pime_core::predict::engine().set_plugin_languages(plugins);
}

impl Inner {
    /// Write the settings back to the file of the user.
    pub(super) fn save_settings(&mut self) {
        let Some(path) = imesettings::settings_path() else {
            return;
        };
        let mut settings = ImeSettings::load(&path);
        settings.effort = self.effort.clone();
        settings.skin = self.skin.clone();
        settings.mouth_place = self.mouth_place.clone();
        settings.set(&self.language, self.settings);
        if let Err(e) = settings.save(&path) {
            trace(&format!("the settings could not be saved: {e}"));
        }
    }

    /// Open the settings window (once; a second click does nothing while it is open).
    pub(super) fn open_settings(&mut self) {
        if self.settings_window.is_some() {
            return;
        }
        let settings = load_settings();
        let sink = self.settings_sink.clone();
        self.settings_window = SettingsWindow::open(settings, sink);
    }

    /// Read the settings file again and apply what it says.
    pub(super) fn reload_settings(&mut self) {
        let settings = load_settings();
        apply_plugins(&settings.plugins);
        let language = self.language.clone();
        let value = settings.get(&language);
        self.settings = value;
        self.effort = settings.effort.clone();
        self.skin = settings.skin.clone();
        self.mouth_place = settings.mouth_place.clone();
        self.controller.apply_settings(&value);
        // The vertical writing mode is a global setting: the file says whether the text stands as a column.
        // Ctrl+Alt+V switches it for the session only; saving the settings makes the file the truth again.
        self.controller.set_vertical_mode(settings.vertical);
        self.sync_laser();
    }
}
