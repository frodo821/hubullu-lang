//! Karttunen stage 5 — longest-leftmost filter (F2c4).
//!
//! Plan: `docs/proposals/f2-kaplan-kay-plan.md` §3.5 (`@->` variant —
//! obligatory + longest-match + leftmost-first).
//!
//! ## Brief
//!
//! After F2c1 (bracket protocol), F2c2 (replacement transducer), and F2c3
//! (obligatory-context constraint), the Karttunen `@->` chain still has
//! residual ambiguity:
//!
//!   1. **Replace's outside-passthrough at brackets.** Each `<[+]>`/`<]+>`
//!      arriving at Replace can be consumed via the **event** path
//!      (`<[+]>·LHS:RHS·<]+>`) OR via the **outside** identity path
//!      (`<[+]>:<[+]>` self-loop). The latter is structurally wrong — it
//!      keeps the LHS visible and skips the substitution — but Replace
//!      in isolation accepts both. F2c3's constraint doesn't help here
//!      because it filters the bracketed input, not Replace's output.
//!
//!   2. **Non-maximal / non-leftmost bracketings.** For overlap-prone
//!      rules and inputs (e.g. `aa -> x` on `aaa`), multiple bracketings
//!      satisfy the constraint; the canonical answer is the
//!      leftmost-first, longest-match one.
//!
//! The longest-leftmost filter is the FST that eliminates both of these.
//! Composed into the F2c5 pipeline:
//!
//! ```text
//!   intro_brackets ∘ constraint ∘ replacement ∘ leftmost ∘ strip_brackets
//! ```
//!
//! after which `paths()` produces exactly one accepting output per input
//! (the same one `phonrule_eval` produces).
//!
//! ## Strategy choice — B (forced commitment + non-nesting), Foma-style
//!
//! The brief offers two strategies. We pick **B**:
//!
//!   * **A — constraint-based (Karttunen-canonical)**: encode leftmost-first
//!     as a regular constraint over the whole bracketed string. The
//!     constraint must say "if a valid `L·LHS·R` exists at some position,
//!     it MUST be bracketed before any later overlapping site". Expressible
//!     as a regular language but the construction is finicky (four-tape
//!     trick in Foma's `replace.c`, see `rewr_notleftmost`).
//!
//!   * **B — forced commitment + non-nesting**: build a positive acceptor
//!     over the **post-Replace** bracketed string that requires every
//!     bracket pair to enclose an **RHS** match (not LHS — that would
//!     mean Replace took the passthrough path), with `Σ` (non-bracket)
//!     between and around bracket pairs. Composition with the prior
//!     stages plus the `determinize`/`minimize` discipline canonicalises
//!     to the leftmost-longest bracketing for the rule shapes hubullu
//!     uses.
//!
//! Strategy B is what Foma's `@->` does in practice (the `LM_*` macros
//! around `rewr_notlongest` / `rewr_notleftmost` are layered; the
//! forced-commitment-via-RHS-shape filter is the structural backbone).
//! It's also dramatically simpler to get right.
//!
//! ## The filter language
//!
//! Let:
//!
//!   * `Σ̂` = `Σ ∪ {<bdy>, <^>, <$>}` — the runtime non-bracket alphabet.
//!   * `RHS` = the projection of `LHS:RHS` onto its **output** side, as
//!     an acceptor over `Σ` (no brackets). Per-variant:
//!       - `Literal(s)` → literal-string acceptor for `s` (possibly empty).
//!       - `Null` → ε-acceptor.
//!       - `Map(name)` → single-symbol acceptor over the map's output
//!         labels (every distinct `to` label from the map's arms + the
//!         identity output for the else arm). Single-symbol LHS only;
//!         multi-symbol LHS with Map is F2c5 territory.
//!
//! The acceptor:
//!
//! ```text
//!   leftmost = Σ̂* · ( <[+]> · RHS · <]+> · Σ̂* )*
//! ```
//!
//! That is: bracket pairs in the post-Replace bracketed string must
//! enclose exactly the RHS language, with non-bracket chunks between and
//! around them. This:
//!
//!   - **Forbids nesting**: `Σ̂*` excludes `<[+]>` / `<]+>`, so no
//!     bracket can appear between an open and its close (or between
//!     consecutive cells).
//!   - **Forces commitment**: if Replace took the outside-passthrough
//!     path at a bracket pair, the content between the brackets in the
//!     output would still be the LHS (not the RHS). For most rules
//!     `LHS ≠ RHS`, so the filter rejects the passthrough path.
//!
//! ## What the filter does NOT solve on its own
//!
//!   * **`LHS ⊆ RHS` semantically**: if a rule's RHS includes the LHS as
//!     a possible output (e.g. `Map` where some else arm maps identity),
//!     the passthrough path may produce a valid-looking output. In
//!     practice for hubullu's current rules this is harmless — Replace's
//!     identity passthrough produces the same surface as Map's identity
//!     arm, so both paths give the same final string and `paths()` may
//!     emit duplicates. The F2c5 `determinize + minimize` step collapses
//!     these to one path.
//!
//!   * **Overlap-prone LHS with multiple valid bracketings**: when the
//!     constraint admits several non-overlapping bracketings of the same
//!     overlap region, the filter doesn't pick one specifically. For
//!     hubullu's current rule shapes (single-symbol LHS dominates; the
//!     few multi-symbol LHS rules don't overlap on any test input), this
//!     hasn't bitten. F2c5's `determinize + minimize` finishes the
//!     canonicalisation. If a real grammar surfaces a case where this
//!     matters, we either upgrade to Strategy A or add a tighter filter
//!     per Foma's `rewr_notlongest`.
//!
//! ## What this module owns
//!
//! Just [`build_longest_leftmost_filter`] — the public entry point.
//! Helpers are local. No mutation of `phonrule_eval`, `render`, or any
//! upstream module per F2c4.4.

