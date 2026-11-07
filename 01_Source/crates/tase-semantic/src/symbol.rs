//! The identity of a symbol, the symbol itself, and the source view the language adapters share.
//!
//! The **semantic key** ([`SymbolIdentity::symbol_key`]) is the stable name of a symbol inside a repository: the path,
//! the container, the kind and the name. The detailed design (§4.1) calls it the "symbol locator" and asks for the
//! path, the qualified name, the kind and two digests; the digests belong to the executor (they need a hash of the
//! declaration and of its context), so this crate keeps the part that identifies a symbol and stays stable while the
//! file moves around.
//!
//! The **source view** ([`SourceView`]) is the shared scaffolding of the language adapters: it masks the comments and
//! the string literals (so that a declaration inside a comment is not a declaration), it keeps the line table and it
//! matches the braces. It is deliberately **not** an AST parser: see the crate documentation.

use crate::position::Range;
use pime_core::validators::{Style, mask_source};
use std::fmt;

/// What a symbol is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A function of a type: a method of an `impl` or of a trait, a method of a class.
    Method,
    /// A type alias or any other type declaration without a more precise kind.
    Type,
    /// A structure (or a union).
    Struct,
    /// An enumeration.
    Enum,
    /// A trait.
    Trait,
    /// A module or a namespace.
    Module,
    /// A constant (or a static).
    Const,
    /// A variable.
    Variable,
    /// An interface.
    Interface,
    /// A class.
    Class,
    /// A test function.
    Test,
    /// A declaration this crate does not know how to name.
    Unknown,
}

impl SymbolKind {
    /// The stable lower-case name of the kind (the wire name).
    pub const fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Type => "type",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "module",
            SymbolKind::Const => "const",
            SymbolKind::Variable => "variable",
            SymbolKind::Interface => "interface",
            SymbolKind::Class => "class",
            SymbolKind::Test => "test",
            SymbolKind::Unknown => "unknown",
        }
    }

    /// The kind of a wire name, or [`SymbolKind::Unknown`] when the name is not known.
    pub fn parse(name: &str) -> SymbolKind {
        match name {
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "type" => SymbolKind::Type,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "module" => SymbolKind::Module,
            "const" => SymbolKind::Const,
            "variable" => SymbolKind::Variable,
            "interface" => SymbolKind::Interface,
            "class" => SymbolKind::Class,
            "test" => SymbolKind::Test,
            _ => SymbolKind::Unknown,
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identity of a symbol: what makes it the same symbol and not another one.
///
/// The identity never holds an offset: a symbol keeps its identity when the file moves, when the line moves and when
/// the text around it changes. Only the [`Symbol`] holds a range, and the range is only valid for the text it was
/// computed from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolIdentity {
    /// The path of the file, relative to the repository and normalised with `/`.
    pub path: String,
    /// What the symbol is.
    pub kind: SymbolKind,
    /// The name of the symbol.
    pub name: String,
    /// The container of the symbol: the module, the type or the trait that holds it.
    pub container: Option<String>,
}

impl SymbolIdentity {
    /// An identity from its four parts.
    pub fn new(
        path: impl Into<String>,
        kind: SymbolKind,
        name: impl Into<String>,
        container: Option<String>,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            name: name.into(),
            container,
        }
    }

    /// The **semantic key**: the unique and stable name of the symbol inside the repository.
    ///
    /// The key is `path#container#kind#name`. The path comes first, the container then, the kind and the name last.
    /// The kind is part of the key because an `impl Foo` and a `struct Foo` share the path, the container and the name:
    /// without the kind the two would collide. The key holds no offset and no line, so it survives a move of the file.
    pub fn symbol_key(&self) -> String {
        format!(
            "{}#{}#{}#{}",
            self.path,
            self.container.as_deref().unwrap_or(""),
            self.kind.as_str(),
            self.name
        )
    }

    /// The qualified name of the symbol: `container::name`, or the name alone when there is no container.
    pub fn qualified_name(&self) -> String {
        match &self.container {
            Some(container) if !container.is_empty() => format!("{container}::{}", self.name),
            _ => self.name.clone(),
        }
    }
}

impl fmt::Display for SymbolIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.symbol_key())
    }
}

