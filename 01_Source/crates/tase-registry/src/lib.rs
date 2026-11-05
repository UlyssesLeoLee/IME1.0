//! TASE registry: the template/asset registry of the template-aware semantic execution (design §3, §19).
//!
//! This crate is the read-only door between the catalog on the disk and the rest of TASE:
//!
//! * [`loader`] loads a catalog from the disk (`catalog.json` plus one `manifest.json` and one `card.md` per
//!   asset) and refuses a catalog whose index and whose assets do not agree;
//! * [`manifest`] parses the manifest of an asset (design §19.1) and the index of the catalog, field by field;
//! * [`digest`] computes the content digest of an asset (the id, the version and the body, design §3.2);
//! * [`policy`] decides what may be done with an asset: the state of the approval and the license;
//! * [`query`] searches the catalog by language, tag, category and keyword, with an opaque cursor;
//! * [`validator`] checks an instance against its asset (design §3.4: the mismatch rules).
//!
//! # The one hard rule: a knowledge card is not a certified implementation
//!
//! The catalog of this repository holds **93 knowledge cards** and **not one certified language implementation**
//! (`status = DOCUMENTED`, `implementations = []`). A card may be read, searched and quoted; it may not be
//! instantiated. The registry keeps that difference visible everywhere:
//!
//! * [`policy::Approval`] has three values, and an asset without an approved implementation is never more than
//!   [`policy::Approval::KnowledgeOnly`] - the state of the card says nothing about the state of an implementation;
//! * [`policy::Policy::may_instantiate`] only lets an asset through when an implementation of the **requested
//!   language** carries the state `APPROVED` (requirements §14.4). An implementation that is approved for Rust
//!   says nothing about TypeScript;
//! * [`query::Searcher::search`] writes the real state of every hit into `approval_status`, so a caller can
//!   never read a knowledge card as an executable template;
//! * [`validator::Validator::validate_instance`] returns [`validator::Mismatch::MissingImplementation`] for
//!   every asset of this repository, before any other rule is even looked at.
//!
//! The rule is also tested against the real catalog: the tests of [`loader`] and [`policy`] load the 93 assets
//! of `assets/tase-catalog` and assert that every one of them is a knowledge card and nothing else.
//!
//! The crate has no third-party dependency: the strict JSON parser and the SHA-256 come from `pime-core`, the
//! DTOs, the identifiers and the error codes from `tase-contract`.

#![forbid(unsafe_code)]

pub mod digest;
pub mod loader;
pub mod manifest;
pub mod policy;
pub mod query;
pub mod validator;

pub use digest::{asset_digest, is_digest, verify_digest};
pub use loader::Registry;
pub use manifest::{
    AssetEntry, CatalogRow, ImplementationEntry, parse_catalog_index, parse_manifest,
};
pub use policy::{Approval, Policy};
pub use query::{Query, Searcher};
pub use validator::{Mismatch, Validator, validate_digest};
