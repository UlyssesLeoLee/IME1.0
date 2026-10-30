//! Lightweight structural validators (FR-038, DD section 5.2), hand-written port of `validators.js`.
//! They are NOT compilers or full parsers: "valid" means "no structural problem found by these checks".
//! Source is treated purely as text; nothing is executed. Regular expressions of the reference engine are
//! replaced by equivalent matchers (ASCII `\w`, JavaScript white space, same backtracking results).

use crate::util::{
    boundary, chars_of, find_seq, is_js_space, is_word, js_trim, js_trim_end, skip_ws, split_lines,
    starts_at, starts_at_ci, string_of,
};
use std::collections::BTreeMap;

/// Dialect axis -> value (for example `edition` -> `2024`).
pub type Dialect = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationResult {
    pub ok: bool,
    pub level: &'static str,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Rust,
    C,
    TypeScript,
    Python,
    PowerShell,
}

fn issue(line: usize, code: &'static str, message: impl Into<String>) -> Issue {
    Issue {
        line,
        code,
        message: message.into(),
    }
}

/// `dialect[axis] || default` (empty string counts as unset like in JavaScript).
fn dget<'a>(d: &'a Dialect, axis: &str, default: &'a str) -> &'a str {
    match d.get(axis) {
        Some(v) if !v.is_empty() => v,
        _ => default,
    }
}

// ---------------------------------------------------------------- masking

#[derive(Clone, Copy, PartialEq, Eq)]
enum Esc {
    Backslash,
    Backtick,
    Doubling,
}

