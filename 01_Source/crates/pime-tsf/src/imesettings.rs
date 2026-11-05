//! The basic settings of the input method: one set per language profile, stored in a JSON file in the user's local
//! application data.
//!
//! The model is pure data (no COM, no Win32), so the whole file is testable without Windows: what the settings window
//! shows, what the text service applies and what survives a restart are three separate questions and this file answers
//! the first and the third.
//!
//! Storage rules (a settings file is user data, not a program file):
//! * the file lives in `%LOCALAPPDATA%\Programming IME\settings.json` — the installation folder needs administrator
//!   rights and must stay read-only for the input method;
//! * a missing file, an unreadable file or a file that is not valid JSON means "the defaults", never a failure: the
//!   input method must start even when the settings are broken;
//! * writing is atomic (a temporary file and a rename), so a crash while saving leaves the old file intact;
//! * unknown keys and unknown languages are ignored, and a value out of range falls back to its default, so a file
//!   written by a newer version still loads.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pime_core::json::{Json, Limits};

/// The version of the file format. A file with a higher version is still read (the unknown fields are ignored).
pub const VERSION: i64 = 1;

/// The language profiles the product ships, in the order the settings window lists them.
///
/// The list itself is the one of the core: the switch key walks through the same three languages.
pub const LANGUAGES: [&str; 3] = pime_core::imeswitch::LANGUAGES;

/// The plugin languages the product knows, with the name the settings window shows.
///
/// A plugin adds a whole language: its own rules, its own dialects and its own validator. The ids are the
/// ones of the core (`pime_core::pack::PLUGIN_LANGUAGES`); a unit test pins that the two lists agree, so a
/// plugin that the core knows cannot be forgotten here.
///
/// The product ships with Cypher and PL/SQL on and T-SQL off: the two SQL plugins share the `.sql` extension,
/// so only one of the two may own it at a time.
pub const PLUGINS: [(&str, &str); 3] = [
    ("cypher", "Cypher"),
    ("plsql", "PL/SQL"),
    ("tsql", "T-SQL"),
];

/// The name of the settings file inside the local application data folder.
pub const FILE_NAME: &str = "settings.json";

/// The folder of the product inside the local application data.
pub const APP_DIR: &str = "Programming IME";

/// What the user reads for a language in the settings window.
pub fn display_name(language: &str) -> &'static str {
    match language {
        "zh-CN" => "中文",
        "en-US" => "English",
        "ja-JP" => "日本語",
        _ => "未知",
    }
}

/// The Windows language id of a profile (`langid`).
pub fn language_id(language: &str) -> u16 {
    match language {
        "zh-CN" => 0x0804,
        "en-US" => 0x0409,
        "ja-JP" => 0x0411,
        _ => 0,
    }
}

/// The profile key of a Windows language id. `None` for a language the product does not ship.
pub fn language_for_id(langid: u16) -> Option<&'static str> {
    match langid {
        0x0804 => Some("zh-CN"),
        0x0409 => Some("en-US"),
        0x0411 => Some("ja-JP"),
        _ => None,
    }
}

/// The settings of one language profile.
///
/// The defaults are the ones the product ships with: the input method starts in direct mode (it eats no code
/// character), the candidate page holds five rows, the Tab key moves the highlight of the list (the programmer
/// habit: Tab selects, Enter/Space/digit take the candidate) and the space key is a space (in code it usually is).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageSettings {
    /// Candidates per page (1..=9). The digits 1..=page_size pick within the page.
    pub page_size: usize,
    /// Space confirms the highlighted candidate. Off by default: in code a space is a space.
    pub space_confirms: bool,
    /// Tab walks the candidate list instead of taking the highlighted candidate. **On** by default: the programmer
    /// habit is that Tab moves the highlight (it selects) and that Enter, Space or a digit take the candidate.
    pub tab_cycles: bool,
    /// The Tab key belongs to the host even while the candidate list is on screen (the IDE keeps its Tab, the list
    /// is a preview only). Off by default: Tab belongs to the candidate list.
    pub tab_to_host: bool,
    /// `=`, `.` and `]` page the list forward, `-`, `,` and `[` page it back (only with more than one page). Off by
    /// default: they are code characters.
    pub punct_paging: bool,
    /// The scheme of the keys the user picked.
    pub keymap_preset: KeymapPreset,
    /// A composition is opened as soon as candidates appear (the Sogou behaviour).
    pub auto_compose: bool,
    /// The input method starts in input mode (typing opens a composition) instead of direct mode.
    pub input_mode: bool,
    /// The laser interface (the blue glow at the left of the screen and the panel on it) is shown while this
    /// profile is active.
    pub laser_ui: bool,
}

