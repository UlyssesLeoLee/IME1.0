//! The TypeScript adapter: the declarations of a file, read by a **structured line-level scan**.
//!
//! The scan is not an AST parser: it walks the lines, it knows the keywords of TypeScript, it follows the braces and
//! it stops at the first thing it does not understand. What it reads:
//!
//! | declaration | kind | container |
//! |-------------|------|-----------|
//! | `function` | `function` | the namespace |
//! | `class` | `class` | the namespace |
//! | a method of a class | `method` | the class |
//! | `interface` | `interface` | the namespace |
//! | `type` | `type` | the namespace |
//! | `enum` | `enum` | the namespace |
//! | `namespace`, `module` | `module` | the namespace |
//! | `const` | `const`, or `function` when the value is an arrow function | the namespace |
//!
//! The `export`, `default`, `declare`, `abstract` and `async` modifiers are read and do not change the identity. What
//! it does **not** read: the fields of a class, the members of an interface, the `let` and the `var` bindings, the
//! decorators, the generics and the type annotations (they are part of the signature, not of the identity). A
//! declaration it cannot read is **skipped**, never guessed.

use crate::adapter::SemanticAdapter;
use crate::position::Range;
use crate::symbol::{
    HeaderKind, SourceView, Symbol, SymbolIdentity, SymbolKind, find_header_end, leading_blanks,
    normalise_signature, word_at,
};
use pime_core::validators::Style;

/// The adapter of the TypeScript language.
pub struct TypeScriptAdapter;

impl SemanticAdapter for TypeScriptAdapter {
    fn language(&self) -> &str {
        "typescript"
    }

    fn index(&self, path: &str, text: &str) -> Vec<Symbol> {
        index_typescript(path, text)
    }

    fn occurrences(&self, text: &str, identity: &SymbolIdentity) -> Vec<Range> {
        occurrences_typescript(text, identity)
    }
}

/// The symbols of a TypeScript file.
pub fn index_typescript(path: &str, text: &str) -> Vec<Symbol> {
    let view = SourceView::new(text, Style::TypeScript);
    let mut scanner = TypeScriptScanner {
        path,
        view: &view,
        symbols: Vec::new(),
        frames: Vec::new(),
    };
    scanner.run();
    scanner.symbols
}

/// The places of a TypeScript file where the name of the symbol is written.
pub fn occurrences_typescript(text: &str, identity: &SymbolIdentity) -> Vec<Range> {
    let view = SourceView::new(text, Style::TypeScript);
    view.tokens(true, false)
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
    /// Whether the item is a class: the members inside it are methods.
    is_class: bool,
}

/// Where the header of a declaration ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderEnd {
    /// The body of the item opens with a `{` at this char index.
    Brace(u32),
    /// The declaration ends with a `;` at this char index.
    Semi(u32),
}

/// The scanner of a TypeScript file.
struct TypeScriptScanner<'a, 'b> {
    path: &'a str,
    view: &'b SourceView<'b>,
    symbols: Vec<Symbol>,
    frames: Vec<Frame>,
}

impl TypeScriptScanner<'_, '_> {
    fn run(&mut self) {
        let line_count = self.view.line_count();
        let mut line = 0usize;
        while line < line_count {
            self.close_finished(self.view.line_start(line));
            let chars = self.view.line_chars(line);
            let offset = leading_blanks(chars, 0);
            let rest = &chars[offset..];
            if rest.is_empty() || rest[0] == '@' {
                line += 1;
                continue;
            }
            line = self.read_declaration(line, offset);
        }
    }

