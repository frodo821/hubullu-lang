//! Compose-chain → FST (F4.2 of FST migration).
//!
//! Given:
//!   * the per-slot FSTs produced by F4.1 (`super::slot`),
//!   * a stem-anchor FST (also F4.1, stubbed for F4),
//!   * the per-phonrule FSTs produced by F2 (`super::super::phonrule`),
//!
//! [`build_chain_fst`] walks an [`ComposeExpr`] tree and produces a single
//! FST representing the whole chain.
//!
//! ## Construction
//!
//! For each [`ComposeExpr`] variant:
//!
//!   * `Slot { name, quantifier }`:
//!       - Lookup `slot_fsts[name]`; fall back to `stem_fst` if `name`
//!         matches the configured stem anchor name.
//!       - Wrap by `quantifier`:
//!         * `One`        → no wrap
//!         * `ZeroOrOne`  → `closure_optional`
//!         * `ZeroOrMore` → `closure_star`
//!         * `OneOrMore`  → `closure_plus`
//!         * `Bounded {min, max}` → `closure_bounded(min, max)`
//!   * `Concat(parts)`: build each part's FST, then `concat` them
//!     left-to-right with arc-sort discipline between concats so the
//!     subsequent compose against phonrule FSTs sees the right sort bits.
//!   * `PhonApply { rule, inner }`: build `inner`'s FST, look up
//!     `phonrule_fsts[rule]`, and `compose(inner, phonrule_fst)` with
//!     full arc-sort discipline.
//!
//! ## Arc-sort discipline
//!
//! Every operation that produces an FST that may later be a `compose`
//! operand is left both input-sorted and output-sorted before being
//! returned. Mirrors `super::super::lexicon::compile_lexicon`'s
//! discipline.
//!
//! ## Alphabet caveat
//!
//! Composing the chain FST with a phonrule FST requires the two share
//! an alphabet. F4's stem-anchor stub and lazy-slot FSTs are over the
//! morpheme-/literal-label space populated during `compile_inflection_fst`;
//! the F2 phonrule FSTs are over the same `PhonruleAlphabet`. The
//! contract is therefore "use the same `&mut PhonruleAlphabet` for
//! phonrule compilation and inflection compilation." F5 will enforce
//! this with a single top-level driver.
//!
//! ## What this module owns
//!
//!   * [`build_chain_fst`] — the public entry point.
//!   * [`ChainCompileError`] — typed errors.
//!
//! ## What this module does NOT own
//!
//!   * Per-slot FST construction (F4.1 — see `super::slot`).
//!   * Inflection-level driver (F4.3 — see `super`).
//!   * Per-entry stem binding (F5).

use std::collections::HashMap;

use crate::ast::{ComposeExpr, SlotQuantifier};

use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Errors produced by [`build_chain_fst`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainCompileError {
    /// The chain references a slot name that is neither in `slot_fsts`
    /// nor recognised as a stem anchor.
    UnknownSlot { slot_name: String },
    /// The chain wraps a `PhonApply` whose rule is not in `phonrule_fsts`.
    UnknownPhonrule { rule_name: String },
    /// An empty `Concat([])` — semantically ambiguous, surface as a typed
    /// error rather than silently producing the empty FST.
    EmptyConcat,
    /// Backend FST operation failed.
    Backend(String),
}

impl std::fmt::Display for ChainCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainCompileError::UnknownSlot { slot_name } => write!(
                f,
                "compose chain references slot '{}' which is neither declared nor a stem anchor",
                slot_name
            ),
            ChainCompileError::UnknownPhonrule { rule_name } => write!(
                f,
                "compose chain references phonrule '{}' which is not in the phonrule FST table",
                rule_name
            ),
            ChainCompileError::EmptyConcat => {
                write!(f, "compose chain has an empty Concat — ambiguous, refusing to compile")
            }
            ChainCompileError::Backend(s) => {
                write!(f, "chain compile backend error: {}", s)
            }
        }
    }
}

