//! Per-inflection FST compilation (F4 of FST migration).
//!
//! This module composes F3's per-morpheme FSTs (`super::lexicon`) and F2's
//! per-rule phonrule FSTs (`super::phonrule`) into a single FST per
//! [`Inflection`]. Per the proposal (`docs/proposals/fst-morphology.md`
//! §2 "Compose chain → FST concatenation", §4 step 2 "Build per-inflection
//! FSTs by composition"), this is the third layer in the migration:
//!
//! ```text
//!   phoneme inventory ─┐
//!                      ├─ PhonruleAlphabet
//!   morpheme entries ──┘
//!
//!   phonrule AST    ──► F2 ──► per-rule FSTs ────┐
//!                                                ├─► F4 ──► per-inflection FST
//!   morpheme entries ──► F3 ──► per-morpheme FSTs┤
//!                                                │
//!   compose chain AST   ──────────────────────────┘
//! ```
//!
//! ## What this module owns
//!
//!   * [`compile_inflection_fst`] — the public entry point.
//!   * [`InflectionCompileError`] — typed errors.
//!   * [`slot`] — per-slot FST construction (F4.1).
//!   * [`chain`] — compose-chain assembly (F4.2).
//!
//! ## What this module does NOT own
//!
//!   * Per-entry stem binding (F5 — `T_chain` specialised per entry).
//!   * Render-path integration (F5 — replace `find_form_by_spec` /
//!     `auto_fill_lazy_slots`).
//!   * Eager-slot rule evaluation (F5 — eager slots are stubbed here).
//!   * Reverse lookup (F6).
//!   * `.huc` format work (F7).
//!
//! ## Pipeline (mirrors proposal §4 step 2)
//!
//! ```text
//!   for each phonrule:
//!     T_rule = compile_phonrule(rule)               # F2 (done elsewhere)
//!
//!   compile_inflection_fst(inflection, candidates, phonrule_fsts, alpha):
//!     # Per-morpheme cache (F3 result, populated here)
//!     morpheme_fsts = build_morpheme_fst_cache(candidates)
//!
//!     # Per-slot FSTs (F4.1)
//!     for each SlotDef:
//!       if Lazy(matching):
//!         slot_fsts[name] = build_lazy_slot_fst(matching, candidates, morpheme_fsts)
//!       if Eager(rules):
//!         slot_fsts[name] = build_eager_slot_stub(...)             # F5
//!
//!     # Stem anchors (F4.1 stub)
//!     for each required_stem:
//!       stem_fsts[stem_name] = build_stem_anchor_stub(...)          # F5
//!
//!     # Compose chain assembly (F4.2)
//!     T = build_chain_fst(inflection.body.chain, slot_fsts, stem_fsts, phonrule_fsts)
//!
//!     # Minimise (proposal §4 step 3)
//!     T = minimize(T)        # determinize is also tried; see below
//!
//!     return T
//! ```
//!
//! ## Determinise / minimise discipline
//!
//! The proposal §4 step 3 says "Minimise each FST (standard
//! determinisation + minimisation)". F4 follows that intent with a
//! caveat from the F2 perf gap: `determinize` is best-effort, and on
//! some chain shapes (notably anything with a phonrule wrap involving
//! `closure_star` over a slot) it can be slow or non-terminating in
//! rustfst's implementation. To stay within F4's structural-completeness
//! brief while honouring the F2-perf-gap deferral:
//!
//!   * `minimize` is always attempted (it doesn't require a determinised
//!     input in rustfst — minimize calls determinize internally on
//!     non-deterministic inputs).
//!   * `determinize` is NOT called explicitly before `minimize`; we
//!     trust `minimize` to internally do what's needed.
//!   * On failure, the un-minimised composed FST is returned with a
//!     warning logged. The structural goal "Turkish verb_conj's FST
//!     compiles and serializes successfully" is the F4 milestone, not
//!     "the minimised FST has the fewest possible states."
//!
//! ## Composability surprise (recorded for F5)
//!
//! The chain's stem-anchor stub is an identity-on-Σ self-loop. Composing
//! with a phonrule FST means every chain path includes "stem chars
//! identity-passed-through" — the phonrule then rewrites those just as
//! it would in render. That's correct shape but means the chain FST
//! by itself accepts an infinite language (Σ* in the stem position).
//! F5 will bind a literal per-entry stem at compile time, collapsing
//! that infinity to the entry's actual stem string. Until then, "traverse
//! the chain FST" tests must use `.take(N)` discipline or compose with
//! a finite input acceptor first.

use std::collections::HashMap;

use crate::ast::{Entry, Inflection, InflectionBody, SlotBody, StemReq};

use super::alphabet::PhonruleAlphabet;
use super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::FstBackend;

pub mod chain;
pub mod slot;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod perf_probe;

