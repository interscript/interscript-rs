//! interscript-rs — Rust runtime for Interscript ISC maps.
//!
//! Parses `.isc` maps through the canonical parsanol grammar
//! (`grammar/isc.parg`) and executes the standard map subset
//! end-to-end: parallel stages, sub rules with before/after boundary
//! constraints, any() classes, boundary anchors, aliased map/stage
//! dependencies (`.isc`/`.iml` libraries), and the embedded per-map
//! tests. Semantics mirror `interscript-ruby`'s
//! `Interscript::Interpreter` (the conformance-proven runtime).

pub mod builder;
pub mod document;
pub mod error;
pub mod grammar;
pub mod interpreter;
pub mod item;
pub mod locate;
pub mod stdlib;
pub mod tree;

pub use document::{Document, Stage, StageItem, Sub, SubTo, Tests};
pub use error::{Error, Result};
pub use interpreter::Runtime;
pub use item::Item;