/// A symbol of a file: its identity, where it is, and what it looks like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// What makes it the same symbol and not another one.
    pub identity: SymbolIdentity,
    /// Where the declaration is, in UTF-16 offsets of the text it was indexed from.
    pub range: Range,
    /// The declaration header (`pub fn run(x: u32) -> bool`), when the adapter could read it.
    pub signature: Option<String>,
    /// The documentation comment above the declaration, when there is one.
    pub doc: Option<String>,
}

impl Symbol {
    /// The key of the symbol.
    pub fn symbol_key(&self) -> String {
        self.identity.symbol_key()
    }
}

/// An identifier token of a masked source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    /// The name of the token (without the raw prefix of Rust).
    pub name: String,
    /// The char index of the first char of the token.
    pub start: u32,
    /// The char index just after the last char of the token.
    pub end: u32,
}

/// The shared view of a source file: the masked text, the line table and the braces.
///
/// The masking keeps the length of the text (the comments and the string literals become blanks) so that every char
/// index of the masked text is the char index of the original text.
pub(crate) struct SourceView<'a> {
    /// The original text.
    pub text: &'a str,
    /// The masked text: the comments and the string literals are blanks, the newlines are kept.
    pub chars: Vec<char>,
    /// The UTF-16 offset of every char, plus a last entry for the end of the text.
    pub u16_at_char: Vec<u32>,
    /// The char index of the first char of every line.
    pub line_start: Vec<u32>,
    /// The char index just after the last char of the content of every line.
    pub line_end: Vec<u32>,
    /// The char index of the `}` that closes the `{` of a char index, or `-1`.
    pub brace_close: Vec<i64>,
    /// The UTF-8 byte offset of every char of the **original** text, plus a last entry.
    byte_at_char: Vec<u32>,
}

impl<'a> SourceView<'a> {
    /// Builds the view of a text with the masking of a language.
    pub fn new(text: &'a str, style: Style) -> Self {
        let masked = mask_source(text, style);
        let chars: Vec<char> = masked.chars().collect();
        // The masking keeps the number of chars but not their width: an emoji in a comment becomes a blank of one
        // UTF-16 unit. The offsets are therefore taken from the **original** text, whose chars are the same ones.
        let mut u16_at_char: Vec<u32> = Vec::with_capacity(chars.len() + 1);
        let mut u16_pos: u32 = 0;
        for c in text.chars() {
            u16_at_char.push(u16_pos);
            u16_pos += c.len_utf16() as u32;
        }
        u16_at_char.push(u16_pos);

        let mut line_start: Vec<u32> = vec![0];
        let mut line_end: Vec<u32> = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if c == '\r' || c == '\n' {
                line_end.push(i as u32);
                let mut next = i + 1;
                if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                    next = i + 2;
                }
                line_start.push(next as u32);
                i = next;
            } else {
                i += 1;
            }
        }
        line_end.push(chars.len() as u32);

        let mut brace_close: Vec<i64> = vec![-1; chars.len()];
        let mut stack: Vec<u32> = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            if c == '{' {
                stack.push(i as u32);
            } else if c == '}' && let Some(open) = stack.pop() {
                brace_close[open as usize] = i as i64;
            }
        }

        let mut byte_at_char: Vec<u32> = Vec::with_capacity(chars.len() + 1);
        let mut byte_pos = 0u32;
        for c in text.chars() {
            byte_at_char.push(byte_pos);
            byte_pos += c.len_utf8() as u32;
        }
        byte_at_char.push(byte_pos);

        Self {
            text,
            chars,
            u16_at_char,
            line_start,
            line_end,
            brace_close,
            byte_at_char,
        }
    }

    /// The number of lines.
    pub fn line_count(&self) -> usize {
        self.line_start.len()
    }

    /// The chars of the content of a line (the end of line is not part of it).
    pub fn line_chars(&self, line: usize) -> &[char] {
        let start = self.line_start[line] as usize;
        let end = self.line_end[line] as usize;
        &self.chars[start..end]
    }

    /// The content of a line of the **original** text: the comments and the strings are not masked here.
    pub fn original_line(&self, line: usize) -> &str {
        let start = self.byte_at_char[self.line_start[line] as usize] as usize;
        let end = self.byte_at_char[self.line_end[line] as usize] as usize;
        &self.text[start..end]
    }

    /// The char index of the first char of a line.
    pub fn line_start(&self, line: usize) -> u32 {
        self.line_start[line]
    }

    /// The UTF-16 offset of a char index.
    pub fn u16(&self, char_index: u32) -> u32 {
        self.u16_at_char[(char_index as usize).min(self.u16_at_char.len() - 1)]
    }

    /// A range of UTF-16 offsets from a char range.
    pub fn range(&self, start_char: u32, end_char: u32) -> Range {
        Range::new(self.u16(start_char), self.u16(end_char))
    }

    /// The char index of the `}` that closes the `{` of a char index.
    pub fn close_of(&self, char_index: u32) -> Option<u32> {
        let close = self.brace_close.get(char_index as usize).copied().unwrap_or(-1);
        if close < 0 {
            None
        } else {
            Some(close as u32)
        }
    }

    /// The identifier tokens of the masked text.
    ///
    /// `allow_dollar` accepts the `$` of JavaScript identifiers, `raw_prefix` drops the `r#` of a Rust raw identifier.
    pub fn tokens(&self, allow_dollar: bool, raw_prefix: bool) -> Vec<Token> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < self.chars.len() {
            let c = self.chars[i];
            if !is_ident_start(c, allow_dollar) {
                i += 1;
                continue;
            }
            let start = i;
            while i < self.chars.len() && is_ident_continue(self.chars[i], allow_dollar) {
                i += 1;
            }
            let mut text: String = self.chars[start..i].iter().collect();
            if raw_prefix && text.starts_with("r#") && text.len() > 2 {
                text.remove(0);
                text.remove(0);
            }
            out.push(Token {
                name: text,
                start: start as u32,
                end: i as u32,
            });
        }
        out
    }
}

