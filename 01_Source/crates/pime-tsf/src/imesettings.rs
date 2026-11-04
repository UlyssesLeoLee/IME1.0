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
pub const LANGUAGES: [&str; 3] = ["zh-CN", "en-US", "ja-JP"];

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
/// character), the candidate page holds five rows, the Tab key confirms the highlighted candidate (the mainstream
/// behaviour) and the space key is a space (in code it usually is).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageSettings {
    /// Candidates per page (1..=9). The digits 1..=page_size pick within the page.
    pub page_size: usize,
    /// Space confirms the highlighted candidate. Off by default: in code a space is a space.
    pub space_confirms: bool,
    /// Tab walks the candidate list instead of taking the highlighted candidate. Off by default: Tab confirms the
    /// highlighted candidate, like the mainstream input methods.
    pub tab_cycles: bool,
    /// The Tab key belongs to the host even while the candidate list is on screen (the IDE keeps its Tab, the list
    /// is a preview only). Off by default: Tab confirms the highlighted candidate.
    pub tab_to_host: bool,
    /// A composition is opened as soon as candidates appear (the Sogou behaviour).
    pub auto_compose: bool,
    /// The input method starts in input mode (typing opens a composition) instead of direct mode.
    pub input_mode: bool,
    /// The floating bar is shown while this profile is active.
    pub floating_bar: bool,
}

impl Default for LanguageSettings {
    fn default() -> LanguageSettings {
        LanguageSettings {
            page_size: 5,
            // The mainstream behaviour: while the candidate list is on screen, Space takes the highlighted
            // candidate. Direct mode (Alt+F1) still lets every character through to the host.
            space_confirms: true,
            tab_cycles: false,
            tab_to_host: false,
            auto_compose: false,
            // The input method starts in input mode (the mainstream behaviour), not in direct mode.
            input_mode: true,
            floating_bar: true,
        }
    }
}

impl LanguageSettings {
    /// The page size, clamped into the range the digits can address.
    pub fn page_size(&self) -> usize {
        self.page_size.clamp(1, 9)
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
                        ("autoCompose".to_string(), Json::Bool(value.auto_compose)),
                        ("inputMode".to_string(), Json::Bool(value.input_mode)),
                        ("floatingBar".to_string(), Json::Bool(value.floating_bar)),
                    ]),
                )
            })
            .collect();
        Json::Obj(vec![
            ("version".to_string(), Json::Num(VERSION as f64)),
            ("effort".to_string(), Json::Str(self.effort.clone())),
            ("skin".to_string(), Json::Str(self.skin.clone())),
            ("mouthPlace".to_string(), Json::Str(self.mouth_place.clone())),
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
    LanguageSettings {
        page_size,
        space_confirms: value
            .get("spaceConfirms")
            .and_then(Json::as_bool)
            .unwrap_or(default.space_confirms),
        tab_cycles: value
            .get("tabCycles")
            .and_then(Json::as_bool)
            .unwrap_or(default.tab_cycles),
        tab_to_host: value
            .get("tabToHost")
            .and_then(Json::as_bool)
            .unwrap_or(default.tab_to_host),
        auto_compose: value
            .get("autoCompose")
            .and_then(Json::as_bool)
            .unwrap_or(default.auto_compose),
        input_mode: value
            .get("inputMode")
            .and_then(Json::as_bool)
            .unwrap_or(default.input_mode),
        floating_bar: value
            .get("floatingBar")
            .and_then(Json::as_bool)
            .unwrap_or(default.floating_bar),
    }
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
                !value.tab_cycles,
                "Tab confirms the highlighted candidate (the mainstream behaviour)"
            );
            assert!(
                !value.tab_to_host,
                "the Tab key belongs to the candidate list, not to the IDE"
            );
            assert!(!value.auto_compose);
            assert!(
                value.input_mode,
                "the input method starts in input mode (the mainstream behaviour)"
            );
            assert!(value.floating_bar);
        }
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
            auto_compose: true,
            input_mode: true,
            floating_bar: false,
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
            !read.get("zh-CN").tab_cycles,
            "a missing field is the default"
        );
        assert_eq!(read.get("en-US"), LanguageSettings::default());
    }

    #[test]
    fn a_value_out_of_range_falls_back_to_the_default() {
        let text = r#"{"languages":{"zh-CN":{"pageSize":99,"tabCycles":"yes"}}}"#;
        let read = ImeSettings::from_text(text).expect("a bad value does not break the file");
        assert_eq!(read.get("zh-CN").page_size, 9, "clamped into the range");
        assert!(
            !read.get("zh-CN").tab_cycles,
            "a value of the wrong type is the default"
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
}
