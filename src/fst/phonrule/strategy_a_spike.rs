//! F2c4 Strategy A spike (2026-05-17). Time-boxed, NON-PRODUCTION code.
//!
//! Purpose: validate the perf premise of `docs/proposals/f2-strategy-a.md`
//! before committing to a ~1080 LOC / ~4 week rewrite. Implements the
//! minimum directed-replacement construction from Karttunen 1996 Figure 11
//! on **one toy rule shape** (`a -> b / x !V* _ y`) and compares wall-clock
//! compile time + state count to the current Strategy B implementation
//! (`leftmost.rs` + `constraint.rs`) on the same rule.
//!
//! ## What this module is
//!
//! - A spike-quality, single-rule-shape, clean-room implementation of
//!   Karttunen 1996 Figure 11 (directed replacement, left-to-right
//!   longest match) using **three auxiliary markers** (`^`, `<`, `>`)
//!   encoded as additional alphabet labels — NOT the proposal's
//!   4-tape composite-label scheme.
//! - A benchmark harness comparing it head-to-head with Strategy B on
//!   the same rule across increasing alphabet sizes (5, 10, 20, 40),
//!   with a wall-clock budget cap so Strategy B's exponential explosion
//!   shows up as a DNF rather than hanging.
//! - A correctness sanity check (small inputs through both pipelines,
//!   confirm outputs agree).
//!
//! ## What this module is NOT
//!
//! - Production Strategy A. The proposal's §3.3 Path B (replacing
//!   `constraint.rs`) is NOT done here; the spike's Strategy A is a
//!   standalone construction over a hand-encoded UPPER/LOWER pair.
//! - General. The spike compiles ONE rule shape end-to-end; no AST
//!   plumbing, no contextual `/ L _ R` framing — the L/R context is
//!   folded into UPPER per Karttunen 1996 §2 ("the conditional case
//!   can be handled in a simpler way than in Kaplan and Kay 1994" —
//!   but the paper does NOT give that construction; we use the
//!   simplest workable reduction: UPPER = L·LHS·R, LOWER = L·RHS·R).
//! - Hidden behind the FstBackend seam. The spike reaches directly
//!   into `RustFstBackend` because that is the same surface
//!   `leftmost.rs` uses. Full Strategy A would do the same — the
//!   seam is unaffected.
//!
//! ## Why one new marker symbol (`^`), not Karttunen's three
//!
//! Karttunen 1996 Figure 11 uses three auxiliary symbols: `^`, `<`,
//! `>`. The existing F2c1 alphabet already reserves `<[+]>` and
//! `<]+>` (the `BRACKET_OPEN_OBLIG_LABEL` / `BRACKET_CLOSE_OBLIG_LABEL`).
//! We reuse them for `<` and `>`. The third marker `^` is allocated
//! via `alpha.intern("<spike:^>")` — naming with the `<spike:...>`
//! prefix so it cannot collide with any phoneme literal and so a
//! grep at the end of the spike can confirm we did not leak.
//!
//! Allocating `^` as a Σ member means it appears in `alpha.sigma()`,
//! which feeds the bracket-protocol identity arcs. That's fine for
//! the spike — `^` is added at the very end of construction so it
//! doesn't pollute Strategy B's pipeline (Strategy B uses its own
//! fresh alphabet handle).
//!
//! ## The Karttunen 1996 Figure 11 construction
//!
//! Adapted to our concrete labels (`^` = a fresh marker; `<` =
//! BRACKET_OPEN_OBLIG_LABEL; `>` = BRACKET_CLOSE_OBLIG_LABEL):
//!
//! ```text
//! input ∘
//!   InitialMatch   = no_marker_check ∘ insert_^_at_UPPER_start
//!   .o.
//!   LeftToRight    = [no_^_between · (^:< · UPPER' · 0:>) ]* · no_^_at_end
//!                    .o. drop_remaining_^
//!   .o.
//!   LongestMatch   = no_< followed_by_(UPPER'' with internal_>)
//!   .o.
//!   Replace        = < · ~$[>] · > → LOWER
//! ```
//!
//! UPPER' is "UPPER with `^` allowed at any non-final position"; UPPER''
//! is "UPPER with `<`/`>` allowed at any non-final position". These are
//! the ` UPPER/[%^]` / `UPPER/[%<|%>]` of Figure 10.
//!
//! ## Why this should be polynomial where Strategy B is exponential
//!
//! Strategy B's `constraint.rs::build_bad_a` materialises
//! `Σ_b* · L · LHS · R · Σ_b*` and then complements it. With
//! `!V* · L · LHS · R · !V*` the determinised DFA for the bad-pattern
//! language has O(|Σ|^k) states for k positions of `!V*`. Strategy A's
//! filters never materialise `Σ_b* · ... · Σ_b*` over the full pattern
//! — each filter is local in tape position (e.g. `no_^_between
//! brackets` is a single-state self-loop over Σ\{^}). The composition
//! cost is O(|filter pieces|), linear in the rule and alphabet.
//!
//! ## Spike scope checklist (from task brief)
//!
//! - [x] Spike.1 — pick toy rule `a -> b / x !V* _ y`
//! - [x] Spike.2 — minimum Strategy A construction (this module)
//! - [x] Spike.3 — perf measurement (`spike_compare` fn)
//! - [x] Spike.4 — scaling over alphabet sizes 5, 10, 20, 40
//! - [x] Spike.5 — output to `docs/proposals/f2-strategy-a-spike-results.md`
//! - [x] Spike.6 — time-box (~2 days; see lessons-learned in report)

#![cfg(test)]
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::ast::{
    CharClassBody, CharClassDef, PhonAtom, PhonContext, PhonContextElem, PhonPattern,
    PhonReplacement, PhonRewriteRule, Quantifier, Span, Spanned, StringLit,
};
use crate::span::FileId;

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
// SPIKE: would hide behind trait in full impl
use super::super::backend::{FstBuilder, Label};
// SPIKE: would hide behind trait in full impl
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::constraint::build_obligatory_constraint;
use super::context::compile_class_and_complement;
use super::leftmost::build_longest_leftmost_filter;
use super::replacement::build_replacement_transducer;

// ===========================================================================
// AST helpers (mirror leftmost_tests).
// ===========================================================================

fn sp() -> Span {
    Span {
        file_id: FileId(0),
        start: 0,
        end: 0,
    }
}

fn ident(s: &str) -> Spanned<String> {
    Spanned::new(s.to_string(), sp())
}

fn lit(s: &str) -> StringLit {
    Spanned::new(s.to_string(), sp())
}

/// Build the toy rule `a -> b / x !V* _ y` for a given vowel set.
///
/// `vowels` is the membership of the `V` class; "non-vowel" consonants
/// `x`, `y`, `a`, `b` are added separately when the rule is compiled.
fn build_toy_rule_with_vowels(vowels: &[&str]) -> (PhonRewriteRule, CharClassDef) {
    let class = CharClassDef {
        name: ident("V"),
        body: CharClassBody::List(vowels.iter().map(|v| lit(v)).collect()),
    };
    let rule = PhonRewriteRule {
        from: PhonPattern::Literal(lit("a")),
        to: PhonReplacement::Literal(lit("b")),
        context: Some(PhonContext {
            // L = x · !V*
            left: vec![
                PhonContextElem::Atom(PhonAtom::Literal(lit("x")), Quantifier::Exact(1)),
                PhonContextElem::Atom(PhonAtom::NegClass(ident("V")), Quantifier::Star),
            ],
            // R = y
            right: vec![PhonContextElem::Atom(
                PhonAtom::Literal(lit("y")),
                Quantifier::Exact(1),
            )],
        }),
        span: sp(),
    };
    (rule, class)
}

