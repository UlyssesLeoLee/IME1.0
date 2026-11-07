//! The Rust adapter: the declarations of a file, read by a **structured line-level scan**.
//!
//! The scan is not an AST parser: it walks the lines, it knows the keywords of Rust, it follows the braces and it
//! stops at the first thing it does not understand. What it reads:
//!
//! | declaration | kind | container |
//! |-------------|------|-----------|
//! | `fn` | `function`, or `method` inside an `impl` or a `trait`, or `test` with `#[test]` | the module, the type or the trait |
//! | `struct`, `union` | `struct` | the module |
//! | `enum` | `enum` | the module |
//! | `trait` | `trait` | the module |
//! | `impl` | `type` (the type it is for) | the module |
//! | `mod` | `module` | the module |
//! | `const`, `static` | `const` | the module |
//! | `type` | `type` | the module |
//!
//! What it does **not** read: the enum variants, the fields of a structure, the `let` bindings, the macros
//! (`macro_rules!`), the attributes of the items, the lifetimes, the generics and the `where` clauses (they are part of
//! the signature, not of the identity). A declaration it cannot read is **skipped**, never guessed.

use crate::adapter::SemanticAdapter;
use crate::position::Range;
use crate::symbol::{
    HeaderKind, SourceView, Symbol, SymbolIdentity, SymbolKind, find_header_end, leading_blanks,
    normalise_signature, skip_balanced, skip_generic, skip_string, word_at,
};
use pime_core::validators::Style;

/// The adapter of the Rust language.
pub struct RustAdapter;

impl SemanticAdapter for RustAdapter {
    fn language(&self) -> &str {
        "rust"
    }

    fn index(&self, path: &str, text: &str) -> Vec<Symbol> {
        index_rust(path, text)
    }

    fn occurrences(&self, text: &str, identity: &SymbolIdentity) -> Vec<Range> {
        occurrences_rust(text, identity)
    }
}

/// The symbols of a Rust file.
pub fn index_rust(path: &str, text: &str) -> Vec<Symbol> {
    let view = SourceView::new(text, Style::Rust);
    let mut scanner = RustScanner {
        path,
        view: &view,
        symbols: Vec::new(),
        frames: Vec::new(),
        attributes: Vec::new(),
        pending_start: None,
    };
    scanner.run();
    scanner.symbols
}

/// The places of a Rust file where the name of the symbol is written.
pub fn occurrences_rust(text: &str, identity: &SymbolIdentity) -> Vec<Range> {
    let view = SourceView::new(text, Style::Rust);
    view.tokens(false, true)
        .into_iter()
        .filter(|t| t.name == identity.name)
        .map(|t| view.range(t.start, t.end))
        .collect()
}

/// The open block of an item: the container of the items inside it.
struct Frame {
    /// The qualified path of the item that opened the block.
    path: Option<String>,
    /// The char index of the `{` that opened the block.
    open: u32,
    /// Whether the item is a type or a trait: the functions inside it are methods.
    is_type: bool,
    /// Whether the item is a test module: the functions inside it are tests.
    test_module: bool,
}

/// Where the header of a declaration ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderEnd {
    /// The body of the item opens with a `{` at this char index.
    Brace(u32),
    /// The declaration ends with a `;` at this char index.
    Semi(u32),
}

/// The scanner of a Rust file.
struct RustScanner<'a, 'b> {
    path: &'a str,
    view: &'b SourceView<'b>,
    symbols: Vec<Symbol>,
    frames: Vec<Frame>,
    attributes: Vec<String>,
    pending_start: Option<u32>,
}

impl RustScanner<'_, '_> {
    fn run(&mut self) {
        let line_count = self.view.line_count();
        let mut line = 0usize;
        while line < line_count {
            self.close_finished(self.view.line_start(line));
            let chars = self.view.line_chars(line);
            let offset = leading_blanks(chars, 0);
            let rest = &chars[offset..];
            if rest.is_empty() {
                line += 1;
                continue;
            }
            if rest[0] == '#' {
                if rest.starts_with(&['#', '!', '[']) {
                    line += 1;
                    continue;
                }
                if rest.starts_with(&['#', '[']) {
                    let text: String = rest.iter().collect();
                    self.attributes.push(text);
                    if self.pending_start.is_none() {
                        self.pending_start = Some(self.view.line_start(line));
                    }
                    line += 1;
                    continue;
                }
                self.reset_pending();
                line += 1;
                continue;
            }
            line = self.read_declaration(line, offset);
        }
    }