impl Default for LanguageSettings {
    fn default() -> LanguageSettings {
        LanguageSettings {
            page_size: 5,
            // The mainstream behaviour: while the candidate list is on screen, Space takes the highlighted
            // candidate. Direct mode (Alt+F1) still lets every character through to the host.
            space_confirms: true,
            // The programmer habit the user asked for: Tab selects (it moves the highlight), Enter/Space/digit
            // take the candidate.
            tab_cycles: true,
            tab_to_host: false,
            punct_paging: false,
            keymap_preset: KeymapPreset::Programmer,
            auto_compose: false,
            // The input method starts in input mode (the mainstream behaviour), not in direct mode.
            input_mode: true,
            laser_ui: true,
        }
    }
}

/// The schemes of the keys the settings window offers.
///
/// The user asked for the settings to offer the ways of other input methods ("包括不限于微软，日语的Atok等"), so the
/// schemes are named after them. They are **approximations** of the habits those input methods are known for: they
/// describe what this input method does, not a measurement of the other ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeymapPreset {
    /// The programmer habit: Tab moves the highlight of the list (it selects), Enter/Space/digit take the candidate.
    Programmer,
    /// The way of Microsoft Pinyin: Tab takes the highlighted candidate.
    Microsoft,
    /// The way of Sogou: the composition opens by itself and the punctuation pages the list.
    Sogou,
    /// The way of ATOK (the Japanese one): the candidate is taken with the key, the punctuation pages the list.
    Atok,
    /// The switches are the ones of the user.
    Custom,
}

impl KeymapPreset {
    /// Every scheme, in the order the settings window lists them.
    pub const ALL: [KeymapPreset; 5] = [
        KeymapPreset::Programmer,
        KeymapPreset::Microsoft,
        KeymapPreset::Sogou,
        KeymapPreset::Atok,
        KeymapPreset::Custom,
    ];

    /// The name of the scheme in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            KeymapPreset::Programmer => "programmer",
            KeymapPreset::Microsoft => "microsoft",
            KeymapPreset::Sogou => "sogou",
            KeymapPreset::Atok => "atok",
            KeymapPreset::Custom => "custom",
        }
    }

    /// The scheme of a name in the settings file. `None` for a name the product does not know.
    pub fn from_id(id: &str) -> Option<KeymapPreset> {
        KeymapPreset::ALL.into_iter().find(|preset| preset.id() == id)
    }

    /// The name of the scheme in the language of the user.
    pub fn label(self) -> &'static str {
        match self {
            KeymapPreset::Programmer => "程序员（默认）",
            KeymapPreset::Microsoft => "微软拼音",
            KeymapPreset::Sogou => "搜狗拼音",
            KeymapPreset::Atok => "ATOK（日文）",
            KeymapPreset::Custom => "自定义",
        }
    }

    /// What the scheme means. `None` for the custom one: the switches are the ones of the user then.
    pub fn behaviour(self) -> Option<KeyBehaviour> {
        match self {
            // the programmer habit: Tab moves the highlight (it selects), Enter/Space/digit take the candidate
            KeymapPreset::Programmer => Some(KeyBehaviour {
                tab_cycles: true,
                tab_to_host: false,
                space_confirms: true,
                punct_paging: false,
                auto_compose: false,
            }),
            // Microsoft Pinyin: the Tab key takes the highlighted candidate
            KeymapPreset::Microsoft => Some(KeyBehaviour {
                tab_cycles: false,
                tab_to_host: false,
                space_confirms: true,
                punct_paging: false,
                auto_compose: false,
            }),
            // Sogou: the composition opens by itself and the punctuation pages the list
            KeymapPreset::Sogou => Some(KeyBehaviour {
                tab_cycles: false,
                tab_to_host: false,
                space_confirms: true,
                punct_paging: true,
                auto_compose: true,
            }),
            // ATOK (the Japanese one): the candidate is taken with the key, the punctuation pages the list
            KeymapPreset::Atok => Some(KeyBehaviour {
                tab_cycles: false,
                tab_to_host: false,
                space_confirms: true,
                punct_paging: true,
                auto_compose: false,
            }),
            KeymapPreset::Custom => None,
        }
    }
}