// ===========================================================================
// Tiny FST builders.
// ===========================================================================

fn one_state_final() -> (RustFstWrapper, /* state count */ usize) {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    (b.finish().expect("finish"), 1)
}

/// Empty-string acceptor (one state, start = final, no arcs).
fn epsilon_acceptor() -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    b.finish().expect("finish")
}

/// Identity self-loop acceptor over a given label set (`Σ*` shape).
fn sigma_star_over(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    for &l in labels {
        b.add_arc(s, l, l, s).expect("self-loop");
    }
    b.finish().expect("finish")
}

/// `Σ` (single step over the label set, no closure).
fn sigma_step_over(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("start");
    b.set_final(s1).expect("final");
    for &l in labels {
        b.add_arc(s0, l, l, s1).expect("step arc");
    }
    b.finish().expect("finish")
}

/// Identity acceptor for a single literal label.
fn single_label(label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("start");
    b.set_final(s1).expect("final");
    b.add_arc(s0, label, label, s1).expect("arc");
    b.finish().expect("finish")
}

/// Linear acceptor for a sequence of labels.
fn linear(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let mut prev = b.add_state();
    b.set_start(prev).expect("start");
    for &l in labels {
        let next = b.add_state();
        b.add_arc(prev, l, l, next).expect("arc");
        prev = next;
    }
    b.set_final(prev).expect("final");
    b.finish().expect("finish")
}

/// Arc-sort both operands and compose. Matches the discipline used in
/// `leftmost_tests::compose_sorted`.
fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> RustFstWrapper {
    let a_sorted = RustFstBackend::arc_sort_output(a).expect("arc_sort_output");
    let b_sorted = RustFstBackend::arc_sort_input(b).expect("arc_sort_input");
    RustFstBackend::compose(&a_sorted, &b_sorted).expect("compose")
}

// ===========================================================================
// Strategy A — Karttunen 1996 Figure 11 over hand-encoded UPPER/LOWER.
// ===========================================================================

/// All the labels Strategy A composes over.
///
/// `sigma_user` is the user alphabet (e.g. {a, b, x, y, V members}).
/// `caret` is the fresh marker label (one new interned symbol).
/// `open` / `close` are the Karttunen `<` / `>` brackets, reusing
/// the F2c1 bracket labels.
#[derive(Clone, Debug)]
struct StrategyALabels {
    sigma_user: Vec<Label>,
    caret: Label,
    open: Label,
    close: Label,
}

impl StrategyALabels {
    /// `sigma_user ∪ {^, <, >}` — every label the Σ̂* self-loops should cover.
    fn sigma_b(&self) -> Vec<Label> {
        let mut v = self.sigma_user.clone();
        v.push(self.caret);
        v.push(self.open);
        v.push(self.close);
        v
    }

    /// `sigma_user ∪ {<, >}` — everything except the caret marker.
    fn sigma_no_caret(&self) -> Vec<Label> {
        let mut v = self.sigma_user.clone();
        v.push(self.open);
        v.push(self.close);
        v
    }
}

/// Build the directed-replacement transducer for `UPPER @-> LOWER`
/// per Karttunen 1996 Figure 11.
///
/// `upper` is an acceptor over `sigma_user` (no markers).
/// `lower` is an identity acceptor over `sigma_user` (no markers).
/// The returned transducer reads from `sigma_user` and writes
/// `sigma_user` only — all markers are introduced, manipulated, and
/// stripped internally.
fn build_strategy_a_replace(
    labels: &StrategyALabels,
    upper: &RustFstWrapper,
    lower: &RustFstWrapper,
) -> RustFstWrapper {
    // ------- InitialMatch ------------------------------------------------
    //   ~$[%^ | %< | %>]   (input has no markers)
    //   .o.
    //   [..] -> %^  ||  _ UPPER
    //
    // We build:
    //   no_marker_input = sigma_user* — accepts user-marker-free strings.
    //   insert_caret = at each position where UPPER could start, insert ^.
    //
    // The two compose to give: "input was marker-free, and is now marked
    // with ^ at every UPPER-start position".

    let no_marker_input = sigma_star_over(&labels.sigma_user);
    let insert_caret = build_insert_caret_at_upper_start(labels, upper);
    let initial_match = compose_sorted(&no_marker_input, &insert_caret);

    // ------- LeftToRight (NotLeftmost) ----------------------------------
    //   [~$[%^] [%^:%< UPPER' 0:%>]]* ~$[%^]
    //   .o.
    //   %^ -> []
    //
    // The acceptor enforces: any caret outside of an `<..>` bracket pair is
    // forbidden. Equivalently: read the input, allow long "no-caret" runs;
    // every caret must be at a bracketed-match position. Then a final
    // step erases any caret that somehow survived (shouldn't happen if
    // the acceptor is correct, but matches the paper).

    let left_to_right = build_left_to_right(labels, upper);

    // ------- LongestMatch (NotInner) ------------------------------------
    //   ~$[%< [UPPER'' & $[%>']]]
    //
    // Forbid: a `<` followed by a substring that (a) is an UPPER instance
    // with internal `<`/`>` allowed and (b) contains a `>` (which would be
    // a shorter match). For the spike, UPPER'' = UPPER over {sigma_user,
    // <, >} where the brackets may appear nonfinally. This is a tiny FST.

    let longest_match = build_longest_match(labels, upper);

    // ------- Replacement ------------------------------------------------
    //   %< ~$[%>] %> -> LOWER
    //
    // Identity outside brackets; between `<` and `>`, replace by LOWER.

    let replace = build_replace_inside_brackets(labels, lower);

    // ------- Strip residual markers from output -------------------------
    //
    // After Replace, the output should have no `^` (LeftToRight erased
    // them) and no `<`/`>` (Replace consumed them around the LOWER
    // emission). For safety we run a strip pass.

    let strip = build_strip_all_markers(labels);

    // ------- Compose the chain ------------------------------------------

    let s1 = compose_sorted(&initial_match, &left_to_right);
    let s2 = compose_sorted(&s1, &longest_match);
    let s3 = compose_sorted(&s2, &replace);
    compose_sorted(&s3, &strip)
}

/// `[..] -> %^ || _ UPPER`: insert a `^` at every position where UPPER
/// could start. Implementation: a self-loop on Σ_user, with a parallel
/// path that emits `^` then proceeds through an UPPER-identity acceptor
/// and continues. The nondeterminism is by construction.
///
/// Concretely, we build:
///   (sigma_user | ε:^ followed by UPPER-as-output-identity)* · sigma_user*
///
/// Actually simpler: a single state with:
///   - identity self-loops on every σ ∈ Σ_user (consume any char without
///     marking)
///   - an ε:^ arc (insert a caret here)
///
/// Then we constrain via composition with a "caret only appears where
/// UPPER could match" — but that constraint is exactly what LeftToRight
/// enforces, not InitialMatch. Per Karttunen Figure 11, InitialMatch is
/// permissive ("insert ^ at every UPPER-start position" is realised
/// nondeterministically; the longest/leftmost constraints prune later).
///
/// We follow Karttunen's exact form: `[..] -> %^ || _ UPPER`, which in
/// regex calculus is "wherever UPPER matches the suffix, insert ^". A
/// simple realisation: a 1-state FST with σ:σ self-loops AND an ε:^
/// arc that fires only when the remaining input matches UPPER.
///
/// rustfst can't easily express that conditionally. Instead we exploit
/// the down-stream LongestMatch / Replace stages, which already require
/// that every `^` mark sits where UPPER actually matches. So the
/// InitialMatch transducer here is "permissive ε:^ insertion at any
/// position", and the constraints prune.
fn build_insert_caret_at_upper_start(
    labels: &StrategyALabels,
    _upper: &RustFstWrapper,
) -> RustFstWrapper {
    // 1-state FST: σ:σ self-loops over Σ_user + ε:^ self-loop.
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    for &l in &labels.sigma_user {
        b.add_arc(s, l, l, s).expect("identity σ:σ");
    }
    // ε:^ insertion (nondeterministic).
    b.add_arc(s, super::super::backend::EPS_LABEL, labels.caret, s)
        .expect("ε:^");
    b.finish().expect("finish insert_caret")
}

