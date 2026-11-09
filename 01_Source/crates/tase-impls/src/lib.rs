//! TASE standard assets: the Rust implementation of the fifty algorithms of the catalogue.
//!
//! # What this crate is
//!
//! The catalogue (`01_Source/assets/tase-catalog`) holds the **knowledge cards** of the standard assets: the
//! GoF patterns, the engineering patterns and the fifty algorithms. A card says what an asset is for, when it
//! applies, when it does **not** apply, what it costs and what the alternatives are. A card is documentation.
//!
//! This crate holds the **implementations**: real, tested Rust code, one per algorithm of the catalogue, written
//! so that a caller can use them without reading the card first and without trusting the card blindly.
//!
//! # The rules every implementation obeys
//!
//! * **The standard library only.** No third-party crate: the design book (§1.2) asks for a licence review of
//!   every dependency and every line of this crate has to stay auditable.
//! * **No `unsafe`.** `#![forbid(unsafe_code)]` at the crate root.
//! * **Generic where the algorithm is generic, concrete where it is not.** A sort takes `T: Ord`; a graph takes
//!   node ids; a string algorithm works on bytes and on `char`s.
//! * **Deterministic.** The same input gives the same output, always. Where the algorithm allows several correct
//!   answers (the order of equal keys, the choice of a pivot), the choice is fixed and documented, so a test can
//!   pin it.
//! * **No panic on a valid input.** An empty input gives an empty result, not a panic. An input that cannot be
//!   handled (a negative capacity, a cycle in a graph that is supposed to be acyclic) is reported as an `Err` with
//!   a reason, never as a panic.
//! * **Every implementation names its asset.** The doc comment of every public function carries the line
//!   `* asset: \`algorithm.\<category\>.\<name\>\``, and the card of the asset is named next to it. The script
//!   `01_Source/scripts/collect-implementations.js` reads those markers and writes them into the catalogue, so
//!   the catalogue and the code cannot drift apart.
//!
//! # The certification of an implementation
//!
//! The catalogue knows two axes (design §14.4): a **card** is `DOCUMENTED` or `REVIEWED`, an **implementation**
//! is `IMPLEMENTED`, `TESTED`, `APPROVED`, `DEPRECATED` or `REVOKED`. The fifty implementations of this crate are
//! registered as `TESTED`: they exist, they are written down and their tests are named in the catalogue. They are
//! **not** `APPROVED` - that state needs a person to look at the code and say so.
//!
//! # The modules
//!
//! | module | the assets it holds |
//! |---|---|
//! | [`algorithm::sorting`] | the seven sorting algorithms |
//! | [`algorithm::searching`] | the five searching algorithms |
//! | [`algorithm::math`] | the six number algorithms |
//! | [`algorithm::graph`] | the nine graph algorithms |
//! | [`algorithm::dp`] | the six dynamic programming algorithms |
//! | [`algorithm::string`] | the six string algorithms |
//! | [`algorithm::datastructure`] | the six data structures |
//! | [`algorithm::concurrency`] | the five concurrency algorithms |

#![forbid(unsafe_code)]

pub mod algorithm;

/// The version of the crate.
pub const IMPLS_VERSION: &str = "0.1.0";