    /// Reads the declaration that starts on a line, and returns the line to read next.
    fn read_declaration(&mut self, line: usize, offset: usize) -> usize {
        let chars = self.view.line_chars(line);
        let Some(head) = parse_head(&chars[offset..]) else {
            self.reset_pending();
            return line + 1;
        };
        let mut prefix = String::new();
        let mut header_line = line;
        let mut end = None;
        loop {
            let line_chars = self.view.line_chars(header_line);
            let line_offset = if header_line == line { offset } else { 0 };
            let line_text: String = line_chars.iter().collect();
            let line_start = self.view.line_start(header_line);
            match find_header_end(line_chars, line_offset) {
                Some((found, at)) => {
                    let signature = if prefix.is_empty() {
                        line_text[..at].to_string()
                    } else {
                        format!("{prefix} {}", &line_text[..at])
                    };
                    let found = match found {
                        HeaderKind::Brace => HeaderEnd::Brace(line_start + at as u32),
                        HeaderKind::Semi => HeaderEnd::Semi(line_start + at as u32),
                    };
                    end = Some((found, line_start + at as u32, signature));
                    break;
                }
                None => {
                    // the current line is part of the header
                    let current = line_text.trim();
                    prefix = if prefix.is_empty() {
                        current.to_string()
                    } else {
                        format!("{prefix} {current}")
                    };
                    if header_line > line + 19 {
                        break;
                    }
                    let next = header_line + 1;
                    if next >= self.view.line_count() {
                        break;
                    }
                    let next_chars = self.view.line_chars(next);
                    let next_text: String = next_chars.iter().collect();
                    let next_trimmed = next_text.trim();
                    if next_trimmed.is_empty() || looks_like_declaration(next_trimmed) {
                        break;
                    }
                    header_line = next;
                }
            }
        }
        let Some((found, end_char, signature)) = end else {
            self.reset_pending();
            return line + 1;
        };

        let container = self.frames.last().and_then(|f| f.path.clone());
        let test_module = self.frames.last().is_some_and(|f| f.test_module);
        let in_type = self.frames.last().is_some_and(|f| f.is_type);
        let has_test_attribute = self.attributes.iter().any(|a| is_test_attribute(a));

        let kind = match head.kind {
            SymbolKind::Function => {
                if has_test_attribute || test_module {
                    SymbolKind::Test
                } else if in_type {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                }
            }
            other => other,
        };

        let start_char = self.pending_start.unwrap_or(self.view.line_start(line));
        let range = match found {
            HeaderEnd::Brace(brace) => match self.view.close_of(brace) {
                Some(close) => self.view.range(start_char, close + 1),
                None => self.view.range(start_char, end_char),
            },
            HeaderEnd::Semi(semi) => self.view.range(start_char, semi + 1),
        };

        let doc = self.doc_above(line);
        self.symbols.push(Symbol {
            identity: SymbolIdentity {
                path: self.path.to_string(),
                kind,
                name: head.name.clone(),
                container: container.clone(),
            },
            range,
            signature: Some(normalise_signature(&signature)),
            doc,
        });

        if let HeaderEnd::Brace(brace) = found {
            let path = join_path(&container, &head.name);
            self.frames.push(Frame {
                path,
                open: brace,
                is_type: matches!(head.kind, SymbolKind::Type | SymbolKind::Trait),
                test_module: head.kind == SymbolKind::Module
                    && self.attributes.iter().any(|a| a.contains("cfg(test)")),
            });
        }
        self.reset_pending();
        header_line + 1
    }

    /// The documentation comment above a declaration.
    fn doc_above(&self, line: usize) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut i = line;
        while i > 0 {
            let text = self.view.original_line(i - 1);
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("///") {
                lines.push(rest.trim().to_string());
                i -= 1;
                continue;
            }
            if trimmed.starts_with("#[") {
                i -= 1;
                continue;
            }
            break;
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(lines.join("\n"))
    }