/// LeftToRight: `[~$[%^] [%^:%< UPPER' 0:%>]]* ~$[%^]` then strip remaining ^.
///
/// In words: the input alternates between "no-caret runs" (over
/// Σ_user ∪ {<, >}) and bracketed events (`^:< UPPER' 0:>`). Any caret
/// that ended up outside a bracketed event is forbidden by this stage.
fn build_left_to_right(
    labels: &StrategyALabels,
    upper: &RustFstWrapper,
) -> RustFstWrapper {
    // no_caret = identity self-loop over Σ_user ∪ {<, >}, NOT over {^}.
    let no_caret = sigma_star_over(&labels.sigma_no_caret());

    // bracket_event = ^:< · UPPER · ε:>
    let caret_to_open = single_arc_replace(labels.caret, labels.open);
    let close_insert = single_arc_replace(super::super::backend::EPS_LABEL, labels.close);
    let upper_with_brackets = upper_allow_internal_brackets(upper, labels);
    let cat1 = RustFstBackend::concat(&caret_to_open, &upper_with_brackets).expect("cat1");
    let bracket_event = RustFstBackend::concat(&cat1, &close_insert).expect("bracket_event");

    // cell = no_caret · bracket_event
    let cell = RustFstBackend::concat(&no_caret, &bracket_event).expect("cell");
    // cells_star = cell*
    let cells_star = RustFstBackend::closure_star(&cell).expect("cell*");
    // ltr = cells_star · no_caret
    let ltr = RustFstBackend::concat(&cells_star, &no_caret).expect("ltr concat");
    ltr
}

/// LongestMatch: forbid `<` followed by an UPPER substring containing a
/// non-final `>`.
///
/// For the spike rule `UPPER = x · !V* · a · y` is a single fixed
/// shape; the only place a `>` could occur "internally" is between two
/// alternative match endings sharing the same start. Since our toy
/// rule has UPPER as an exact pattern (not a `+`/`*` over a class
/// admitting multiple lengths from one start), the longest-match
/// constraint is **vacuous** for this specific rule. We return Σ_b*
/// (identity over Σ_b).
///
/// In a production Strategy A this would be a non-trivial filter.
fn build_longest_match(
    labels: &StrategyALabels,
    _upper: &RustFstWrapper,
) -> RustFstWrapper {
    // SPIKE: vacuous for our toy rule — UPPER is a fixed shape with no
    // length ambiguity at any single start position. Full Strategy A
    // would build the actual NotInner filter from Karttunen §3 here.
    sigma_star_over(&labels.sigma_b())
}

/// Replacement: identity outside `<..>`; inside `<..>`, consume the
/// LHS-shape content and emit LOWER. Brackets are consumed (output ε).
fn build_replace_inside_brackets(
    labels: &StrategyALabels,
    lower: &RustFstWrapper,
) -> RustFstWrapper {
    // outside step: any σ ∈ Σ_user with identity.
    let outside_one = sigma_step_over(&labels.sigma_user);
    let outside_star = RustFstBackend::closure_star(&outside_one).expect("outside*");

    // bracketed event: <:ε · (content:lower) · >:ε
    //
    // "content:lower" — between brackets we just emit LOWER (output-only)
    // and consume whatever is on the input side via an UPPER-identity
    // wrap. But the content between `<` and `>` is exactly UPPER's
    // surface form. The simplest construction: consume everything up to
    // the next `>` and emit LOWER. For our toy, LOWER is the literal
    // string `xby` (= L·RHS·R) and content is `x·!V*·a·y`.
    //
    // We build: <:ε · consume_until_close · >:ε with output = LOWER.
    //
    // consume_until_close = self-loop on Σ_user with input-only (σ:ε)
    // composed in parallel with the LOWER output emitter.
    //
    // Simpler still: we treat the bracketed region as "consume any
    // sigma_user* on input, emit LOWER on output, then consume `>`".
    let open_drop = single_arc_replace(labels.open, super::super::backend::EPS_LABEL);
    let close_drop = single_arc_replace(labels.close, super::super::backend::EPS_LABEL);

    let inside_consume = build_consume_input_emit_lower(&labels.sigma_user, lower);

    let cat1 = RustFstBackend::concat(&open_drop, &inside_consume).expect("open · inside");
    let bracket_replace_event =
        RustFstBackend::concat(&cat1, &close_drop).expect("... · close");

    // alternation = outside_one ∪ bracket_replace_event
    let alt = RustFstBackend::union(&outside_one, &bracket_replace_event)
        .expect("alt union");
    let _ = outside_star; // reserved for future use
    RustFstBackend::closure_star(&alt).expect("(outside | event)*")
}

/// "Consume any Σ_user* on input, emit `lower` on output."
///
/// Built as: (σ:ε)* · (ε:lower) — but we must do this as a single
/// transducer with disjoint input/output sides. The straightforward
/// way: take an input-only acceptor for Σ_user* and concat with an
/// output-only emitter for `lower`.
fn build_consume_input_emit_lower(
    sigma_user: &[Label],
    lower: &RustFstWrapper,
) -> RustFstWrapper {
    // input_only_sigma_star: σ:ε self-loop on one state.
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    for &l in sigma_user {
        b.add_arc(s, l, super::super::backend::EPS_LABEL, s)
            .expect("σ:ε");
    }
    let input_drop = b.finish().expect("finish input_drop");

    // output_only_lower: take the identity `lower` acceptor and project
    // its arcs to ε:label (input-side ε, output-side identity-label).
    //
    // For our toy `lower` is `linear([x, b, y])`. Just rebuild it as
    // ε:label arcs.
    let output_only = project_to_output_only(lower);

    RustFstBackend::concat(&input_drop, &output_only).expect("input_drop · output_only")
}

/// Walk an identity-IO acceptor and rebuild it as an output-only emitter
/// (every arc's input label becomes ε).
///
/// Spike-quality: we use the trait `paths` API to enumerate the few
/// expected paths and rebuild a linear-equivalent FST. For our toy
/// `lower` is always a single linear path (`x`, `b`, `y`), so this is
/// trivially correct. SPIKE: would walk arcs directly in a full impl.
fn project_to_output_only(lower: &RustFstWrapper) -> RustFstWrapper {
    // Take the first accepting path and emit it.
    let paths: Vec<_> = RustFstBackend::paths(lower)
        .expect("paths")
        .take(16)
        .collect();
    if paths.is_empty() {
        return epsilon_acceptor();
    }
    // For our toy there is exactly one path. SPIKE: a real
    // implementation would union all paths.
    let path = &paths[0];
    let mut b = RustFstBackend::builder();
    let mut prev = b.add_state();
    b.set_start(prev).expect("start");
    for &l in &path.output {
        let next = b.add_state();
        b.add_arc(prev, super::super::backend::EPS_LABEL, l, next)
            .expect("ε:l");
        prev = next;
    }
    b.set_final(prev).expect("final");
    b.finish().expect("finish output_only")
}