    /// Reads the declaration that starts on a line, and returns the line to read next.
    fn read_declaration(&mut self, line: usize, offset: usize) -> usize {
        let chars = self.view.line_chars(line);
        let in_class = self.frames.last().is_some_and(|f| f.is_class);
        let Some(head) = parse_head(&chars[offset..], in_class) else {
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
                Some((kind, at)) => {
                    let signature = if prefix.is_empty() {
                        line_text[..at].to_string()
                    } else {
                        format!("{prefix} {}", &line_text[..at])
                    };
                    let found = match kind {
                        HeaderKind::Brace => HeaderEnd::Brace(line_start + at as u32),
                        HeaderKind::Semi => HeaderEnd::Semi(line_start + at as u32),
                    };
                    end = Some((found, signature));
                    break;
                }
                None => {
                    if header_line > line + 19 {
                        break;
                    }
                    let next = header_line + 1;
                    if next >= self.view.line_count() {
                        break;
                    }
                    let next_text: String = self.view.line_chars(next).iter().collect();
                    let next_trimmed = next_text.trim();
                    if next_trimmed.is_empty() || looks_like_declaration(next_trimmed) {
                        break;
                    }
                    prefix = if prefix.is_empty() {
                        next_trimmed.to_string()
                    } else {
                        format!("{prefix} {next_trimmed}")
                    };
                    header_line = next;
                }
            }
        }
        let Some((found, signature)) = end else {
            return line + 1;
        };

        let container = self.frames.last().and_then(|f| f.path.clone());
        let start_char = self.view.line_start(line);
        let range = match found {
            HeaderEnd::Brace(brace) => match self.view.close_of(brace) {
                Some(close) => self.view.range(start_char, close + 1),
                None => self.view.range(start_char, brace + 1),
            },
            HeaderEnd::Semi(semi) => self.view.range(start_char, semi + 1),
        };

        self.symbols.push(Symbol {
            identity: SymbolIdentity {
                path: self.path.to_string(),
                kind: head.kind,
                name: head.name,
                container: container.clone(),
            },
            range,
            signature: Some(normalise_signature(&signature)),
            doc: self.doc_above(line),
        });

