//! # FST kernel facade (F1 of FST migration)
//!
//! This module is the **only** place in the codebase that may name `rustfst::*`
//! types. Every layer above — morphology compilation, slot grammar, render —
//! goes through the [`FstBackend`] trait. The seam guarantee from
//! `docs/proposals/fst-kernel-design.md` Appendix B says: every line of
//! morphology code must work unchanged if we swap the backend (Option B →
//! Option C, in proposal terms).
//!
//! ## Layout
//!
//! - [`backend`] — backend-agnostic types: [`FstBackend`] trait, [`Path`],
//!   [`SymbolTable`], [`FstError`].
//! - [`rustfst_backend`] — concrete [`RustFstBackend`] wrapping `rustfst`'s
//!   `VectorFst<TropicalWeight>`.
//! - The public type alias [`Backend`] picks the live backend; this is the
//!   one-line swap point referenced in the design doc.
//!
//! ## Why `TropicalWeight` for unweighted
//!
//! `rustfst-survey.md` §2 recommended `BooleanWeight` / `TrivialWeight`.
//! In rustfst 1.3.1, neither implements `WeightQuantize`, which `determinize`
//! and `minimize` require. `TropicalWeight` does, and represents "no cost"
//! cleanly as `TropicalWeight::one() == 0.0`. Multiplying weights along a path
//! sums to `0.0`, so the boolean-acceptor semantics are preserved. This is a
//! divergence from the survey worth flagging — see the F1 report.
//!
//! ## How to swap rustfst for an in-tree kernel
//!
//! See `docs/proposals/fst-kernel-design.md` Appendix B for the full procedure.
//! In short:
//!
//! 1. Implement [`FstBackend`] for `InTreeBackend` in a new sibling module.
//! 2. Change `pub type Backend = …` below from `RustFstBackend` to
//!    `InTreeBackend`. That is the one-line swap.
//! 3. `cargo build`. The compiler enforces the seam: any rustfst type that
//!    leaked above the trait surface would fail to compile.

pub mod alphabet;
pub mod backend;
pub mod inflection;
pub mod lexicon;
pub mod phonrule;
pub mod rustfst_backend;

#[cfg(test)]
mod tests;

pub use alphabet::PhonruleAlphabet;
pub use backend::{FstBackend, FstBuilder, FstError, FstResult, Label, Path, StateId, SymbolTable, EPS_LABEL};
pub use inflection::{
    compile_inflection_fst, InflectionCompileError,
};
pub use lexicon::{
    compile_lexicon, compile_morpheme, lexicon_surface, LexiconCompileError, LexiconLookupError,
};
pub use rustfst_backend::RustFstBackend;

/// The active FST backend. Swap this alias to switch backends.
///
/// All morphology layers (F2+) MUST refer to FSTs through `Backend::Fst`,
/// never directly through `RustFstBackend::Fst` or a concrete rustfst type.
pub type Backend = RustFstBackend;