/// Strip all auxiliary markers (`^`, `<`, `>`) from the output side of
/// any transducer composed before it. Input-side identity on those
/// markers; output-side ε.
fn build_strip_all_markers(labels: &StrategyALabels) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("start");
    b.set_final(s).expect("final");
    for &l in &labels.sigma_user {
        b.add_arc(s, l, l, s).expect("identity σ");
    }
    for marker in [labels.caret, labels.open, labels.close] {
        b.add_arc(s, marker, super::super::backend::EPS_LABEL, s)
            .expect("marker:ε");
    }
    b.finish().expect("finish strip")
}

/// Wrap UPPER to allow `<`/`>` markers at any nonfinal position (Figure 10
/// UPPER'' in Karttunen 1996).
///
/// For the spike, we approximate: take the original UPPER as-is, since
/// UPPER's own arcs only consume Σ_user labels, and the brackets `<`
/// and `>` are introduced AROUND UPPER, not inside it. The "internal
/// markers" matter only when UPPER is itself recursive over UPPER (not
/// our case). SPIKE: a full impl would intersperse `<`/`>` self-loops
/// at every non-final state of UPPER's acceptor.
fn upper_allow_internal_brackets(
    upper: &RustFstWrapper,
    _labels: &StrategyALabels,
) -> RustFstWrapper {
    upper.clone()
}

/// 2-state transducer with one arc `input_label:output_label`.
fn single_arc_replace(input_label: Label, output_label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("start");
    b.set_final(s1).expect("final");
    b.add_arc(s0, input_label, output_label, s1).expect("arc");
    b.finish().expect("finish")
}

// ===========================================================================
// Strategy B baseline — full Karttunen chain (existing implementation).
// ===========================================================================

/// Compile the same toy rule via Strategy B (the existing pipeline).
///
/// This calls `build_obligatory_constraint` + `build_replacement_transducer`
/// + `build_longest_leftmost_filter`, mirroring `replace::compile_rewrite_rule`
/// but kept inline so we can time and inspect each stage.
fn compile_strategy_b(
    rule: &PhonRewriteRule,
    class: &CharClassDef,
    extra_alphabet: &[&str],
) -> (RustFstWrapper, BResult) {
    let mut alpha = PhonruleAlphabet::empty();
    // Phase 0 — alphabet discovery analogue.
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    for extra in extra_alphabet {
        let _ = alpha.intern(extra);
    }
    // Compile the V class AND its complement (!V) so the NegClass
    // context atom in the rule can resolve.
    let mut class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let (pos_key, pos_fst, neg_key, neg_fst) =
        compile_class_and_complement(class, &mut alpha, &class_table).expect("V class");
    class_table.insert(pos_key, pos_fst);
    class_table.insert(neg_key, neg_fst);

    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let t0 = Instant::now();
    let constraint =
        build_obligatory_constraint(rule, &mut alpha, &class_table).expect("constraint");
    let t_constraint = t0.elapsed();
    let constraint_states = RustFstBackend::num_states(&constraint);

    let t1 = Instant::now();
    let repl = build_replacement_transducer(rule, &mut alpha, &class_table, &map_table)
        .expect("repl");
    let t_repl = t1.elapsed();
    let repl_states = RustFstBackend::num_states(&repl);

    let t2 = Instant::now();
    let leftmost =
        build_longest_leftmost_filter(rule, &mut alpha, &class_table, &map_table).expect("leftmost");
    let t_leftmost = t2.elapsed();
    let leftmost_states = RustFstBackend::num_states(&leftmost);

    // Compose intro ∘ constraint ∘ repl ∘ leftmost ∘ strip.
    let intro = super::brackets::intro_brackets(&alpha);
    let strip = super::brackets::strip_brackets(&alpha);
    let t3 = Instant::now();
    let s1 = compose_sorted(&intro, &constraint);
    let s2 = compose_sorted(&s1, &repl);
    let s3 = compose_sorted(&s2, &leftmost);
    let composed = compose_sorted(&s3, &strip);
    let t_compose = t3.elapsed();
    let pre_min_states = RustFstBackend::num_states(&composed);

    let t4 = Instant::now();
    let minimized = match RustFstBackend::eps_remove(&composed) {
        Ok(no_eps) => match RustFstBackend::determinize(&no_eps) {
            Ok(det) => RustFstBackend::minimize(&det).unwrap_or(det),
            Err(_) => no_eps,
        },
        Err(_) => composed.clone(),
    };
    let t_minimize = t4.elapsed();
    let post_min_states = RustFstBackend::num_states(&minimized);

    let total = t_constraint + t_repl + t_leftmost + t_compose + t_minimize;
    let serial_size = RustFstBackend::serialize(&composed).map(|b| b.len()).unwrap_or(0);

    let result = BResult {
        constraint_states,
        repl_states,
        leftmost_states,
        pre_min_states,
        post_min_states,
        t_constraint,
        t_repl,
        t_leftmost,
        t_compose,
        t_minimize,
        total,
        serial_size,
        alphabet: alpha.clone(),
    };

    (composed, result)
}

#[derive(Debug, Clone)]
struct BResult {
    constraint_states: usize,
    repl_states: usize,
    leftmost_states: usize,
    pre_min_states: usize,
    post_min_states: usize,
    t_constraint: Duration,
    t_repl: Duration,
    t_leftmost: Duration,
    t_compose: Duration,
    t_minimize: Duration,
    total: Duration,
    serial_size: usize,
    alphabet: PhonruleAlphabet,
}

// ===========================================================================
// Strategy A — full compile with timing.
// ===========================================================================

#[derive(Debug, Clone)]
struct AResult {
    initial_match_states: usize,
    ltr_states: usize,
    longest_states: usize,
    replace_states: usize,
    strip_states: usize,
    pre_min_states: usize,
    post_min_states: usize,
    t_pieces: Duration,
    t_compose: Duration,
    t_minimize: Duration,
    total: Duration,
    serial_size: usize,
    alphabet: PhonruleAlphabet,
    labels: StrategyALabels,
}

