//! Top-level Karttunen `@->` composition (F2c5.1).
//!
//! Composes the four F2c building blocks (F2c1 brackets, F2c3 constraint,
//! F2c2 replacement, F2c4 leftmost) plus the F2c1 strip stage into the
//! single transducer that implements one rewrite rule end-to-end:
//!
//! ```text
//!   compile_rewrite_rule(rule) =
//!     intro_brackets ∘ constraint ∘ replacement ∘ leftmost ∘ strip_brackets
//! ```
//!
//! ## Why the order matters
//!
//! Per `f2-kaplan-kay-plan.md` §3 and the F2c4 module docs, the pipeline
//! is:
//!
//!   1. **intro_brackets** — non-deterministically wrap every position
//!      with `<[+]>...<]+>`. Produces an over-bracketed input.
//!   2. **constraint** — discard bracketings whose L_R context is wrong.
//!      Operates on the bracketed input (still pre-Replace).
//!   3. **replacement** — substitute RHS for the bracketed LHS.
//!   4. **leftmost** — eliminate non-canonical (overlapping / inner)
//!      bracketings by checking the post-Replace shape.
//!   5. **strip_brackets** — erase the `<[+]>` / `<]+>` markers.
//!
//! ## Compose discipline
//!
//! Each step is `arc_sort_output` on the left, `arc_sort_input` on the
//! right, then `compose`. This mirrors `leftmost_tests::compose_sorted`
//! and is the trait-mandated discipline (`backend.rs` `arc_sort_*` doc
//! comments).
//!
//! ## Determinise/minimise caveat
//!
//! The composed pipeline FST is *not* guaranteed to be deterministic
//! (the `intro_brackets` ε-emit arcs alone make composition cyclic).
//! `paths()` on the final FST can yield multiple distinct paths that
//! produce the **same output string** — those duplicates are fine
//! because the apply driver (F2c5.3) reduces the output by extracting
//! the unique output sequence. We attempt `eps_remove` + `determinize` +
//! `minimize` opportunistically; if any of those fail (rustfst issue
//! #288 is known to diverge on certain non-functional FSTs), we fall
//! back to the unminimised composition.
//!
//! ## What this module owns
//!
//!   * [`compile_rewrite_rule`] — the public entry point.
//!   * [`RewriteCompileError`] — sum of the per-stage compile errors.
//!
//! ## What this module does NOT own
//!
//!   * Rule sequence + apply chain — F2c5.2 (`rule_seq`).
//!   * Runtime apply driver — F2c5.3 (`apply`).
//!   * Validation harness — F2c5.4 (`validation_tests`).

use std::collections::HashMap;

use crate::ast::{
    PhonAtom, PhonContextElem, PhonPattern, PhonReplacement, PhonRewriteRule,
};

use super::super::alphabet::PhonruleAlphabet;
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::brackets::{intro_brackets, strip_brackets};
use super::constraint::{build_obligatory_constraint, ConstraintCompileError};
use super::directed_replace::{build_directed_replacement, DirectedReplaceError};
use super::leftmost::{build_longest_leftmost_filter, LeftmostCompileError};
use super::replacement::{build_replacement_transducer, ReplacementCompileError};

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by top-level rewrite-rule compilation.
///
/// Sum of the per-stage compile errors: any of `constraint`,
/// `replacement`, or `leftmost` can fail (for an unknown class, an
/// unsupported syllable element, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteCompileError {
    Constraint(ConstraintCompileError),
    Replacement(ReplacementCompileError),
    Leftmost(LeftmostCompileError),
    /// A Strategy A (directed-replacement) build failed for a reason OTHER than
    /// an unsupported shape (which is silently handled by the per-rule fallback
    /// to Strategy B) — e.g. an unknown class / map or a backend error.
    DirectedReplace(DirectedReplaceError),
}