        if let HeaderEnd::Brace(brace) = found {
            let path = join_path(&container, &self.symbols.last().map(|s| s.identity.name.clone()).unwrap_or_default());
            self.frames.push(Frame {
                path,
                open: brace,
                is_class: head.kind == SymbolKind::Class,
            });
        }
        header_line + 1
    }

    /// The documentation comment above a declaration.
    fn doc_above(&self, line: usize) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut i = line;
        while i > 0 {
            let text = self.view.original_line(i - 1);
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("//") {
                lines.push(rest.trim().to_string());
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
}

/// The head of a declaration: the kind and the name.
#[derive(Clone, Debug)]
struct Head {
    kind: SymbolKind,
    name: String,
}

/// Reads the head of a declaration: the modifiers, the keyword and the name.
fn parse_head(line: &[char], in_class: bool) -> Option<Head> {
    let mut i = leading_blanks(line, 0);
    while let Some(word) = word_at(line, i) {
        match word.as_str() {
            "export" | "default" | "declare" | "abstract" | "async" | "public" | "private"
            | "protected" | "readonly" | "static" | "override" => {
                i = leading_blanks(line, i + word.len());
            }
            _ => break,
        }
    }
    let word = word_at(line, i)?;
    match word.as_str() {
        "function" => {
            let after = leading_blanks(line, i + 8);
            let after = if line.get(after) == Some(&'*') {
                leading_blanks(line, after + 1)
            } else {
                after
            };
            let (name, _) = read_name(line, after)?;
            Some(Head {
                kind: SymbolKind::Function,
                name,
            })
        }
        "class" => Some(Head {
            kind: SymbolKind::Class,
            name: read_name(line, i + 5)?.0,
        }),
        "interface" => Some(Head {
            kind: SymbolKind::Interface,
            name: read_name(line, i + 9)?.0,
        }),
        "type" => Some(Head {
            kind: SymbolKind::Type,
            name: read_name(line, i + 4)?.0,
        }),
        "enum" => Some(Head {
            kind: SymbolKind::Enum,
            name: read_name(line, i + 4)?.0,
        }),
        "namespace" | "module" => Some(Head {
            kind: SymbolKind::Module,
            name: read_name(line, i + word.len())?.0,
        }),
        "const" => {
            let after = leading_blanks(line, i + 5);
            let (name, name_end) = read_name(line, after)?;
            let kind = if is_function_value(&line[name_end..]) {
                SymbolKind::Function
            } else {
                SymbolKind::Const
            };
            Some(Head { kind, name })
        }
        _ => {
            if !in_class {
                return None;
            }
            // a member of a class: `name(...)`, `get name() {}`, `set name(v) {}`
            let (name, name_end) = read_name(line, i)?;
            let after = leading_blanks(line, name_end);
            if line.get(after) != Some(&'(') {
                return None;
            }
            Some(Head {
                kind: SymbolKind::Method,
                name,
            })
        }
    }
}

/// Whether the value of a `const` is a function: an arrow function or a function expression.
fn is_function_value(rest: &[char]) -> bool {
    let text: String = rest.iter().collect();
    let Some(at) = text.find('=') else {
        return false;
    };
    let text = text[at + 1..].trim_start();
    let text = text.strip_prefix("async").unwrap_or(text).trim_start();
    if text.starts_with("function") {
        return true;
    }
    text.starts_with('(') || text.starts_with('<')
}

/// Whether a line starts a declaration (used to stop the reading of a header that spans several lines).
fn looks_like_declaration(text: &str) -> bool {
    let text = text.trim_start();
    for keyword in [
        "function ", "class ", "interface ", "type ", "enum ", "namespace ", "const ", "export ",
    ] {
        if text.starts_with(keyword) {
            return true;
        }
    }
    false
}

/// The name that follows a keyword: the identifier and the index just after it.
fn read_name(line: &[char], at: usize) -> Option<(String, usize)> {
    let at = leading_blanks(line, at);
    let mut end = at;
    while end < line.len() && (line[end].is_alphanumeric() || line[end] == '_' || line[end] == '$') {
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
        Some(prefix) => Some(format!("{prefix}.{name}")),
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
export function run(a: number): void {}
export class Point { x = 0; }
export interface Shape { x: number; }
export type Alias = string;
export const MAX = 10;
export enum Color { Red }
";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(
            names(&symbols),
            [
                "function:run",
                "class:Point",
                "interface:Shape",
                "type:Alias",
                "const:MAX",
                "enum:Color"
            ]
        );
    }

    #[test]
    fn a_method_of_a_class_has_the_class_as_container() {
        let text = "\
class Point {
    move(dx: number) { return dx; }
    constructor() {}
}
";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(names(&symbols), ["class:Point", "method:move", "method:constructor"]);
        assert_eq!(
            containers(&symbols),
            ["::Point", "Point::move", "Point::constructor"]
        );
    }

    #[test]
    fn an_arrow_function_is_a_function() {
        let text = "export const run = (a: number) => a;\nconst MAX = 10;\n";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(names(&symbols), ["function:run", "const:MAX"]);
    }

    #[test]
    fn the_export_modifier_does_not_change_the_identity() {
        let text = "export function run() {}\nfunction plain() {}\n";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(containers(&symbols), ["::run", "::plain"]);
        assert_eq!(symbols[0].identity.container, None);
    }

    #[test]
    fn the_comments_and_the_strings_hold_no_declaration() {
        let text = "// function hidden() {}\n/* function alsoHidden() {} */\nfunction shown() { const s = \"function fake()\"; }\n";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(names(&symbols), ["function:shown"]);
    }

    #[test]
    fn a_namespace_holds_its_items() {
        let text = "namespace Util {\n    export function run() {}\n}\n";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(names(&symbols), ["module:Util", "function:run"]);
        assert_eq!(symbols[1].identity.container.as_deref(), Some("Util"));
    }

    #[test]
    fn the_signature_and_the_doc_are_read() {
        let text = "// Runs the thing.\nexport function run(a: number): void {}\n";
        let symbols = index_typescript("src/a.ts", text);
        assert_eq!(
            symbols[0].signature.as_deref(),
            Some("export function run(a: number): void")
        );
        assert_eq!(symbols[0].doc.as_deref(), Some("Runs the thing."));
    }

    #[test]
    fn the_occurrences_of_a_name_are_found() {
        let text = "function run() {}\nrun();\n";
        let identity = SymbolIdentity::new("src/a.ts", SymbolKind::Function, "run", None);
        let ranges = occurrences_typescript(text, &identity);
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn a_default_export_without_a_name_is_skipped() {
        let text = "export default function () {}\n";
        let symbols = index_typescript("src/a.ts", text);
        assert!(symbols.is_empty());
    }
}