/// What a scheme of the keys means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBehaviour {
    /// Tab moves the highlight of the candidate list.
    pub tab_cycles: bool,
    /// The Tab key belongs to the host.
    pub tab_to_host: bool,
    /// The space key takes the highlighted candidate.
    pub space_confirms: bool,
    /// The punctuation pages the list.
    pub punct_paging: bool,
    /// A composition opens as soon as candidates appear.
    pub auto_compose: bool,
}

impl LanguageSettings {
    /// The page size, clamped into the range the digits can address.
    pub fn page_size(&self) -> usize {
        self.page_size.clamp(1, 9)
    }

    /// Put the switches of a scheme into place. `false` for the custom one: the switches stay as they are.
    pub fn apply_preset(&mut self, preset: KeymapPreset) -> bool {
        self.keymap_preset = preset;
        let Some(behaviour) = preset.behaviour() else {
            return false;
        };
        self.tab_cycles = behaviour.tab_cycles;
        self.tab_to_host = behaviour.tab_to_host;
        self.space_confirms = behaviour.space_confirms;
        self.punct_paging = behaviour.punct_paging;
        self.auto_compose = behaviour.auto_compose;
        true
    }
}

/// The effort levels the AI features offer (`auto` lets the service decide).
pub const EFFORT_LEVELS: [&str; 4] = ["auto", "low", "medium", "high"];

/// The skins the floating bar offers.
///
/// "laser" is the look of the product (the dark glass with the rim of the brand rainbow); the other three follow the
/// theme of Windows or are fixed light/dark.
pub const SKINS: [&str; 4] = ["laser", "system", "light", "dark"];

/// The effort the AI features run with.
pub fn effort_label(effort: &str) -> &'static str {
    match effort {
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        _ => "自动",
    }
}

/// The name of a skin.
pub fn skin_label(skin: &str) -> &'static str {
    match skin {
        "laser" => "激光彩虹",
        "light" => "浅色",
        "dark" => "深色",
        _ => "跟随系统",
    }
}

/// The settings of every language profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImeSettings {
    /// One entry per language, in the order of [`LANGUAGES`].
    pub languages: Vec<(String, LanguageSettings)>,
    /// The effort the AI features run with (a global setting: the AI does not speak a language).
    pub effort: String,
    /// The skin of the floating bar (a global setting).
    pub skin: String,
    /// Where the mouth of the light is on the screen (a global setting): "left:520", "top:700" or "right:480".
    /// Empty is the default place: the left edge, in the middle of the screen.
    pub mouth_place: String,
    /// The plugin languages that are on (a global setting: a plugin is a language, not a way of typing).
    ///
    /// The product ships with Cypher and PL/SQL on and T-SQL off.
    pub plugins: Vec<String>,
}