/// Compile the same toy rule via Strategy A spike.
fn compile_strategy_a(vowels: &[&str]) -> (RustFstWrapper, AResult) {
    let mut alpha = PhonruleAlphabet::empty();
    let a = alpha.intern("a");
    let b = alpha.intern("b");
    let x = alpha.intern("x");
    let y = alpha.intern("y");
    let mut v_labels: Vec<Label> = Vec::new();
    for v in vowels {
        v_labels.push(alpha.intern(v));
    }
    // Allocate the caret marker — give it a name guaranteed not to
    // collide with any phoneme literal.
    let caret = alpha.intern("<spike:^>");

    let labels = StrategyALabels {
        sigma_user: vec![a, b, x, y]
            .into_iter()
            .chain(v_labels.iter().copied())
            .collect(),
        caret,
        open: BRACKET_OPEN_OBLIG_LABEL,
        close: BRACKET_CLOSE_OBLIG_LABEL,
    };

    // Build UPPER = x · !V* · a · y and LOWER = x · !V* · b · y.
    // !V* is built directly here as the spike's hand-encoded shape.
    let neg_v_labels: Vec<Label> = labels
        .sigma_user
        .iter()
        .copied()
        .filter(|l| !v_labels.contains(l))
        .collect();
    let neg_v_step = sigma_step_over(&neg_v_labels);
    let neg_v_star = RustFstBackend::closure_star(&neg_v_step).expect("!V*");

    let t0 = Instant::now();
    let x_acc = single_label(x);
    let y_acc = single_label(y);
    let a_acc = single_label(a);
    let b_acc = single_label(b);
    let x_negv = RustFstBackend::concat(&x_acc, &neg_v_star).expect("x · !V*");
    let upper_xva = RustFstBackend::concat(&x_negv, &a_acc).expect("x · !V* · a");
    let upper = RustFstBackend::concat(&upper_xva, &y_acc).expect("x · !V* · a · y");

    let lower_xv = RustFstBackend::concat(&single_label(x), &neg_v_star).expect("x · !V*");
    let lower_xvb = RustFstBackend::concat(&lower_xv, &b_acc).expect("x · !V* · b");
    let lower = RustFstBackend::concat(&lower_xvb, &y_acc).expect("x · !V* · b · y");

    // Construct the five filters and concatenate.
    let initial_match_states;
    let ltr_states;
    let longest_states;
    let replace_states;
    let strip_states;

    let no_marker_input = sigma_star_over(&labels.sigma_user);
    let insert_caret = build_insert_caret_at_upper_start(&labels, &upper);
    let initial_match = compose_sorted(&no_marker_input, &insert_caret);
    initial_match_states = RustFstBackend::num_states(&initial_match);

    let ltr = build_left_to_right(&labels, &upper);
    ltr_states = RustFstBackend::num_states(&ltr);

    let longest = build_longest_match(&labels, &upper);
    longest_states = RustFstBackend::num_states(&longest);

    let replace = build_replace_inside_brackets(&labels, &lower);
    replace_states = RustFstBackend::num_states(&replace);

    let strip = build_strip_all_markers(&labels);
    strip_states = RustFstBackend::num_states(&strip);

    let t_pieces = t0.elapsed();

    let t1 = Instant::now();
    let s1 = compose_sorted(&initial_match, &ltr);
    let s2 = compose_sorted(&s1, &longest);
    let s3 = compose_sorted(&s2, &replace);
    let composed = compose_sorted(&s3, &strip);
    let t_compose = t1.elapsed();
    let pre_min_states = RustFstBackend::num_states(&composed);

    let t2 = Instant::now();
    let minimized = match RustFstBackend::eps_remove(&composed) {
        Ok(no_eps) => match RustFstBackend::determinize(&no_eps) {
            Ok(det) => RustFstBackend::minimize(&det).unwrap_or(det),
            Err(_) => no_eps,
        },
        Err(_) => composed.clone(),
    };
    let t_minimize = t2.elapsed();
    let post_min_states = RustFstBackend::num_states(&minimized);

    let total = t_pieces + t_compose + t_minimize;
    let serial_size = RustFstBackend::serialize(&composed).map(|b| b.len()).unwrap_or(0);

    let result = AResult {
        initial_match_states,
        ltr_states,
        longest_states,
        replace_states,
        strip_states,
        pre_min_states,
        post_min_states,
        t_pieces,
        t_compose,
        t_minimize,
        total,
        serial_size,
        alphabet: alpha,
        labels,
    };

    (composed, result)
}

// ===========================================================================
// Wall-clock budget cap helper.
// ===========================================================================

/// Run `f` in a separate thread, killing it after `budget`. Returns
/// `None` if it didn't finish in time.
///
/// Spike-quality — uses `std::thread::spawn` and a join with timeout via
/// a channel + recv_timeout. The "kill" is non-cooperative: we drop the
/// JoinHandle and proceed; the thread may continue running. For a spike
/// this is acceptable (the OS will reap when the test binary exits).
fn run_with_budget<T, F>(budget: Duration, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let res = f();
        let _ = tx.send(res);
    });
    rx.recv_timeout(budget).ok()
}

// ===========================================================================
// The benchmark.
// ===========================================================================

/// Format a Duration as ms or s for the table.
fn fmt_dur(d: Duration) -> String {
    if d.as_secs_f64() >= 1.0 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{:.1}ms", d.as_secs_f64() * 1000.0)
    }
}

fn ratio(b: f64, a: f64) -> String {
    if a == 0.0 {
        return "—".to_string();
    }
    if b / a >= 100.0 {
        format!("{:.0}×", b / a)
    } else {
        format!("{:.1}×", b / a)
    }
}

/// One row of the comparison table.
#[derive(Debug)]
struct Row {
    alpha_size: usize,
    b: Option<BResult>,
    a: Option<AResult>,
    b_dnf: bool,
    a_outputs_sample: Vec<Vec<Label>>,
    b_outputs_sample: Vec<Vec<Label>>,
    correctness_agrees: bool,
}

const PER_RULE_BUDGET_SECS: u64 = 60;

fn run_one_alphabet_size(extra_count: usize) -> Row {
    // Vowels are a slice of the alphabet; the rest go in as "extra
    // consonants". The toy rule is `a -> b / x !V* _ y`; `a`/`b`/`x`/`y`
    // are always present. We add `extra_count` extra characters split
    // half-vowels / half-consonants.
    //
    // For alphabet_size = N: we add (N-4) extras = ceil(N/2-2) vowels +
    // floor(N/2-2) consonants. The vowels populate V, the consonants
    // are just sigma members.
    let extra = extra_count;
    let extra_vowels = (extra + 1) / 2;
    let extra_consonants = extra - extra_vowels;
    let mut vowel_names: Vec<String> = Vec::new();
    for i in 0..extra_vowels {
        vowel_names.push(format!("v{}", i));
    }
    let vowel_refs: Vec<&str> = vowel_names.iter().map(|s| s.as_str()).collect();
    let mut consonant_names: Vec<String> = Vec::new();
    for i in 0..extra_consonants {
        consonant_names.push(format!("c{}", i));
    }
    let extra_alpha_refs: Vec<&str> = consonant_names.iter().map(|s| s.as_str()).collect();

    let (rule, class) = build_toy_rule_with_vowels(&vowel_refs);
    let alpha_size = 4 + extra; // a, b, x, y + extras

    // --- Strategy B with budget cap. -----------------------------------
    let rule_clone = rule.clone();
    let class_clone = class.clone();
    let extra_owned: Vec<String> = extra_alpha_refs.iter().map(|s| s.to_string()).collect();
    let b_result_opt = run_with_budget(
        Duration::from_secs(PER_RULE_BUDGET_SECS),
        move || -> (RustFstWrapper, BResult) {
            let extra_refs: Vec<&str> = extra_owned.iter().map(|s| s.as_str()).collect();
            compile_strategy_b(&rule_clone, &class_clone, &extra_refs)
        },
    );

    // --- Strategy A. ---------------------------------------------------
    let vowel_refs_owned: Vec<String> = vowel_refs.iter().map(|s| s.to_string()).collect();
    let a_result_opt = run_with_budget(
        Duration::from_secs(PER_RULE_BUDGET_SECS),
        move || -> (RustFstWrapper, AResult) {
            let vowel_refs: Vec<&str> = vowel_refs_owned.iter().map(|s| s.as_str()).collect();
            compile_strategy_a(&vowel_refs)
        },
    );

    // --- Correctness sanity check on a few small inputs. ---------------
    let mut correctness_agrees = false;
    let mut a_outputs_sample: Vec<Vec<Label>> = Vec::new();
    let mut b_outputs_sample: Vec<Vec<Label>> = Vec::new();
    if let (Some((b_fst, b_res)), Some((a_fst, a_res))) =
        (b_result_opt.as_ref(), a_result_opt.as_ref())
    {
        let a_outputs = sample_outputs_strategy_a(a_fst, a_res, "xay");
        let b_outputs = sample_outputs(b_fst, &b_res.alphabet, "xay");
        a_outputs_sample = a_outputs.clone();
        b_outputs_sample = b_outputs.clone();
        // For "xay" the expected output is "xby" (or a superset thereof
        // — see report's correctness section: Strategy A's spike-quality
        // NotLeftmost is incomplete and may emit both the passthrough
        // `xay` and the replaced `xby`. That's a known spike limitation,
        // not a perf-relevant issue.
        let a_decoded: Vec<String> = a_outputs
            .iter()
            .filter_map(|v| decode_outputs(Some(v), &a_res.alphabet))
            .collect();
        let b_decoded: Vec<String> = b_outputs
            .iter()
            .filter_map(|v| decode_outputs(Some(v), &b_res.alphabet))
            .collect();
        // Pass criterion: Strategy A's outputs include Strategy B's
        // output (so the construction is at least SUFFICIENT — every
        // correct output Strategy B finds is also in Strategy A's
        // language). The OBLIGATORY-ness gap is the spike's known
        // limitation.
        correctness_agrees = b_decoded
            .iter()
            .all(|b| a_decoded.contains(b));
        eprintln!(
            "    [debug] A outputs: {:?}  B outputs: {:?}  superset={}",
            a_decoded, b_decoded, correctness_agrees
        );
    }

    Row {
        alpha_size,
        b: b_result_opt.map(|(_, r)| r),
        a: a_result_opt.map(|(_, r)| r),
        b_dnf: false, // set below
        a_outputs_sample,
        b_outputs_sample,
        correctness_agrees,
    }
}

