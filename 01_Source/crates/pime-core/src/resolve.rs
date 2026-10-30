//! Language resolution (FR-010/011, DD section 5.3): port of `resolveLanguage` / `lexicalGuess`.
//! A manual lock always wins; weak evidence never switches the language automatically.

use crate::pack::{Registry, LANGUAGES};
use crate::util::head_utf16;

const STRONG_EXT: [(&str, &str); 11] = [
    (".rs", "rust"),
    (".py", "python"),
    (".pyw", "python"),
    (".ts", "typescript"),
    (".tsx", "typescript"),
    (".mts", "typescript"),
    (".cts", "typescript"),
    (".ps1", "powershell"),
    (".psm1", "powershell"),
    (".psd1", "powershell"),
    (".c", "c"),
];

const AMBIGUOUS_EXT: [(&str, &str); 6] = [
    (".h", "c"),
    (".asm", "assembly"),
    (".s", "assembly"),
    (".nasm", "assembly"),
    (".bin", "machine-code"),
    (".hex", "machine-code"),
];

const LEXICAL: [(&str, &[&str]); 6] = [
    (
        "rust",
        &["fn main()", "impl ", "let mut ", "println!", "use std::"],
    ),
    ("python", &["def ", "import ", "self.", "elif ", "__name__"]),
    (
        "typescript",
        &[
            "interface ",
            "export const ",
            ": string",
            "Promise<",
            "=> {",
        ],
    ),
    (
        "powershell",
        &[
            "Get-",
            "Write-Host",
            "Where-Object",
            "$PSVersionTable",
            "param(",
        ],
    ),
    ("c", &["#include", "printf(", "typedef struct", "int main("]),
    (
        "assembly",
        &[
            "section .text",
            "mov rax",
            "global _start",
            "[rbp",
            "push rbp",
        ],
    ),
];

#[derive(Clone, Copy, Debug, Default)]
pub struct ResolveContext<'a> {
    pub language_id: Option<&'a str>,
    pub file_name: Option<&'a str>,
    pub text: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub language: String,
    pub dialect_hints: Vec<(String, String)>,
    pub confidence: u32,
    pub reason: &'static str,
    pub changed: bool,
    pub uncertain: bool,
    pub suggestion: Option<String>,
    pub locked: bool,
}

/// `extOf`: lower-cased last extension (including the dot) of the file name, or `""`.
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => {
            let ext = &name[i..];
            if ext.len() > 1 && !ext[1..].contains(['/', '\\']) {
                ext.to_lowercase()
            } else {
                String::new()
            }
        }
        None => String::new(),
    }
}

/// One stray `if`/`for` proves nothing: two independent hints and no competing language are required.
pub fn lexical_guess(text: &str) -> Option<&'static str> {
    if text.is_empty() {
        return None;
    }
    let sample = head_utf16(text, 20000);
    let mut scored: Vec<(&'static str, usize)> = LEXICAL
        .iter()
        .map(|(lang, tokens)| {
            (
                *lang,
                tokens.iter().filter(|t| sample.contains(**t)).count(),
            )
        })
        .collect();
    scored.sort_by_key(|a| std::cmp::Reverse(a.1)); // stable: equal scores keep table order
    if scored[0].1 >= 2 && scored[1].1 == 0 {
        Some(scored[0].0)
    } else {
        None
    }
}

fn lookup(table: &[(&'static str, &'static str)], ext: &str) -> Option<&'static str> {
    table.iter().find(|(e, _)| *e == ext).map(|(_, l)| *l)
}

