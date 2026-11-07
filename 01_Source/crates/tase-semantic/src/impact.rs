//! The affected set of an edit: who uses the symbol that the edit changes, and in which files.
//!
//! The set is computed from the identity of the symbol, never from its bare name: a symbol is `path + container +
//! kind + name` and a reference is counted only when it belongs to that symbol. A name that is written inside
//! another symbol of the same name but of another container (a method of another type) is **not** a reference to the
//! edited symbol.
//!
//! The rule is deliberately conservative: when a file declares the same name under another container, the places of
//! that file that cannot be attributed to the edited symbol are **not** reported. The set can then miss a place; it
//! never invents one. The places that are written with the container (`Point::run`) are always reported.

use crate::adapter::{Edit, SemanticIndex};
use crate::position::{Range, Utf16Offset};
use crate::symbol::{Symbol, SymbolIdentity, SymbolKind};

/// What a place of use is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    /// The name is written in an `import` or a `use` statement.
    Import,
    /// The name is used as a call.
    Call,
    /// The name is used inside a test.
    TestReference,
}

/// A place where a symbol is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The path of the file that holds the place.
    pub path: String,
    /// The place, in UTF-16 offsets of the text of the file.
    pub range: Range,
    /// The symbol that is used.
    pub target: SymbolIdentity,
    /// What the place is.
    pub kind: ReferenceKind,
}

/// The affected set of an edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImpactSet {
    /// The symbols that the edit touches: the edited symbol first, then the symbols that hold the places of use.
    pub symbols: Vec<SymbolIdentity>,
    /// The paths of the files that the edit touches, sorted and without a duplicate.
    pub files: Vec<String>,
    /// The places where the edited symbol is used, in the order of the files.
    pub references: Vec<Reference>,
}

impl ImpactSet {
    /// Whether the edit touches nothing else than the file it edits.
    pub fn is_local(&self) -> bool {
        self.files.len() <= 1
    }

    /// The number of places where the edited symbol is used.
    pub fn reference_count(&self) -> usize {
        self.references.len()
    }
}

/// Computes the affected set of an edit.
pub(crate) fn compute_impact(index: &SemanticIndex, edit: &Edit) -> ImpactSet {
    let mut impact = ImpactSet::default();
    let Some(file) = index.file(&edit.path) else {
        return impact;
    };
    let Some(target) = file.symbol_at(edit.range.start) else {
        return impact;
    };
    let identity = target.identity.clone();
    impact.symbols.push(identity.clone());
    let mut files: Vec<String> = vec![edit.path.clone()];

    for other in index.files() {
        let Some(adapter) = index.adapter_for(&other.language) else {
            continue;
        };
        let occurrences = adapter.occurrences(&other.text, &identity);
        // The declaration is a place of the file that holds it: the ranges of another file are another coordinate
        // space and must never be compared with it.
        let declaration = if other.path == target.identity.path {
            declaration_occurrence(&occurrences, target)
        } else {
            None
        };
        let ambiguous = other
            .symbols_named(&identity.name)
            .any(|s| s.identity.container != identity.container);
        let mut used_here = false;
        for range in occurrences {
            if Some(range) == declaration {
                continue;
            }
            let owner = owner_of(other, range);
            let belongs = match &owner {
                Some(symbol) if symbol.identity.name == identity.name => {
                    symbol.identity.container == identity.container
                }
                _ => !ambiguous || is_qualified(&other.text, range, &identity),
            };
            if !belongs {
                continue;
            }
            used_here = true;
            let kind = reference_kind(&other.text, range, owner);
            impact.references.push(Reference {
                path: other.path.clone(),
                range,
                target: identity.clone(),
                kind,
            });
            if let Some(symbol) = &owner
                && !impact.symbols.contains(&symbol.identity)
            {
                impact.symbols.push(symbol.identity.clone());
            }
        }
        if used_here && !files.contains(&other.path) {
            files.push(other.path.clone());
        }
    }
    files.sort();
    impact.files = files;
    impact
}

/// The place of the declaration of a symbol: the first place of its name inside its own range.
fn declaration_occurrence(occurrences: &[Range], target: &Symbol) -> Option<Range> {
    occurrences
        .iter()
        .find(|r| r.start >= target.range.start && r.end <= target.range.end)
        .copied()
}

/// The symbol that holds a place: the innermost one.
pub(crate) fn owner_of(file: &crate::adapter::IndexedFile, range: Range) -> Option<&Symbol> {
    file.symbols
        .iter()
        .filter(|s| s.range.start <= range.start && range.end <= s.range.end)
        .min_by_key(|s| s.range.len())
}

/// Whether the place is written with the container of the symbol (`Point::run`, `Point.run`).
fn is_qualified(text: &str, range: Range, identity: &SymbolIdentity) -> bool {
    let Some(container) = &identity.container else {
        return false;
    };
    if container.is_empty() {
        return false;
    }
    let Some(byte) = crate::position::byte_offset_at(text, Utf16Offset(range.start())) else {
        return false;
    };
    let prefix = &text[..byte.0 as usize];
    prefix.ends_with(&format!("{container}::"))
        || prefix.ends_with(&format!("{container}."))
}