impl Default for ImeSettings {
    fn default() -> ImeSettings {
        ImeSettings {
            languages: LANGUAGES
                .iter()
                .map(|language| ((*language).to_string(), LanguageSettings::default()))
                .collect(),
            effort: "auto".to_string(),
            skin: "laser".to_string(),
            mouth_place: String::new(),
            // Cypher and PL/SQL are on, T-SQL is off: the two SQL plugins share the `.sql` extension.
            plugins: pime_core::pack::default_plugin_ids(),
        }
    }
}

impl ImeSettings {
    /// The settings of one language; the defaults for a language the product does not know.
    pub fn get(&self, language: &str) -> LanguageSettings {
        self.languages
            .iter()
            .find(|(key, _)| key == language)
            .map(|(_, value)| *value)
            .unwrap_or_default()
    }

    /// Whether the plugin language is on.
    pub fn plugin_is_on(&self, id: &str) -> bool {
        self.plugins.iter().any(|plugin| plugin == id)
    }

    /// Turn a plugin language on or off.
    ///
    /// A plugin the product does not know is ignored: the list of the plugins is the one of the core.
    pub fn set_plugin(&mut self, id: &str, on: bool) {
        if !PLUGINS.iter().any(|(plugin, _)| *plugin == id) {
            return;
        }
        self.plugins.retain(|plugin| plugin != id);
        if on {
            self.plugins.push(id.to_string());
        }
    }

    /// Set the settings of one language (an unknown language is added, so that a future profile is not lost).
    pub fn set(&mut self, language: &str, value: LanguageSettings) {
        match self.languages.iter_mut().find(|(key, _)| key == language) {
            Some(entry) => entry.1 = value,
            None => self.languages.push((language.to_string(), value)),
        }
    }

    /// The file as a JSON value.
    pub fn to_json(&self) -> Json {
        let languages: Vec<(String, Json)> = self
            .languages
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    Json::Obj(vec![
                        ("pageSize".to_string(), Json::Num(value.page_size as f64)),
                        (
                            "spaceConfirms".to_string(),
                            Json::Bool(value.space_confirms),
                        ),
                        ("tabCycles".to_string(), Json::Bool(value.tab_cycles)),
                        ("tabToHost".to_string(), Json::Bool(value.tab_to_host)),
                        ("punctPaging".to_string(), Json::Bool(value.punct_paging)),
                        (
                            "keymapPreset".to_string(),
                            Json::Str(value.keymap_preset.id().to_string()),
                        ),
                        ("autoCompose".to_string(), Json::Bool(value.auto_compose)),
                        ("inputMode".to_string(), Json::Bool(value.input_mode)),
                        ("laserUi".to_string(), Json::Bool(value.laser_ui)),
                    ]),
                )
            })
            .collect();
        Json::Obj(vec![
            ("version".to_string(), Json::Num(VERSION as f64)),
            ("effort".to_string(), Json::Str(self.effort.clone())),
            ("skin".to_string(), Json::Str(self.skin.clone())),
            ("mouthPlace".to_string(), Json::Str(self.mouth_place.clone())),
            ("plugins".to_string(), Json::strs(&self.plugins)),
            ("languages".to_string(), Json::Obj(languages)),
        ])
    }

    /// The file as text.
    pub fn to_text(&self) -> String {
        self.to_json().to_json_string()
    }

    /// Read the settings out of a JSON text. Every problem is a problem of the *file*, not of the input method: the
    /// caller falls back to the defaults. `Err` names the reason, so that the settings window can tell the user that
    /// its file was not readable.
    pub fn from_text(text: &str) -> Result<ImeSettings, String> {
        let value = pime_core::json::parse(text, Limits::DEFAULT)
            .map_err(|e| format!("不是有效的 JSON：{e}"))?;
        let mut settings = ImeSettings::default();
        // The languages section is optional: a file that only carries the global settings still loads.
        match value.get("languages") {
            None => {}
            Some(languages) => {
                let Some(languages) = languages.as_object() else {
                    return Err("languages 段不是一个对象".to_string());
                };
                for (key, entry) in languages {
                    if !LANGUAGES.contains(&key.as_str()) {
                        continue; // a language the product does not ship: kept in the file, ignored here
                    }
                    settings.set(key, language_settings_of(entry));
                }
            }
        }
        // The global settings: an unknown value falls back to the default.
        if let Some(effort) = value.get("effort").and_then(Json::as_str)
            && EFFORT_LEVELS.contains(&effort)
        {
            settings.effort = effort.to_string();
        }
        if let Some(skin) = value.get("skin").and_then(Json::as_str)
            && SKINS.contains(&skin)
        {
            settings.skin = skin.to_string();
        }
        // The place of the mouth: a value that is not one is ignored (the mouth goes to its default place).
        if let Some(place) = value.get("mouthPlace").and_then(Json::as_str)
            && !place.trim().is_empty()
        {
            settings.mouth_place = place.trim().to_string();
        }
        // The plugins that are on: a plugin the product does not know is ignored, and the ones that are not
        // written in the file keep the default (on for Cypher and PL/SQL, off for T-SQL).
        if let Some(Json::Arr(list)) = value.get("plugins") {
            let mut plugins: Vec<String> = Vec::new();
            for item in list {
                let Some(id) = item.as_str() else {
                    continue;
                };
                let id = id.trim();
                if !PLUGINS.iter().any(|(plugin, _)| *plugin == id) {
                    continue;
                }
                if !plugins.iter().any(|plugin| plugin == id) {
                    plugins.push(id.to_string());
                }
            }
            settings.plugins = plugins;
        }
        Ok(settings)
    }

    /// Read the settings from a file. A missing, unreadable or broken file means the defaults.
    pub fn load(path: &Path) -> ImeSettings {
        ImeSettings::load_checked(path).unwrap_or_default()
    }

    /// Read the settings from a file and say what went wrong, if anything.
    pub fn load_checked(path: &Path) -> Result<ImeSettings, String> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        ImeSettings::from_text(&text)
    }

    /// Write the settings to a file, atomically: a temporary file next to it and a rename over it. The rename is
    /// atomic on Windows, so a crash while saving leaves the old file readable.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, self.to_text().as_bytes())?;
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                // The rename failed: do not leave the temporary file behind.
                let _ = fs::remove_file(&temporary);
                Err(e)
            }
        }
    }
}

