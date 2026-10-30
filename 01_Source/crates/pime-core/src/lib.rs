//! Programming IME deterministic local engine (PoC, not a production parser).
//! No network, executable code generation, file writes, or API keys.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub text: String,
    pub origin: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageDecision {
    pub language: String,
    pub confidence: u8,
    pub reason: String,
}

pub const LANGUAGES: [&str; 7] = [
    "rust", "python", "typescript", "powershell", "c", "assembly", "machine-code",
];

/// File extension is authoritative when provided by the trusted editor.
/// Lexical fallbacks are conservative; never overwrite a user's locked profile.
pub fn detect_language(filename: &str, text: &str, locked: Option<&str>) -> LanguageDecision {
    if let Some(lang) = locked.filter(|s| LANGUAGES.contains(s)) {
        return LanguageDecision { language: lang.into(), confidence: 100, reason: "manual-lock".into() };
    }
    let name = filename.to_ascii_lowercase();
    let extension = name.rsplit('.').next().unwrap_or("");
    let by_extension = match extension {
        "rs" => "rust", "py" | "pyw" => "python", "ts" | "tsx" | "mts" | "cts" => "typescript",
        "ps1" | "psm1" | "psd1" => "powershell", "c" | "h" => "c",
        "asm" | "s" | "nasm" => "assembly", "hex" | "binhex" => "machine-code", _ => "",
    };
    if !by_extension.is_empty() {
        return LanguageDecision { language: by_extension.into(), confidence: 95, reason: "extension".into() };
    }
    let candidates: [(&str, &[&str]); 6] = [
        ("rust", &["fn main()", "impl ", "let mut ", "println!"]),
        ("python", &["def ", "import ", "self.", "elif "]),
        ("typescript", &["interface ", "export const ", "=>", ": string"]),
        ("powershell", &["Get-", "$PS", "Write-Host", "Where-Object"]),
        ("c", &["#include", "printf(", "typedef struct", "int main("]),
        ("assembly", &["section .text", "mov rax", "global _start", "[rbp"]),
    ];
    let mut best = ("", 0);
    for (lang, tokens) in candidates {
        let score = tokens.iter().filter(|t| text.contains(**t)).count();
        if score > best.1 { best = (lang, score); }
    }
    if best.1 >= 2 {
        LanguageDecision { language: best.0.into(), confidence: 75, reason: "lexical".into() }
    } else {
        LanguageDecision { language: "unknown".into(), confidence: 0, reason: "insufficient-evidence".into() }
    }
}

const RUST: &[(&str, &str, &str)] = &[
    ("fnm", "main function", "fn main() {\n    \n}"),
    ("fn", "function", "fn name() {\n    \n}"),
    ("fori", "iterator loop", "for item in items.iter() {\n    \n}"),
    ("match", "match expression", "match value {\n    _ => {}\n}"),
    ("iflet", "Option match", "if let Some(value) = option {\n    \n}"),
    ("impl", "implementation", "impl Type {\n    \n}"),
    ("enum", "enum", "enum Name {\n    Variant,\n}"),
];
const PYTHON: &[(&str, &str, &str)] = &[
    ("def", "function", "def function():\n    pass"),
    ("defmain", "main entry", "def main():\n    pass\n\nif __name__ == '__main__':\n    main()"),
    ("fori", "for loop", "for item in items:\n    pass"),
    ("class", "class", "class Name:\n    def __init__(self):\n        pass"),
    ("try", "exception handling", "try:\n    pass\nexcept Exception as exc:\n    raise"),
    ("with", "context manager", "with resource as value:\n    pass"),
];
const TS: &[(&str, &str, &str)] = &[
    ("fn", "arrow function", "const name = (value: unknown): void => {\n  \n};"),
    ("interface", "interface", "interface Name {\n  id: string;\n}"),
    ("type", "type alias", "type Name = string | number;"),
    ("async", "async function", "async function name(): Promise<void> {\n  \n}"),
    ("fori", "for loop", "for (const item of items) {\n  \n}"),
];
const PS: &[(&str, &str, &str)] = &[
    ("func", "function", "function Invoke-Name {\n    param()\n}"),
    ("foreach", "ForEach-Object", "$items | ForEach-Object {\n    $_\n}"),
    ("try", "try/catch", "try {\n    \n} catch {\n    throw\n}"),
    ("param", "parameter block", "param(\n    [string]$Name\n)"),
];
const C: &[(&str, &str, &str)] = &[
    ("main", "main function", "int main(void) {\n    return 0;\n}"),
    ("fori", "for loop", "for (size_t i = 0; i < count; ++i) {\n    \n}"),
    ("struct", "struct", "struct Name {\n    int value;\n};"),
    ("include", "include header", "#include <stdio.h>"),
    ("ifndef", "header guard", "#ifndef HEADER_H\n#define HEADER_H\n\n#endif"),
];
const ASM: &[(&str, &str, &str)] = &[
    ("mov", "move operand (Intel)", "mov rax, rbx"),
    ("push", "push register", "push rbp"),
    ("prologue", "frame prologue (x86-64)", "push rbp\nmov rbp, rsp"),
    ("ret", "return", "ret"),
    ("nop", "no operation", "nop"),
];
const MACHINE: &[(&str, &str, &str)] = &[
    ("nop", "x86-64 NOP", "90"),
    ("ret", "x86-64 RET", "C3"),
    ("int3", "x86-64 breakpoint byte", "CC"),
    ("xor eax,eax", "x86-64 XOR EAX,EAX", "31 C0"),
    ("syscall", "x86-64 SYSCALL", "0F 05"),
];

