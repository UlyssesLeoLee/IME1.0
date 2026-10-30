//! Context analysis (FR-025/030): literal/statement/expression/type classification, enclosing block and
//! nearby identifiers. Port of `classifyContext`, `enclosingBlock` and `nearbyIdentifiers` from `languages.js`;
//! the regular expressions are replaced by equivalent hand-written matchers.

use crate::pack::{Context, Registry};
use crate::util::{
    boundary, chars_of, contains_word, ends_with_word, is_js_space, is_word, js_trim, js_trim_end,
    locale_compare_ascii, rfind_char, skip_ws, split_lines, starts_at, string_of, tail_utf16,
    trailing_word,
};
use crate::validators::{Style, mask_chars};
use std::collections::HashMap;

pub fn style_of(profile: &str) -> Option<Style> {
    match profile {
        "rust" => Some(Style::Rust),
        "c" => Some(Style::C),
        "typescript" => Some(Style::TypeScript),
        "python" => Some(Style::Python),
        "powershell" => Some(Style::PowerShell),
        _ => None,
    }
}

fn ends_with_any_char(t: &[char], set: &str) -> bool {
    t.last().is_some_and(|c| set.contains(*c))
}

fn ends_with_str(t: &[char], s: &str) -> bool {
    let p = chars_of(s);
    t.len() >= p.len() && t[t.len() - p.len()..] == p[..]
}