/// The settings of one language out of a JSON object. A missing or malformed field falls back to its default.
fn language_settings_of(value: &Json) -> LanguageSettings {
    let default = LanguageSettings::default();
    let page_size = value
        .get("pageSize")
        .and_then(Json::as_f64)
        .map(|n| n.clamp(1.0, 9.0) as usize)
        .unwrap_or(default.page_size);
    let mut settings = LanguageSettings {
        page_size,
        space_confirms: default.space_confirms,
        tab_cycles: default.tab_cycles,
        tab_to_host: default.tab_to_host,
        punct_paging: default.punct_paging,
        keymap_preset: default.keymap_preset,
        auto_compose: default.auto_compose,
        input_mode: default.input_mode,
        laser_ui: default.laser_ui,
    };
    // The scheme of the keys comes first: a file that only carries the scheme gets the switches of the scheme, and
    // the switches that are written in the file win over it (a file may have been edited by hand).
    if let Some(preset) = value.get("keymapPreset").and_then(Json::as_str)
        && let Some(preset) = KeymapPreset::from_id(preset)
    {
        settings.apply_preset(preset);
    }
    if let Some(flag) = value.get("spaceConfirms").and_then(Json::as_bool) {
        settings.space_confirms = flag;
    }
    if let Some(flag) = value.get("tabCycles").and_then(Json::as_bool) {
        settings.tab_cycles = flag;
    }
    if let Some(flag) = value.get("tabToHost").and_then(Json::as_bool) {
        settings.tab_to_host = flag;
    }
    if let Some(flag) = value.get("punctPaging").and_then(Json::as_bool) {
        settings.punct_paging = flag;
    }
    if let Some(flag) = value.get("autoCompose").and_then(Json::as_bool) {
        settings.auto_compose = flag;
    }
    if let Some(flag) = value.get("inputMode").and_then(Json::as_bool) {
        settings.input_mode = flag;
    }
    // The old name of the setting (it showed the floating bar) is read as well: the file of the user may still
    // have it, and the meaning is the same one - the floating interface of the product is shown.
    settings.laser_ui = value
        .get("laserUi")
        .or_else(|| value.get("floatingBar"))
        .and_then(Json::as_bool)
        .unwrap_or(default.laser_ui);
    settings
}