fn scan_quoted(t: &[char], i: usize, quote: char, esc: Esc) -> usize {
    let n = t.len();
    let mut j = i + 1;
    while j < n {
        let c = t[j];
        if esc == Esc::Backslash && c == '\\' {
            j += 2;
            continue;
        }
        if esc == Esc::Backtick && c == '`' {
            j += 2;
            continue;
        }
        if c == quote {
            if (esc == Esc::Doubling || esc == Esc::Backtick) && t.get(j + 1) == Some(&quote) {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    n
}

fn line_end(t: &[char], i: usize) -> usize {
    t[i..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(t.len(), |p| i + p)
}

/// `/^'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^\\'\n])'/` on a Rust char literal; returns the length.
/// Astral characters (two UTF-16 units in JavaScript) never match, exactly like the reference.
fn rust_char_literal(t: &[char], i: usize) -> Option<usize> {
    let win: Vec<char> = t[i..t.len().min(i + 14)].to_vec();
    let bmp = |c: char| (c as u32) <= 0xFFFF;
    let at = |k: usize| win.get(k).copied();
    if at(0) != Some('\'') {
        return None;
    }
    if at(1) == Some('\\') {
        // x HH
        if at(2) == Some('x')
            && at(3).is_some_and(|c| c.is_ascii_hexdigit())
            && at(4).is_some_and(|c| c.is_ascii_hexdigit())
            && at(5) == Some('\'')
        {
            return Some(6);
        }
        // u{...}
        if at(2) == Some('u') && at(3) == Some('{') {
            let mut k = 4;
            while at(k).is_some_and(|c| c.is_ascii_hexdigit() || c == '_') {
                k += 1;
            }
            if k > 4 && at(k) == Some('}') && at(k + 1) == Some('\'') {
                return Some(k + 2);
            }
        }
        // any single unit except a line terminator
        if let Some(c) = at(2)
            && bmp(c)
            && !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
            && at(3) == Some('\'')
        {
            return Some(4);
        }
        return None;
    }
    match at(1) {
        Some(c) if c != '\'' && c != '\n' && bmp(c) && at(2) == Some('\'') => Some(3),
        _ => None,
    }
}

/// Raw string opener `/^b?r(#*)"/`; returns (length of opener, number of hashes).
fn rust_raw_opener(t: &[char], i: usize) -> Option<(usize, usize)> {
    let mut k = i;
    if t.get(k) == Some(&'b') {
        k += 1;
    }
    if t.get(k) != Some(&'r') {
        return None;
    }
    k += 1;
    let mut hashes = 0;
    while t.get(k) == Some(&'#') {
        hashes += 1;
        k += 1;
    }
    if t.get(k) != Some(&'"') || k + 1 - i > 40 {
        return None;
    }
    Some((k + 1 - i, hashes))
}

/// What a masked region is: a comment or a string literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Comment,
    String,
}

/// A region blanked by the masking, in char indices (`end` exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
}

/// Replaces comments and literals by blanks (newlines kept) so bracket/indent logic only sees code.
pub fn mask_source(text: &str, style: Style) -> String {
    string_of(&mask_chars(&chars_of(text), style))
}

pub fn mask_chars(text: &[char], style: Style) -> Vec<char> {
    mask_chars_spans(text, style).0
}

/// Like [`mask_chars`], and also reports every blanked region (the `spans` argument of `maskSource`).
pub fn mask_chars_spans(text: &[char], style: Style) -> (Vec<char>, Vec<Span>) {
    let n = text.len();
    let mut out = text.to_vec();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < n {
        let c = text[i];
        let d = text.get(i + 1).copied();
        let mut end: Option<usize> = None;
        let mut kind = SpanKind::String;
        match style {
            Style::Python => {
                if c == '#' {
                    end = Some(line_end(text, i));
                    kind = SpanKind::Comment;
                } else if c == '"' || c == '\'' {
                    if text.get(i + 1) == Some(&c) && text.get(i + 2) == Some(&c) {
                        let e = find_seq(text, &[c, c, c], i + 3);
                        end = Some(e.map_or(n, |e| e + 3));
                    } else {
                        end = Some(scan_quoted(text, i, c, Esc::Backslash));
                    }
                }
            }
            Style::PowerShell => {
                if c == '<' && d == Some('#') {
                    let e = find_seq(text, &['#', '>'], i + 2);
                    end = Some(e.map_or(n, |e| e + 2));
                    kind = SpanKind::Comment;
                } else if c == '#' {
                    end = Some(line_end(text, i));
                    kind = SpanKind::Comment;
                } else if c == '@' && matches!(d, Some('"' | '\'')) && {
                    // rest of the line after @" must be blanks only (/^[ \t]*\r?$/)
                    let le = line_end(text, i);
                    let rest = &text[(i + 2).min(le)..le];
                    let body = if rest.last() == Some(&'\r') {
                        &rest[..rest.len() - 1]
                    } else {
                        rest
                    };
                    body.iter().all(|&x| x == ' ' || x == '\t')
                } {
                    let q = d.unwrap_or('"');
                    let e = find_seq(text, &['\n', q, '@'], i);
                    end = Some(e.map_or(n, |e| e + 3));
                } else if c == '\'' {
                    end = Some(scan_quoted(text, i, '\'', Esc::Doubling));
                } else if c == '"' {
                    end = Some(scan_quoted(text, i, '"', Esc::Backtick));
                }
            }
            Style::Rust | Style::C | Style::TypeScript => {
                if c == '/' && d == Some('/') {
                    end = Some(line_end(text, i));
                    kind = SpanKind::Comment;
                } else if c == '/' && d == Some('*') {
                    kind = SpanKind::Comment;
                    let mut depth = 1usize;
                    let mut j = i + 2;
                    while j < n && depth > 0 {
                        if style == Style::Rust && text[j] == '/' && text.get(j + 1) == Some(&'*') {
                            depth += 1;
                            j += 2;
                        } else if text[j] == '*' && text.get(j + 1) == Some(&'/') {
                            depth -= 1;
                            j += 2;
                        } else {
                            j += 1;
                        }
                    }
                    end = Some(j);
                } else if style == Style::Rust
                    && (c == 'r' || c == 'b')
                    && !(i > 0 && is_word(text[i - 1]))
                    && let Some((len, hashes)) = rust_raw_opener(text, i)
                {
                    let mut closer = vec!['"'];
                    closer.extend(std::iter::repeat_n('#', hashes));
                    let e = find_seq(text, &closer, i + len);
                    end = Some(e.map_or(n, |e| e + 1 + hashes));
                }
                if end.is_none() && c == '"' {
                    end = Some(scan_quoted(text, i, '"', Esc::Backslash));
                } else if end.is_none() && c == '`' && style == Style::TypeScript {
                    end = Some(scan_quoted(text, i, '`', Esc::Backslash));
                } else if end.is_none() && c == '\'' {
                    if style == Style::Rust {
                        end = rust_char_literal(text, i).map(|l| i + l);
                    } else {
                        end = Some(scan_quoted(text, i, '\'', Esc::Backslash));
                    }
                }
            }
        }
        match end {
            Some(e) if e > i => {
                spans.push(Span {
                    start: i,
                    end: e.min(n),
                    kind,
                });
                for slot in out.iter_mut().take(e.min(n)).skip(i) {
                    if *slot != '\n' && *slot != '\r' {
                        *slot = ' ';
                    }
                }
                i = e;
            }
            _ => i += 1,
        }
    }
    (out, spans)
}

// ---------------------------------------------------------------- brackets

fn closer_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

pub fn bracket_issues(masked: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut line = 1usize;
    for c in masked.chars() {
        if c == '\n' {
            line += 1;
        } else if closer_of(c).is_some() {
            stack.push((c, line));
        } else if c == ')' || c == ']' || c == '}' {
            match stack.pop() {
                None => {
                    issues.push(issue(
                        line,
                        "BRACKET_UNEXPECTED",
                        format!("unexpected '{c}'"),
                    ));
                    if issues.len() >= 20 {
                        break;
                    }
                }
                Some((open, open_line)) if closer_of(open) != Some(c) => {
                    issues.push(issue(
                        line,
                        "BRACKET_MISMATCH",
                        format!("'{open}' from line {open_line} closed by '{c}'"),
                    ));
                    if issues.len() >= 20 {
                        break;
                    }
                }
                Some(_) => {}
            }
        }
    }
    for (c, l) in stack {
        if issues.len() < 20 {
            issues.push(issue(
                l,
                "BRACKET_UNCLOSED",
                format!("'{c}' is never closed"),
            ));
        }
    }
    issues
}

// ---------------------------------------------------------------- python

fn leading_blank_len(line: &str) -> (usize, bool, bool) {
    let mut n = 0;
    let (mut sp, mut tb) = (false, false);
    for c in line.chars() {
        match c {
            ' ' => sp = true,
            '\t' => tb = true,
            _ => break,
        }
        n += 1;
    }
    (n, sp, tb)
}

pub fn python_issues(text: &str) -> Vec<Issue> {
    let masked = mask_source(text, Style::Python);
    let lines = split_lines(text);
    let mlines = split_lines(&masked);
    let mut issues: Vec<Issue> = Vec::new();
    let mut stack: Vec<usize> = vec![0];
    let mut depth = 0usize;
    let (mut opener, mut cont) = (false, false);
    let mut opener_line = 0usize;
    let (mut saw_tab, mut saw_space) = (false, false);
    for (k, line) in lines.iter().enumerate() {
        let m = mlines.get(k).copied().unwrap_or("");
        if js_trim(m).is_empty() {
            continue;
        }
        let (indent, has_space, has_tab) = leading_blank_len(line);
        if has_space && has_tab {
            issues.push(issue(
                k + 1,
                "TAB_SPACE_MIX",
                "tabs and spaces mixed in one indentation",
            ));
        }
        if has_tab {
            saw_tab = true;
        }
        if has_space {
            saw_space = true;
        }
        let top = *stack.last().unwrap_or(&0);
        if depth == 0 && !cont {
            if opener {
                if indent <= top {
                    issues.push(issue(
                        opener_line,
                        "EXPECTED_INDENT",
                        "expected an indented block",
                    ));
                } else {
                    stack.push(indent);
                }
            } else if indent > top {
                issues.push(issue(k + 1, "UNEXPECTED_INDENT", "unexpected indent"));
            } else if indent < top {
                while stack.len() > 1 && *stack.last().unwrap_or(&0) > indent {
                    stack.pop();
                }
                if *stack.last().unwrap_or(&0) != indent {
                    issues.push(issue(
                        k + 1,
                        "UNINDENT_MISMATCH",
                        "unindent does not match any outer indentation level",
                    ));
                }
            }
        }
        for c in m.chars() {
            if closer_of(c).is_some() {
                depth += 1;
            } else if c == ')' || c == ']' || c == '}' {
                depth = depth.saturating_sub(1);
            }
        }
        let trimmed = js_trim_end(m);
        cont = trimmed.ends_with('\\');
        opener = depth == 0 && !cont && trimmed.ends_with(':');
        if opener {
            opener_line = k + 1;
        }
    }
    if opener {
        issues.push(issue(
            opener_line,
            "EXPECTED_INDENT",
            "expected an indented block",
        ));
    }
    if saw_tab && saw_space && !issues.iter().any(|x| x.code == "TAB_SPACE_MIX") {
        issues.push(issue(
            0,
            "TAB_SPACE_MIX",
            "file mixes tab-indented and space-indented lines",
        ));
    }
    issues.extend(bracket_issues(&masked));
    issues
}

// ---------------------------------------------------------------- powershell

fn ps_has(line: &[char], pat: &str) -> bool {
    let p = chars_of(pat);
    find_seq(line, &p, 0).is_some()
}

/// `/(?<!\|)\s\S[^?\n]*\s\?\s+\S[^:\n]*\s:\s+\S/` (see the reasoning in the module docs: the first `?` / `:`
/// after each anchor is the only possible candidate, so the regex is deterministic).
fn ps_ternary(line: &[char]) -> bool {
    let n = line.len();
    for i in 0..n {
        if !is_js_space(line[i])
            || (i > 0 && line[i - 1] == '|')
            || i + 1 >= n
            || is_js_space(line[i + 1])
        {
            continue;
        }
        let Some(q) = (i + 2..n).find(|&k| line[k] == '?') else {
            continue;
        };
        if q < i + 3 || !is_js_space(line[q - 1]) {
            continue;
        }
        let r0 = q + 1;
        if r0 >= n || !is_js_space(line[r0]) {
            continue;
        }
        let r = skip_ws(line, r0);
        if r >= n {
            continue;
        }
        let Some(m) = (r + 1..n).find(|&k| line[k] == ':') else {
            continue;
        };
        if m < r + 2 || !is_js_space(line[m - 1]) {
            continue;
        }
        let s0 = m + 1;
        if s0 >= n || !is_js_space(line[s0]) {
            continue;
        }
        if skip_ws(line, s0) < n {
            return true;
        }
    }
    false
}

/// `/\|\s*\?\s*\{/` - the `? { }` Where-Object alias.
fn ps_where_alias(line: &[char]) -> bool {
    for i in 0..line.len() {
        if line[i] == '|' {
            let q = skip_ws(line, i + 1);
            if q < line.len() && line[q] == '?' {
                let r = skip_ws(line, q + 1);
                if r < line.len() && line[r] == '{' {
                    return true;
                }
            }
        }
    }
    false
}

fn ps7_hits(line: &[char]) -> Vec<(&'static str, &'static str)> {
    let mut hits = Vec::new();
    if ps_has(line, "??") {
        hits.push((
            "PS7_NULL_COALESCING",
            "null-coalescing operators need PowerShell 7+",
        ));
    }
    if ps_has(line, "?.") || ps_has(line, "?[") {
        hits.push((
            "PS7_NULL_CONDITIONAL",
            "null-conditional access needs PowerShell 7+",
        ));
    }
    if ps_has(line, "&&") || ps_has(line, "||") {
        hits.push((
            "PS7_PIPELINE_CHAIN",
            "pipeline chain operators need PowerShell 7+",
        ));
    }
    let mut parallel = false;
    let mut from = 0;
    while let Some(i) = (from..line.len()).find(|&k| line[k] == '-') {
        if starts_at_ci(line, i, "-parallel") && !line.get(i + 9).copied().is_some_and(is_word) {
            parallel = true;
            break;
        }
        from = i + 1;
    }
    if parallel {
        hits.push((
            "PS7_PARALLEL",
            "ForEach-Object -Parallel needs PowerShell 7+",
        ));
    }
    if !ps_where_alias(line) && ps_ternary(line) {
        hits.push(("PS7_TERNARY", "ternary operator needs PowerShell 7+"));
    }
    hits
}

pub fn powershell_issues(text: &str, dialect: &Dialect) -> Vec<Issue> {
    let masked = mask_source(text, Style::PowerShell);
    let mut issues = bracket_issues(&masked);
    if dget(dialect, "version", "7") == "5.1" {
        for (idx, line) in split_lines(&masked).iter().enumerate() {
            for (code, message) in ps7_hits(&chars_of(line)) {
                issues.push(issue(idx + 1, code, message));
            }
        }
    }
    issues
}

// ---------------------------------------------------------------- c / typescript

fn c23_hits(line: &str) -> Vec<(&'static str, &'static str)> {
    let t = chars_of(line);
    let mut hits = Vec::new();
    // whole-word runs
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if is_word(t[i]) {
            let s = i;
            while i < t.len() && is_word(t[i]) {
                i += 1;
            }
            words.push(t[s..i].iter().collect());
        } else {
            i += 1;
        }
    }
    let has = |w: &str| words.iter().any(|x| x == w);
    if has("nullptr") {
        hits.push(("C23_NULLPTR", "nullptr needs C23"));
    }
    if has("constexpr") {
        hits.push(("C23_CONSTEXPR", "constexpr needs C23"));
    }
    if has("typeof") || has("typeof_unqual") {
        hits.push(("C23_TYPEOF", "typeof needs C23"));
    }
    if find_seq(&t, &['[', '['], 0).is_some() {
        hits.push(("C23_ATTRIBUTE", "[[attribute]] syntax needs C23"));
    }
    // /^\s*#\s*embed\b/
    let a = skip_ws(&t, 0);
    if a < t.len() && t[a] == '#' {
        let b = skip_ws(&t, a + 1);
        if starts_at(&t, b, "embed") && boundary(&t, b + 5) {
            hits.push(("C23_EMBED", "#embed needs C23"));
        }
    }
    if words.iter().any(|w| {
        let b: Vec<char> = w.chars().collect();
        b.len() >= 3
            && b[0] == '0'
            && (b[1] == 'b' || b[1] == 'B')
            && b[2..].iter().all(|&c| c == '0' || c == '1')
    }) {
        hits.push(("C23_BINARY_LITERAL", "binary literals need C23"));
    }
    if has("_BitInt") {
        hits.push(("C23_BITINT", "_BitInt needs C23"));
    }
    hits
}

pub fn c_issues(text: &str, dialect: &Dialect) -> Vec<Issue> {
    let masked = mask_source(text, Style::C);
    let mut issues = bracket_issues(&masked);
    if dget(dialect, "std", "c17") != "c23" {
        for (idx, line) in split_lines(&masked).iter().enumerate() {
            for (code, message) in c23_hits(line) {
                issues.push(issue(idx + 1, code, message));
            }
        }
    }
    issues
}

/// `/(?:=|\()\s*<[A-Za-z_]\w*>\s*\(/`
fn tsx_generic_arrow(line: &[char]) -> bool {
    for i in 0..line.len() {
        if line[i] != '=' && line[i] != '(' {
            continue;
        }
        let a = skip_ws(line, i + 1);
        if a >= line.len() || line[a] != '<' {
            continue;
        }
        let b = a + 1;
        if b >= line.len() || !(line[b].is_ascii_alphabetic() || line[b] == '_') {
            continue;
        }
        let mut e = b + 1;
        while e < line.len() && is_word(line[e]) {
            e += 1;
        }
        if e < line.len() && line[e] == '>' {
            let f = skip_ws(line, e + 1);
            if f < line.len() && line[f] == '(' {
                return true;
            }
        }
    }
    false
}

pub fn ts_issues(text: &str, dialect: &Dialect) -> Vec<Issue> {
    let masked = mask_source(text, Style::TypeScript);
    let mut issues = bracket_issues(&masked);
    if dget(dialect, "mode", "") == "tsx" {
        for (idx, line) in split_lines(&masked).iter().enumerate() {
            if tsx_generic_arrow(&chars_of(line)) {
                issues.push(issue(
                    idx + 1,
                    "TSX_GENERIC_AMBIGUOUS",
                    "a bare <T> generic arrow is parsed as JSX in TSX; write <T,>",
                ));
            }
        }
    }
    issues
}

// ---------------------------------------------------------------- assembly

/// REG64 | REG32 | REG16_8 of the reference engine.
const REGISTERS: [&str; 52] = [
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15", "eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp", "r8d", "r9d", "r10d",
    "r11d", "r12d", "r13d", "r14d", "r15d", "ax", "bx", "cx", "dx", "si", "di", "bp", "sp", "al",
    "bl", "cl", "dl", "ah", "bh", "ch", "dh", "sil", "dil", "bpl", "spl",
];

fn is_register_name(lower: &str) -> bool {
    !lower.is_empty() && REGISTERS.contains(&lower)
}

/// `REG_RE`: a bare register name (a whole word) not preceded by `\w % $ .`.
fn has_bare_register(s: &[char]) -> bool {
    let mut i = 0;
    while i < s.len() {
        if is_word(s[i]) {
            let a = i;
            while i < s.len() && is_word(s[i]) {
                i += 1;
            }
            let prev_ok = a == 0 || !matches!(s[a - 1], '%' | '$' | '.');
            if prev_ok && is_register_name(&s[a..i].iter().collect::<String>().to_ascii_lowercase())
            {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// First `ATT_REG_RE` match (`%reg` as a whole word): span in chars.
fn first_att_reg(s: &[char]) -> Option<(usize, usize)> {
    for k in 0..s.len() {
        if s[k] == '%' {
            let mut e = k + 1;
            while e < s.len() && is_word(s[e]) {
                e += 1;
            }
            if e > k + 1
                && is_register_name(&s[k + 1..e].iter().collect::<String>().to_ascii_lowercase())
            {
                return Some((k, e));
            }
        }
    }
    None
}

/// `/(?:^|[\s,])\$-?(?:0x[0-9a-f]+|\d+)\b/i`
fn has_att_immediate(s: &[char]) -> bool {
    for k in 0..s.len() {
        if s[k] != '$' || !(k == 0 || is_js_space(s[k - 1]) || s[k - 1] == ',') {
            continue;
        }
        let mut a = k + 1;
        if s.get(a) == Some(&'-') {
            a += 1;
        }
        let mut e = a;
        while e < s.len() && is_word(s[e]) {
            e += 1;
        }
        let run: String = s[a..e].iter().collect::<String>().to_ascii_lowercase();
        let hex = run
            .strip_prefix("0x")
            .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()));
        if hex || (!run.is_empty() && run.chars().all(|c| c.is_ascii_digit())) {
            return true;
        }
    }
    false
}

/// `/\[[^\]]+\]/`
fn has_bracket_operand(s: &[char]) -> bool {
    for i in 0..s.len() {
        if s[i] == '['
            && let Some(j) = (i + 1..s.len()).find(|&k| s[k] == ']')
            && j >= i + 2
        {
            return true;
        }
    }
    false
}

fn directive_word(line: &[char], dotted: bool) -> Option<String> {
    let mut i = skip_ws(line, 0);
    if dotted {
        if line.get(i) != Some(&'.') {
            return None;
        }
        i += 1;
    }
    let s = i;
    while i < line.len() && is_word(line[i]) {
        i += 1;
    }
    Some(line[s..i].iter().collect::<String>().to_ascii_lowercase())
}

fn is_nasm_directive(line: &[char]) -> bool {
    directive_word(line, false).is_some_and(|w| {
        matches!(
            w.as_str(),
            "section" | "global" | "extern" | "bits" | "default"
        )
    })
}

fn is_gas_directive(line: &[char]) -> bool {
    directive_word(line, true).is_some_and(|w| {
        matches!(
            w.as_str(),
            "text"
                | "data"
                | "bss"
                | "globl"
                | "global"
                | "section"
                | "intel_syntax"
                | "att_syntax"
        )
    })
}

fn strip_asm_comment(raw: &str, gas: bool) -> String {
    let t = chars_of(raw);
    let cut = if gas {
        (0..t.len()).find(|&i| t[i] == '#' || (t[i] == '/' && t.get(i + 1) == Some(&'/')))
    } else {
        t.iter().position(|&c| c == ';')
    };
    string_of(&t[..cut.unwrap_or(t.len())])
}

pub fn assembly_issues(text: &str, dialect: &Dialect) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let mut syntax = dget(dialect, "syntax", "intel").to_string();
    let assembler = dget(dialect, "assembler", "nasm");
    let (mut seen_att, mut seen_intel) = (false, false);
    for (idx, raw) in split_lines(text).iter().enumerate() {
        let stripped_comment = strip_asm_comment(raw, assembler == "gas");
        let line = js_trim(&stripped_comment);
        if line.is_empty() {
            continue;
        }
        let lc = chars_of(line);
        if assembler == "gas" {
            if directive_word(&lc, true).as_deref() == Some("intel_syntax") {
                syntax = "intel".into();
                continue;
            }
            if directive_word(&lc, true).as_deref() == Some("att_syntax") {
                syntax = "att".into();
                continue;
            }
            if is_nasm_directive(&lc) && !is_gas_directive(&lc) {
                issues.push(issue(
                    idx + 1,
                    "ASM_DIRECTIVE_MISMATCH",
                    "NASM/MASM directive in a GAS file",
                ));
            }
        } else if assembler == "nasm" && is_gas_directive(&lc) {
            issues.push(issue(
                idx + 1,
                "ASM_DIRECTIVE_MISMATCH",
                "GAS directive in a NASM file",
            ));
        }
        let has_att = first_att_reg(&lc).is_some() || has_att_immediate(&lc);
        // line.replace(/^[\w.$]+:\s*/, '') then drop the first %reg
        let mut s = lc.clone();
        let mut lbl = 0;
        while lbl < s.len() && (is_word(s[lbl]) || s[lbl] == '.' || s[lbl] == '$') {
            lbl += 1;
        }
        if lbl > 0 && s.get(lbl) == Some(&':') {
            let after = skip_ws(&s, lbl + 1);
            s = s[after..].to_vec();
        }
        if let Some((a, b)) = first_att_reg(&s) {
            s.drain(a..b);
        }
        // stripped.replace(/^\S+\s*/, '')
        let mut m = 0;
        while m < s.len() && !is_js_space(s[m]) {
            m += 1;
        }
        let rest = &s[skip_ws(&s, m)..];
        let has_intel = has_bracket_operand(&s) || has_bare_register(rest);
        if has_att {
            seen_att = true;
        }
        if has_intel {
            seen_intel = true;
        }
        if syntax == "intel" && has_att {
            issues.push(issue(
                idx + 1,
                "ASM_SYNTAX_MIX",
                "AT&T operand (%reg or $imm) in Intel syntax",
            ));
        }
        if syntax == "att" && has_intel {
            issues.push(issue(
                idx + 1,
                "ASM_SYNTAX_MIX",
                "Intel operand (bare register or [mem]) in AT&T syntax",
            ));
        }
    }
    if seen_att && seen_intel && issues.is_empty() {
        issues.push(issue(
            0,
            "ASM_SYNTAX_MIX",
            "Intel and AT&T syntax mixed in one file",
        ));
    }
    issues
}

// ---------------------------------------------------------------- machine code hex

pub fn hex_issues(text: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (idx, line) in split_lines(text).iter().enumerate() {
        let c = chars_of(line);
        let cut =
            (0..c.len()).find(|&i| c[i] == ';' || (c[i] == '/' && c.get(i + 1) == Some(&'/')));
        let kept = string_of(&c[..cut.unwrap_or(c.len())]);
        let t = js_trim(&kept);
        if t.is_empty() {
            continue;
        }
        // remove every 0x/0X, then white space and commas
        let tc = chars_of(t);
        let mut compact = String::new();
        let mut i = 0;
        while i < tc.len() {
            if tc[i] == '0' && matches!(tc.get(i + 1), Some('x' | 'X')) {
                i += 2;
            } else {
                if !(is_js_space(tc[i]) || tc[i] == ',') {
                    compact.push(tc[i]);
                }
                i += 1;
            }
        }
        if compact.is_empty() || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
            issues.push(issue(idx + 1, "INVALID_HEX", "non-hex character"));
        } else if !compact.len().is_multiple_of(2) {
            issues.push(issue(idx + 1, "ODD_HEX_LENGTH", "incomplete byte"));
        }
    }
    issues
}

// ---------------------------------------------------------------- entry point

pub fn validate_source(profile: &str, text: &str, dialect: &Dialect) -> ValidationResult {
    let issues = match profile {
        "rust" => bracket_issues(&mask_source(text, Style::Rust)),
        "python" => python_issues(text),
        "typescript" => ts_issues(text, dialect),
        "powershell" => powershell_issues(text, dialect),
        "c" => c_issues(text, dialect),
        "assembly" => assembly_issues(text, dialect),
        "machine-code" => hex_issues(text),
        _ => {
            return ValidationResult {
                ok: false,
                level: "unchecked",
                issues: vec![issue(0, "NO_VALIDATOR", "no validator for profile")],
            };
        }
    };
    ValidationResult {
        ok: issues.is_empty(),
        level: if issues.is_empty() {
            "valid"
        } else {
            "invalid"
        },
        issues,
    }
}

/// Result for a request whose `text` is not a string.
pub fn not_text() -> ValidationResult {
    ValidationResult {
        ok: false,
        level: "invalid",
        issues: vec![issue(0, "NOT_TEXT", "text expected")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(profile: &str, text: &str, d: &[(&str, &str)]) -> Vec<&'static str> {
        let dialect: Dialect = d
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        validate_source(profile, text, &dialect)
            .issues
            .iter()
            .map(|i| i.code)
            .collect()
    }

    #[test]
    fn masking_hides_strings_and_comments() {
        assert!(codes("rust", "fn main() {\n    println!(\"}\");\n}\n", &[]).is_empty());
        assert_eq!(
            codes("rust", "fn main() {\n", &[]),
            vec!["BRACKET_UNCLOSED"]
        );
        assert!(codes("rust", "let s = r#\"raw } text\"#;", &[]).is_empty());
        assert!(codes("rust", "/* a /* nested */ still */ fn f() {}", &[]).is_empty());
        assert_eq!(mask_source("'a' 'b", Style::Rust), "    'b");
    }

    #[test]
    fn python_indentation_rules() {
        assert!(codes("python", "def f():\n    pass\n", &[]).is_empty());
        assert_eq!(codes("python", "def f():\n", &[]), vec!["EXPECTED_INDENT"]);
        assert_eq!(
            codes("python", "if x:\n    y = 1\n  z = 2\n", &[]),
            vec!["UNINDENT_MISMATCH"]
        );
    }

    #[test]
    fn dialect_gated_features() {
        assert_eq!(
            codes("powershell", "$a ?? 1", &[("version", "5.1")]),
            vec!["PS7_NULL_COALESCING"]
        );
        assert!(codes("powershell", "$a ?? 1", &[("version", "7")]).is_empty());
        assert_eq!(
            codes("c", "constexpr int a = 1;", &[("std", "c11")]),
            vec!["C23_CONSTEXPR"]
        );
        assert!(codes("c", "constexpr int a = 1;", &[("std", "c23")]).is_empty());
        assert_eq!(
            codes(
                "typescript",
                "const f = <T>(v: T) => v;",
                &[("mode", "tsx")]
            ),
            vec!["TSX_GENERIC_AMBIGUOUS"]
        );
    }

    #[test]
    fn assembly_syntax_mix_and_hex() {
        assert_eq!(
            codes("assembly", "movq %rbx, %rax\n", &[("syntax", "intel")]),
            vec!["ASM_SYNTAX_MIX"]
        );
        assert!(
            codes(
                "assembly",
                "movq %rbx, %rax\n",
                &[("syntax", "att"), ("assembler", "gas")]
            )
            .is_empty()
        );
        assert_eq!(codes("machine-code", "GG", &[]), vec!["INVALID_HEX"]);
        assert_eq!(codes("machine-code", "90 C", &[]), vec!["ODD_HEX_LENGTH"]);
        assert!(codes("machine-code", "0x90, 0xC3\n// comment\n48 89 d8", &[]).is_empty());
    }

    #[test]
    fn unknown_profile_has_no_validator() {
        assert_eq!(
            validate_source("cobol", "x", &Dialect::new()).issues[0].code,
            "NO_VALIDATOR"
        );
        assert_eq!(not_text().issues[0].code, "NOT_TEXT");
    }
}