pub fn resolve_language(
    reg: &Registry,
    ctx: &ResolveContext,
    current: &str,
    lock: Option<&str>,
) -> Decision {
    let ext = ext_of(ctx.file_name.unwrap_or(""));
    let mut hints: Vec<(String, String)> = Vec::new();
    if ctx.language_id == Some("typescriptreact") || ext == ".tsx" {
        hints.push(("mode".into(), "tsx".into()));
    } else if ctx.language_id == Some("typescript") || ext == ".ts" {
        hints.push(("mode".into(), "ts".into()));
    }
    let hints_for = |lang: &str| {
        if lang == "typescript" {
            hints.clone()
        } else {
            Vec::new()
        }
    };
    if let Some(l) = lock.filter(|l| LANGUAGES.contains(l)) {
        return Decision {
            language: l.to_string(),
            dialect_hints: hints_for(l),
            confidence: 100,
            reason: "manual-lock",
            changed: l != current,
            uncertain: false,
            suggestion: None,
            locked: true,
        };
    }
    if let Some(by_id) = ctx.language_id.and_then(|id| reg.language_for_id(id)) {
        return Decision {
            language: by_id.to_string(),
            dialect_hints: hints_for(by_id),
            confidence: 95,
            reason: "bridge-languageId",
            changed: by_id != current,
            uncertain: false,
            suggestion: None,
            locked: false,
        };
    }
    if let Some(strong) = lookup(&STRONG_EXT, &ext) {
        return Decision {
            language: strong.to_string(),
            dialect_hints: hints_for(strong),
            confidence: 85,
            reason: "extension",
            changed: strong != current,
            uncertain: false,
            suggestion: None,
            locked: false,
        };
    }
    let mut out = Decision {
        language: current.to_string(),
        dialect_hints: Vec::new(),
        confidence: 0,
        reason: "kept",
        changed: false,
        uncertain: false,
        suggestion: None,
        locked: false,
    };
    let ambiguous = lookup(&AMBIGUOUS_EXT, &ext);
    if ambiguous.is_some() || ext.is_empty() {
        out.reason = "ambiguous-extension";
        out.uncertain = true;
        out.suggestion = ambiguous.map(str::to_string);
    }
    if let Some(guess) = ctx.text.and_then(lexical_guess) {
        if guess != current {
            if out.suggestion.is_none() {
                out.suggestion = Some(guess.to_string());
            }
            if out.reason == "kept" {
                out.reason = "lexical-suggestion";
            }
            out.uncertain = true;
        }
    }
    out // never switches automatically on weak evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        Registry::builtin().unwrap()
    }

    fn ctx<'a>(language_id: &'a str, file_name: &'a str) -> ResolveContext<'a> {
        ResolveContext {
            language_id: Some(language_id),
            file_name: Some(file_name),
            text: None,
        }
    }

    #[test]
    fn ext_of_matches_the_reference() {
        assert_eq!(ext_of("src/Main.RS"), ".rs");
        assert_eq!(ext_of("Makefile"), "");
        assert_eq!(ext_of("dir.d/file"), "");
        assert_eq!(ext_of("trailing."), "");
    }

    #[test]
    fn manual_lock_beats_everything_and_bridge_id_beats_extension() {
        let d = resolve_language(&reg(), &ctx("python", "x.py"), "python", Some("typescript"));
        assert_eq!(
            (d.language.as_str(), d.reason, d.changed, d.locked),
            ("typescript", "manual-lock", true, true)
        );
        let d = resolve_language(&reg(), &ctx("rust", "x.py"), "unknown", None);
        assert_eq!(
            (d.language.as_str(), d.reason),
            ("rust", "bridge-languageId")
        );
    }

    #[test]
    fn tsx_hint_and_weak_evidence() {
        let d = resolve_language(&reg(), &ctx("typescriptreact", "App.tsx"), "unknown", None);
        assert_eq!(
            d.dialect_hints,
            vec![("mode".to_string(), "tsx".to_string())]
        );
        let d = resolve_language(&reg(), &ctx("plaintext", "a.h"), "rust", None);
        assert_eq!(
            (
                d.language.as_str(),
                d.reason,
                d.changed,
                d.uncertain,
                d.suggestion.as_deref()
            ),
            ("rust", "ambiguous-extension", false, true, Some("c"))
        );
        let text = ResolveContext {
            language_id: Some("plaintext"),
            file_name: Some("notes.txt"),
            text: Some("if x then for y"),
        };
        assert_eq!(
            resolve_language(&reg(), &text, "python", None).reason,
            "kept"
        );
        let rusty = ResolveContext {
            language_id: Some("plaintext"),
            file_name: Some("Untitled-1"),
            text: Some("fn main() {\n    let mut x = 1;\n}\n"),
        };
        let d = resolve_language(&reg(), &rusty, "python", None);
        assert_eq!(
            (d.language.as_str(), d.suggestion.as_deref(), d.changed),
            ("python", Some("rust"), false)
        );
    }
}