    /// Pops the blocks that closed before a char index.
    fn close_finished(&mut self, before: u32) {
        while let Some(frame) = self.frames.last() {
            match self.view.close_of(frame.open) {
                Some(close) if close < before => {
                    self.frames.pop();
                }
                _ => break,
            }
        }
    }

    /// Forgets the attributes of a declaration that was not read.
    fn reset_pending(&mut self) {
        self.attributes.clear();
        self.pending_start = None;
    }
}

/// The head of a declaration: the keyword, the name and the kind.
#[derive(Clone, Debug)]
struct Head {
    kind: SymbolKind,
    name: String,
}

/// Reads the head of a declaration: the visibility, the modifiers, the keyword and the name.
fn parse_head(line: &[char]) -> Option<Head> {
    let mut i = leading_blanks(line, 0);
    if word_at(line, i).as_deref() == Some("pub") {
        i = leading_blanks(line, i + 3);
        if line.get(i) == Some(&'(') {
            i = skip_balanced(line, i, '(', ')')?;
        }
        i = leading_blanks(line, i);
    }
    while let Some(word) = word_at(line, i) {
        match word.as_str() {
            "async" | "unsafe" | "default" => {
                i = leading_blanks(line, i + word.len());
            }
            "extern" => {
                i = leading_blanks(line, i + word.len());
                if line.get(i) == Some(&'"') {
                    i = skip_string(line, i);
                }
                i = leading_blanks(line, i);
            }
            _ => break,
        }
    }
    let word = word_at(line, i)?;
    match word.as_str() {
        "fn" => {
            let (name, _) = read_name(line, i + 2)?;
            Some(Head {
                kind: SymbolKind::Function,
                name,
            })
        }
        "struct" => Some(Head {
            kind: SymbolKind::Struct,
            name: read_name(line, i + 6)?.0,
        }),
        "enum" => Some(Head {
            kind: SymbolKind::Enum,
            name: read_name(line, i + 4)?.0,
        }),
        "union" => Some(Head {
            kind: SymbolKind::Struct,
            name: read_name(line, i + 5)?.0,
        }),
        "trait" => Some(Head {
            kind: SymbolKind::Trait,
            name: read_name(line, i + 5)?.0,
        }),
        "mod" => {
            let (name, at) = read_name(line, i + 3)?;
            // `mod a::b;` is a path, not a declaration of this file: it is not read.
            if line.get(at) == Some(&':') {
                return None;
            }
            Some(Head {
                kind: SymbolKind::Module,
                name,
            })
        }
        "type" => Some(Head {
            kind: SymbolKind::Type,
            name: read_name(line, i + 4)?.0,
        }),
        "const" => {
            let after = leading_blanks(line, i + 5);
            if word_at(line, after).as_deref() == Some("fn") {
                let (name, _) = read_name(line, after + 2)?;
                return Some(Head {
                    kind: SymbolKind::Function,
                    name,
                });
            }
            Some(Head {
                kind: SymbolKind::Const,
                name: read_name(line, after)?.0,
            })
        }
        "static" => {
            let after = leading_blanks(line, i + 6);
            let after = if word_at(line, after).as_deref() == Some("mut") {
                leading_blanks(line, after + 3)
            } else {
                after
            };
            Some(Head {
                kind: SymbolKind::Const,
                name: read_name(line, after)?.0,
            })
        }
        "impl" => {
            let after = leading_blanks(line, i + 4);
            let after = if line.get(after) == Some(&'<') {
                let end = skip_generic(line, after)?;
                leading_blanks(line, end)
            } else {
                after
            };
            let rest: String = line[after..].iter().collect();
            let name = impl_type_name(&rest)?;
            Some(Head {
                kind: SymbolKind::Type,
                name,
            })
        }
        _ => None,
    }
}

/// The type an `impl` block is for: the self type of `impl Trait for Type`.
fn impl_type_name(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let mut tail = rest;
    for (at, _) in rest.match_indices(" for ") {
        let after = &rest[at + 5..];
        if !after.trim().is_empty() {
            tail = after;
        }
    }
    type_name_of(tail)
}