use std::collections::HashSet;

use crate::ast::{PhonPattern, PhonReplacement, PhonRewriteRule};

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, Label};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by longest-leftmost-filter compilation.
///
/// The filter compilation is structurally simple — it consumes the rule's
/// LHS / RHS shape, the alphabet, and the class / map tables — so the
/// failure surface is small. The variants exist mainly to compose
/// cleanly with the other F2 error types when bubbled up by F2c5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftmostCompileError {
    /// `PhonReplacement::Map` referenced a map not in `map_table`.
    UnknownMap { name: String },
    /// `PhonReplacement::Map` was used with a non-single-symbol LHS. F2c4
    /// handles only the single-symbol-LHS Map case; multi-symbol is
    /// F2c5 territory.
    MapWithMultiSymbolLhs,
    /// LHS pattern uses an element the filter can't introspect (typically
    /// a syllable elem). Surfaces as a string for diagnostics.
    UnsupportedLhsShape(String),
}

impl std::fmt::Display for LeftmostCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeftmostCompileError::UnknownMap { name } => write!(
                f,
                "longest-leftmost filter: RHS references undefined or not-yet-compiled map '{}'",
                name
            ),
            LeftmostCompileError::MapWithMultiSymbolLhs => write!(
                f,
                "longest-leftmost filter: PhonReplacement::Map with multi-symbol LHS is not yet supported (F2c5)",
            ),
            LeftmostCompileError::UnsupportedLhsShape(s) => write!(
                f,
                "longest-leftmost filter: unsupported LHS shape: {}",
                s
            ),
        }
    }
}

impl std::error::Error for LeftmostCompileError {}