/// Whether a char can start an identifier.
pub(crate) fn is_ident_start(c: char, allow_dollar: bool) -> bool {
    c == '_' || c.is_alphabetic() || (allow_dollar && c == '$')
}

/// Whether a char can continue an identifier.
pub(crate) fn is_ident_continue(c: char, allow_dollar: bool) -> bool {
    c == '_' || c.is_alphanumeric() || (allow_dollar && c == '$')
}

/// The normalised text of a declaration header: the blanks are collapsed into one.
pub(crate) fn normalise_signature(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out.trim().to_string()
}

/// Where the header of a declaration ends inside a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeaderKind {
    /// The body of the item opens with a `{`.
    Brace,
    /// The declaration ends with a `;`.
    Semi,
}

/// Finds where the header of a declaration ends: the `{` of the body or the `;` of the declaration.
///
/// The index is relative to the line. The brackets that are open are not counted: a header that spans several lines
/// is read again by the caller, line by line.
pub(crate) fn find_header_end(line: &[char], from: usize) -> Option<(HeaderKind, usize)> {
    let mut depth = 0i32;
    for (at, &c) in line.iter().enumerate().skip(from) {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' if depth <= 0 => return Some((HeaderKind::Brace, at)),
            ';' if depth <= 0 => return Some((HeaderKind::Semi, at)),
            _ => {}
        }
    }
    None
}

/// The index of the first char that is not a blank, starting at an index.
pub(crate) fn leading_blanks(line: &[char], from: usize) -> usize {
    line[from..]
        .iter()
        .take_while(|c| c.is_whitespace())
        .count()
        + from
}

/// The word that starts at an index, when it is an identifier.
pub(crate) fn word_at(line: &[char], at: usize) -> Option<String> {
    let mut end = at;
    while end < line.len() && (line[end].is_alphanumeric() || line[end] == '_') {
        end += 1;
    }
    if end == at {
        return None;
    }
    Some(line[at..end].iter().collect())
}

/// Skips a balanced pair of brackets; returns the index just after the closing bracket.
pub(crate) fn skip_balanced(line: &[char], at: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, &c) in line[at..].iter().enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + offset + 1);
            }
        }
    }
    None
}

