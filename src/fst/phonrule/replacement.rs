//! Karttunen step 3 — `Replace`: the replacement transducer (F2c2).
//!
//! The piece of the Karttunen `@->` chain that performs the actual
//! substitution. Composed downstream as the third stage in the four-stage
//! chain (Mark — Constraint — **Replace** — Unmark; plan §3).
//!
//! ## Brief
//!
//! Given an input stream that has already been **bracketed** by F2c1's
//! [`super::brackets::intro_brackets`] and (in the full chain)
//! **constrained** by F2c3's obligatory-context stage, the replacement
//! transducer traverses the input and:
//!
//!   * **Inside `<[+]>...<]+>` brackets**: consume the LHS pattern
//!     symbols on the input side, emit the RHS replacement on the output
//!     side. The brackets themselves are passed through (input=output) so
//!     downstream stages still see them — F2c1's `strip_brackets` handles
//!     the final cleanup.
//!
//!   * **Outside brackets**: identity on Σ symbols. Stray bracket markers
//!     not enclosing a valid LHS match are not the replacement
//!     transducer's concern — F2c3 will reject such paths. To keep
//!     `Replace` composable in isolation, outside-state self-loops include
//!     identity passthrough on both bracket labels (so a malformed input
//!     doesn't trap the FST).
//!
//! ## Construction (compositional, trait-only)
//!
//! Built from three FSTs assembled with the existing `FstBackend`
//! combinators — no reach into `rustfst::*` types is needed:
//!
//! ```text
//!   one_outside    := identity over Σ ∪ {<[+]>, <]+>}, single-symbol-wide
//!   one_event      := <[+]>:<[+]> · LHS:RHS · <]+>:<]+>   (concat)
//!   replacement    := closure_star(union(one_outside, one_event))
//! ```
//!
//! Per loop iteration the FST either consumes one identity symbol OR one
//! whole bracketed replacement event. The star closes the loop so
//! arbitrary mixes of identity passthrough and bracketed events are
//! accepted, in any order.
//!
//! ## RHS variants (`PhonReplacement`)
//!
//! The `LHS:RHS` sub-transducer differs per [`PhonReplacement`] variant:
//!
//!   * **`Literal(s)`** — concat: input-only LHS (`σᵢ:ε` per LHS symbol)
//!     followed by output-only RHS (`ε:cⱼ` per RHS char).
//!
//!   * **`Null`** — input-only LHS only; no output. (Same as Literal with
//!     the RHS leg being the ε-acceptor.)
//!
//!   * **`Map(name)`** — compose the LHS identity acceptor with
//!     `closure_star(map_fst)`. The map FST is a symbol→symbol transducer
//!     (F2a); its Kleene-star lets us apply it to multi-symbol LHS. For
//!     the single-symbol case (the only one in v1 grammars) the star
//!     collapses to one application at runtime.
//!
//! ## LHS source
//!
//! [`PhonPattern`](crate::ast::PhonPattern) has three variants:
//!
//!   * `Class(ident)` — single-symbol acceptor, looked up in `class_table`.
//!   * `Literal(s)`   — multi-char path acceptor, identity I/O.
//!   * `Range(elems)` — [`super::context::compile_pattern_sequence`] over a
//!     context-elem sequence (F2b code path).
//!
//! All three produce **acceptors** (identity I=O). The replacement
//! transducer's "LHS as input-only" trick re-projects them by composing
//! with a `σ:ε`-self-loop transducer (see [`make_input_only`]).
//!
//! ## What this module does NOT own
//!
//!   * Bracket introduction / removal — F2c1's [`super::brackets`].
//!   * Obligatory-context constraint — F2c3.
//!   * Longest-leftmost filter — F2c4.
//!   * Top-level `compile_rewrite_rule` — F2c5.
//!
//! ## Caveats
//!
//!   * **Insertion rules** (`"" -> X`, where LHS is empty) are not handled
//!     specially. The LHS acceptor is the ε-acceptor; the event becomes
//!     `<[+]>:<[+]> · ε · X_emit · <]+>:<]+>`. Plan §3.6 item 5 expects
//!     this to "work without changes" — F2c5 validation will exercise it.
//!
//!   * **Multi-char Map RHS**: see RHS Map note above. The construction
//!     uses `closure_star(map)`, which handles the multi-symbol case
//!     generically. The single-symbol case (Turkish harmony/elision) is
//!     the only one exercised in current grammars.
//!
//!   * The "outside" passthrough on bracket labels means a stray bracket
//!     without a balanced pair is silently accepted by `Replace` in
//!     isolation. F2c3's constraint rejects those paths in the full chain.