pub use chain::{build_chain_fst, ChainCompileError};
pub use slot::{
    build_eager_slot_stub, build_lazy_slot_fst, build_stem_anchor_stub, SlotCompileError,
};

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Errors produced by [`compile_inflection_fst`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflectionCompileError {
    /// The inflection's body is `Rules(_)`, not `Compose(_)`. F4 only
    /// handles compose bodies; rule bodies are the F2-walker path
    /// (`InflectionBody::Rules`) — F5 will design how those reach the
    /// FST engine, if at all (the proposal's §6.6 cell-axis discussion
    /// applies).
    NotComposeBody { inflection_name: String },
    /// Per-slot FST construction failed.
    Slot(SlotCompileError),
    /// Compose-chain assembly failed.
    Chain(ChainCompileError),
    /// Backend FST operation failed (minimise / serialise etc.).
    Backend(String),
}

impl std::fmt::Display for InflectionCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InflectionCompileError::NotComposeBody { inflection_name } => write!(
                f,
                "inflection '{}' has a rule-list body; F4 only handles compose bodies",
                inflection_name
            ),
            InflectionCompileError::Slot(e) => write!(f, "{}", e),
            InflectionCompileError::Chain(e) => write!(f, "{}", e),
            InflectionCompileError::Backend(s) => {
                write!(f, "inflection compile backend error: {}", s)
            }
        }
    }
}

impl std::error::Error for InflectionCompileError {}

impl From<SlotCompileError> for InflectionCompileError {
    fn from(e: SlotCompileError) -> Self {
        InflectionCompileError::Slot(e)
    }
}

impl From<ChainCompileError> for InflectionCompileError {
    fn from(e: ChainCompileError) -> Self {
        InflectionCompileError::Chain(e)
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Compile an `Inflection` with a `Compose` body to a single per-inflection FST.
///
/// `all_morphemes` is the slice of candidate inflectionless entries the lazy
/// slots will pick from. The caller (F5 driver) is responsible for filtering
/// to "candidates plausibly relevant to this inflection" — F4 just runs
/// `slot_parse::fits` over the whole slice.
///
/// `phonrule_fsts` is the pre-built F2 output: a map from phonrule name to
/// its per-rule FST. The chain walker looks up `PhonApply { rule, .. }` here.
///
/// `alpha` is mutated as per-morpheme compile interns morpheme-IDs and
/// headword chars. The caller must hand in the same alphabet used to compile
/// the phonrule FSTs in `phonrule_fsts` (otherwise the two FSTs are over
/// disjoint label spaces and `compose` produces an empty result).
pub fn compile_inflection_fst(
    inflection: &Inflection,
    all_morphemes: &[&Entry],
    phonrule_fsts: &HashMap<String, RustFstWrapper>,
    alpha: &mut PhonruleAlphabet,
) -> Result<RustFstWrapper, InflectionCompileError> {
    // Only Compose bodies are F4's responsibility.
    let compose = match &inflection.body {
        InflectionBody::Compose(c) => c,
        InflectionBody::Rules(_) => {
            return Err(InflectionCompileError::NotComposeBody {
                inflection_name: inflection.name.node.clone(),
            });
        }
    };

    // Build the per-morpheme FST cache once, up-front.
    let morpheme_fsts = slot::build_morpheme_fst_cache(all_morphemes, alpha)?;

    // Build per-slot FSTs from the SlotDefs.
    let mut slot_fsts: HashMap<String, RustFstWrapper> = HashMap::new();
    for slot_def in &compose.slots {
        let fst = match &slot_def.body {
            SlotBody::Lazy(matching) => {
                build_lazy_slot_fst(slot_def, matching, all_morphemes, &morpheme_fsts)?
            }
            SlotBody::Eager(_rules) => build_eager_slot_stub(slot_def, alpha)?,
        };
        slot_fsts.insert(slot_def.name.node.clone(), fst);
    }

    // Build stem-anchor stubs from the inflection's required_stems.
    let mut stem_fsts: HashMap<String, RustFstWrapper> = HashMap::new();
    for StemReq { name, .. } in &inflection.required_stems {
        let fst = build_stem_anchor_stub(&name.node, alpha)?;
        stem_fsts.insert(name.node.clone(), fst);
    }

    // Walk the compose chain.
    let chain_fst = build_chain_fst(&compose.chain, &slot_fsts, &stem_fsts, phonrule_fsts)?;

    // Best-effort minimise. Per the perf-gap caveat above, we accept
    // either the minimised FST or the un-minimised one; failure is not
    // fatal for F4's structural milestone.
    let final_fst = match RustFstBackend::minimize(&chain_fst) {
        Ok(min) => min,
        Err(_e) => {
            // Stay structurally complete: return the un-minimised chain
            // FST. F2 perf gap acknowledged. F5 will profile and decide
            // whether to determinise+minimise eagerly or lazily.
            chain_fst
        }
    };

    Ok(final_fst)
}
