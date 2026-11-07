//! The facade of the semantic layer: the adapter trait, the index and the edit that drives it.
//!
//! A [`SemanticAdapter`] is the language-specific part: it reads a file and gives the symbols of the file and the
//! places where a symbol is used. The [`SemanticIndex`] is the language-independent part: it holds the adapters, the
//! indexed files and answers "where is this symbol" and "what does this edit touch".
//!
//! The trait is **synchronous** on purpose: the adapters read a text that the caller already has, they never wait for
//! anything, and the executor drives them from its own worker loop. The detailed design (§1.2) writes the trait as
//! `async`; the difference is a signature, not a behaviour, and it is listed as a deviation.

use crate::impact::{ImpactSet, Reference, compute_impact};
use crate::position::{Range, Utf16Offset};
use crate::symbol::{Symbol, SymbolIdentity};
use std::fmt;

/// A language adapter: it knows one language and reads the symbols of a text.
pub trait SemanticAdapter: Send + Sync {
    /// The name of the language (`rust`, `typescript`).
    fn language(&self) -> &str;

    /// The symbols of a text, in the order of the file.
    fn index(&self, path: &str, text: &str) -> Vec<Symbol>;

    /// The places of a text where the name of the symbol is used, in the order of the file.
    ///
    /// The declaration itself is one of the places: the caller decides what a reference is.
    fn occurrences(&self, text: &str, identity: &SymbolIdentity) -> Vec<Range>;

    /// Whether the adapter reads the files of this path.
    fn supports_path(&self, path: &str) -> bool {
        language_of_path(path) == self.language()
    }
}

/// The language of a path, from its extension (`""` when the extension is not known).
pub fn language_of_path(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return "";
    };
    if stem.is_empty() {
        return "";
    }
    match extension {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        _ => "",
    }
}

/// A file of the repository: its path and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    /// The path of the file, relative to the repository.
    pub path: String,
    /// The text of the file.
    pub text: String,
}

impl SourceFile {
    /// A file from its path and its text.
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
        }
    }
}

/// An edit of a file: the range it replaces and the text it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The path of the edited file, relative to the repository.
    pub path: String,
    /// The replaced range, in UTF-16 offsets.
    pub range: Range,
    /// The text that replaces the range.
    pub new_text: String,
}

impl Edit {
    /// An edit from its path, its range and its text.
    pub fn new(path: impl Into<String>, range: Range, new_text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            range,
            new_text: new_text.into(),
        }
    }
}

/// A file of the index: its text, its language and its symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFile {
    /// The path of the file, relative to the repository.
    pub path: String,
    /// The text of the file, as it was indexed.
    pub text: String,
    /// The language of the file (`""` when no adapter reads it).
    pub language: String,
    /// The symbols of the file.
    pub symbols: Vec<Symbol>,
}

impl IndexedFile {
    /// The symbol whose range holds the offset, the innermost one when there are several.
    pub fn symbol_at(&self, offset: Utf16Offset) -> Option<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.range.touches(offset))
            .min_by_key(|s| s.range.len())
    }

    /// The symbols of the file that have this name.
    pub fn symbols_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Symbol> {
        self.symbols.iter().filter(move |s| s.identity.name == name)
    }
}

/// The semantic index of a repository: the adapters, the files and their symbols.
pub struct SemanticIndex {
    adapters: Vec<Box<dyn SemanticAdapter>>,
    files: Vec<IndexedFile>,
}

impl Default for SemanticIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SemanticIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SemanticIndex")
            .field("adapters", &self.adapters.len())
            .field("files", &self.files.len())
            .finish()
    }
}