/// Skips a generic list `<...>`; returns the index just after the closing angle bracket.
pub(crate) fn skip_generic(line: &[char], at: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, &c) in line[at..].iter().enumerate() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Skips a string literal; returns the index just after the closing quote.
pub(crate) fn skip_string(line: &[char], at: usize) -> usize {
    let mut i = at + 1;
    while i < line.len() {
        if line[i] == '\\' {
            i += 2;
            continue;
        }
        if line[i] == '"' {
            return i + 1;
        }
        i += 1;
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symbol_key_is_unique_and_stable() {
        let free = SymbolIdentity::new("src/a.rs", SymbolKind::Function, "run", None);
        let method = SymbolIdentity::new("src/a.rs", SymbolKind::Method, "run", Some("Point".into()));
        let other = SymbolIdentity::new("src/b.rs", SymbolKind::Function, "run", None);
        assert_ne!(free.symbol_key(), method.symbol_key());
        assert_ne!(free.symbol_key(), other.symbol_key());
        assert_eq!(free.symbol_key(), free.symbol_key());
        assert_eq!(free.symbol_key(), SymbolIdentity::new("src/a.rs", SymbolKind::Function, "run", None).symbol_key());
    }

    #[test]
    fn an_impl_and_a_struct_of_the_same_name_do_not_collide() {
        let structure = SymbolIdentity::new("src/a.rs", SymbolKind::Struct, "Point", None);
        let implementation = SymbolIdentity::new("src/a.rs", SymbolKind::Type, "Point", None);
        assert_ne!(structure.symbol_key(), implementation.symbol_key());
    }

    #[test]
    fn the_qualified_name_holds_the_container() {
        let free = SymbolIdentity::new("src/a.rs", SymbolKind::Function, "run", None);
        let method = SymbolIdentity::new("src/a.rs", SymbolKind::Method, "run", Some("Point".into()));
        assert_eq!(free.qualified_name(), "run");
        assert_eq!(method.qualified_name(), "Point::run");
    }

    #[test]
    fn the_kind_has_a_stable_wire_name() {
        assert_eq!(SymbolKind::Function.as_str(), "function");
        assert_eq!(SymbolKind::parse("class"), SymbolKind::Class);
        assert_eq!(SymbolKind::parse("nope"), SymbolKind::Unknown);
        for kind in [
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Type,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Trait,
            SymbolKind::Module,
            SymbolKind::Const,
            SymbolKind::Variable,
            SymbolKind::Interface,
            SymbolKind::Class,
            SymbolKind::Test,
            SymbolKind::Unknown,
        ] {
            assert_eq!(SymbolKind::parse(kind.as_str()), kind);
        }
    }

    #[test]
    fn the_view_masks_the_comments_and_the_strings() {
        let text = "// fn hidden() {}\nfn shown() { let s = \"fn fake()\"; }\n";
        let view = SourceView::new(text, Style::Rust);
        let tokens = view.tokens(false, false);
        let names: Vec<&str> = tokens.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"hidden"));
        assert!(!names.contains(&"fake"));
        assert!(names.contains(&"shown"));
        assert_eq!(view.line_count(), 3);
    }

    #[test]
    fn the_view_matches_the_braces() {
        let text = "fn a() { if true { } }\nfn b() {}\n";
        let view = SourceView::new(text, Style::Rust);
        let opens: Vec<u32> = text
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '{')
            .map(|(at, _)| at as u32)
            .collect();
        assert_eq!(opens.len(), 3);
        assert_eq!(view.close_of(opens[0]), Some(21));
        assert_eq!(view.close_of(opens[1]), Some(19));
        assert_eq!(view.close_of(opens[2]), Some(31));
        assert_eq!(view.close_of(0), None);
    }

    #[test]
    fn the_view_keeps_the_utf16_offsets() {
        let text = "\u{6c49} fn a() {}\n";
        let view = SourceView::new(text, Style::Rust);
        let tokens = view.tokens(false, false);
        let token = tokens.iter().find(|t| t.name == "fn");
        assert!(token.is_some());
        let token = token.expect("a token");
        assert_eq!(view.u16(token.start), 2);
    }

    #[test]
    fn the_signature_is_normalised() {
        assert_eq!(normalise_signature("pub  fn  run( x : u32 ) -> bool"), "pub fn run( x : u32 ) -> bool");
        assert_eq!(normalise_signature("  pub fn run()  "), "pub fn run()");
    }
}