/// The name of a type: the last segment of its path, without the generics and the lifetimes.
fn type_name_of(text: &str) -> Option<String> {
    let mut t = text.trim();
    while let Some(rest) = t.strip_prefix('&') {
        t = rest.trim_start();
        if let Some(rest) = t.strip_prefix("mut ") {
            t = rest.trim_start();
        }
    }
    t = t.strip_prefix("*").unwrap_or(t).trim_start();
    if t.starts_with('\'') {
        let mut chars = t.char_indices();
        chars.next();
        if let Some((_, c)) = chars.next() {
            let after = &t[1 + c.len_utf8()..];
            t = after.trim_start();
        }
    }
    if let Some(rest) = t.strip_prefix("mut ") {
        t = rest.trim_start();
    }
    if let Some(rest) = t.strip_prefix("dyn ") {
        t = rest.trim_start();
    }
    let mut depth = 0i32;
    let mut end = t.len();
    for (at, c) in t.char_indices() {
        match c {
            '<' => {
                if depth == 0 {
                    end = at;
                    break;
                }
                depth += 1;
            }
            '>' => {
                if depth == 0 {
                    end = at;
                    break;
                }
                depth -= 1;
            }
            '(' | ',' | '{' | ' ' | ';' if depth == 0 => {
                end = at;
                break;
            }
            _ => {}
        }
    }
    let head = t[..end].trim();
    let last = head.rsplit("::").next().unwrap_or(head).trim();
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

/// Whether a line starts a declaration (used to stop the reading of a header that spans several lines).
fn looks_like_declaration(text: &str) -> bool {
    let text = text.trim_start();
    for keyword in [
        "fn ", "pub ", "struct ", "enum ", "trait ", "impl ", "mod ", "const ", "static ", "type ",
    ] {
        if text.starts_with(keyword) {
            return true;
        }
    }
    false
}

/// Whether an attribute marks a test.
fn is_test_attribute(attribute: &str) -> bool {
    let inner = attribute.trim().trim_start_matches('#').trim_start_matches('!');
    let inner = inner.trim_start_matches('[');
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner
        .split_whitespace()
        .any(|w| w == "test" || w.ends_with("::test"))
}

/// The name that follows a keyword: the identifier and the index just after it.
///
/// A raw identifier (`r#match`) gives the name it carries (`match`).
fn read_name(line: &[char], at: usize) -> Option<(String, usize)> {
    let mut at = leading_blanks(line, at);
    if line.get(at) == Some(&'r') && line.get(at + 1) == Some(&'#') {
        at += 2;
    }
    let mut end = at;
    while end < line.len() && (line[end].is_alphanumeric() || line[end] == '_') {
        end += 1;
    }
    if end == at {
        return None;
    }
    Some((line[at..end].iter().collect(), end))
}

/// The path of an item inside a container.
fn join_path(prefix: &Option<String>, name: &str) -> Option<String> {
    match prefix {
        Some(prefix) => Some(format!("{prefix}::{name}")),
        None => Some(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|s| format!("{}:{}", s.identity.kind.as_str(), s.identity.name))
            .collect()
    }

    fn containers(symbols: &[Symbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|s| {
                format!(
                    "{}::{}",
                    s.identity.container.as_deref().unwrap_or(""),
                    s.identity.name
                )
            })
            .collect()
    }

    #[test]
    fn the_declarations_of_a_file_are_read() {
        let text = "\
pub struct Point { x: u32 }

pub enum Shape { Circle, Square }

pub trait Draw {
    fn draw(&self);
}

impl Point {
    pub fn new() -> Self { Point { x: 0 } }
}

pub const MAX: u32 = 10;

pub fn run() -> bool { true }
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(
            names(&symbols),
            [
                "struct:Point",
                "enum:Shape",
                "trait:Draw",
                "method:draw",
                "type:Point",
                "method:new",
                "const:MAX",
                "function:run"
            ]
        );
    }

    #[test]
    fn the_container_of_a_symbol_is_its_module_or_its_type() {
        let text = "\
impl Point {
    pub fn new() -> Self { Point { x: 0 } }
}
mod util {
    pub fn helper() {}
}
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(
            containers(&symbols),
            ["::Point", "Point::new", "::util", "util::helper"]
        );
        let impl_symbol = symbols.iter().find(|s| s.identity.kind == SymbolKind::Type);
        assert_eq!(impl_symbol.map(|s| s.identity.container.clone()), Some(None));
    }

    #[test]
    fn a_nested_module_has_a_path() {
        let text = "\
mod outer {
    mod inner {
        fn deep() {}
    }
    fn shallow() {}
}
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(
            containers(&symbols),
            ["::outer", "outer::inner", "outer::inner::deep", "outer::shallow"]
        );
        assert_eq!(symbols[0].identity.kind, SymbolKind::Module);
        assert_eq!(symbols[2].identity.kind, SymbolKind::Function);
    }

    #[test]
    fn a_test_is_a_test() {
        let text = "\
#[test]
fn it_works() {}

#[cfg(test)]
mod tests {
    #[test]
    fn inside() {}
    fn helper() {}
}
";
        let symbols = index_rust("src/a.rs", text);
        let kinds: Vec<&str> = symbols.iter().map(|s| s.identity.kind.as_str()).collect();
        assert_eq!(kinds, ["test", "module", "test", "test"]);
    }

    #[test]
    fn an_unknown_way_of_writing_is_skipped() {
        let text = "\
macro_rules! shout { () => {}; }
let x = 5;
mod a::b;
fn ok() {}
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(names(&symbols), ["function:ok"]);
    }

    #[test]
    fn the_comments_and_the_strings_hold_no_declaration() {
        let text = "\
// fn hidden() {}
/* fn also_hidden() {} */
fn shown() { let s = \"fn fake()\"; }
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(names(&symbols), ["function:shown"]);
    }

    #[test]
    fn the_signature_and_the_doc_are_read() {
        let text = "\
/// Draws the point.
///
/// The second line of the doc.
pub fn draw(x: u32) -> bool { true }
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(
            symbols[0].signature.as_deref(),
            Some("pub fn draw(x: u32) -> bool")
        );
        assert_eq!(
            symbols[0].doc.as_deref(),
            Some("Draws the point.\n\nThe second line of the doc.")
        );
    }

    #[test]
    fn a_signature_that_spans_several_lines_is_read() {
        let text = "\
pub fn wide(
    a: u32,
    b: u32,
) -> u32 {
    a + b
}
";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(symbols.len(), 1);
        assert_eq!(
            symbols[0].signature.as_deref(),
            Some("pub fn wide( a: u32, b: u32, ) -> u32")
        );
    }

    #[test]
    fn the_range_of_a_symbol_covers_its_body() {
        let text = "fn a() { let x = 1; }\nfn b() {}\n";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(symbols[0].range.start(), 0);
        assert_eq!(symbols[0].range.end(), 21);
        assert_eq!(symbols[1].range.start(), 22);
    }

    #[test]
    fn the_occurrences_of_a_name_are_found() {
        let text = "fn run() {}\nfn other() { run(); }\n";
        let identity = SymbolIdentity::new("src/a.rs", SymbolKind::Function, "run", None);
        let ranges = occurrences_rust(text, &identity);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start(), 3);
    }

    #[test]
    fn a_raw_identifier_is_the_name_it_carries() {
        let text = "fn r#match() {}\n";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(names(&symbols), ["function:match"]);
    }

    #[test]
    fn a_trait_method_has_no_body() {
        let text = "trait T {\n    fn m(&self);\n}\n";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(names(&symbols), ["trait:T", "method:m"]);
        assert_eq!(containers(&symbols), ["::T", "T::m"]);
    }

    #[test]
    fn an_impl_of_a_trait_is_the_type_it_is_for() {
        let text = "impl std::fmt::Display for Point {\n    fn fmt(&self) {}\n}\n";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(names(&symbols), ["type:Point", "method:fmt"]);
    }

    #[test]
    fn the_range_of_a_symbol_is_in_utf16_units() {
        // the emoji of the comment takes two UTF-16 units and one char: the range of the next symbol
        // tells the difference.
        let text = "fn a() {}\n// \u{1f600}\nfn b() {}\n";
        let symbols = index_rust("src/a.rs", text);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[1].range.start(), 16);
        assert_eq!(symbols[1].range.end(), 25);
        // the char offsets would be 15 and 24
        assert_eq!(text.chars().count(), 25);
        assert_eq!(text.encode_utf16().count(), 26);
    }
}
