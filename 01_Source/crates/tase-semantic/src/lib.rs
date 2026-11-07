//! TASE semantic layer: the symbol index, the position mapping and the affected set of an edit.
//!
//! The crate answers three questions of the executor (detailed design §4, "semantic index and symbol modification"):
//! **which symbols does this file hold**, **where is this symbol** and **what does this edit touch**.
//!
//! # The three rules of this crate
//!
//! * **UTF-16 is the basis.** Every edit of this repository is a UTF-16 offset (`pime-core/src/edit/transaction.rs`:
//!   "`edits` are UTF-16 offsets into `snapshot.content`"), so every range of this crate is a UTF-16 range and the
//!   `char` and the byte views are derived. The detailed design (§4.1, §7.2 of the basic design) assumed a char-based
//!   editor and a byte-based parser: the first assumption does not hold here, so the basis is UTF-16 and the mapping
//!   lives in one module ([`position`]). An emoji is one `char` and two UTF-16 units: the two are never mixed.
//! * **The reading is a structured line-level scan, not an AST.** The detailed design (§4, §7.3) plans `syn` and
//!   Tree-sitter. They are not used: a third-party dependency needs an approval (D-51 is still open) and the
//!   dependency policy of the repository allows nothing but `pime-core` and the crates of this repository. The
//!   adapters therefore walk the lines, they know the keywords of their language, they follow the braces and they
//!   **skip what they do not understand** instead of guessing it. The consequence is written down in the
//!   documentation of every adapter: what is read, and what is not.
//! * **A symbol is identified by its identity, never by its name.** The semantic key ([`SymbolIdentity::symbol_key`])
//!   is the path, the container, the kind and the name. Two symbols of the same name under another container are two
//!   symbols, and the affected set ([`ImpactSet`]) never mixes them.
//!
//! # What the crate holds
//!
//! * [`position`] — the mapping between the UTF-16 basis and the char and byte views;
//! * [`symbol`] — the identity of a symbol and the shared view of a source file;
//! * [`rust`] and [`typescript`] — the language adapters;
//! * [`impact`] — the affected set of an edit;
//! * [`adapter`] — the trait, the index and the edit that drives them.
//!
//! The crate has no third-party dependency: the masking of the comments and the string literals comes from
//! `pime-core`.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod impact;
pub mod position;
pub mod rust;
pub mod symbol;
pub mod typescript;

pub use adapter::{Edit, IndexedFile, SemanticAdapter, SemanticIndex, SourceFile, language_of_path};
pub use impact::{ImpactSet, Reference, ReferenceKind};
pub use position::{ByteOffset, CharOffset, LineIndex, Position, Range, Utf16Offset};
pub use rust::RustAdapter;
pub use symbol::{Symbol, SymbolIdentity, SymbolKind};
pub use typescript::TypeScriptAdapter;

/// The version of the semantic layer.
pub const SEMANTIC_VERSION: &str = "1.1.0";