pub fn predict_local(language: &str, prefix: &str, limit: usize) -> Vec<Candidate> {
    let entries = match language {
        "rust" => RUST, "python" => PYTHON, "typescript" => TS,
        "powershell" => PS, "c" => C, "assembly" => ASM,
        "machine-code" => MACHINE, _ => return Vec::new(),
    };
    let key = prefix.trim().to_ascii_lowercase();
    if key.is_empty() || key.len() > 128 || limit == 0 { return Vec::new(); }
    entries.iter()
        .filter(|(trigger, _, _)| trigger.starts_with(&key) || trigger == &key)
        .take(limit.min(20))
        .map(|(_, name, code)| Candidate {
            label: (*name).into(), text: (*code).into(), origin: "local".into(),
        }).collect()
}

/// Strict sample x86-64 opcode lookup. Does NOT assemble arbitrary source and NEVER executes.
pub fn encode_x86_64(instruction: &str) -> Result<String, &'static str> {
    let normalized = instruction.trim().to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    match normalized.as_str() {
        "nop" => Ok("90".into()), "ret" => Ok("C3".into()),
        "int3" => Ok("CC".into()), "syscall" => Ok("0F 05".into()),
        "xor eax, eax" => Ok("31 C0".into()),
        _ => Err("UNSUPPORTED_ENCODING: limited sample table, not a general assembler"),
    }
}

pub fn validate_hex(value: &str) -> Result<Vec<u8>, &'static str> {
    let compact: String = value.chars().filter(|x| !x.is_ascii_whitespace()).collect();
    if compact.len() % 2 != 0 { return Err("ODD_HEX_LENGTH"); }
    if compact.len() > 4096 { return Err("HEX_TOO_LARGE"); }
    compact.as_bytes().chunks(2).map(|chunk| {
        let s = std::str::from_utf8(chunk).map_err(|_| "INVALID_HEX")?;
        u8::from_str_radix(s, 16).map_err(|_| "INVALID_HEX")
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn seven_languages_present() { assert_eq!(LANGUAGES.len(), 7); }
    #[test] fn explicit_lock_wins() { assert_eq!(detect_language("a.py", "fn main()", Some("rust")).language, "rust"); }
    #[test] fn extension_detected() { assert_eq!(detect_language("x.ps1", "", None).language, "powershell"); }
    #[test] fn ambiguous_not_fabricated() { assert_eq!(detect_language("README", "foo bar", None).language, "unknown"); }
    #[test] fn prefix_candidates() { assert!(predict_local("rust", "fnm", 3)[0].text.starts_with("fn main")); }
    #[test] fn unknown_language_safe() { assert!(predict_local("unknown", "fn", 3).is_empty()); }
    #[test] fn empty_prefix_safe() { assert!(predict_local("rust", "", 3).is_empty()); }
    #[test] fn limit_respected() { assert_eq!(predict_local("rust", "f", 1).len(), 1); }
    #[test] fn code_is_language_specific() { assert_ne!(predict_local("rust", "fori", 1)[0].text, predict_local("python", "fori", 1)[0].text); }
    #[test] fn x64_nop_exact() { assert_eq!(encode_x86_64("nop").unwrap(), "90"); }
    #[test] fn x64_unknown_rejected() { assert!(encode_x86_64("jmp mylabel").is_err()); }
    #[test] fn hex_pairs() { assert_eq!(validate_hex("90 C3").unwrap(), vec![0x90, 0xC3]); }
    #[test] fn odd_hex_rejected() { assert!(validate_hex("ABC").is_err()); }
    #[test] fn invalid_hex_rejected() { assert!(validate_hex("GG").is_err()); }
}