/// What a place of use is.
pub(crate) fn reference_kind(text: &str, range: Range, owner: Option<&Symbol>) -> ReferenceKind {
    if let Some(line) = line_at(text, range.start()) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("use ")
            || trimmed.starts_with("import ")
            || trimmed.starts_with("export {")
            || trimmed.starts_with("export *")
        {
            return ReferenceKind::Import;
        }
    }
    match owner {
        Some(symbol) if symbol.identity.kind == SymbolKind::Test => ReferenceKind::TestReference,
        _ => ReferenceKind::Call,
    }
}

/// The line that holds a UTF-16 offset.
fn line_at(text: &str, offset: u32) -> Option<&str> {
    let byte = crate::position::byte_offset_at(text, Utf16Offset(offset))?.0 as usize;
    let start = text[..byte].rfind('\n').map(|at| at + 1).unwrap_or(0);
    let end = text[byte..].find('\n').map(|at| byte + at).unwrap_or(text.len());
    Some(text[start..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{SemanticIndex, SourceFile};

    fn index_of(files: &[(&str, &str)]) -> SemanticIndex {
        let mut index = SemanticIndex::with_default_adapters();
        let files: Vec<SourceFile> = files
            .iter()
            .map(|(path, text)| SourceFile::new(*path, *text))
            .collect();
        index.index_repository(&files);
        index
    }

    fn edit_on(path: &str, index: &SemanticIndex, name: &str) -> Edit {
        let file = index.file(path).expect("the file is indexed");
        let symbol = file
            .symbols_named(name)
            .next()
            .expect("the symbol is indexed");
        Edit::new(path, symbol.range, "renamed")
    }

    #[test]
    fn a_rename_finds_every_place() {
        let index = index_of(&[
            ("src/a.rs", "pub fn run() {}\n"),
            ("src/b.rs", "fn other() { run(); }\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 1);
        assert_eq!(impact.references[0].path, "src/b.rs");
        assert_eq!(impact.files, ["src/a.rs", "src/b.rs"]);
        assert_eq!(impact.symbols.len(), 2);
    }

    #[test]
    fn a_place_in_the_same_file_is_found() {
        let index = index_of(&[("src/a.rs", "pub fn run() {}\nfn other() { run(); }\n")]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 1);
        assert_eq!(impact.references[0].path, "src/a.rs");
    }

    #[test]
    fn the_declaration_is_not_a_place_of_use() {
        let index = index_of(&[("src/a.rs", "pub fn run() {}\n")]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 0);
        assert!(impact.is_local());
    }

    #[test]
    fn the_same_name_under_another_container_is_not_reported() {
        let index = index_of(&[
            ("src/a.rs", "impl Point {\n    pub fn run() {}\n}\n"),
            ("src/b.rs", "impl Other {\n    pub fn run() {}\n    fn caller() { run(); }\n}\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 0);
    }

    #[test]
    fn an_import_is_a_place_of_use() {
        let index = index_of(&[
            ("src/a.rs", "pub fn run() {}\n"),
            ("src/b.rs", "use crate::a::run;\nfn caller() { run(); }\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.references.len(), 2);
        assert_eq!(impact.references[0].kind, ReferenceKind::Import);
        assert_eq!(impact.references[1].kind, ReferenceKind::Call);
    }

    #[test]
    fn a_test_that_uses_the_symbol_is_a_test_place() {
        let index = index_of(&[
            ("src/a.rs", "pub fn run() {}\n"),
            ("src/b.rs", "#[test]\nfn it_runs() { run(); }\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.references.len(), 1);
        assert_eq!(impact.references[0].kind, ReferenceKind::TestReference);
    }

    #[test]
    fn a_qualified_place_is_reported_even_with_another_container() {
        let index = index_of(&[
            ("src/a.rs", "impl Point {\n    pub fn run() {}\n}\n"),
            ("src/b.rs", "impl Other {\n    pub fn run() {}\n}\nfn caller() { Point::run(); }\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 1);
        assert_eq!(impact.references[0].path, "src/b.rs");
    }

    #[test]
    fn a_place_of_another_file_is_never_the_declaration() {
        // the place of the second file falls inside the range of the edited symbol by its numbers: the two files
        // are two coordinate spaces and the place must still be reported.
        let index = index_of(&[
            ("src/a.rs", "pub fn run() {}\n"),
            ("src/b.rs", "run();\n"),
        ]);
        let edit = edit_on("src/a.rs", &index, "run");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 1);
        assert_eq!(impact.references[0].path, "src/b.rs");
    }

    #[test]
    fn an_edit_outside_of_any_symbol_touches_nothing() {
        let index = index_of(&[("src/a.rs", "pub fn run() {}\n")]);
        let edit = Edit::new("src/a.rs", Range::new(100, 110), "x");
        let impact = index.impact_of(&edit);
        assert_eq!(impact.reference_count(), 0);
        assert!(impact.symbols.is_empty());
    }

    #[test]
    fn an_edit_of_an_unknown_file_touches_nothing() {
        let index = index_of(&[("src/a.rs", "pub fn run() {}\n")]);
        let edit = Edit::new("src/zz.rs", Range::new(0, 1), "x");
        let impact = index.impact_of(&edit);
        assert!(impact.symbols.is_empty());
        assert!(impact.files.is_empty());
    }
}