pub fn classify_context(profile: &str, before: &str) -> Context {
    if profile == "machine-code" {
        return Context::Unknown;
    }
    let tail = tail_utf16(before, 6000);
    let style = style_of(profile);
    let tail_chars = chars_of(tail);
    if let Some(st) = style {
        let mut probe = tail_chars.clone();
        probe.push('Q');
        if mask_chars(&probe, st).last() == Some(&' ') {
            return Context::Literal;
        }
    }
    if profile == "assembly" {
        let last_line = tail.rsplit('\n').next().unwrap_or("");
        if last_line.contains([';', '#']) {
            return Context::Literal;
        }
    }
    let masked: String = match style {
        Some(st) => string_of(&mask_chars(&tail_chars, st)),
        None => tail.to_string(),
    };
    let lines = split_lines(&masked);
    let cur = lines[lines.len() - 1];
    let prev = lines[..lines.len() - 1]
        .iter()
        .rev()
        .find(|l| !js_trim(l).is_empty());
    let cur_blank = js_trim(cur).is_empty();
    let probe = if cur_blank { prev.copied() } else { Some(cur) };
    let Some(probe) = probe else {
        return Context::Statement;
    };
    let t = chars_of(js_trim_end(probe));
    if profile == "assembly" {
        return Context::Statement;
    }
    if profile == "python" {
        let mut depth = 0usize;
        for c in masked.chars() {
            if matches!(c, '(' | '[' | '{') {
                depth += 1;
            } else if matches!(c, ')' | ']' | '}') {
                depth = depth.saturating_sub(1);
            }
        }
        if depth > 0 {
            return Context::Expression;
        }
        if cur_blank {
            return if t.last() == Some(&'\\') {
                Context::Expression
            } else {
                Context::Statement
            };
        }
        let word: String = trailing_word(&t).iter().collect();
        let cont = ends_with_any_char(&t, "=(,[{+-*/%<>!&|^~")
            || matches!(
                word.as_str(),
                "return"
                    | "yield"
                    | "await"
                    | "in"
                    | "and"
                    | "or"
                    | "not"
                    | "is"
                    | "if"
                    | "elif"
                    | "while"
                    | "lambda"
            );
        return if cont {
            Context::Expression
        } else {
            Context::Unknown
        };
    }
    let generic = profile == "rust" || profile == "typescript";
    let type_pos = ends_with_str(&t, "->")
        || ends_with_word(&t, "as")
        || ends_with_word(&t, "extends")
        || ends_with_word(&t, "implements")
        || (t.last() == Some(&':') && (t.len() < 2 || t[t.len() - 2] != ':'))
        || (generic && t.last() == Some(&'<'));
    if type_pos {
        return if cur_blank {
            Context::Unknown
        } else {
            Context::Type
        };
    }
    if ends_with_str(&t, "=>")
        || ends_with_any_char(&t, "=(,[+-*/%>!&|^~?")
        || ends_with_word(&t, "return")
    {
        return Context::Expression;
    }
    if ends_with_any_char(&t, ";{}") || t.is_empty() {
        return Context::Statement;
    }
    // TypeScript and PowerShell statements need no terminator: a new line that does not continue an expression
    // starts a statement
    if (profile == "typescript" || profile == "powershell") && cur_blank {
        Context::Statement
    } else {
        Context::Unknown
    }
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// `^\s*kw\b`
fn line_starts_with_word(line: &[char], kw: &str) -> bool {
    let i = skip_ws(line, 0);
    starts_at(line, i, kw) && boundary(line, i + kw.chars().count())
}

/// `^\s*(?:async\s+)?def\b`
fn line_is_def(line: &[char]) -> bool {
    let i = skip_ws(line, 0);
    if starts_at(line, i, "async") {
        let j = skip_ws(line, i + 5);
        if j > i + 5 && starts_at(line, j, "def") && boundary(line, j + 3) {
            return true;
        }
    }
    starts_at(line, i, "def") && boundary(line, i + 3)
}

/// `=>\s*$|\)\s*(?::\s*[\w<>\[\] |]+)?\s*$`
fn ts_fn_header(h: &[char]) -> bool {
    let is_class = |c: char| is_word(c) || matches!(c, '<' | '>' | '[' | ']' | ' ' | '|');
    let trimmed_len = {
        let mut n = h.len();
        while n > 0 && is_js_space(h[n - 1]) {
            n -= 1;
        }
        n
    };
    if trimmed_len >= 2 && h[trimmed_len - 2] == '=' && h[trimmed_len - 1] == '>' {
        return true;
    }
    let tail_ok = |r: &[char]| -> bool {
        let i = skip_ws(r, 0);
        if i == r.len() {
            return true;
        }
        if r[i] != ':' {
            return false;
        }
        let x = &r[i + 1..];
        let max_ws = skip_ws(x, 0);
        let mut t = x.len();
        while t > 0 && is_js_space(x[t - 1]) {
            t -= 1;
        }
        for m in 0..=max_ws {
            if m >= x.len() {
                break;
            }
            if !is_class(x[m]) {
                continue;
            }
            let mut c_end = m;
            while c_end < x.len() && is_class(x[c_end]) {
                c_end += 1;
            }
            if (m + 1).max(t) <= c_end {
                return true;
            }
        }
        false
    };
    (0..h.len()).any(|k| h[k] == ')' && tail_ok(&h[k + 1..]))
}

fn brace_header_kind(profile: &str, header: &[char]) -> Option<&'static str> {
    match profile {
        "rust" => {
            for kw in ["fn", "impl", "trait", "mod"] {
                if contains_word(header, kw) {
                    return Some(match kw {
                        "fn" => "fn",
                        "impl" => "impl",
                        "trait" => "trait",
                        _ => "mod",
                    });
                }
            }
            None
        }
        "c" => {
            let mut n = header.len();
            while n > 0 && is_js_space(header[n - 1]) {
                n -= 1;
            }
            if n > 0 && header[n - 1] == ')' {
                Some("fn")
            } else {
                None
            }
        }
        "typescript" => {
            if contains_word(header, "class") {
                Some("class")
            } else if contains_word(header, "interface") {
                Some("interface")
            } else if ts_fn_header(header) {
                Some("fn")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Innermost enclosing construct: `module`, `class`, `def`, `fn`, `impl`, `trait`, `mod`, `interface` or `block`.
pub fn enclosing_block(profile: &str, before: &str) -> &'static str {
    let tail = tail_utf16(before, 12000);
    if profile == "python" {
        let lines = split_lines(tail);
        let cur_indent = leading_indent(lines[lines.len() - 1]);
        let mut want = cur_indent;
        let mut i = lines.len() as isize - 2;
        while i >= 0 && want > 0 {
            let line = lines[i as usize];
            i -= 1;
            let lc = chars_of(line);
            if js_trim(line).is_empty() || lc.get(skip_ws(&lc, 0)) == Some(&'#') {
                continue;
            }
            let indent = leading_indent(line);
            if indent < want {
                if line_starts_with_word(&lc, "class") {
                    return "class";
                }
                if line_is_def(&lc) {
                    return "def";
                }
                want = indent;
            }
        }
        return "module";
    }
    let Some(style) = style_of(profile) else {
        return "module";
    };
    if !matches!(profile, "rust" | "c" | "typescript") {
        return "module";
    }
    let masked = mask_chars(&chars_of(tail), style);
    let mut stack: Vec<usize> = Vec::new();
    for (i, c) in masked.iter().enumerate() {
        if *c == '{' {
            stack.push(i);
        } else if *c == '}' {
            stack.pop();
        }
    }
    let Some(&open) = stack.last() else {
        return "module";
    };
    let found = |c: char, from: usize| rfind_char(&masked, c, from).map_or(-1isize, |p| p as isize);
    // lastIndexOf('{', open - 1) with open == 0 is clamped to index 0 by JavaScript
    let brace = if open == 0 { 0 } else { found('{', open - 1) };
    let header_start = (found(';', open).max(found('}', open)).max(brace) + 1) as usize;
    let header: &[char] = if header_start > open {
        &[]
    } else {
        &masked[header_start..open]
    };
    brace_header_kind(profile, header).unwrap_or("block")
}

/// Up to `max` identifiers near the cursor that extend `key`, most frequent first (ties: locale order).
pub fn nearby_identifiers(
    reg: &Registry,
    profile: &str,
    text: &str,
    key: &str,
    max: usize,
) -> Vec<String> {
    let kw: Vec<String> = reg
        .keywords(profile)
        .iter()
        .map(|k| k.to_lowercase())
        .collect();
    let lower = key.to_lowercase();
    let key_len = crate::util::utf16_len(key);
    let sample = chars_of(tail_utf16(text, 4000));
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < sample.len() {
        let c = sample[i];
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let s = i;
            while i < sample.len() && (is_word(sample[i]) || sample[i] == '$') {
                i += 1;
            }
            let word: String = sample[s..i].iter().collect();
            let wl = word.to_lowercase();
            if word.len() <= key_len || kw.contains(&wl) || !wl.starts_with(&lower) {
                continue;
            }
            match index.get(&word) {
                Some(&k) => counts[k].1 += 1,
                None => {
                    index.insert(word.clone(), counts.len());
                    counts.push((word, 1));
                }
            }
        } else {
            i += 1;
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_compare_ascii(&a.0, &b.0)));
    counts.into_iter().take(max).map(|(w, _)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_positions_are_detected() {
        assert_eq!(classify_context("rust", "let s = \"abc "), Context::Literal);
        assert_eq!(classify_context("rust", "// hello "), Context::Literal);
        assert_eq!(classify_context("python", "# comment "), Context::Literal);
        assert_eq!(
            classify_context("assembly", "mov rax, rbx ; copy "),
            Context::Literal
        );
    }

    #[test]
    fn statement_expression_and_type_positions() {
        assert_eq!(classify_context("rust", ""), Context::Statement);
        assert_eq!(classify_context("rust", "let x = "), Context::Expression);
        assert_eq!(classify_context("rust", "fn f() -> "), Context::Type);
        assert_eq!(classify_context("rust", "let x: "), Context::Type);
        assert_eq!(classify_context("rust", "foo.bar baz "), Context::Unknown);
        assert_eq!(classify_context("python", "x = 1\n"), Context::Statement);
    }

    #[test]
    fn typescript_and_powershell_new_lines_start_statements() {
        assert_eq!(
            classify_context("typescript", "const y = 2\n"),
            Context::Statement
        );
        assert_eq!(
            classify_context("powershell", "$y = 2\n"),
            Context::Statement
        );
        assert_eq!(classify_context("rust", "let y = 2\n"), Context::Unknown);
        assert_eq!(classify_context("c", "int y = 2\n"), Context::Unknown);
    }

    #[test]
    fn enclosing_blocks() {
        assert_eq!(enclosing_block("rust", "impl Foo {\n    "), "impl");
        assert_eq!(enclosing_block("rust", "trait T {\n    "), "trait");
        assert_eq!(
            enclosing_block("rust", "impl Foo {\n    fn a() {\n        "),
            "fn"
        );
        assert_eq!(enclosing_block("rust", "struct A;\n\n"), "module");
        assert_eq!(enclosing_block("python", "class A:\n    "), "class");
        assert_eq!(
            enclosing_block("python", "class A:\n    def g(self):\n        "),
            "def"
        );
        assert_eq!(enclosing_block("typescript", "class A {\n  "), "class");
        assert_eq!(enclosing_block("c", "int main(void) {\n"), "fn");
    }

    #[test]
    fn nearby_identifiers_rank_by_frequency() {
        let reg = Registry::builtin().unwrap();
        let text = "let counter = 1;\nlet count_total = counter + 2;\n";
        assert_eq!(
            nearby_identifiers(&reg, "rust", text, "cou", 3),
            vec!["counter", "count_total"]
        );
    }
}
