//! Per-slot FST construction (F4.1 of FST migration).
//!
//! Each `SlotDef` in an inflection's compose body becomes a small FST whose
//! accepting paths are the morphemes that may fill that slot. The compose-chain
//! walker (F4.2, `super::chain`) concatenates / quantifies / phon-wraps these
//! per-slot FSTs to produce the per-inflection FST.
//!
//! ## Three slot kinds
//!
//! The slot-morphology design (`docs/proposals/slot-morphology.md`) distinguishes
//! lazy slots, eager slots, and the implicit stem slot:
//!
//!   * **Lazy** (`slot NAME matching <filter>`): the filler is selected from
//!     the morpheme inventory at render time by `slot_parse::fits`. F4
//!     handles this fully: filter the morpheme list with `fits`, then union
//!     the matching per-morpheme FSTs.
//!   * **Eager** (`slot NAME { rules }`): the filler is computed from
//!     per-cell rules. Rule evaluation needs cell-axis input labels which
//!     F5 will design properly. F4 stubs eager slots as the empty acceptor
//!     (one start+final state, no arcs) — clearly signals "not yet
//!     implemented" if a downstream test traverses it.
//!   * **Stem anchor** (`root`, `pres`, etc. — referenced in the chain but
//!     not declared as a `slot`): the stem is per-entry data, bound at
//!     F5 by `compile_inflection_fst`'s per-entry specialisation. F4
//!     stubs the stem as identity-on-Σ self-loop (accepts any stem chars
//!     because we don't know which entry yet).
//!
//! ## What this module owns
//!
//!   * [`build_lazy_slot_fst`] — filter+union for `LazyMatching` slots.
//!   * [`build_eager_slot_stub`] — F5 placeholder for eager slots.
//!   * [`build_stem_anchor_stub`] — F5 placeholder for stem references.
//!   * [`SlotCompileError`] — typed errors.
//!
//! ## What this module does NOT own
//!
//!   * Compose-chain walking (F4.2 — see `super::chain`).
//!   * Per-entry stem binding (F5).
//!   * Eager-rule cell-axis encoding (F5).

use std::collections::HashMap;

use crate::ast::{Entry, LazyMatching, SlotDef};
use crate::slot_parse::{fits, MorphemeInstance};

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBuilder, Label};
use super::super::lexicon::{compile_morpheme, LexiconCompileError};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Errors produced by per-slot FST construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotCompileError {
    /// The lazy slot's filter selected zero morphemes from the inventory.
    /// For quantifiers `One` / `OneOrMore` / `Bounded { min >= 1, .. }`
    /// this would yield an FST that accepts nothing — almost certainly
    /// a profile bug. For `ZeroOrOne` / `ZeroOrMore` / `Bounded { min == 0, .. }`
    /// the chain walker can still wrap it via `closure_optional` /
    /// `closure_star`, so F4 returns the empty FST and lets the chain
    /// walker decide. The error is reserved for the case where the
    /// chain walker can confirm the empty result is fatal.
    EmptyLazySlot { slot_name: String },
    /// Per-morpheme compile failed.
    Lexicon(LexiconCompileError),
    /// Backend FST operation failed (union etc.).
    Backend(String),
}

impl std::fmt::Display for SlotCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlotCompileError::EmptyLazySlot { slot_name } => write!(
                f,
                "lazy slot '{}' has no morphemes matching its filter",
                slot_name
            ),
            SlotCompileError::Lexicon(e) => write!(f, "{}", e),
            SlotCompileError::Backend(s) => write!(f, "slot compile backend error: {}", s),
        }
    }
}

impl std::error::Error for SlotCompileError {}

impl From<LexiconCompileError> for SlotCompileError {
    fn from(e: LexiconCompileError) -> Self {
        SlotCompileError::Lexicon(e)
    }
}

// ---------------------------------------------------------------------------
// Lazy slot.
// ---------------------------------------------------------------------------

