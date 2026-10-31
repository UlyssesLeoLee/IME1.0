//! Interface texts (FR-044), port of `core/i18n.js`: the interface language is independent of the Windows input
//! language and of the programming profile, and code text is never translated.
//!
//! The rules are those of the reference: a text is looked up by key; a missing key answers the key itself; a missing
//! or empty translation falls back to the English column; `{0}`, `{1}`, ... are replaced in ONE pass by the arguments
//! (a missing argument becomes the empty string, and a replaced value is never scanned again). The tables are in
//! `i18n_table.rs`.
//!
//! One deliberate difference: the reference looks keys up in a plain object, so a key that is the name of an
//! `Object.prototype` member (`constructor`, `toString`, ...) makes it throw a `TypeError`. Here such a key is simply
//! unknown and answers itself.

use crate::i18n_table::{ERR, S};
use crate::json::Json;
use crate::jsval::{js_order, to_js_string};

/// The interface locales, in the order of the reference's `LOCALES`.
pub const LOCALES: [&str; 3] = ["zh-CN", "en-US", "ja-JP"];

/// The column of a locale in the tables (`[en-US, zh-CN, ja-JP]`); every other locale reads English.
fn column(locale: &str) -> usize {
    match locale {
        "zh-CN" => 1,
        "ja-JP" => 2,
        _ => 0,
    }
}

fn pick(row: &[&'static str; 3], locale: &str) -> &'static str {
    let text = row[column(locale)];
    if text.is_empty() { row[0] } else { text }
}

fn lookup(
    table: &'static [(&'static str, [&'static str; 3])],
    key: &str,
) -> Option<&'static [&'static str; 3]> {
    table.iter().find(|(k, _)| *k == key).map(|(_, row)| row)
}

/// `resolveLocale(setting, vscodeLanguage)`: an explicit interface locale wins; otherwise the editor language decides
/// (`zh*` -> zh-CN, `ja*` -> ja-JP, anything else en-US). `language` is the value as the reference stringifies it.
pub fn resolve_locale(setting: &str, language: &str) -> &'static str {
    if let Some(known) = LOCALES.iter().find(|l| **l == setting) {
        return known;
    }
    let lower = language.to_lowercase();
    if lower.starts_with("zh") {
        "zh-CN"
    } else if lower.starts_with("ja") {
        "ja-JP"
    } else {
        "en-US"
    }
}

/// `resolveLocale` for values as they arrive in a request: a setting that is not a string is never a locale, and a
/// language that is not given is `en` (the reference's default); any other value reads as `String(value)`.
pub fn resolve_locale_of(setting: Option<&Json>, language: Option<&Json>) -> &'static str {
    let setting = setting.and_then(Json::as_str).unwrap_or("");
    let language = language.map_or_else(|| "en".to_string(), to_js_string);
    resolve_locale(setting, &language)
}

/// A replacement argument as the reference's `replace` callback sees it: `null` is the empty string (`args[n] ?? ''`),
/// everything else is `String(value)`.
pub fn arg_text(v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        other => to_js_string(other),
    }
}

/// Replaces `{n}` (ASCII digits) by `args[n]` in one pass; an argument that is missing counts as empty.
fn fill(template: &str, args: &[String]) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start && end < bytes.len() && bytes[end] == b'}' {
                let n = bytes[start..end].iter().fold(0usize, |acc, d| {
                    acc.saturating_mul(10).saturating_add(usize::from(d - b'0'))
                });
                if let Some(arg) = args.get(n) {
                    out.push_str(arg);
                }
                i = end + 1;
                continue;
            }
        }
        let ch = template[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `t(locale, key, ...args)`.
pub fn text(locale: &str, key: &str, args: &[String]) -> String {
    match lookup(S, key) {
        Some(row) => fill(pick(row, locale), args),
        None => fill(key, args),
    }
}

/// The unfilled template of a key (`{0}` kept), or `None` for an unknown key.
pub fn template(locale: &str, key: &str) -> Option<&'static str> {
    lookup(S, key).map(|row| pick(row, locale))
}

/// `describeError(locale, code, fallbackMessage)`: the sentence of a stable error code, else the fallback message,
/// else the code itself.
pub fn describe_error(locale: &str, code: &str, fallback: &str) -> String {
    match lookup(ERR, code) {
        Some(row) => pick(row, locale).to_string(),
        None if !fallback.is_empty() => fallback.to_string(),
        None => code.to_string(),
    }
}

/// Every interface key, in the order of the reference table.
pub fn keys() -> impl Iterator<Item = &'static str> {
    S.iter().map(|(k, _)| *k)
}

/// Every error code that has a sentence, in the order of the reference table.
pub fn error_codes() -> impl Iterator<Item = &'static str> {
    ERR.iter().map(|(k, _)| *k)
}

/// The sentence of every error code for one locale (data for a cache in the bridge).
pub fn error_table(locale: &str) -> Vec<(String, String)> {
    ERR.iter()
        .map(|(k, row)| ((*k).to_string(), pick(row, locale).to_string()))
        .collect()
}

