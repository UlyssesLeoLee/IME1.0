//! Helpers that reproduce the JavaScript string semantics the reference engine relies on
//! (`\w`, `\s`, `trim`, UTF-16 lengths, `split(/\r\n|\r|\n/)`, `\b`).
//! Everything here is pure and allocation-light; nothing touches the OS.

/// JavaScript `\w` without the `u` flag: ASCII letters, digits and underscore.
pub fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JavaScript `\s` / `String.prototype.trim` white space (WhiteSpace + LineTerminator).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

/// Length in UTF-16 code units (what `String.prototype.length` reports).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The last `n` UTF-16 code units (`slice(-n)`). A surrogate pair that would be cut in half is dropped
/// completely, because a Rust `str` cannot hold a lone surrogate.
pub fn tail_utf16(s: &str, n: usize) -> &str {
    let mut units = 0usize;
    let mut cut = s.len();
    for (idx, c) in s.char_indices().rev() {
        let l = c.len_utf16();
        if units + l > n {
            break;
        }
        units += l;
        cut = idx;
    }
    &s[cut..]
}

/// The first `n` UTF-16 code units (`slice(0, n)`), never splitting a code point.
pub fn head_utf16(s: &str, n: usize) -> &str {
    let mut units = 0usize;
    for (idx, c) in s.char_indices() {
        let l = c.len_utf16();
        if units + l > n {
            return &s[..idx];
        }
        units += l;
    }
    s
}

/// `text.split(/\r\n|\r|\n/)`.
pub fn split_lines(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'\n' {
            out.push(&s[start..i]);
            i += 1;
            start = i;
        } else if b[i] == b'\r' {
            out.push(&s[start..i]);
            i += if i + 1 < b.len() && b[i + 1] == b'\n' {
                2
            } else {
                1
            };
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&s[start..]);
    out
}

pub fn chars_of(s: &str) -> Vec<char> {
    s.chars().collect()
}

pub fn string_of(t: &[char]) -> String {
    t.iter().collect()
}

/// Regex `\b` at index `q` of `t` (between `t[q-1]` and `t[q]`, ASCII word characters).
pub fn boundary(t: &[char], q: usize) -> bool {
    let before = q > 0 && q <= t.len() && is_word(t[q - 1]);
    let after = q < t.len() && is_word(t[q]);
    before != after
}

pub fn skip_ws(t: &[char], mut i: usize) -> usize {
    while i < t.len() && is_js_space(t[i]) {
        i += 1;
    }
    i
}

/// `t[i..]` starts with the ASCII/Unicode literal `pat` (case-sensitive).
pub fn starts_at(t: &[char], i: usize, pat: &str) -> bool {
    for (k, p) in (i..).zip(pat.chars()) {
        if k >= t.len() || t[k] != p {
            return false;
        }
    }
    true
}

/// ASCII case-insensitive variant of [`starts_at`] (`pat` must be lower-case ASCII).
pub fn starts_at_ci(t: &[char], i: usize, pat: &str) -> bool {
    for (k, p) in (i..).zip(pat.chars()) {
        if k >= t.len() || t[k].to_ascii_lowercase() != p {
            return false;
        }
    }
    true
}

/// `indexOf(pat, from)` over a char slice.
pub fn find_seq(t: &[char], pat: &[char], from: usize) -> Option<usize> {
    if pat.is_empty() {
        return Some(from.min(t.len()));
    }
    if t.len() < pat.len() {
        return None;
    }
    let last = t.len() - pat.len();
    let mut i = from;
    while i <= last {
        if t[i..i + pat.len()] == *pat {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `lastIndexOf(c, from)` over a char slice (`from` inclusive).
pub fn rfind_char(t: &[char], c: char, from: usize) -> Option<usize> {
    if t.is_empty() {
        return None;
    }
    let mut i = from.min(t.len() - 1) as isize;
    while i >= 0 {
        if t[i as usize] == c {
            return Some(i as usize);
        }
        i -= 1;
    }
    None
}

/// The word (`\w+`) run that ends exactly at the end of `t`, if any.
pub fn trailing_word(t: &[char]) -> &[char] {
    let mut s = t.len();
    while s > 0 && is_word(t[s - 1]) {
        s -= 1;
    }
    &t[s..]
}

/// `/\bword$/` for a literal lower/upper-case `word` (case-sensitive).
pub fn ends_with_word(t: &[char], word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    if t.len() < w.len() || t[t.len() - w.len()..] != w[..] {
        return false;
    }
    boundary(t, t.len() - w.len())
}

/// Whether `t` contains the whole word `word` (`/\bword\b/`).
pub fn contains_word(t: &[char], word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    let mut from = 0usize;
    while let Some(i) = find_seq(t, &w, from) {
        if boundary(t, i) && boundary(t, i + w.len()) {
            return true;
        }
        from = i + 1;
    }
    false
}

/// ASCII-only variant of JavaScript `localeCompare` for identifiers (`[A-Za-z0-9_$]`):
/// punctuation (`_` < `$`) sorts before digits, digits before letters, letters case-insensitively first
/// and lower case before upper case on a tie (ICU root collation order used by V8).
pub fn locale_compare_ascii(a: &str, b: &str) -> std::cmp::Ordering {
    fn primary(c: char) -> (u8, u32) {
        match c {
            '_' => (0, 0),
            '$' => (0, 1),
            '0'..='9' => (1, c as u32),
            _ if c.is_ascii_alphabetic() => (2, c.to_ascii_lowercase() as u32),
            _ => (3, c as u32),
        }
    }
    let pa: Vec<(u8, u32)> = a.chars().map(primary).collect();
    let pb: Vec<(u8, u32)> = b.chars().map(primary).collect();
    pa.cmp(&pb).then_with(|| {
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                // lower case before upper case
                return if x.is_ascii_lowercase() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
        }
        std::cmp::Ordering::Equal
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_helpers_do_not_split_pairs() {
        let s = "a\u{1F600}b";
        assert_eq!(utf16_len(s), 4);
        assert_eq!(tail_utf16(s, 2), "b");
        assert_eq!(tail_utf16(s, 3), "\u{1F600}b");
        assert_eq!(head_utf16(s, 2), "a");
        assert_eq!(head_utf16(s, 3), "a\u{1F600}");
    }

    #[test]
    fn split_lines_matches_js() {
        assert_eq!(split_lines("a\r\nb\rc\nd"), vec!["a", "b", "c", "d"]);
        assert_eq!(split_lines(""), vec![""]);
        assert_eq!(split_lines("a\n"), vec!["a", ""]);
    }

    #[test]
    fn boundary_and_words() {
        let t = chars_of("fn main");
        assert!(contains_word(&t, "fn"));
        assert!(!contains_word(&chars_of("fnx"), "fn"));
        assert!(ends_with_word(&chars_of("x return"), "return"));
        assert!(!ends_with_word(&chars_of("xreturn"), "return"));
    }

    #[test]
    fn locale_compare_orders_like_icu() {
        use std::cmp::Ordering::*;
        assert_eq!(locale_compare_ascii("a", "B"), Less);
        assert_eq!(locale_compare_ascii("a", "A"), Less);
        assert_eq!(locale_compare_ascii("_x", "x"), Less);
        assert_eq!(locale_compare_ascii("a1", "ab"), Less);
    }
}
