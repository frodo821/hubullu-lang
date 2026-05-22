//! Phonrule → FST compilation (F2 of FST migration).
//!
//! ## F2a — landed (this commit)
//!
//! Foundation pieces only:
//!
//!   * [`PhonruleAlphabet`](super::alphabet::PhonruleAlphabet) — closed
//!     alphabet with reserved control-marker labels.
//!   * [`compile_class`] / [`compile_class_complement`] — `CharClassDef`
//!     becomes a 2-state identity acceptor.
//!   * [`compile_map`] — `PhonMapDef` becomes a 2-state transducer.
//!
//! ## F2b — partially landed (this commit)
//!
//! Context- and pattern-element compilation:
//!
//!   * [`compile_context_elem`] / [`compile_context_sequence`] — each
//!     [`crate::ast::PhonContextElem`] becomes a small acceptor; a
//!     sequence concatenates them.
//!   * [`compile_pattern_sequence`] — same shape for the LHS of a
//!     rewrite rule (`PhonPattern::Range`).
//!   * [`ContextCompileError`] — class-lookup failures and the
//!     "syllable element not yet supported" diagnostic (plan §10
//!     non-goal 5).
//!
//! ## F2c1 — landed (this commit)
//!
//! Bracket machinery only — three small focused FSTs that the Karttunen
//! `@->` construction's bracket protocol needs:
//!
//!   * [`intro_brackets`] — Karttunen `Mark` step (plan §3.1): identity
//!     on Σ + nondeterministic ε-insertion of `<[+]>` / `<]+>`.
//!   * [`strip_brackets`] — Karttunen `Unmark` step (plan §3.4): identity
//!     on Σ + bracket-input-consumed-ε-output.
//!   * [`identity_outside_brackets`] — the "Σ\* skipping brackets"
//!     helper: identity on Σ AND identity on bracket labels.
//!
//! The replacement transducer, obligatory constraint, longest-leftmost
//! filter, and top-level rule-compile composition are F2c2..F2c5. See
//! `docs/proposals/f2-kaplan-kay-plan.md` §§3–4 and §6 for the full
//! chain context.
//!
//! ## F2c2 — landed (this commit)
//!
//! Karttunen step 3 — `Replace`:
//!
//!   * [`build_replacement_transducer`] — given a [`crate::ast::PhonRewriteRule`]
//!     and pre-compiled class/map tables, produces the FST that consumes
//!     the LHS pattern inside `<[+]>...<]+>` brackets and emits the RHS
//!     replacement (literal / null / map). Outside brackets the FST is
//!     identity on Σ. See [`replacement`] for the full construction.
//!
//! ## F2c5 — landed (this commit)
//!
//! Top-level composition + apply driver + validation harness:
//!
//!   * [`compile_rewrite_rule`] (in [`replace`]) — composes
//!     `intro_brackets ∘ constraint ∘ replacement ∘ leftmost ∘ strip_brackets`
//!     into a single per-rule FST. Includes a Phase 0 alphabet-discovery
//!     pass that pre-interns every literal symbol from LHS / RHS /
//!     context so per-stage Σ snapshots see the same closed alphabet.
//!   * [`compile_phonrule`] (in [`rule_seq`]) — composes all body
//!     rewrites + apply chain into a single phonrule FST, with cycle
//!     detection on the apply graph.
//!   * [`apply_phonrule_fst`] (in [`apply`]) — runtime iteration loop
//!     bounded by [`MAX_PHONRULE_ITER`] (default 64). Reports
//!     [`ApplyError::ConvergenceLimit`] on runaway rules — strictly
//!     better than `phonrule_eval`'s unbounded loop.
//!   * Validation harness (in [`validation_tests`]) — golden corpus
//!     (15 hand-curated rules + 100+ inputs) plus fuzz corpus (3000+
//!     deterministic-seeded inputs across the rule shapes). All asserts
//!     byte-identical match against `phonrule_eval`. 19/19 tests pass.
//!
//! F2c5 also enabled BOUNDARY transparency in context atoms
//! ([`compile_context_sequence`]) and reserved stream-marker identity
//! arcs in the bracket-protocol FSTs ([`intro_brackets`] /
//! [`strip_brackets`] / replacement's outside step / the eraser) so
//! the chain accepts `\0` morpheme markers transparently — matching
//! `phonrule_eval`'s semantics.
//!
//! ## Reading order
//!
//! 1. `docs/proposals/f2-kaplan-kay-plan.md` — the plan F2a/F2b/F2c1..c5 implement.
//! 2. [`super::alphabet`] — control-marker reservation and Σ management.
//! 3. [`class`] — class → acceptor compilation.
//! 4. [`map`] — map → transducer compilation.
//! 5. [`context`] — context-/pattern-elem → acceptor compilation.
//! 6. [`brackets`] — Karttunen bracket-protocol FSTs.
//! 7. [`replacement`] — Karttunen step 3 (replacement transducer).
//! 8. [`constraint`] — Karttunen step 2 (obligatory-context constraint).
//! 9. [`leftmost`] — F2c4 longest-leftmost filter.
//! 10. [`replace`] — F2c5 top-level rule compile (full Karttunen chain).
//! 11. [`rule_seq`] — F2c5 phonrule body + apply chain compile.
//! 12. [`apply`] — F2c5 iterative-to-convergence apply driver.
//! 13. [`tests`] — equivalence tests against `phonrule_eval`.
//! 14. [`validation_tests`] — F2c5 strict-gate harness.

pub mod apply;
pub mod brackets;
pub mod class;
pub mod constraint;
pub mod context;
pub mod directed_replace;
pub mod leftmost;
pub mod map;
pub mod replace;
pub mod replacement;
pub mod rule_seq;

#[cfg(test)]
mod brackets_tests;
#[cfg(test)]
mod constraint_tests;
#[cfg(test)]
mod leftmost_tests;
#[cfg(test)]
mod replacement_tests;
#[cfg(test)]
mod strategy_a_spike;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod validation_tests;

pub use apply::{apply_phonrule_fst, ApplyError, MAX_PHONRULE_ITER};
pub use brackets::{identity_outside_brackets, intro_brackets, strip_brackets};
pub use class::{compile_class, compile_class_complement, ClassCompileError};
pub use constraint::{build_obligatory_constraint, ConstraintCompileError};
pub use context::{
    compile_class_and_complement, compile_context_elem, compile_context_sequence,
    compile_pattern_sequence, neg_class_key, ContextCompileError,
};
pub use leftmost::{build_longest_leftmost_filter, LeftmostCompileError};
pub use map::compile_map;
pub use directed_replace::{build_directed_replacement, DirectedReplaceError};
pub use replace::{
    compile_rewrite_rule, compile_rewrite_rule_dispatch, RewriteCompileError, RewriteEngineOptions,
};
pub use replacement::{build_replacement_transducer, ReplacementCompileError};
pub use rule_seq::{compile_phonrule, PhonRuleAstResolver, RuleSeqCompileError};

// Re-export the alphabet handle from the F2a alphabet module so that
// callers can do `use hubullu::fst::phonrule::PhonruleAlphabet;` symmetrically
// with `compile_class` / `compile_map`. The canonical definition lives in
// `super::alphabet`.
pub use super::alphabet::PhonruleAlphabet;