/// `bundle(locale, keys)`: the texts of the listed keys as a dictionary, each rendered WITHOUT arguments (so `{n}`
/// placeholders become empty, as in the reference); duplicate keys collapse and integer-like keys sort first.
/// `raw` keeps the `{n}` placeholders instead: the template table a bridge can cache and fill itself.
pub fn bundle(locale: &str, keys: &[String], raw: bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for key in keys {
        let value = if raw {
            template(locale, key).map_or_else(|| key.clone(), str::to_string)
        } else {
            text(locale, key, &[])
        };
        match out.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => out.push((key.clone(), value)),
        }
    }
    js_order(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn placeholders(text: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'{' {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 && j < b.len() && b[j] == b'}' {
                    found.push(text[i + 1..j].to_string());
                    i = j;
                }
            }
            i += 1;
        }
        found.sort();
        found
    }

    #[test]
    fn ut_056_the_interface_locale_is_independent_of_the_editor_language() {
        assert_eq!(resolve_locale("zh-CN", "ja"), "zh-CN");
        assert_eq!(resolve_locale("auto", "ja-JP"), "ja-JP");
        assert_eq!(resolve_locale("auto", "zh-cn"), "zh-CN");
        assert_eq!(resolve_locale("auto", "fr"), "en-US");
        assert_eq!(resolve_locale("", "en"), "en-US");
        assert_eq!(resolve_locale("en-US", "zh-CN"), "en-US");
        assert_eq!(
            resolve_locale("ZH-CN", "ja"),
            "ja-JP",
            "the setting is case sensitive"
        );
        assert_eq!(resolve_locale("auto", "ZH-HANS"), "zh-CN");
    }

    #[test]
    fn ut_044_texts_are_translated_per_locale_and_arguments_fill_in_one_pass() {
        assert!(text("zh-CN", "f1.tab.full", &[]).contains("整文件"));
        assert!(text("ja-JP", "f1.tab.full", &[]).contains("ファイル全体"));
        assert!(text("en-US", "f1.tab.full", &[]).contains("Whole file"));
        assert_eq!(
            text("en-US", "f1.hunk", &args(&["2", "7", "1", "3"])),
            "Block 2 (old lines 7, 1 removed, 3 added)"
        );
        assert_eq!(
            text("fr-FR", "status.off", &[]),
            "OFF",
            "another locale reads English"
        );
        assert_eq!(text("en-US", "does.not.exist", &[]), "does.not.exist");
        // an unknown key is still filled; a missing argument is empty; a value is never scanned again
        assert_eq!(
            text("en-US", "x {0} y {1} z {2}", &args(&["{1}", "b"])),
            "x {1} y b z "
        );
        assert_eq!(
            text("en-US", "{01}|{ 1}|{1 }|{}|{-1}|{1", &args(&["a", "b"])),
            "b|{ 1}|{1 }|{}|{-1}|{1"
        );
        assert_eq!(text("en-US", "{{0}}", &args(&["v"])), "{v}");
        assert_eq!(
            text("en-US", "{99999999999999999999999}", &args(&["a"])),
            ""
        );
        assert_eq!(text("en-US", "$& $1 {0}", &args(&["$&"])), "$& $1 $&");
    }

    #[test]
    fn errors_describe_themselves_and_unknown_codes_fall_back() {
        assert_eq!(
            describe_error("zh-CN", "NOPE", "raw message"),
            "raw message"
        );
        assert_eq!(describe_error("zh-CN", "NOPE", ""), "NOPE");
        assert_eq!(
            describe_error("en-US", "STALE_DOCUMENT", "ignored"),
            "The document changed; nothing was written."
        );
        assert_eq!(
            describe_error("ja-JP", "AI_DISABLED", ""),
            "AI は無効です。"
        );
    }

    #[test]
    fn every_key_has_every_locale_and_the_same_placeholders_everywhere() {
        let mut seen = std::collections::HashSet::new();
        for (key, row) in S.iter().chain(ERR.iter()) {
            for (i, t) in row.iter().enumerate() {
                assert!(!t.is_empty(), "{key} is missing in column {i}");
            }
            assert!(
                seen.insert(*key) || ERR.iter().any(|(k, _)| k == key),
                "duplicate key {key}"
            );
            let want = placeholders(row[0]);
            for t in &row[1..] {
                assert_eq!(
                    placeholders(t),
                    want,
                    "placeholders of {key} differ between locales"
                );
            }
        }
        for loc in LOCALES {
            assert_eq!(keys().count(), 158);
            assert_eq!(error_codes().count(), error_table(loc).len());
        }
    }

    #[test]
    fn bundles_render_without_arguments_unless_raw() {
        let keys = args(&["f1.hunk", "status.off", "10", "status.off", "nope", "2"]);
        let b = bundle("en-US", &keys, false);
        assert_eq!(
            b.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["2", "10", "f1.hunk", "status.off", "nope"]
        );
        assert_eq!(b[2].1, "Block  (old lines ,  removed,  added)");
        let raw = bundle("en-US", &keys, true);
        assert_eq!(
            raw[2].1,
            "Block {0} (old lines {1}, {2} removed, {3} added)"
        );
        assert_eq!(raw[0].1, "2", "an unknown key answers itself");
    }
}