/// The file the settings live in, inside the user's local application data. `None` when the environment has no
/// `LOCALAPPDATA` (the settings then live only for the session).
pub fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join(APP_DIR).join(FILE_NAME))
}

/// The settings of every language as a map, for the parts that want to look at several languages at once.
pub fn by_language(settings: &ImeSettings) -> BTreeMap<String, LanguageSettings> {
    settings.languages.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("pime-imesettings-{}-{}", std::process::id(), name));
        path
    }

    #[test]
    fn the_three_profiles_are_known_in_both_directions() {
        assert_eq!(LANGUAGES.len(), 3);
        for language in LANGUAGES {
            assert_eq!(language_for_id(language_id(language)), Some(language));
            assert_ne!(display_name(language), "未知");
        }
        assert_eq!(language_for_id(0x0409), Some("en-US"));
        assert_eq!(language_for_id(0x1234), None);
        assert_eq!(language_id("de-DE"), 0);
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let settings = ImeSettings::default();
        assert_eq!(settings.languages.len(), 3);
        assert_eq!(settings.effort, "auto");
        assert_eq!(settings.skin, "laser");
        for language in LANGUAGES {
            let value = settings.get(language);
            assert_eq!(value.page_size, 5);
            assert!(
                value.space_confirms,
                "Space takes the highlighted candidate while the list is on screen"
            );
            assert!(
                value.tab_cycles,
                "Tab moves the highlight of the list (the programmer habit the user asked for)"
            );
            assert!(
                !value.tab_to_host,
                "the Tab key belongs to the candidate list, not to the IDE"
            );
            assert!(!value.punct_paging, "the punctuation is code, not a pager");
            assert_eq!(value.keymap_preset, KeymapPreset::Programmer);
            assert!(!value.auto_compose);
            assert!(
                value.input_mode,
                "the input method starts in input mode (the mainstream behaviour)"
            );
            assert!(value.laser_ui);
        }
    }

    #[test]
    fn the_schemes_of_the_keys_are_the_ones_the_user_asked_for() {
        // the user asked for the settings to offer the ways of other input methods ("包括不限于微软，日语的Atok等")
        for preset in [
            KeymapPreset::Microsoft,
            KeymapPreset::Sogou,
            KeymapPreset::Atok,
            KeymapPreset::Programmer,
            KeymapPreset::Custom,
        ] {
            assert!(KeymapPreset::ALL.contains(&preset), "{preset:?}");
            assert!(!preset.label().is_empty(), "{preset:?}");
            assert_eq!(KeymapPreset::from_id(preset.id()), Some(preset));
        }
        assert_eq!(KeymapPreset::from_id("nonsense"), None);
        let mut settings = LanguageSettings::default();
        // the programmer habit: Tab selects (it moves the highlight)
        assert!(settings.apply_preset(KeymapPreset::Programmer));
        assert!(settings.tab_cycles && !settings.tab_to_host);
        // the Microsoft way: Tab takes the highlighted candidate
        assert!(settings.apply_preset(KeymapPreset::Microsoft));
        assert!(!settings.tab_cycles && !settings.tab_to_host);
        // the Sogou way: the composition opens by itself and the punctuation pages the list
        assert!(settings.apply_preset(KeymapPreset::Sogou));
        assert!(settings.auto_compose && settings.punct_paging);
        // ATOK: the punctuation pages the list, the composition does not open by itself
        assert!(settings.apply_preset(KeymapPreset::Atok));
        assert!(settings.punct_paging && !settings.auto_compose);
        // "custom" keeps the switches of the user and only renames the scheme
        let before = settings;
        assert!(!settings.apply_preset(KeymapPreset::Custom));
        assert_eq!(settings.tab_cycles, before.tab_cycles);
        assert_eq!(settings.tab_to_host, before.tab_to_host);
        assert_eq!(settings.keymap_preset, KeymapPreset::Custom);
    }

    #[test]
    fn a_file_with_only_a_scheme_gets_the_switches_of_the_scheme() {
        let text = r#"{"languages":{"zh-CN":{"keymapPreset":"microsoft"}}}"#;
        let read = ImeSettings::from_text(text).expect("the file reads");
        let value = read.get("zh-CN");
        assert_eq!(value.keymap_preset, KeymapPreset::Microsoft);
        assert!(
            !value.tab_cycles,
            "the Microsoft way: Tab takes the highlighted candidate"
        );
        // a file that writes the switches itself is the last word
        let text = r#"{"languages":{"zh-CN":{"keymapPreset":"microsoft","tabCycles":true}}}"#;
        let read = ImeSettings::from_text(text).expect("the file reads");
        assert!(read.get("zh-CN").tab_cycles, "the file wins over the scheme");
    }

    #[test]
    fn the_effort_and_the_skin_are_global_and_survive_the_file() {
        let settings = ImeSettings {
            effort: "high".to_string(),
            skin: "dark".to_string(),
            ..ImeSettings::default()
        };
        let text = settings.to_text();
        let read = ImeSettings::from_text(&text).expect("the file reads back");
        assert_eq!(read.effort, "high");
        assert_eq!(read.skin, "dark");
        // an unknown value is the default, not an error
        let read = ImeSettings::from_text(r#"{"effort":"insane","skin":"neon"}"#)
            .expect("a file without languages still loads");
        assert_eq!(read.effort, "auto");
        assert_eq!(read.skin, "laser");
    }

    #[test]
    fn a_set_and_get_round_trip() {
        let mut settings = ImeSettings::default();
        let changed = LanguageSettings {
            page_size: 9,
            space_confirms: true,
            tab_cycles: false,
            tab_to_host: true,
            punct_paging: true,
            keymap_preset: KeymapPreset::Microsoft,
            auto_compose: true,
            input_mode: true,
            laser_ui: false,
        };
        settings.set("ja-JP", changed);
        assert_eq!(settings.get("ja-JP"), changed);
        assert_eq!(settings.get("zh-CN"), LanguageSettings::default());
        assert_eq!(settings.get("de-DE"), LanguageSettings::default());
    }

    #[test]
    fn the_file_round_trips_through_text() {
        let mut settings = ImeSettings::default();
        settings.set(
            "zh-CN",
            LanguageSettings {
                page_size: 7,
                space_confirms: true,
                ..LanguageSettings::default()
            },
        );
        let text = settings.to_text();
        let read = ImeSettings::from_text(&text).expect("the text it just wrote must read back");
        assert_eq!(read, settings);
    }

    #[test]
    fn a_broken_file_does_not_take_the_input_method_down() {
        assert!(ImeSettings::from_text("not json at all").is_err());
        assert!(
            ImeSettings::from_text(r#"{"languages":[]}"#).is_err(),
            "the languages section must be an object"
        );
        // a file without any language still loads (the defaults)
        let read = ImeSettings::from_text("{}").expect("an empty file loads");
        assert_eq!(read, ImeSettings::default());
        // a file with unknown languages and unknown fields still loads
        let text = r#"{"version":9,"languages":{"de-DE":{"pageSize":3},"zh-CN":{"pageSize":8,"unknown":1}}}"#;
        let read = ImeSettings::from_text(text).expect("a newer file still loads");
        assert_eq!(read.get("zh-CN").page_size, 8);
        assert!(
            read.get("zh-CN").tab_cycles,
            "a missing field is the default (Tab selects)"
        );
        assert_eq!(read.get("en-US"), LanguageSettings::default());
    }

    #[test]
    fn a_value_out_of_range_falls_back_to_the_default() {
        let text = r#"{"languages":{"zh-CN":{"pageSize":99,"tabCycles":"yes"}}}"#;
        let read = ImeSettings::from_text(text).expect("a bad value does not break the file");
        assert_eq!(read.get("zh-CN").page_size, 9, "clamped into the range");
        assert!(
            read.get("zh-CN").tab_cycles,
            "a value of the wrong type is the default (Tab selects)"
        );
        let text = r#"{"languages":{"zh-CN":{"pageSize":0}}}"#;
        let read = ImeSettings::from_text(text).unwrap();
        assert_eq!(read.get("zh-CN").page_size, 1);
    }

    #[test]
    fn saving_and_loading_a_file_keeps_the_settings() {
        let path = temp_file("roundtrip.json");
        let _ = fs::remove_file(&path);
        let mut settings = ImeSettings::default();
        settings.set(
            "en-US",
            LanguageSettings {
                page_size: 3,
                ..Default::default()
            },
        );
        settings.save(&path).expect("the settings must be writable");
        let read = ImeSettings::load(&path);
        assert_eq!(read, settings);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_means_the_defaults() {
        let path = temp_file("missing.json");
        let _ = fs::remove_file(&path);
        assert_eq!(ImeSettings::load(&path), ImeSettings::default());
        assert!(ImeSettings::load_checked(&path).is_err());
    }

    #[test]
    fn saving_twice_leaves_no_temporary_file_behind() {
        let path = temp_file("atomic.json");
        let _ = fs::remove_file(&path);
        let settings = ImeSettings::default();
        settings.save(&path).unwrap();
        settings.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        assert!(path.exists());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_settings_file_lives_in_the_local_application_data() {
        let path = settings_path().expect("a Windows session has LOCALAPPDATA");
        assert!(
            path.parent().is_some_and(|p| p.ends_with(APP_DIR)),
            "{path:?}"
        );
        assert!(path.to_string_lossy().contains(FILE_NAME));
    }

    #[test]
    fn the_product_ships_with_plsql_on_and_tsql_off() {
        let settings = ImeSettings::default();
        assert!(settings.plugin_is_on("plsql"), "PL/SQL is on out of the box");
        assert!(settings.plugin_is_on("cypher"), "Cypher is on out of the box");
        assert!(
            !settings.plugin_is_on("tsql"),
            "T-SQL is off out of the box: it shares the .sql extension with PL/SQL"
        );
    }

    #[test]
    fn the_plugins_survive_the_round_trip_through_the_file() {
        let mut settings = ImeSettings::default();
        settings.set_plugin("tsql", true);
        settings.set_plugin("plsql", false);
        let text = settings.to_text();
        let read = ImeSettings::from_text(&text).expect("the file is readable");
        assert!(read.plugin_is_on("tsql"), "T-SQL was turned on");
        assert!(!read.plugin_is_on("plsql"), "PL/SQL was turned off");
        assert!(read.plugin_is_on("cypher"), "the plugins that were not touched stay");
    }

    #[test]
    fn a_plugin_the_product_does_not_know_is_ignored() {
        let mut settings = ImeSettings::default();
        settings.set_plugin("cobol", true);
        assert!(!settings.plugin_is_on("cobol"));
    }

    #[test]
    fn the_list_of_the_plugins_is_the_one_of_the_core() {
        let core: Vec<&str> = pime_core::pack::PLUGIN_LANGUAGES.to_vec();
        let ours: Vec<&str> = PLUGINS.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ours, core,
            "the settings window has to know every plugin the core has"
        );
    }

}