/// Build the per-slot FST for a `SlotBody::Lazy(matching)` slot.
///
/// For each morpheme entry whose tags satisfy `matching` (via
/// `slot_parse::fits`), look up the entry's per-morpheme FST in
/// `morpheme_fsts`; union all matches together.
///
/// `morpheme_fsts` is the cache built by the inflection-level compiler
/// (`super::compile_inflection_fst`): every inflectionless candidate
/// entry has its per-morpheme FST compiled once and stored by name.
/// Per F3's report, this is the recommended pattern: keep the
/// `HashMap<entry_name, RustFstWrapper>` and filter+union by tag predicate
/// to form each slot's sub-FST.
///
/// `all_morphemes` is the slice of candidate inflectionless entries whose
/// per-morpheme FSTs were compiled (i.e. the keys of `morpheme_fsts`); we
/// iterate this slice in the input order to keep the union deterministic
/// (HashMap iteration is not).
///
/// **Empty result**: if no morpheme fits, returns the empty FST (one
/// start+final state, no arcs). This is the empty language `{}` — it
/// accepts nothing. The chain walker is responsible for deciding whether
/// that's fatal (e.g. for a `One`-quantified slot) or harmless (e.g. for
/// a `ZeroOrMore`-quantified slot where the wrap will add the empty
/// path back in).
///
/// **Arc-sort discipline**: the returned FST is arc-sorted on input
/// (right-operand-of-compose readiness) and on output
/// (left-operand-of-compose readiness). Matches the discipline established
/// by `compile_lexicon`.
pub fn build_lazy_slot_fst(
    slot_def: &SlotDef,
    matching: &LazyMatching,
    all_morphemes: &[&Entry],
    morpheme_fsts: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, SlotCompileError> {
    // Filter candidates by `fits`. Construct a `MorphemeInstance` per
    // entry so we can re-use the existing `slot_parse::fits` predicate
    // without duplication. `surface` and `is_peripheral` are filled in
    // from the entry; only `tags` matter for `fits`, but `MorphemeInstance`
    // wants the whole struct.
    let mut matched: Vec<&Entry> = Vec::new();
    for entry in all_morphemes {
        let instance = MorphemeInstance {
            entry_id: entry.name.node.clone(),
            surface: String::new(), // unused by `fits`
            tags: entry
                .tags
                .iter()
                .map(|c| (c.axis.node.clone(), c.value.node.clone()))
                .collect(),
            is_peripheral: entry.is_peripheral,
        };
        if fits(&instance, matching) {
            matched.push(entry);
        }
    }

    if matched.is_empty() {
        // Return the empty acceptor (one state, start = final, no arcs).
        // L(this) = {ε}. The chain walker's quantifier wrap may turn
        // this into something sensible (closure_star/optional → still
        // includes ε), or may pass it through as the slot's contribution
        // and let downstream callers surface "no morphemes match" if
        // it matters.
        //
        // NOTE: this is NOT the empty-language FST (which would be
        // one state, no finals). We return the ε-acceptor so that
        // `concat` with the rest of the chain doesn't kill all paths.
        // A literal "rejects everything" slot would be a profile bug
        // the chain walker can detect via the quantifier semantics
        // (F5 will harden this — for now F4 lets the slot be ε and
        // the chain walker concatenate the rest).
        let mut b = RustFstBackend::builder();
        let s = b.add_state();
        b.set_start(s)
            .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
        b.set_final(s)
            .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
        let fst = b
            .finish()
            .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
        let _ = slot_def; // Silence unused; reserved for diagnostics.
        return finalize_sort(fst);
    }

    // Union all matching morpheme FSTs. The cache is keyed by entry name.
    // For each matched entry, clone the cached FST (cheap — VectorFst is
    // a Vec of states+arcs) and union it into the accumulator.
    let mut iter = matched.iter();
    let first_entry = iter.next().expect("non-empty checked above");
    let first_fst = morpheme_fsts.get(&first_entry.name.node).ok_or_else(|| {
        SlotCompileError::Backend(format!(
            "lazy slot '{}': morpheme '{}' not in cache",
            slot_def.name.node, first_entry.name.node
        ))
    })?;
    let mut acc = first_fst.clone();
    for entry in iter {
        let sub = morpheme_fsts.get(&entry.name.node).ok_or_else(|| {
            SlotCompileError::Backend(format!(
                "lazy slot '{}': morpheme '{}' not in cache",
                slot_def.name.node, entry.name.node
            ))
        })?;
        acc = RustFstBackend::union(&acc, sub)
            .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    }
    finalize_sort(acc)
}

// ---------------------------------------------------------------------------
// Eager slot stub (F5 work).
// ---------------------------------------------------------------------------

/// Build the per-slot FST for a `SlotBody::Eager(rules)` slot.
///
/// **F5 boundary**: eager-slot rule evaluation is per-cell — needs
/// cell-axis input labels which F5 will design properly. F4 stubs this
/// as an FST accepting the empty input → empty output (single state,
/// start = final, no arcs). The accepted language is `{ε}`. The chain
/// walker can concat this with adjacent slots; the slot contributes
/// nothing to the surface, which makes it obvious in any F4 path
/// traversal that the eager slot is unwired.
///
/// We chose **empty-accept** over **identity-on-Σ self-loop** because:
///   * Empty is more conservative — a path through an eager-slot stub
///     visibly fails to emit surface chars rather than emitting an
///     accidental wildcard match.
///   * Easier to detect "I forgot to wire this" in tests: a traversal
///     that should have used an eager rule will produce no surface
///     contribution from this slot.
///   * F5 will replace this with the real rule-list compilation; the
///     contract of "the slot's FST is constructed by this function"
///     stays the same.
pub fn build_eager_slot_stub(
    slot_def: &SlotDef,
    _alpha: &PhonruleAlphabet,
) -> Result<RustFstWrapper, SlotCompileError> {
    // F5: eager slot FST will branch on cell axes and emit per-rule
    // surface. For F4, return a one-state ε-acceptor so the chain
    // walker can concat it without killing the rest of the path.
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    b.set_final(s)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    let fst = b
        .finish()
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    let _ = slot_def;
    finalize_sort(fst)
}

// ---------------------------------------------------------------------------
// Stem anchor stub (F5 work).
// ---------------------------------------------------------------------------

/// Build the per-slot FST for a chain-referenced stem name (e.g. `root`,
/// `pres`, `past`) that does not appear in the inflection's `slots`.
///
/// **F5 boundary**: the stem is per-entry data (each entry brings its
/// own `stems: [..]`). F5 will bind the stem at compile time by
/// specialising the inflection FST per entry. For F4, stub the stem as
/// an identity-on-Σ self-loop that accepts any sequence of stem chars:
///   * one state, start = final
///   * one identity arc per Σ member (each `(label, label)` self-loop)
///   * also identity loops on the reserved boundary marker so the
///     phonrule wrap (which expects boundaries between morphemes) is
///     transparent.
///
/// The accepted language is `Σ*` (modulo reserved markers). Composing
/// the chain with a specific input acceptor at render time will then
/// constrain the stem to the entry's actual stem string. F5 will
/// replace this with a per-entry literal acceptor.
pub fn build_stem_anchor_stub(
    stem_name: &str,
    alpha: &PhonruleAlphabet,
) -> Result<RustFstWrapper, SlotCompileError> {
    // F5: the stem will be bound to a literal per-entry acceptor.
    // For F4, identity on Σ (plus stream markers) is the most permissive
    // shape that lets compose succeed without injecting fake stem chars.
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    b.set_final(s)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    use std::collections::HashSet;
    let mut seen: HashSet<Label> = HashSet::new();
    for label in alpha.sigma() {
        if seen.insert(label) {
            b.add_arc(s, label, label, s)
                .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
        }
    }
    for marker in [
        alpha.boundary_label(),
        alpha.word_start_label(),
        alpha.word_end_label(),
    ] {
        if seen.insert(marker) {
            b.add_arc(s, marker, marker, s)
                .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
        }
    }
    let fst = b
        .finish()
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    let _ = stem_name;
    finalize_sort(fst)
}

// ---------------------------------------------------------------------------
// Internals.
// ---------------------------------------------------------------------------

/// Apply the standard arc-sort discipline (output then input) so the
/// resulting FST is ready to sit on either side of `compose`.
fn finalize_sort(fst: RustFstWrapper) -> Result<RustFstWrapper, SlotCompileError> {
    let out_sorted = RustFstBackend::arc_sort_output(&fst)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    let in_sorted = RustFstBackend::arc_sort_input(&out_sorted)
        .map_err(|e| SlotCompileError::Backend(e.to_string()))?;
    Ok(in_sorted)
}

/// Helper used by the inflection-level compile to populate the
/// per-morpheme FST cache from a slice of candidate entries. Each
/// entry is compiled once via [`compile_morpheme`], and the result
/// is stored under `entry.name`.
///
/// Inflectional entries (those with `inflection.is_some()`) are
/// silently skipped — `compile_morpheme` would reject them with
/// `LexiconCompileError::NotAMorpheme` and the F4 contract is that
/// the inflection-level compiler hands us only candidate inflectionless
/// entries.
pub(crate) fn build_morpheme_fst_cache(
    morphemes: &[&Entry],
    alpha: &mut PhonruleAlphabet,
) -> Result<HashMap<String, RustFstWrapper>, SlotCompileError> {
    let mut cache: HashMap<String, RustFstWrapper> = HashMap::new();
    for entry in morphemes {
        if entry.inflection.is_some() {
            continue;
        }
        let (_lbl, fst) = compile_morpheme(entry, alpha)?;
        cache.insert(entry.name.node.clone(), fst);
    }
    Ok(cache)
}
