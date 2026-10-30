//! Heuristic symbol index (names only), port of `symbols.js`: read-only graph context for the Cypher annotation.
//! It is not a parser: it recognises common top-level declarations per language so that the AI can refer to symbols
//! that really exist. Only `{kind, name}` pairs ever leave the machine, never the code of other files.
//!
//! The declaration patterns are the JavaScript sources of the reference engine, run by the crate's own matcher
//! (`regex.rs`), so every line is matched exactly like `RegExp.exec` does.

use crate::pack::LANGUAGES;
use crate::regex::{Regex, group};
use crate::secrets::is_sensitive_file_name;
use crate::util::{chars_of, head_utf16, split_lines, string_of};
use crate::validators::{Style, mask_chars};
use std::sync::OnceLock;

/// Only the first 200 KiB (UTF-16 units) of a file are scanned.
pub const MAX_SCAN: usize = 200 * 1024;
/// Default and largest number of symbols one call returns.
pub const DEFAULT_MAX: usize = 60;
pub const LIMIT_MAX: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub kind: String,
    pub name: String,
}

/// How a rule names the symbol: the kind is capture group 1 or a fixed word; the name is the given group.
enum Kind {
    Group,
    Fixed(&'static str),
}

struct Rule {
    re: Regex,
    kind: Kind,
    name_group: usize,
}

const NAME: &str = r"([A-Za-z_][\w$]*)";

fn rule(pattern: &str, icase: bool, kind: Kind, name_group: usize) -> Rule {
    Rule {
        re: Regex::new(pattern, icase),
        kind,
        name_group,
    }
}

fn rules_for(profile: &str) -> &'static [Rule] {
    static RULES: OnceLock<Vec<(&'static str, Vec<Rule>)>> = OnceLock::new();
    let all = RULES.get_or_init(|| {
        vec![
            (
                "rust",
                vec![
                    rule(
                        &format!(
                            r#"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+|extern\s+"[^"]*"\s+)*(fn|struct|enum|trait|mod|type|static)\s+{NAME}"#
                        ),
                        false,
                        Kind::Group,
                        2,
                    ),
                    rule(
                        &format!(r"^\s*impl(?:<[^>]*>)?\s+(?:[\w:<>,\s&']+?\s+for\s+)?{NAME}"),
                        false,
                        Kind::Fixed("impl"),
                        1,
                    ),
                ],
            ),
            (
                "python",
                vec![rule(
                    &format!(r"^\s*(?:async\s+)?(def|class)\s+{NAME}"),
                    false,
                    Kind::Group,
                    2,
                )],
            ),
            (
                "typescript",
                vec![
                    rule(
                        &format!(
                            r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?(function|class|interface|enum|type|namespace)\s+{NAME}"
                        ),
                        false,
                        Kind::Group,
                        2,
                    ),
                    rule(
                        &format!(
                            r"^\s*(?:export\s+)?const\s+{NAME}\s*(?::[^=]+)?=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_]\w*)\s*(?::[^=]+)?=>"
                        ),
                        false,
                        Kind::Fixed("function"),
                        1,
                    ),
                ],
            ),
            (
                "powershell",
                vec![rule(
                    r"^\s*(?:filter|function)\s+([\w-]+)",
                    true,
                    Kind::Fixed("function"),
                    1,
                )],
            ),
            (
                "c",
                vec![
                    rule(
                        &format!(r"^\s*(?:typedef\s+)?(struct|enum|union)\s+{NAME}"),
                        false,
                        Kind::Group,
                        2,
                    ),
                    rule(
                        &format!(
                            r"^(?!\s*(?:if|for|while|switch|return|else|do|case|sizeof)\b)[A-Za-z_][\w\s\*]*?[\s\*]{NAME}\s*\([^;]*\)\s*\{{?\s*$"
                        ),
                        false,
                        Kind::Fixed("function"),
                        1,
                    ),
                    rule(
                        &format!(r"^\s*#\s*define\s+{NAME}"),
                        false,
                        Kind::Fixed("macro"),
                        1,
                    ),
                ],
            ),
            (
                "assembly",
                vec![rule(
                    r"^\s*([A-Za-z_.$][\w.$]*):",
                    false,
                    Kind::Fixed("label"),
                    1,
                )],
            ),
        ]
    });
    all.iter()
        .find(|(p, _)| *p == profile)
        .map_or(&[][..], |(_, r)| r.as_slice())
}

pub(crate) fn style_of(profile: &str) -> Option<Style> {
    match profile {
        "rust" => Some(Style::Rust),
        "c" => Some(Style::C),
        "typescript" => Some(Style::TypeScript),
        "python" => Some(Style::Python),
        "powershell" => Some(Style::PowerShell),
        _ => None,
    }
}

/// Declarations in file order, de-duplicated by `kind:name`, at most `max` entries.
pub fn extract_symbols(profile: &str, text: &str, max: usize) -> Vec<Symbol> {
    let rules = rules_for(profile);
    if rules.is_empty() {
        return Vec::new();
    }
    let sample = head_utf16(text, MAX_SCAN);
    // comments and string literals must not produce symbols
    let code = match style_of(profile) {
        Some(style) => string_of(&mask_chars(&chars_of(sample), style)),
        None => sample.to_string(),
    };
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    for line in split_lines(&code) {
        let t = chars_of(line);
        for r in rules {
            let Some(caps) = r.re.exec(&t) else { continue };
            let kind = match r.kind {
                Kind::Group => group(&t, &caps, 1).unwrap_or_default(),
                Kind::Fixed(k) => k.to_string(),
            };
            let name = group(&t, &caps, r.name_group).unwrap_or_default();
            if !seen.iter().any(|(k, n)| *k == kind && *n == name) {
                seen.push((kind.clone(), name.clone()));
                out.push(Symbol { kind, name });
            }
            break;
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

/// One entry of a directory listing.
pub struct DirEntry<'a> {
    pub name: &'a str,
    pub is_file: bool,
}

/// The files of a directory whose symbols are worth reading (the pure decisions of the directory scan): regular
/// files other than the current one, not credential-like by name, whose language is recognised by the extension.
/// Returns `(name, language)` in listing order; reading the files stays with the caller.
pub fn directory_candidates<'a>(
    current_name: &str,
    entries: &[DirEntry<'a>],
) -> Vec<(&'a str, String)> {
    let registry = crate::predict::engine().registry();
    let mut out = Vec::new();
    for e in entries {
        if !e.is_file || e.name == current_name || is_sensitive_file_name(e.name) {
            continue;
        }
        let ctx = crate::resolve::ResolveContext {
            language_id: Some(""),
            file_name: Some(e.name),
            text: None,
        };
        let decision = crate::resolve::resolve_language(registry, &ctx, "unknown", None);
        if (decision.reason == "extension" || decision.reason == "bridge-languageId")
            && LANGUAGES.contains(&decision.language.as_str())
        {
            out.push((e.name, decision.language));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(profile: &str, text: &str) -> Vec<String> {
        extract_symbols(profile, text, DEFAULT_MAX)
            .into_iter()
            .map(|s| format!("{}:{}", s.kind, s.name))
            .collect()
    }

    #[test]
    fn ann_07_rust_symbols_ignore_comments_and_strings() {
        let rs = pairs(
            "rust",
            "pub struct Point;\n// fn hidden() {}\nlet s = \"fn fake()\";\npub(crate) async fn run() {}\nimpl<T> Trait for Point {}\nmod util;\nenum E {}\nconst X: u8 = 1;\n",
        );
        assert_eq!(
            rs,
            ["struct:Point", "fn:run", "impl:Point", "mod:util", "enum:E"]
        );
    }

    #[test]
    fn ann_07_every_language_has_its_declarations() {
        assert_eq!(
            pairs(
                "python",
                "class A:\n    def m(self): pass\nasync def go(): pass\n# def hidden()\n"
            ),
            ["class:A", "def:m", "def:go"]
        );
        assert_eq!(
            pairs(
                "typescript",
                "export interface I {}\nexport const f = (x: number) => x;\nexport default class C {}\ntype T = string;\n"
            ),
            ["interface:I", "function:f", "class:C", "type:T"]
        );
        assert_eq!(
            pairs(
                "powershell",
                "function Get-Thing {\n}\nFilter Only-Odd {}\n"
            ),
            ["function:Get-Thing", "function:Only-Odd"]
        );
        assert_eq!(
            pairs(
                "c",
                "#define MAX 3\nstruct node { int v; };\nint add(int a, int b)\n{\n    if (a) return b;\n}\n"
            ),
            ["macro:MAX", "struct:node", "function:add"]
        );
        assert_eq!(
            pairs("assembly", "_start:\n    mov rax, 1 ; x:\nloop_a:\n"),
            ["label:_start", "label:loop_a"]
        );
        assert!(pairs("machine-code", "48 89 D8").is_empty());
        assert!(pairs("unknown", "fn a() {}").is_empty());
    }

    #[test]
    fn the_result_is_capped_and_deduplicated() {
        let many: String = (0..200).map(|i| format!("fn f{i}() {{}}\n")).collect();
        assert_eq!(extract_symbols("rust", &many, DEFAULT_MAX).len(), 60);
        assert_eq!(extract_symbols("rust", &many, 5).len(), 5);
        assert_eq!(
            pairs("rust", "fn a() {}\nfn a() {}\nstruct a;\n"),
            ["fn:a", "struct:a"]
        );
    }

    #[test]
    fn only_the_first_200_kib_are_scanned() {
        let text = format!("{}fn late() {{}}\n", "// pad\n".repeat(40_000));
        assert!(pairs("rust", &text).is_empty());
    }

    #[test]
    fn directory_candidates_skip_self_credentials_and_unknown_extensions() {
        let entries = [
            ("a.rs", true),
            ("b.py", true),
            (".env", true),
            ("notes.txt", true),
            ("sub", false),
            ("c.ts", true),
            ("id_rsa", true),
            ("secret.key", true),
        ]
        .map(|(name, is_file)| DirEntry { name, is_file });
        let got = directory_candidates("a.rs", &entries);
        assert_eq!(
            got,
            [
                ("b.py", "python".to_string()),
                ("c.ts", "typescript".to_string())
            ]
        );
    }
}