use std::collections::HashMap;

use crate::ast::{PhonPattern, PhonReplacement, PhonRewriteRule};

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, Label, EPS_LABEL};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::context::{compile_pattern_sequence, ContextCompileError};

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by replacement-transducer compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementCompileError {
    /// `PhonPattern::Class` referenced a class not in `class_table`.
    UnknownClass { name: String },
    /// `PhonReplacement::Map` referenced a map not in `map_table`.
    UnknownMap { name: String },
    /// LHS pattern compilation surfaced an error (e.g. syllable element).
    Context(ContextCompileError),
}

impl std::fmt::Display for ReplacementCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplacementCompileError::UnknownClass { name } => write!(
                f,
                "rewrite rule LHS references undefined or not-yet-compiled class '{}'",
                name
            ),
            ReplacementCompileError::UnknownMap { name } => write!(
                f,
                "rewrite rule RHS references undefined or not-yet-compiled map '{}'",
                name
            ),
            ReplacementCompileError::Context(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ReplacementCompileError {}

impl From<ContextCompileError> for ReplacementCompileError {
    fn from(e: ContextCompileError) -> Self {
        ReplacementCompileError::Context(e)
    }
}

impl From<ReplacementCompileError> for super::super::backend::FstError {
    fn from(e: ReplacementCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Build the replacement transducer for a single rewrite rule.
///
/// The returned FST, when given a well-bracketed input (every
/// `<[+]>...<]+>` enclosing exactly an LHS match), produces the
/// corresponding RHS-replaced output on the output side. Outside
/// brackets the FST is identity on Σ; stray brackets are passed
/// through (see module docs).
///
/// Σ is taken from `alpha` **after** all LHS / RHS literal interning, so
/// the outside-state self-loop covers any new symbols the rule
/// introduced. `class_table` and `map_table` must already contain the
/// FSTs for any class / map the rule references; forward references are
/// an error.
pub fn build_replacement_transducer(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
    map_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ReplacementCompileError> {
    // 1. Compile the LHS as an identity acceptor.
    let lhs_acceptor = compile_lhs(&rule.from, alpha, class_table)?;

    // 2. Compile the LHS:RHS sub-transducer (interns any RHS Literal chars).
    let lhs_rhs = compile_lhs_to_rhs(&lhs_acceptor, &rule.to, alpha, map_table)?;

    // 3. Build the bracket-enclosed event FST: <[+]>:<[+]> · LHS:RHS · <]+>:<]+>.
    let bracket_open = single_arc(
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_OPEN_OBLIG_LABEL,
    );
    let bracket_close = single_arc(
        BRACKET_CLOSE_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
    );
    let open_then_lhs_rhs = RustFstBackend::concat(&bracket_open, &lhs_rhs)
        .expect("concat <[+]> · LHS:RHS");
    let one_event = RustFstBackend::concat(&open_then_lhs_rhs, &bracket_close)
        .expect("concat ... · <]+>");

    // 4. Build the one-step outside FST: identity on Σ ∪ {<[+]>, <]+>}.
    //
    // Σ snapshot is taken AFTER all LHS/RHS interning, so any literal
    // that the rule introduced is a first-class member of the outside
    // alphabet. (If we built this before step 2, a rule like
    // `a -> "newchar"` would have `newchar` invisible to the outside
    // self-loop, breaking identity passthrough on it.)
    let one_outside = build_one_outside_step(alpha);

    // 5. Combine: closure_star(union(one_outside, one_event)).
    let alternation = RustFstBackend::union(&one_outside, &one_event)
        .expect("union(one_outside, one_event)");
    let star = RustFstBackend::closure_star(&alternation)
        .expect("closure_star(union)");

    Ok(star)
}

// ---------------------------------------------------------------------------
// LHS compilation — turn PhonPattern into an identity acceptor.
// ---------------------------------------------------------------------------

/// Compile the LHS of a rewrite rule to an identity acceptor.
///
/// All three [`PhonPattern`] variants share a uniform output shape: an
/// FST whose accepting paths each have `input == output` (identity).
/// Downstream construction uses this as both an acceptor (for the
/// `Null` and direct cases) and re-projects it (for `Literal` and
/// `Map`) into input-only / transduced forms.
fn compile_lhs(
    pat: &PhonPattern,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ReplacementCompileError> {
    match pat {
        PhonPattern::Class(ident) => class_table
            .get(&ident.node)
            .cloned()
            .ok_or(ReplacementCompileError::UnknownClass {
                name: ident.node.clone(),
            }),
        PhonPattern::Literal(lit) => Ok(build_literal_acceptor(&lit.node, alpha)),
        PhonPattern::Range(elems) => Ok(compile_pattern_sequence(elems, alpha, class_table)?),
    }
}

/// Build an identity acceptor for a literal string — one arc per char.
///
/// Mirrors `context::compile_literal` (kept local here to avoid widening
/// the `context` module's public surface for an internal helper). Empty
/// literal yields the one-state ε-acceptor (which serves as the LHS for
/// insertion rules `"" -> X`).
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
// LHS:RHS sub-transducer.
// ---------------------------------------------------------------------------

/// Build the LHS:RHS sub-transducer per [`PhonReplacement`] variant.
///
/// Returns an FST whose accepting paths each have input = the LHS
/// symbols and output = the RHS replacement of that LHS. See module
/// docs for the per-variant construction sketch.
fn compile_lhs_to_rhs(
    lhs_acceptor: &RustFstWrapper,
    rhs: &PhonReplacement,
    alpha: &mut PhonruleAlphabet,
    map_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ReplacementCompileError> {
    match rhs {
        PhonReplacement::Null => Ok(make_input_only(lhs_acceptor, alpha)),
        PhonReplacement::Literal(lit) => {
            let lhs_in = make_input_only(lhs_acceptor, alpha);
            let rhs_out = build_output_only_emitter(&lit.node, alpha);
            // concat: consume LHS (input only), then emit RHS (output only).
            Ok(RustFstBackend::concat(&lhs_in, &rhs_out)
                .expect("concat LHS-in with RHS-out"))
        }
        PhonReplacement::Map(map_ident) => {
            let map_fst = map_table.get(&map_ident.node).ok_or(
                ReplacementCompileError::UnknownMap {
                    name: map_ident.node.clone(),
                },
            )?;
            // The map FST is a 2-state symbol→symbol transducer (one
            // accept per single input symbol). To apply it across a
            // possibly multi-symbol LHS, take its Kleene star and
            // compose: each LHS symbol gets its own through-the-map
            // traversal. For single-symbol LHS (the only case in
            // current grammars) the star contracts to a single
            // application at runtime, but the generic construction
            // handles N > 1 too.
            //
            // Pre-compose arc-sort: rustfst checks property bits, not
            // arc order. F2c2.1 added the trait methods; we use them.
            let map_star = RustFstBackend::closure_star(map_fst)
                .expect("closure_star on map fst");
            let lhs_sorted = RustFstBackend::arc_sort_output(lhs_acceptor)
                .expect("arc_sort_output on LHS");
            let map_sorted = RustFstBackend::arc_sort_input(&map_star)
                .expect("arc_sort_input on map*");
            Ok(RustFstBackend::compose(&lhs_sorted, &map_sorted)
                .expect("LHS ∘ map* compose"))
        }
    }
}

/// Project an identity acceptor to input-only via composition with a
/// `σ:ε`-self-loop transducer.
///
/// Goal: given an LHS acceptor whose arcs are `σ:σ`, produce an FST
/// whose arcs are `σ:ε` (input preserved, output erased). We achieve
/// this by composing the acceptor with an "Σ → ε" transducer:
///
///   * `eraser` is a one-state self-loop FST with arcs `σ:ε` for every
///     σ ∈ Σ (plus the bracket labels, in case the LHS contains them —
///     unusual but not forbidden).
///   * `LHS ∘ eraser` yields paths where input = LHS's input symbols and
///     output = ε for each.
///
/// Σ snapshot is taken at call time. The eraser must cover every label
/// the LHS acceptor uses on its output side — for class/literal/range
/// LHSes those labels are exactly Σ members (the LHS only references
/// symbols already interned by class compilation or literal interning).
fn make_input_only(
    acceptor: &RustFstWrapper,
    alpha: &PhonruleAlphabet,
) -> RustFstWrapper {
    let eraser = build_eraser(alpha);
    // compose discipline: left output-sorted, right input-sorted.
    let lhs_sorted =
        RustFstBackend::arc_sort_output(acceptor).expect("arc_sort_output LHS");
    let eraser_sorted =
        RustFstBackend::arc_sort_input(&eraser).expect("arc_sort_input eraser");
    RustFstBackend::compose(&lhs_sorted, &eraser_sorted)
        .expect("compose LHS with eraser")
}

/// One-state self-loop FST whose arcs are `σ:ε` for every σ ∈ Σ and
/// `bracket:ε` for both bracket labels.
///
/// "Erases" every symbol it consumes. Used by [`make_input_only`] to
/// project an identity acceptor to its input-only form via composition.
fn build_eraser(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    for label in alpha.sigma() {
        b.add_arc(s, label, EPS_LABEL, s).expect("eraser σ:ε");
    }
    // Stream markers (defensive; LHS rarely contains markers but
    // boundary context elements compile to single-arc consumers of
    // these labels — the input-only projection of those acceptors
    // composes through here).
    b.add_arc(s, alpha.boundary_label(), EPS_LABEL, s)
        .expect("eraser <bdy>:ε");
    b.add_arc(s, alpha.word_start_label(), EPS_LABEL, s)
        .expect("eraser <^>:ε");
    b.add_arc(s, alpha.word_end_label(), EPS_LABEL, s)
        .expect("eraser <$>:ε");
    // Brackets too — defensive; LHS won't contain brackets in normal use.
    b.add_arc(s, BRACKET_OPEN_OBLIG_LABEL, EPS_LABEL, s)
        .expect("eraser <[+]>:ε");
    b.add_arc(s, BRACKET_CLOSE_OBLIG_LABEL, EPS_LABEL, s)
        .expect("eraser <]+>:ε");
    b.finish().expect("eraser finish")
}

/// Build a one-state-chain output-only emitter for a literal string.
///
/// One arc per char: `ε → chᵢ`. Like [`build_literal_acceptor`] but with
/// input = ε. Empty literal yields a one-state ε-acceptor.
fn build_output_only_emitter(lit: &str, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
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
        b.add_arc(prev, EPS_LABEL, label, next)
            .expect("output-only arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish output-only")
}

// ---------------------------------------------------------------------------
// One-step outside FST.
// ---------------------------------------------------------------------------

/// Build a 2-state FST accepting exactly **one** identity-IO symbol from
/// `Σ ∪ {<[+]>, <]+>}`.
///
/// Σ is snapshot-read from `alpha` — see the call site in
/// [`build_replacement_transducer`] for why timing matters.
///
/// Used as the base of the outside-state Kleene star: one iteration =
/// one identity symbol passed through.
fn build_one_outside_step(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    for label in alpha.sigma() {
        b.add_arc(s0, label, label, s1).expect("outside σ");
    }
    // Identity on the three reserved stream markers — `<bdy>`, `<^>`,
    // `<$>` can appear in runtime input streams (BOUNDARY morpheme
    // separator from compose, word-edge markers from apply driver).
    // Without these arcs, an input with a stream marker would have no
    // path through Replace's outside-state. F2c5.1 added this to mirror
    // the same fix in `brackets::intro_brackets` / `strip_brackets`.
    b.add_arc(s0, alpha.boundary_label(), alpha.boundary_label(), s1)
        .expect("outside <bdy>");
    b.add_arc(s0, alpha.word_start_label(), alpha.word_start_label(), s1)
        .expect("outside <^>");
    b.add_arc(s0, alpha.word_end_label(), alpha.word_end_label(), s1)
        .expect("outside <$>");
    b.add_arc(
        s0,
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_OPEN_OBLIG_LABEL,
        s1,
    )
    .expect("outside <[+]> passthrough");
    b.add_arc(
        s0,
        BRACKET_CLOSE_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
        s1,
    )
    .expect("outside <]+> passthrough");
    b.finish().expect("one_outside finish")
}

// ---------------------------------------------------------------------------
// Tiny builders.
// ---------------------------------------------------------------------------

/// Build a 2-state FST with a single arc reading `input_label` and writing
/// `output_label`. Used for the bracket-open / bracket-close one-arc FSTs.
fn single_arc(input_label: Label, output_label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    b.add_arc(s0, input_label, output_label, s1)
        .expect("single_arc add_arc");
    b.finish().expect("single_arc finish")
}