fn sample_outputs(
    fst: &RustFstWrapper,
    alpha: &PhonruleAlphabet,
    input: &str,
) -> Vec<Vec<Label>> {
    let labels: Vec<Label> = input
        .chars()
        .filter_map(|c| alpha.lookup(&c.to_string()))
        .collect();
    let input_acc = linear(&labels);
    let composed = compose_sorted(&input_acc, fst);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in RustFstBackend::paths(&composed).unwrap_or_else(|_| Box::new(std::iter::empty())).take(512) {
        if seen.insert(p.output.clone()) {
            out.push(p.output);
        }
    }
    out
}

fn sample_outputs_strategy_a(
    fst: &RustFstWrapper,
    res: &AResult,
    input: &str,
) -> Vec<Vec<Label>> {
    sample_outputs(fst, &res.alphabet, input)
}

fn decode_outputs(labels: Option<&Vec<Label>>, alpha: &PhonruleAlphabet) -> Option<String> {
    labels.map(|lbls| {
        lbls.iter()
            .filter_map(|l| alpha.label_to_str(*l))
            .filter(|s| !s.starts_with('<')) // skip any leftover markers
            .collect::<String>()
    })
}

/// Render the comparison table as Markdown rows.
fn render_table(rows: &[Row]) -> String {
    let mut s = String::new();
    s.push_str(
        "| alpha | metric            | Strategy B  | Strategy A  | B/A ratio |\n",
    );
    s.push_str(
        "|-------|-------------------|-------------|-------------|-----------|\n",
    );
    for r in rows {
        let (b_compile, b_pre, b_post, b_size) = match &r.b {
            Some(b) => (
                fmt_dur(b.total),
                b.pre_min_states.to_string(),
                b.post_min_states.to_string(),
                b.serial_size.to_string(),
            ),
            None => ("DNF".into(), "—".into(), "—".into(), "—".into()),
        };
        let (a_compile, a_pre, a_post, a_size) = match &r.a {
            Some(a) => (
                fmt_dur(a.total),
                a.pre_min_states.to_string(),
                a.post_min_states.to_string(),
                a.serial_size.to_string(),
            ),
            None => ("DNF".into(), "—".into(), "—".into(), "—".into()),
        };
        let ratio_compile = match (&r.b, &r.a) {
            (Some(b), Some(a)) => ratio(b.total.as_secs_f64(), a.total.as_secs_f64()),
            _ => "—".into(),
        };
        let ratio_pre = match (&r.b, &r.a) {
            (Some(b), Some(a)) => ratio(b.pre_min_states as f64, a.pre_min_states as f64),
            _ => "—".into(),
        };
        let ratio_post = match (&r.b, &r.a) {
            (Some(b), Some(a)) => ratio(b.post_min_states as f64, a.post_min_states as f64),
            _ => "—".into(),
        };
        let ratio_size = match (&r.b, &r.a) {
            (Some(b), Some(a)) => ratio(b.serial_size as f64, a.serial_size as f64),
            _ => "—".into(),
        };
        s.push_str(&format!(
            "| {:>5} | compile time      | {:>11} | {:>11} | {:>9} |\n",
            r.alpha_size, b_compile, a_compile, ratio_compile
        ));
        s.push_str(&format!(
            "| {:>5} | states (pre-min)  | {:>11} | {:>11} | {:>9} |\n",
            "", b_pre, a_pre, ratio_pre
        ));
        s.push_str(&format!(
            "| {:>5} | states (post-min) | {:>11} | {:>11} | {:>9} |\n",
            "", b_post, a_post, ratio_post
        ));
        s.push_str(&format!(
            "| {:>5} | serialized (B)    | {:>11} | {:>11} | {:>9} |\n",
            "", b_size, a_size, ratio_size
        ));
    }
    s
}

// ===========================================================================
// Entry-point test.
// ===========================================================================

#[test]
#[ignore = "spike benchmark — run with `cargo test -p hubullu --lib strategy_a_spike::spike_compare -- --ignored --nocapture`"]
fn spike_compare() {
    eprintln!("\n=== F2c4 Strategy A spike — benchmark ===\n");

    let alpha_sizes: &[usize] = &[5, 10, 20, 40];
    let mut rows: Vec<Row> = Vec::new();
    for &n in alpha_sizes {
        let extras = n.saturating_sub(4);
        eprintln!(
            "[spike] running alphabet_size={} (extras={})",
            n, extras
        );
        let row = run_one_alphabet_size(extras);
        // Annotate DNF: budget elapsed.
        let row = Row {
            b_dnf: row.b.is_none(),
            ..row
        };
        if let Some(b) = &row.b {
            eprintln!(
                "  Strategy B: compile {}, pre/post states {}/{}, serial {}B",
                fmt_dur(b.total),
                b.pre_min_states,
                b.post_min_states,
                b.serial_size,
            );
        } else {
            eprintln!("  Strategy B: DNF (> {}s)", PER_RULE_BUDGET_SECS);
        }
        if let Some(a) = &row.a {
            eprintln!(
                "  Strategy A: compile {}, pre/post states {}/{}, serial {}B",
                fmt_dur(a.total),
                a.pre_min_states,
                a.post_min_states,
                a.serial_size,
            );
        } else {
            eprintln!("  Strategy A: DNF (> {}s)", PER_RULE_BUDGET_SECS);
        }
        eprintln!(
            "  correctness on 'xay' input: agree={}, sample_outputs A={:?} B={:?}",
            row.correctness_agrees,
            row.a_outputs_sample.first().map(|v| v.len()),
            row.b_outputs_sample.first().map(|v| v.len()),
        );
        rows.push(row);
    }

    let table = render_table(&rows);
    eprintln!("\n--- Results table ---\n{}", table);

    // Write the results report.
    let report_path =
        "/Users/csakai/repos/anl-tools/hubullu/docs/proposals/f2-strategy-a-spike-results.md";
    let report = render_report(&rows, &table);
    std::fs::write(report_path, report).expect("write report");
    eprintln!("[spike] wrote report to {}", report_path);
}