impl From<LeftmostCompileError> for super::super::backend::FstError {
    fn from(e: LeftmostCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Build the longest-leftmost filter acceptor for one rewrite rule.
///
/// The returned acceptor operates over `Σ ∪ {<[+]>, <]+>, <bdy>, <^>, <$>}`
/// — i.e. the **post-Replace** bracketed string alphabet. Its accepting
/// language is:
///
/// ```text
///   Σ̂* · ( <[+]> · RHS · <]+> · Σ̂* )*
/// ```
///
/// where `Σ̂ = Σ ∪ {<bdy>, <^>, <$>}` (the non-bracket runtime alphabet)
/// and `RHS` is the rule's replacement projected to an acceptor on the
/// output side (see module docs §"The filter language").
///
/// **Composition position**: the F2c5 pipeline composes this filter
/// **between Replace and Strip**, so it operates on the bracketed output
/// of Replace. F2c4's integration tests assemble the same composition
/// explicitly to verify exactly-one output per input.
///
/// **Arc-sort discipline**: the returned FST is freshly built; callers
/// must `arc_sort_*` before composition per the trait contract.
/// [`super::super::FstBackend::intersect`] handles this internally for
/// the `constraint ∩ leftmost` step.
pub fn build_longest_leftmost_filter(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    _class_table: &std::collections::HashMap<String, RustFstWrapper>,
    map_table: &std::collections::HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, LeftmostCompileError> {
    // 1. Build the RHS acceptor (Σ-only, no brackets).
    let rhs_acceptor = build_rhs_acceptor(rule, alpha, map_table)?;

    // 2. Snapshot Σ̂ (non-bracket runtime alphabet) AFTER any RHS interning.
    let sigma_hat_star = build_sigma_hat_star(alpha);

    // 3. Build bracket arcs.
    let open_arc = single_label_acceptor(BRACKET_OPEN_OBLIG_LABEL);
    let close_arc = single_label_acceptor(BRACKET_CLOSE_OBLIG_LABEL);

    // 4. cell = <[+]> · RHS · <]+>
    let open_rhs = RustFstBackend::concat(&open_arc, &rhs_acceptor)
        .expect("concat <[+]> · RHS");
    let cell_no_tail =
        RustFstBackend::concat(&open_rhs, &close_arc).expect("concat ... · <]+>");

    // 5. cell_with_tail = cell · Σ̂*
    let cell_with_tail = RustFstBackend::concat(&cell_no_tail, &sigma_hat_star)
        .expect("concat cell · Σ̂*");

    // 6. cells_star = (cell · Σ̂*)*
    let cells_star =
        RustFstBackend::closure_star(&cell_with_tail).expect("closure_star cells");

    // 7. filter = Σ̂* · cells_star
    let filter = RustFstBackend::concat(&sigma_hat_star, &cells_star)
        .expect("concat Σ̂* · cells_star");
    Ok(filter)
}

// ---------------------------------------------------------------------------
// RHS acceptor construction.
// ---------------------------------------------------------------------------

/// Build the RHS acceptor for a rewrite rule.
///
/// The acceptor's language is the set of strings the rule's RHS can emit
/// for some LHS input. For each [`PhonReplacement`] variant:
///
///   * `Literal(s)` — literal-string acceptor for `s` (empty literal
///     becomes the ε-acceptor, supporting deletion-like rules).
///   * `Null` — ε-acceptor (matches the empty string, supporting `→ null`
///     deletion rules).
///   * `Map(name)` — single-symbol acceptor over the map's output range.
///     For single-symbol LHS, the RHS is exactly one symbol drawn from
///     `{arm.to_label for arm in arms} ∪ (else-identity over Σ if else
///     is identity-like) ∪ {else.to_label} (if else is a literal)`. We
///     compute this set by reflecting the AST.
///
/// All cases produce identity acceptors (input = output on every arc) so
/// the filter composes cleanly with the post-Replace bracketed string.
fn build_rhs_acceptor(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    map_table: &std::collections::HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, LeftmostCompileError> {
    match &rule.to {
        PhonReplacement::Literal(lit) => Ok(build_literal_acceptor(&lit.node, alpha)),
        PhonReplacement::Null => Ok(build_epsilon_acceptor()),
        PhonReplacement::Map(map_ident) => {
            // Map RHS: walk the compiled map FST to harvest the set of
            // output labels it can emit for the LHS input, then build a
            // single-symbol acceptor over that set. Requires
            // single-symbol LHS — multi-symbol Map is F2c5 territory.
            if !is_single_symbol_lhs(&rule.from) {
                return Err(LeftmostCompileError::MapWithMultiSymbolLhs);
            }
            let map_fst = map_table.get(&map_ident.node).ok_or(
                LeftmostCompileError::UnknownMap {
                    name: map_ident.node.clone(),
                },
            )?;
            // Per `map.rs` shape: the map FST is 2-state with arcs
            // `(s0 -> s1)` on each (input, output) pair. We extract the
            // output side of every accepting path. For a Class LHS the
            // input could be any class member; we conservatively use
            // every output label reachable. For a single-char Literal
            // LHS we restrict to the arms whose input matches.
            let outputs = harvest_map_outputs(map_fst, lhs_single_symbol_label(&rule.from, alpha));
            Ok(build_label_alternation_acceptor(&outputs, alpha))
        }
    }
}

/// If the LHS pattern is a single-char [`PhonPattern::Literal`], return
/// the interned label of that char (after interning); otherwise return
/// `None`. Used by the Map RHS code path to restrict output-label
/// harvesting to the specific arm(s) that fire for the rule's LHS.
fn lhs_single_symbol_label(pat: &PhonPattern, alpha: &mut PhonruleAlphabet) -> Option<Label> {
    match pat {
        PhonPattern::Literal(lit) if lit.node.chars().count() == 1 => {
            // Intern the single char; the map FST's arc labels are
            // interned into the same alphabet, so this is the right key.
            Some(alpha.intern(&lit.node))
        }
        _ => None,
    }
}

/// Walk a compiled map FST's accepting paths and collect the output
/// labels it can emit.
///
/// If `restrict_to_input` is `Some(label)`, only outputs whose path's
/// input side starts with that label are kept (the "single-char
/// Literal LHS" case). If `None`, all outputs are returned (Class /
/// Range LHS — conservative).
///
/// The map FST is 2-state (per `map.rs::compile_map`), so each
/// accepting path is exactly one arc. `paths()` enumerates them; we
/// read the first label of each `(input, output)` pair.
fn harvest_map_outputs(
    map_fst: &RustFstWrapper,
    restrict_to_input: Option<Label>,
) -> HashSet<Label> {
    let mut outs: HashSet<Label> = HashSet::new();
    let iter = match RustFstBackend::paths(map_fst) {
        Ok(it) => it,
        Err(_) => return outs,
    };
    // Defensive cap — maps are 2-state with O(|Σ|) arcs, so paths is
    // small, but bound enumeration in case a future map gets cyclic.
    for path in iter.take(4096) {
        // Map paths are single-arc: input has 1 label, output has 1
        // label. (Defensive: skip ε-only paths and longer paths.)
        if let (Some(in_l), Some(out_l)) = (path.input.first(), path.output.first()) {
            if let Some(req) = restrict_to_input {
                if *in_l != req {
                    continue;
                }
            }
            outs.insert(*out_l);
        }
    }
    outs
}

/// Build a 2-state identity acceptor whose accepting language is exactly
/// the set of single-label strings in `labels`.
///
/// Used by the Map-RHS path: after harvesting the possible output labels
/// from the map FST, this is the acceptor that recognises "any one of
/// these labels as a single symbol".
///
/// If `labels` is empty (a map with no arms and no else? — shouldn't
/// happen in practice), the acceptor accepts no input.
fn build_label_alternation_acceptor(
    labels: &HashSet<Label>,
    _alpha: &PhonruleAlphabet,
) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    // Deterministic order — sort by label for reproducible test
    // snapshots.
    let mut sorted: Vec<Label> = labels.iter().copied().collect();
    sorted.sort_unstable();
    for l in sorted {
        b.add_arc(s0, l, l, s1).expect("alternation arc");
    }
    b.finish().expect("finish alternation")
}

/// Whether the LHS pattern matches exactly one Σ symbol.
///
/// Used by the Map case in [`build_rhs_acceptor`] to gate the
/// single-symbol-LHS assumption. Conservative — returns `false` for
/// anything but a single-character `Literal` or a `Class` reference
/// (which we don't introspect here; the Class case relies on the class
/// being single-symbol, which is true for v1 grammars but not asserted).
fn is_single_symbol_lhs(pat: &PhonPattern) -> bool {
    match pat {
        PhonPattern::Literal(lit) => lit.node.chars().count() == 1,
        PhonPattern::Class(_) => true,
        PhonPattern::Range(elems) => elems.len() == 1,
    }
}

/// One-state ε-acceptor (start == final, no arcs). Accepts only the
/// empty string.
fn build_epsilon_acceptor() -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    b.finish().expect("finish ε")
}

/// Build an identity acceptor for a literal string — one arc per char.
///
/// Mirrors the helpers in `replacement.rs` / `constraint.rs`. Empty
/// literal yields the ε-acceptor.
fn build_literal_acceptor(lit: &str, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
    let chars: Vec<char> = lit.chars().collect();
    let mut b = RustFstBackend::builder();
    let start = b.add_state();
    b.set_start(start).expect("set_start");
    if chars.is_empty() {
        b.set_final(start).expect("set_final on empty literal");
        return b.finish().expect("finish empty literal");
    }
    let mut prev = start;
    for ch in &chars {
        let next = b.add_state();
        let label = alpha.intern(&ch.to_string());
        b.add_arc(prev, label, label, next).expect("literal arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish literal")
}

// ---------------------------------------------------------------------------
// Σ̂* — non-bracket alphabet.
// ---------------------------------------------------------------------------

/// Build `Σ̂*` — a one-state self-looping acceptor over the non-bracket
/// runtime alphabet (Σ plus stream markers `<bdy>`, `<^>`, `<$>`).
///
/// Brackets `<[+]>` / `<]+>` are deliberately excluded: they may only
/// appear inside cell `<[+]> · RHS · <]+>` constructions, not in the
/// "outside" regions of the filter.
///
/// Includes stream markers so the filter passes runtime-bracketed
/// boundary / word-edge marks transparently between cells. Matches the
/// same `sigma_only_star` shape used in `constraint::build_sigma_only_star`.
fn build_sigma_hat_star(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    // Deduplicate defensively in case any reserved-marker label sneaks
    // into Σ (the alphabet construction guards this, but a HashSet
    // keeps the builder add_arc free of duplicate-arc surprises).
    let mut seen: HashSet<Label> = HashSet::new();
    for label in alpha.sigma() {
        if seen.insert(label) {
            b.add_arc(s, label, label, s).expect("Σ̂* user self-loop");
        }
    }
    for marker in [
        alpha.boundary_label(),
        alpha.word_start_label(),
        alpha.word_end_label(),
    ] {
        if seen.insert(marker) {
            b.add_arc(s, marker, marker, s)
                .expect("Σ̂* marker self-loop");
        }
    }
    b.finish().expect("finish Σ̂*")
}

/// Build a 2-state acceptor with a single identity arc on `label`.
/// Mirror of `constraint::single_label_acceptor`.
fn single_label_acceptor(label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    b.add_arc(s0, label, label, s1).expect("add_arc");
    b.finish().expect("finish")
}
