//! The implementations of the catalogue, grouped by the category of the asset.
//!
//! Every module holds the algorithms of one category of the catalogue. The assets are the ones of
//! `01_Source/assets/tase-catalog/assets/<asset id>/`; the doc comment of every public function names its asset
//! and its card.

pub mod concurrency;
pub mod datastructure;
pub mod dp;
pub mod graph;
pub mod math;
pub mod searching;
pub mod sorting;
pub mod string;