impl std::error::Error for ChainCompileError {}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Walk `expr` and build the per-inflection compose-chain FST.
///
/// `slot_fsts` maps slot names (lazy or eager) to their pre-built per-slot
/// FSTs. `stem_fsts` maps stem-anchor names (e.g. `root`) to their
/// pre-built stem FSTs (stubbed for F4 — see `super::slot::build_stem_anchor_stub`).
/// `phonrule_fsts` maps phonrule names to their pre-built per-rule FSTs
/// (F2 output).
///
/// The chain-walker looks up `Slot { name, .. }` in `slot_fsts` first,
/// then `stem_fsts`. The two namespaces overlap (a `root` can be either
/// a stem or a slot in principle); the chain-walker prefers `slot_fsts`
/// so an explicit slot declaration always wins.
pub fn build_chain_fst(
    expr: &ComposeExpr,
    slot_fsts: &HashMap<String, RustFstWrapper>,
    stem_fsts: &HashMap<String, RustFstWrapper>,
    phonrule_fsts: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ChainCompileError> {
    let raw = build_inner(expr, slot_fsts, stem_fsts, phonrule_fsts)?;
    finalize_sort(raw)
}

// ---------------------------------------------------------------------------
// Recursive walker.
// ---------------------------------------------------------------------------

fn build_inner(
    expr: &ComposeExpr,
    slot_fsts: &HashMap<String, RustFstWrapper>,
    stem_fsts: &HashMap<String, RustFstWrapper>,
    phonrule_fsts: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ChainCompileError> {
    match expr {
        ComposeExpr::Slot { name, quantifier } => {
            let base = slot_fsts
                .get(&name.node)
                .or_else(|| stem_fsts.get(&name.node))
                .ok_or_else(|| ChainCompileError::UnknownSlot {
                    slot_name: name.node.clone(),
                })?;
            wrap_quantifier(base, *quantifier)
        }
        ComposeExpr::Concat(parts) => {
            if parts.is_empty() {
                return Err(ChainCompileError::EmptyConcat);
            }
            let mut iter = parts.iter();
            let head = iter.next().expect("non-empty checked above");
            let mut acc = build_inner(head, slot_fsts, stem_fsts, phonrule_fsts)?;
            for part in iter {
                let next = build_inner(part, slot_fsts, stem_fsts, phonrule_fsts)?;
                acc = concat_sorted(&acc, &next)?;
            }
            Ok(acc)
        }
        ComposeExpr::PhonApply { rule, inner } => {
            let inner_fst = build_inner(inner, slot_fsts, stem_fsts, phonrule_fsts)?;
            let rule_fst = phonrule_fsts.get(&rule.node).ok_or_else(|| {
                ChainCompileError::UnknownPhonrule {
                    rule_name: rule.node.clone(),
                }
            })?;
            compose_sorted(&inner_fst, rule_fst)
        }
    }
}

// ---------------------------------------------------------------------------
// Quantifier wrap.
// ---------------------------------------------------------------------------

fn wrap_quantifier(
    fst: &RustFstWrapper,
    quantifier: SlotQuantifier,
) -> Result<RustFstWrapper, ChainCompileError> {
    let wrapped = match quantifier {
        SlotQuantifier::One => fst.clone(),
        SlotQuantifier::ZeroOrOne => RustFstBackend::closure_optional(fst)
            .map_err(|e| ChainCompileError::Backend(e.to_string()))?,
        SlotQuantifier::ZeroOrMore => RustFstBackend::closure_star(fst)
            .map_err(|e| ChainCompileError::Backend(e.to_string()))?,
        SlotQuantifier::OneOrMore => RustFstBackend::closure_plus(fst)
            .map_err(|e| ChainCompileError::Backend(e.to_string()))?,
        SlotQuantifier::Bounded { min, max } => RustFstBackend::closure_bounded(fst, min, max)
            .map_err(|e| ChainCompileError::Backend(e.to_string()))?,
    };
    Ok(wrapped)
}

// ---------------------------------------------------------------------------
// Sort-discipline helpers.
// ---------------------------------------------------------------------------

/// Arc-sort both operands and concat. Concat itself doesn't require sort
/// bits, but the result will be composed against phonrule FSTs later;
/// re-sorting after concat keeps the property bits current.
fn concat_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> Result<RustFstWrapper, ChainCompileError> {
    let out = RustFstBackend::concat(a, b)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    finalize_sort(out)
}

/// Arc-sort both operands and compose. Same discipline as
/// `phonrule::rule_seq::compose_sorted`.
fn compose_sorted(
    a: &RustFstWrapper,
    b: &RustFstWrapper,
) -> Result<RustFstWrapper, ChainCompileError> {
    let a_sorted = RustFstBackend::arc_sort_output(a)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    let b_sorted = RustFstBackend::arc_sort_input(b)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    let out = RustFstBackend::compose(&a_sorted, &b_sorted)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    finalize_sort(out)
}

/// Apply the standard arc-sort discipline (output then input) so the
/// resulting FST is ready to sit on either side of `compose`.
fn finalize_sort(fst: RustFstWrapper) -> Result<RustFstWrapper, ChainCompileError> {
    let out_sorted = RustFstBackend::arc_sort_output(&fst)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    let in_sorted = RustFstBackend::arc_sort_input(&out_sorted)
        .map_err(|e| ChainCompileError::Backend(e.to_string()))?;
    Ok(in_sorted)
}