impl SemanticIndex {
    /// An index without adapter.
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
            files: Vec::new(),
        }
    }

    /// An index with the adapters of the languages this crate reads: Rust and TypeScript.
    pub fn with_default_adapters() -> Self {
        let mut index = Self::new();
        index.register(Box::new(crate::rust::RustAdapter));
        index.register(Box::new(crate::typescript::TypeScriptAdapter));
        index
    }

    /// Adds an adapter. The last adapter that supports a path wins.
    pub fn register(&mut self, adapter: Box<dyn SemanticAdapter>) {
        self.adapters.push(adapter);
    }

    /// The number of indexed files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The number of indexed symbols.
    pub fn symbol_count(&self) -> usize {
        self.files.iter().map(|f| f.symbols.len()).sum()
    }

    /// The indexed files, in the order they were indexed.
    pub fn files(&self) -> &[IndexedFile] {
        &self.files
    }

    /// Indexes the files of the repository.
    ///
    /// A file that is already indexed is read again; a file that is not in the list keeps its symbols. The method is
    /// therefore the incremental one too: to re-read one file, pass that file alone.
    pub fn index_repository(&mut self, files: &[SourceFile]) {
        for file in files {
            self.update_file(&file.path, &file.text);
        }
    }

    /// Reads one file again and replaces its symbols.
    pub fn update_file(&mut self, path: &str, text: &str) {
        let adapter = self
            .adapters
            .iter()
            .rev()
            .find(|a| a.supports_path(path))
            .map(|a| a.as_ref());
        let (language, symbols) = match adapter {
            None => (String::new(), Vec::new()),
            Some(adapter) => (adapter.language().to_string(), adapter.index(path, text)),
        };
        let indexed = IndexedFile {
            path: path.to_string(),
            text: text.to_string(),
            language,
            symbols,
        };
        match self.files.iter().position(|f| f.path == path) {
            Some(at) => self.files[at] = indexed,
            None => self.files.push(indexed),
        }
    }

    /// Removes a file from the index.
    pub fn remove_file(&mut self, path: &str) -> bool {
        match self.files.iter().position(|f| f.path == path) {
            Some(at) => {
                self.files.remove(at);
                true
            }
            None => false,
        }
    }

    /// The indexed file of a path.
    pub fn file(&self, path: &str) -> Option<&IndexedFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// The adapter of a language.
    pub fn adapter_for(&self, language: &str) -> Option<&dyn SemanticAdapter> {
        self.adapters
            .iter()
            .rev()
            .find(|a| a.language() == language)
            .map(|a| a.as_ref())
    }

    /// The symbol of this identity, the first one when the identity is declared twice.
    pub fn find(&self, identity: &SymbolIdentity) -> Option<&Symbol> {
        self.files
            .iter()
            .flat_map(|f| f.symbols.iter())
            .find(|s| &s.identity == identity)
    }

    /// Every symbol of this identity.
    pub fn find_all(&self, identity: &SymbolIdentity) -> Vec<&Symbol> {
        self.files
            .iter()
            .flat_map(|f| f.symbols.iter())
            .filter(|s| &s.identity == identity)
            .collect()
    }

    /// The symbol of this semantic key.
    pub fn find_by_key(&self, key: &str) -> Option<&Symbol> {
        self.files
            .iter()
            .flat_map(|f| f.symbols.iter())
            .find(|s| s.symbol_key() == key)
    }

    /// Every symbol of the index.
    pub fn symbols(&self) -> impl Iterator<Item = &Symbol> {
        self.files.iter().flat_map(|f| f.symbols.iter())
    }

    /// The places where the symbol is used, in every indexed file.
    ///
    /// The places are the same as the ones of the affected set, with the kind of each place: the declaration itself
    /// is one of them.
    pub fn occurrences(&self, identity: &SymbolIdentity) -> Vec<Reference> {
        let mut out = Vec::new();
        for file in &self.files {
            let Some(adapter) = self.adapter_for(&file.language) else {
                continue;
            };
            for range in adapter.occurrences(&file.text, identity) {
                let owner = crate::impact::owner_of(file, range);
                out.push(Reference {
                    path: file.path.clone(),
                    range,
                    target: identity.clone(),
                    kind: crate::impact::reference_kind(&file.text, range, owner),
                });
            }
        }
        out
    }

    /// The affected set of an edit: the symbols, the files and the places that the edit touches.
    ///
    /// The edit must hold the range of a declaration (or a part of it): the symbol that holds the range is the symbol
    /// the edit changes, and the impact set holds every place where that symbol is used.
    pub fn impact_of(&self, edit: &Edit) -> ImpactSet {
        compute_impact(self, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_of_a_path_comes_from_the_extension() {
        assert_eq!(language_of_path("src/a.rs"), "rust");
        assert_eq!(language_of_path("src/a.ts"), "typescript");
        assert_eq!(language_of_path("src/a.tsx"), "typescript");
        assert_eq!(language_of_path("src/a.md"), "");
        assert_eq!(language_of_path("Makefile"), "");
        assert_eq!(language_of_path("dir.with.dots/file"), "");
    }

    #[test]
    fn the_default_adapters_read_the_two_languages() {
        let index = SemanticIndex::with_default_adapters();
        assert!(index.adapter_for("rust").is_some());
        assert!(index.adapter_for("typescript").is_some());
        assert!(index.adapter_for("python").is_none());
    }

    #[test]
    fn the_index_reads_and_re_reads_a_file() {
        let mut index = SemanticIndex::with_default_adapters();
        index.index_repository(&[SourceFile::new("src/a.rs", "fn a() {}\n")]);
        assert_eq!(index.file_count(), 1);
        assert_eq!(index.symbol_count(), 1);
        index.update_file("src/a.rs", "fn a() {}\nfn b() {}\n");
        assert_eq!(index.symbol_count(), 2);
        assert_eq!(index.file("src/a.rs").map(|f| f.symbols.len()), Some(2));
    }

    #[test]
    fn a_file_without_an_adapter_is_indexed_without_symbols() {
        let mut index = SemanticIndex::with_default_adapters();
        index.index_repository(&[SourceFile::new("README.md", "# hello\n")]);
        assert_eq!(index.file_count(), 1);
        assert_eq!(index.symbol_count(), 0);
        assert_eq!(index.file("README.md").map(|f| f.language.as_str()), Some(""));
    }

    #[test]
    fn re_reading_one_file_keeps_the_identity_of_the_other_symbols() {
        let mut index = SemanticIndex::with_default_adapters();
        index.index_repository(&[
            SourceFile::new("src/a.rs", "pub fn run() {}\n"),
            SourceFile::new("src/b.rs", "pub fn other() {}\n"),
        ]);
        let before: Vec<String> = index.symbols().map(|s| s.symbol_key()).collect();
        assert_eq!(before.len(), 2);
        index.index_repository(&[SourceFile::new(
            "src/a.rs",
            "pub fn run() {}\npub fn added() {}\n",
        )]);
        let after: Vec<String> = index.symbols().map(|s| s.symbol_key()).collect();
        assert_eq!(after.len(), 3);
        assert!(after.contains(&"src/b.rs##function#other".to_string()));
        assert!(after.contains(&"src/a.rs##function#run".to_string()));
        assert!(after.contains(&"src/a.rs##function#added".to_string()));
        // the symbol of the file that did not change is the very same one
        let other = index
            .find_by_key("src/b.rs##function#other")
            .expect("the symbol is indexed");
        assert_eq!(other.range.start(), 0);
        assert_eq!(other.range.end(), 17);
    }

    #[test]
    fn removing_a_file_removes_its_symbols() {
        let mut index = SemanticIndex::with_default_adapters();
        index.index_repository(&[SourceFile::new("src/a.rs", "pub fn run() {}\n")]);
        assert!(index.remove_file("src/a.rs"));
        assert_eq!(index.file_count(), 0);
        assert_eq!(index.symbol_count(), 0);
        assert!(!index.remove_file("src/a.rs"));
    }
}