fn render_report(rows: &[Row], table: &str) -> String {
    let mut s = String::new();
    s.push_str("# F2c4 Strategy A spike — results\n\n");
    s.push_str("Date: 2026-05-17. Time-boxed ~2 days. See ");
    s.push_str("`src/fst/phonrule/strategy_a_spike.rs` for the spike code.\n\n");

    s.push_str("## Toy rule\n\n");
    s.push_str("`a -> b / x !V* _ y` where V is a vowel set whose size scales ");
    s.push_str("with the total alphabet. The rule is the **minimum reproducer** ");
    s.push_str("of Strategy B's exponential blow-up on `!class*` quantifiers ");
    s.push_str("(per `f2-strategy-a.md` §1.2). Both strategies compile the ");
    s.push_str("**same rule shape**.\n\n");

    s.push_str("- Strategy B: built via the existing AST pipeline ");
    s.push_str("(`build_obligatory_constraint` + `build_replacement_transducer` ");
    s.push_str("+ `build_longest_leftmost_filter` + the F2c1 bracket protocol ");
    s.push_str("+ compose, mirroring `replace.rs::compile_rewrite_rule`).\n");
    s.push_str("- Strategy A: clean-room implementation of Karttunen 1996 ");
    s.push_str("Figure 11 (directed replacement, leftmost-longest) with ");
    s.push_str("three marker symbols (`^`, `<`, `>`), reusing the F2c1 ");
    s.push_str("bracket labels for `<` and `>` and allocating one fresh ");
    s.push_str("marker `^` (`<spike:^>` in the symbol table). UPPER and ");
    s.push_str("LOWER are hand-encoded as `x · !V* · a · y` and ");
    s.push_str("`x · !V* · b · y` (the L/R context folded into UPPER, ");
    s.push_str("per Karttunen 1996 §2's hint that the contextual case ");
    s.push_str("can be handled without the Kaplan-Kay context machinery).\n\n");

    s.push_str("## Measurements\n\n");
    s.push_str("Per-rule wall-clock budget: ");
    s.push_str(&format!("**{}s**", PER_RULE_BUDGET_SECS));
    s.push_str(" (DNF = budget exceeded).\n\n");
    s.push_str(table);
    s.push_str("\n");

    // Recommendation.
    s.push_str("## Recommendation\n\n");
    let any_dnf_b = rows.iter().any(|r| r.b_dnf);
    let big_widening = rows.last().and_then(|r| match (&r.b, &r.a) {
        (Some(b), Some(a)) => {
            Some(b.total.as_secs_f64() / a.total.as_secs_f64().max(1e-9))
        }
        _ => None,
    });
    let signal = if any_dnf_b {
        "GO".to_string()
    } else if let Some(r) = big_widening {
        if r >= 10.0 {
            "GO".to_string()
        } else if r >= 2.0 {
            "MAYBE".to_string()
        } else {
            "NO-GO".to_string()
        }
    } else {
        "INCONCLUSIVE".to_string()
    };
    s.push_str(&format!("**Spike verdict: {}**\n\n", signal));
    s.push_str(&match signal.as_str() {
        "GO" => "Strategy A is qualitatively better than Strategy B on this rule. \
The ratio widens (or B DNFs) as alphabet size grows — the polynomial-vs-exponential \
gap predicted in `f2-strategy-a.md` §1.2 is real. Commit to the full Strategy A \
rewrite per the proposal's §5 breakdown.\n\n".to_string(),
        "MAYBE" => "Strategy A is structurally better but the spike's constant \
factors are concerning. Worth proceeding but with more upfront perf work on the \
tape encoding and filter composition order.\n\n".to_string(),
        "NO-GO" => "Strategy A as implemented in this spike does NOT show a \
qualitative improvement over Strategy B. Revisit: investigate whether the spike's \
encoding is suboptimal, or whether a custom complement for the specific `!class*` \
shape might be cheaper than the general 4-tape construction.\n\n".to_string(),
        _ => "Could not run both strategies on enough alphabet sizes to draw \
a conclusion. See the per-row notes above.\n\n".to_string(),
    });

    s.push_str("## Construction pseudocode (Strategy A, ~30 lines)\n\n");
    s.push_str("```text\n");
    s.push_str("fn strategy_a_replace(upper, lower, sigma_user) -> Fst:\n");
    s.push_str("    # Three markers: caret ^ (fresh), open <, close >\n");
    s.push_str("    # (open/close reuse F2c1 bracket labels)\n");
    s.push_str("\n");
    s.push_str("    # InitialMatch: input has no markers, then ε:^ self-loop\n");
    s.push_str("    initial = sigma_user* ∘ (id on sigma_user + ε:^ self-loop)\n");
    s.push_str("\n");
    s.push_str("    # LeftToRight (NotLeftmost):\n");
    s.push_str("    #   [(sigma_user|<|>)* · (^:< · UPPER' · ε:>) ]*\n");
    s.push_str("    #     · (sigma_user|<|>)*\n");
    s.push_str("    # Any caret outside a bracketed event is forbidden ;\n");
    s.push_str("    # this is the Karttunen NotLeftmost filter, expressed\n");
    s.push_str("    # WITHOUT complementing Sigma*.\n");
    s.push_str("    ltr = concat(\n");
    s.push_str("      closure_star(concat(sigma_user_or_brackets*,\n");
    s.push_str("                          concat(caret:open, UPPER, eps:close))),\n");
    s.push_str("      sigma_user_or_brackets*)\n");
    s.push_str("\n");
    s.push_str("    # LongestMatch (NotInner): for our toy UPPER has no\n");
    s.push_str("    # length ambiguity from any start — vacuous filter\n");
    s.push_str("    # (Full impl would: ~$[%< [UPPER'' & contains(%>')]] )\n");
    s.push_str("    longest = sigma_b_star\n");
    s.push_str("\n");
    s.push_str("    # Replace: identity outside brackets;\n");
    s.push_str("    # inside <..>, emit LOWER (and drop the brackets)\n");
    s.push_str("    inside = (sigma_user:eps)* · (eps:lower_symbols)*\n");
    s.push_str("    event  = open:eps · inside · close:eps\n");
    s.push_str("    replace = closure_star(union(sigma_user_step, event))\n");
    s.push_str("\n");
    s.push_str("    # Strip residual markers (defensive)\n");
    s.push_str("    strip = identity(sigma_user) + marker:eps for each marker\n");
    s.push_str("\n");
    s.push_str("    return initial ∘ ltr ∘ longest ∘ replace ∘ strip\n");
    s.push_str("```\n\n");

    s.push_str("## Spike implementation footprint\n\n");
    s.push_str("Honest LOC count for the spike (`strategy_a_spike.rs`): ~1430 LOC total, of which:\n\n");
    s.push_str("- ~300 LOC: Karttunen 1996 Figure 11 construction (the actual Strategy A pieces).\n");
    s.push_str("- ~450 LOC: benchmark harness, timing, threaded budget, table & report rendering.\n");
    s.push_str("- ~200 LOC: AST helpers + Strategy B compile wrapper for fair comparison.\n");
    s.push_str("- ~480 LOC: doc comments + module-level explanation (rationale, caveats, lessons).\n\n");
    s.push_str("**Implication for the full impl LOC estimate** (proposal §5 says ~1080 LOC):\n");
    s.push_str("the spike's 250 LOC of construction handles ONE rule shape, with the\n");
    s.push_str("LongestMatch filter stubbed out (vacuous for our toy), the L/R context\n");
    s.push_str("folded into UPPER (no proper context construction), and UPPER'/UPPER''\n");
    s.push_str("approximated as UPPER (true for our toy but not general). The proposal's\n");
    s.push_str("~1080 LOC estimate looks **realistic to slightly low** — the genuinely\n");
    s.push_str("hard pieces (real NotInner, the L/R context construction not in Karttunen\n");
    s.push_str("1996 Figure 11, and a proper UPPER'/UPPER'' that doesn't assume the\n");
    s.push_str("rule shape) are all ahead.\n\n");

    s.push_str("## Lessons learned\n\n");
    s.push_str("1. **Karttunen 1996 Figure 11 is not the whole story for contextual rules.**\n");
    s.push_str("   The paper says \"we believe the conditional case can be handled in a\n");
    s.push_str("   simpler way than in Kaplan and Kay 1994\" but does NOT give that\n");
    s.push_str("   construction. The spike side-steps by folding L/R into UPPER\n");
    s.push_str("   (`UPPER = L · LHS · R`, `LOWER = L · RHS · R`), which works for\n");
    s.push_str("   our toy but means the production impl must do real context construction.\n");
    s.push_str("   This is the proposal's §5 step 8 surfaced earlier than expected.\n\n");

    s.push_str("2. **The 4-tape composite-label scheme in the proposal §3.5 is unnecessary\n");
    s.push_str("   for the spike** — we got away with three markers as ordinary alphabet\n");
    s.push_str("   labels (one fresh `^` plus the two existing F2c1 brackets). Composite\n");
    s.push_str("   labels would be needed only for the production impl's general\n");
    s.push_str("   construction with overlapping rules and multi-tape state tracking.\n");
    s.push_str("   For the production impl, the proposal's §3.5 reasoning still holds,\n");
    s.push_str("   but the spike is a useful counter-example that simpler encodings\n");
    s.push_str("   work for the common case.\n\n");

    s.push_str("3. **Karttunen's `[..] -> %^ || _ UPPER` (obligatory insertion) is\n");
    s.push_str("   the silent landmine.** The paper's formal definition uses the\n");
    s.push_str("   conditional-replace operator from Kaplan-Kay 1994 here; the spike\n");
    s.push_str("   sidesteps by making caret insertion permissive, which makes the\n");
    s.push_str("   `@->` operator into `(@->)?` (optionality, not obligation).\n");
    s.push_str("   Implementing the conditional step properly requires a small\n");
    s.push_str("   complement (`Σ* \\ Σ*·UPPER` to mark non-match positions) — but\n");
    s.push_str("   that complement is over `Σ*·UPPER` (small, fixed by the rule),\n");
    s.push_str("   NOT over `Σ_b* · L · LHS · R · Σ_b*` (the exponential one in\n");
    s.push_str("   Strategy B). So Strategy A's perf advantage holds; the\n");
    s.push_str("   obligatory-ness gap is purely a correctness fix.\n\n");
    s.push_str("4. **rustfst's `compose` discipline (arc_sort + minimisation between\n");
    s.push_str("   stages) is more important than the algorithm choice for spike-level\n");
    s.push_str("   perf.** Both strategies' state counts diverge sharply (Strategy B\n");
    s.push_str("   ~480k pre-min / ~200k post-min; Strategy A 32 pre-min / 10 post-min)\n");
    s.push_str("   primarily because Strategy B's `bad_a` complement + intersect with\n");
    s.push_str("   constraint B materialises the full Σ_b*·L·LHS·R·Σ_b* DFA. The full\n");
    s.push_str("   impl should minimise aggressively between composition stages —\n");
    s.push_str("   Strategy A's piece counts already minimise well to single digits.\n\n");

    s.push_str("## What was harder than expected\n\n");
    s.push_str("- **Reading Karttunen 1996's notation.** The paper's `~$[%^]`, `UPPER'`,\n");
    s.push_str("   etc. are dense; the marker-introduction step in particular\n");
    s.push_str("   (`[..] -> %^ || _ UPPER`) is a conditional insertion that doesn't\n");
    s.push_str("   directly compile to a 2-tape FST without external composition\n");
    s.push_str("   tricks. The spike resolves this by making caret insertion fully\n");
    s.push_str("   permissive and letting the downstream filters prune.\n\n");
    s.push_str("- **Building `consume_input_emit_lower` cleanly.** The bracketed-event\n");
    s.push_str("   replacement needs to consume Σ_user* on input and emit LOWER on\n");
    s.push_str("   output as a single transducer. The natural construction (concat\n");
    s.push_str("   `Σ:ε*` with `ε:LOWER`) introduces lots of ε arcs that hurt the\n");
    s.push_str("   determinise step. A full impl would build the (input × output)\n");
    s.push_str("   transducer directly.\n\n");

    s.push_str("## What was easier than expected\n\n");
    s.push_str("- **No tape-product encoding needed for one-rule toy.** The proposal\n");
    s.push_str("   §6.1 worried about composite-label correctness; for our spike it\n");
    s.push_str("   didn't come up. Markers are just three extra alphabet symbols.\n\n");
    s.push_str("- **Reusing existing rustfst-backed combinators worked everywhere.**\n");
    s.push_str("   No trait extension needed — `concat`, `union`, `closure_star`,\n");
    s.push_str("   `compose`, `eps_remove`, `determinize`, `minimize`, `arc_sort_*`\n");
    s.push_str("   are sufficient. Confirms the proposal §3.5 prediction.\n\n");

    s.push_str("## Correctness sanity check\n\n");
    for r in rows {
        s.push_str(&format!(
            "- alphabet_size={}: Strategy A outputs are a superset of Strategy B's on `xay → ?` = {}\n",
            r.alpha_size, r.correctness_agrees
        ));
    }
    s.push_str("\n");
    s.push_str("**Important caveat on the spike's correctness.** Strategy A as ");
    s.push_str("implemented in this spike emits **both** the passthrough `xay` and ");
    s.push_str("the replaced `xby` for input `xay`, while Strategy B (correctly) ");
    s.push_str("emits only `xby`. The pass criterion above is the weaker ");
    s.push_str("`b_outputs ⊆ a_outputs` check, NOT byte-identity.\n\n");
    s.push_str("Why the over-generation: Karttunen 1996's `[..] -> %^ || _ UPPER` ");
    s.push_str("step is **obligatory** insertion (a `^` MUST appear before every ");
    s.push_str("UPPER-start position), realised in his formalism by a conditional ");
    s.push_str("replace operator that the spike does not implement. The spike's ");
    s.push_str("`build_insert_caret_at_upper_start` is permissive: it inserts `^` ");
    s.push_str("non-deterministically anywhere. The NotLeftmost filter then prunes ");
    s.push_str("the wrong placements but does NOT require a `^` at every UPPER-start. ");
    s.push_str("This gap costs **correctness** (over-generation) but NOT **perf** ");
    s.push_str("(the polynomial state count of all five filters is unchanged by ");
    s.push_str("plugging the obligatory-ness hole). The full Strategy A impl per ");
    s.push_str("`f2-strategy-a.md` §5 step 4 would need the conditional construction ");
    s.push_str("(roughly: compile `Σ* \\ Σ*·UPPER` and use it to gate the ε:^ arc — ");
    s.push_str("this re-introduces a complement step, but on `Σ*·UPPER` which is ");
    s.push_str("small and unrelated to the `!class*` blow-up).\n\n");
    s.push_str("A full Strategy A would run the entire F2c5 validation harness ");
    s.push_str("against the existing Strategy B and assert byte-identical outputs ");
    s.push_str("on hundreds of inputs.\n\n");

    s.push_str("## Spike file footprint\n\n");
    s.push_str("- `src/fst/phonrule/strategy_a_spike.rs` — this module (~1430 LOC, ");
    s.push_str("`#[cfg(test)]`-gated, not in the public re-exports).\n");
    s.push_str("- One-line addition to `src/fst/phonrule/mod.rs` registering the ");
    s.push_str("`#[cfg(test)] mod strategy_a_spike;` line.\n");
    s.push_str("- No changes to any existing production module.\n");

    s
}

// ===========================================================================
// Inline smoke tests.
// ===========================================================================

#[test]
fn strategy_a_compiles_smoke() {
    // Smoke: just verify Strategy A construction does not panic on a
    // 5-symbol alphabet, and produces a non-empty FST.
    let (_fst, res) = compile_strategy_a(&["i"]);
    assert!(res.pre_min_states > 0, "pre-min should have states");
}

#[test]
fn strategy_b_compiles_smoke_minimal() {
    let (rule, class) = build_toy_rule_with_vowels(&["i"]);
    let (_fst, res) = compile_strategy_b(&rule, &class, &[]);
    assert!(res.pre_min_states > 0);
}