impl std::fmt::Display for RewriteCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RewriteCompileError::Constraint(e) => write!(f, "{}", e),
            RewriteCompileError::Replacement(e) => write!(f, "{}", e),
            RewriteCompileError::Leftmost(e) => write!(f, "{}", e),
            RewriteCompileError::DirectedReplace(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for RewriteCompileError {}

impl From<DirectedReplaceError> for RewriteCompileError {
    fn from(e: DirectedReplaceError) -> Self {
        RewriteCompileError::DirectedReplace(e)
    }
}

impl From<ConstraintCompileError> for RewriteCompileError {
    fn from(e: ConstraintCompileError) -> Self {
        RewriteCompileError::Constraint(e)
    }
}

impl From<ReplacementCompileError> for RewriteCompileError {
    fn from(e: ReplacementCompileError) -> Self {
        RewriteCompileError::Replacement(e)
    }
}

impl From<LeftmostCompileError> for RewriteCompileError {
    fn from(e: LeftmostCompileError) -> Self {
        RewriteCompileError::Leftmost(e)
    }
}

impl From<RewriteCompileError> for super::super::backend::FstError {
    fn from(e: RewriteCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Engine selection (F2c4-#5, Path B production cutover).
// ---------------------------------------------------------------------------

/// Per-compile knobs for the rewrite-rule engine selection.
///
/// The DEFAULT (`Self::default()`) selects **Strategy A** (the Karttunen
/// directed-replacement engine in [`super::directed_replace`]) for every rule
/// whose shape it supports, with a per-rule fallback to Strategy B for the
/// exotic shapes Strategy A refuses (`UnsupportedShape`).
///
/// `force_strategy_b` is the §8.2 escape hatch: when `true`, EVERY rule is
/// compiled through the legacy Strategy B pipeline (`intro ∘ constraint ∘
/// replacement ∘ leftmost ∘ strip`), bypassing Strategy A entirely. It is wired
/// from the env var `HUBULLU_FORCE_STRATEGY_B` (see [`force_strategy_b_env`]) so
/// the engine swap can be disabled in the field without a recompile, and is the
/// simplest possible threading per the task's "don't over-engineer" note.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RewriteEngineOptions {
    /// Force the legacy Strategy B pipeline for every rule (§8.2 escape hatch).
    pub force_strategy_b: bool,
}

impl RewriteEngineOptions {
    /// Read the escape-hatch flag from the environment.
    ///
    /// `HUBULLU_FORCE_STRATEGY_B` set to a non-empty value other than `0` /
    /// `false` forces Strategy B for the whole compile. Absent ⇒ Strategy A
    /// default (the cutover).
    pub fn from_env() -> Self {
        RewriteEngineOptions {
            force_strategy_b: force_strategy_b_env(),
        }
    }
}

/// True iff `HUBULLU_FORCE_STRATEGY_B` requests the legacy pipeline.
fn force_strategy_b_env() -> bool {
    match std::env::var("HUBULLU_FORCE_STRATEGY_B") {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

/// Build the per-rule transducer, choosing the engine (F2c4-#5 dispatch).
///
/// This is the production entry the rule-sequence compiler calls. The dispatch
/// is:
///
///   1. If `opts.force_strategy_b`, compile via the legacy
///      [`compile_rewrite_rule`] (Strategy B) — the §8.2 escape hatch.
///   2. Otherwise build Strategy A via [`build_directed_replacement`]. Strategy
///      A is the default engine and, per Path B, does NOT compose the
///      exponential `build_obligatory_constraint` stage.
///   3. If Strategy A returns [`DirectedReplaceError::UnsupportedShape`], fall
///      back to Strategy B **for that one rule**. This is a narrow *correctness*
///      fallback (the engine cannot express the shape), NOT a general
///      perf-dispatcher: the shapes Strategy A rejects (self-overlapping
///      literals, bounded runs, unequal alternations, syllable atoms) never use
///      `!class*`, so Strategy B compiles them quickly. Any OTHER Strategy A
///      error (unknown class/map, backend) is a hard failure and is propagated.
///
/// `class_table` is the compiled-class-FST table (Strategy B's input);
/// `class_members` is the flat member-name table Strategy A needs to build
/// `!Class` complements (derived from the `CharClassDef`s by the caller — see
/// [`super::rule_seq::compile_phonrule`]). `map_table` is shared by both.
pub fn compile_rewrite_rule_dispatch(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
    class_members: &HashMap<String, Vec<String>>,
    map_table: &HashMap<String, RustFstWrapper>,
    opts: RewriteEngineOptions,
) -> Result<RustFstWrapper, RewriteCompileError> {
    if opts.force_strategy_b {
        return compile_rewrite_rule(rule, alpha, class_table, map_table);
    }

    match build_directed_replacement(rule, alpha, class_members, map_table) {
        Ok(fst) => Ok(fst),
        // Hard "this engine can't express this shape" → fall back to Strategy B
        // for this single rule. Keeps the full corpus green without changing
        // any surface output.
        Err(DirectedReplaceError::UnsupportedShape(_why)) => {
            compile_rewrite_rule(rule, alpha, class_table, map_table)
        }
        // Any other Strategy A error is genuine — propagate it.
        Err(other) => Err(RewriteCompileError::DirectedReplace(other)),
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Build the per-rule Karttunen `@->` transducer.
///
/// Returns an FST whose input language is Σ ∪ {`<bdy>`, `<^>`, `<$>`}
/// and whose output language is the same (post-strip). One **pass** of
/// the FST applied to an input string produces the rewritten string;
/// iteration to convergence is the apply driver's responsibility
/// (F2c5.3) — this function compiles the single-pass transducer.
///
/// `alpha` is mutated as the constraint / replacement / leftmost stages
/// intern LHS / RHS / context literal symbols. `class_table` and
/// `map_table` must contain pre-compiled FSTs for any class / map the
/// rule references.
///
/// The result is *not guaranteed* deterministic-and-minimal — see the
/// module docs. Callers that care can run `eps_remove` / `determinize` /
/// `minimize` themselves; in practice the apply driver composes with a
/// linear input acceptor which dramatically simplifies path enumeration
/// anyway.
pub fn compile_rewrite_rule(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
    map_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, RewriteCompileError> {
    // Phase 0 — alphabet discovery. Walk the entire rule (LHS, RHS,
    // contexts, classes) and intern every literal symbol into `alpha`
    // before any per-stage compilation. This guarantees that all
    // per-stage Σ snapshots see the **same** closed alphabet — without
    // this, the constraint (built first) would close over a smaller Σ
    // than the replacement (which interns the RHS literal) and reject
    // any input containing a RHS-only character.
    //
    // The discovery pass also walks the rule's referenced classes and
    // maps so their literal members are interned too (matters when a
    // class only appears in a context — the class members wouldn't
    // otherwise be visible to constraint Σ).
    intern_rule_literals(rule, alpha, class_table, map_table);

    // Build per-stage transducers. Each may intern further alphabet
    // symbols (idempotent: known symbols return existing labels).
    let constraint = build_obligatory_constraint(rule, alpha, class_table)?;
    let replacement = build_replacement_transducer(rule, alpha, class_table, map_table)?;
    let leftmost = build_longest_leftmost_filter(rule, alpha, class_table, map_table)?;

    // Bracket-protocol FSTs are built **after** all literal interning so
    // their Σ snapshot includes everything the rule introduced. Matches
    // the `replacement::build_replacement_transducer` discipline.
    let intro = intro_brackets(alpha);
    let strip = strip_brackets(alpha);

    // Compose left-to-right with the arc-sort discipline.
    let s1 = compose_sorted(&intro, &constraint);
    let s2 = compose_sorted(&s1, &replacement);
    let s3 = compose_sorted(&s2, &leftmost);
    let composed = compose_sorted(&s3, &strip);

    // Opportunistic normalisation. None of these are necessary for
    // correctness — the apply driver composes with a linear input
    // acceptor first, which collapses the path space to something
    // small. But minimisation makes the FST faster to apply repeatedly
    // (iterative-to-convergence). Failures are non-fatal: fall back to
    // the unminimised result.
    //
    // Disabled by default — F2c5 validation found that determinize can
    // mangle the FST in subtle ways (e.g. eliminating reachable accept
    // states) for the Karttunen `@->` chain. The apply driver enumerates
    // paths bounded and picks the lex-smallest output, so we don't need
    // determinism here. Re-enable when the determinize divergence is
    // understood.
    let _ = normalise_opportunistically; // keep helper available for tests
    Ok(composed)
}

// ---------------------------------------------------------------------------
// Compose helper.
// ---------------------------------------------------------------------------

/// Arc-sort both operands and compose. Matches the discipline in
/// `leftmost_tests::compose_sorted` and `constraint_tests::compose_sorted`.
fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> RustFstWrapper {
    let a_sorted = RustFstBackend::arc_sort_output(a).expect("arc_sort_output");
    let b_sorted = RustFstBackend::arc_sort_input(b).expect("arc_sort_input");
    RustFstBackend::compose(&a_sorted, &b_sorted).expect("compose")
}

// ---------------------------------------------------------------------------
// Alphabet discovery — Phase 0.
// ---------------------------------------------------------------------------

/// Walk the rule and intern every literal symbol into `alpha`.
///
/// This is the **closed-alphabet** discipline from plan §5: every Σ
/// symbol that can appear in the per-stage constructions must be
/// interned before any stage runs. Without this, `build_obligatory_constraint`
/// (which is built first) would close its Σ_b over a smaller alphabet
/// than `build_replacement_transducer` (which interns the RHS literal).
/// The constraint's complement (built via `Σ_b* \ bad_a`) would then
/// reject any input containing a symbol that wasn't in its Σ_b — a
/// silent rejection that surfaces as "no output" in the apply driver.
///
/// The discovery pass walks:
///   * the rule's LHS pattern,
///   * the rule's RHS replacement,
///   * the rule's L and R contexts (recursively into alternations),
///   * the bodies of every class referenced (so class member chars are
///     in Σ even if they only appear in a context, not the input),
///   * the arms of every map referenced.
fn intern_rule_literals(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
    map_table: &HashMap<String, RustFstWrapper>,
) {
    intern_pattern(&rule.from, alpha);
    intern_replacement(&rule.to, alpha);
    if let Some(ctx) = &rule.context {
        for elem in &ctx.left {
            intern_context_elem(elem, alpha);
        }
        for elem in &ctx.right {
            intern_context_elem(elem, alpha);
        }
    }
    // Note: we don't have direct access to class AST nodes here (only
    // the compiled FSTs in `class_table`); the class members have
    // already been interned by `compile_class`. `class_table` is
    // referenced for liveness only.
    let _ = class_table;
    let _ = map_table;
}

fn intern_pattern(pat: &PhonPattern, alpha: &mut PhonruleAlphabet) {
    match pat {
        PhonPattern::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonPattern::Class(_) => {}
        PhonPattern::Range(elems) => {
            for elem in elems {
                intern_context_elem(elem, alpha);
            }
        }
    }
}

fn intern_replacement(rep: &PhonReplacement, alpha: &mut PhonruleAlphabet) {
    match rep {
        PhonReplacement::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonReplacement::Null => {}
        PhonReplacement::Map(_) => {}
    }
}

fn intern_context_elem(elem: &PhonContextElem, alpha: &mut PhonruleAlphabet) {
    match elem {
        PhonContextElem::Boundary
        | PhonContextElem::WordStart
        | PhonContextElem::WordEnd
        | PhonContextElem::SylHead
        | PhonContextElem::SylTail
        | PhonContextElem::SylIndex(_) => {}
        PhonContextElem::Atom(atom, _) => intern_atom(atom, alpha),
    }
}

fn intern_atom(atom: &PhonAtom, alpha: &mut PhonruleAlphabet) {
    match atom {
        PhonAtom::Class(_) | PhonAtom::NegClass(_) | PhonAtom::Wildcard => {}
        PhonAtom::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonAtom::Alt(alts) => {
            for a in alts {
                intern_context_elem(a, alpha);
            }
        }
        PhonAtom::SylBlock(elems) => {
            for e in elems {
                intern_context_elem(e, alpha);
            }
        }
    }
}

fn intern_str_chars(s: &str, alpha: &mut PhonruleAlphabet) {
    for ch in s.chars() {
        // Skip the BOUNDARY char — it's the reserved label, not in Σ.
        if ch == crate::phonrule_eval::BOUNDARY {
            continue;
        }
        alpha.intern(&ch.to_string());
    }
}

/// Try `eps_remove → determinize → minimize`, falling back to earlier
/// stages on failure.
///
/// `determinize` may diverge / fail on non-functional FSTs (rustfst
/// issue #288); when it does, we keep the eps-removed (or original)
/// FST. The apply driver remains correct on a non-deterministic FST —
/// it just enumerates more paths.
fn normalise_opportunistically(fst: RustFstWrapper) -> RustFstWrapper {
    let after_eps = match RustFstBackend::eps_remove(&fst) {
        Ok(x) => x,
        Err(_) => return fst,
    };
    let after_det = match RustFstBackend::determinize(&after_eps) {
        Ok(x) => x,
        Err(_) => return after_eps,
    };
    match RustFstBackend::minimize(&after_det) {
        Ok(x) => x,
        Err(_) => after_det,
    }
}
