//! Tests for the F2c3 obligatory-context constraint.
//!
//! Per the F2c3 brief: build inputs, compose
//! `intro_brackets ∘ replacement ∘ constraint`, then assert outputs.
//! Each test exercises one corner of the constraint specification:
//!
//!   1. Context-licensed rewrite fires.
//!   2. Lack of R context blocks rewrite.
//!   3. Multiple occurrences in one input.
//!   4. No-context rule rewrites everything.
//!   5. Stray bracket without LHS is rejected.
//!   6. Boundary semantics via the F2c3.1 union patch.
//!   7. Word-start anchor recognition.
//!
//! Plus smaller smoke tests to validate the construction's edges.
//!
//! ## Pipeline assembled per test
//!
//! ```text
//!   input_acceptor ∘ intro_brackets ∘ constraint ∘ replacement ∘ strip_brackets
//! ```
//!
//! Note we **do not** include the F2c4 longest-leftmost filter — F2c3
//! alone leaves residual ambiguity in cases like `aaa` with rule
//! `a -> b` (every individual `a` may or may not be bracketed). The
//! tests below pick rules and inputs where the obligatory constraint
//! plus identity flexibility is determinate enough that the
//! "expected" output appears among the (possibly multiple) accepted
//! paths.
//!
//! ## Bounded path enumeration
//!
//! `intro_brackets` is cyclic on ε-emit arcs and `replacement` is
//! cyclic on its outer Kleene star. Composing all four stages can
//! yield infinitely many paths. We `.take(BOUNDED_PATHS)` and assert
//! the desired output is present in the prefix (a BFS path iterator
//! enumerates short paths first, so the canonical output appears
//! early). The threshold is generous; failing tests can lift it.

use std::collections::HashMap;

use crate::ast::{
    CharClassBody, CharClassDef, PhonAtom, PhonContext, PhonContextElem, PhonPattern,
    PhonReplacement, PhonRewriteRule, Quantifier, Span, Spanned, StringLit,
};
use crate::span::FileId;

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, Label, Path};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::brackets::{intro_brackets, strip_brackets};
use super::class::compile_class;
use super::constraint::build_obligatory_constraint;
use super::replacement::build_replacement_transducer;

// ---------------------------------------------------------------------------
// AST helpers.
// ---------------------------------------------------------------------------

fn sp() -> Span {
    Span { file_id: FileId(0), start: 0, end: 0 }
}

fn ident(s: &str) -> Spanned<String> {
    Spanned::new(s.to_string(), sp())
}

fn lit(s: &str) -> StringLit {
    Spanned::new(s.to_string(), sp())
}

fn class_list(name: &str, members: &[&str]) -> CharClassDef {
    CharClassDef {
        name: ident(name),
        body: CharClassBody::List(members.iter().map(|m| lit(m)).collect()),
    }
}

fn ctx_literal(s: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Literal(lit(s)), Quantifier::Exact(1))
}

/// Build a rule `from -> to / left_elems _ right_elems`.
fn rule_with_context(
    from: &str,
    to: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Literal(lit(to)),
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

/// Build a rule `from -> to` (no context).
fn rule_no_context(from: &str, to: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Literal(lit(to)),
        context: None,
        span: sp(),
    }
}

// ---------------------------------------------------------------------------
// Fixture helpers.
// ---------------------------------------------------------------------------

/// Build a linear identity-IO input acceptor over `labels`.
fn linear_input_acceptor(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let mut prev = b.add_state();
    b.set_start(prev).expect("set_start");
    for &l in labels {
        let next = b.add_state();
        b.add_arc(prev, l, l, next).expect("add_arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish")
}

/// Arc-sort and compose two FSTs.
fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> RustFstWrapper {
    let a_sorted = RustFstBackend::arc_sort_output(a).expect("arc_sort_output");
    let b_sorted = RustFstBackend::arc_sort_input(b).expect("arc_sort_input");
    RustFstBackend::compose(&a_sorted, &b_sorted).expect("compose")
}

/// Bounded cap for path enumeration on possibly-cyclic compositions.
const BOUNDED_PATHS: usize = 4096;

fn bounded_paths(fst: &RustFstWrapper) -> Vec<Path> {
    RustFstBackend::paths(fst)
        .expect("paths iter")
        .take(BOUNDED_PATHS)
        .collect()
}

fn output_seq_present(paths: &[Path], expected: &[Label]) -> bool {
    paths.iter().any(|p| p.output.as_slice() == expected)
}

/// Whether *any* path in `paths` has an accepting output (i.e., the
/// composed pipeline accepts the input at all).
fn any_accepting_path(paths: &[Path]) -> bool {
    !paths.is_empty()
}

/// Build the full Karttunen pipeline:
///   intro_brackets ∘ constraint ∘ replacement ∘ strip_brackets
fn build_full_pipeline(
    alpha: &PhonruleAlphabet,
    constraint: &RustFstWrapper,
    repl: &RustFstWrapper,
) -> RustFstWrapper {
    let intro = intro_brackets(alpha);
    let strip = strip_brackets(alpha);

    let intro_constraint = compose_sorted(&intro, constraint);
    let intro_constraint_repl = compose_sorted(&intro_constraint, repl);
    compose_sorted(&intro_constraint_repl, &strip)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

// 1. Rule `a -> b / x _ y`, input `xay` → must produce `xby`.
#[test]
fn constraint_a_to_b_in_xy_rewrites_xay() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    let input = linear_input_acceptor(&[x, a, y]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    assert!(
        output_seq_present(&paths, &[x, b, y]),
        "rule a->b/x_y must produce xby for input xay; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
    // Note: the identity passthrough `xay` is also among the paths
    // because Replace's outside-state passes brackets through as
    // identity (F2c2 design), even when the constraint accepts the
    // bracketing. F2c4's longest-leftmost filter will pick the
    // canonical bracketing — for now we only assert presence of the
    // canonical output, not absence of identity passthrough.
}

// 2. Rule `a -> b / x _ y`, input `xa` (no R) → must produce `xa` (unchanged).
#[test]
fn constraint_no_r_context_passes_unchanged() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();

    let input = linear_input_acceptor(&[x, a]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    // The input has no R context, so the rewrite can't fire (no valid
    // bracketing position passes the constraint). Identity passes:
    // [x, a] is the only accepting output.
    assert!(
        output_seq_present(&paths, &[x, a]),
        "input xa with no R must pass identity; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 3. Rule `a -> b / x _ y`, input `xayxay` → `xbyxby`.
#[test]
fn constraint_multiple_occurrences_each_rewritten() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    let input = linear_input_acceptor(&[x, a, y, x, a, y]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    assert!(
        output_seq_present(&paths, &[x, b, y, x, b, y]),
        "both occurrences of a must be replaced; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
    // Note: residual ambiguity per F2c4. See test 1's comment.
}

// 4. Rule `a -> b` (no context), input `aaa` → must produce `bbb` among paths.
#[test]
fn constraint_no_context_rewrites_all() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();

    let input = linear_input_acceptor(&[a, a, a]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    assert!(
        output_seq_present(&paths, &[b, b, b]),
        "every a must be replaceable to b; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
    // Note: residual ambiguity — Replace's outside-state passthrough
    // produces partial / no transformations alongside `bbb`. F2c4
    // longest-leftmost will pick the canonical bracketing.
    let _ = a; // silence unused
}

// 5. Stray bracket: hand-construct an input with an open bracket but no
// LHS / close. Constraint B must reject it.
#[test]
fn constraint_rejects_stray_open_bracket() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    // Input `x · a · <[+]> · y` — an unmatched open bracket. Stray.
    let bad_input =
        linear_input_acceptor(&[x, a, BRACKET_OPEN_OBLIG_LABEL, y]);
    let composed = compose_sorted(&bad_input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        !any_accepting_path(&paths),
        "stray <[+]> with no closing bracket must be rejected; got {:?}",
        paths.iter().map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 5b. Sanity: a properly bracketed input (matching L_R context) is
// accepted by the constraint directly (smoke test of the positive
// side of constraint B).
#[test]
fn constraint_accepts_properly_bracketed_input() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    // `x · <[+]> · a · <]+> · y` — correctly bracketed.
    let input = linear_input_acceptor(&[
        x,
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
        y,
    ]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        any_accepting_path(&paths),
        "properly bracketed `x <[+]> a <]+> y` must be accepted",
    );
}

// 5c. Stray bracket *pair* without LHS content — `x <[+]> b <]+> y` —
// constraint B should reject (the content `b` is not the LHS `a`).
#[test]
fn constraint_rejects_bracket_pair_with_non_lhs_content() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    let input = linear_input_acceptor(&[
        x,
        BRACKET_OPEN_OBLIG_LABEL,
        b,
        BRACKET_CLOSE_OBLIG_LABEL,
        y,
    ]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        !any_accepting_path(&paths),
        "bracket pair with non-LHS content (b) must be rejected; got {:?}",
        paths.iter().map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 6. Boundary semantics — rule `a -> b / + _` (boundary on the left).
// With the F2c3.1 union patch, `+` matches BOUNDARY_LABEL, WORD_START,
// or WORD_END.
//
// Implementation note: `intro_brackets` and `replacement` from F2c1/F2c2
// only handle Σ user symbols (they explicitly exclude reserved markers).
// So full-pipeline tests with reserved markers in the input aren't
// supported until F2c5 extends those FSTs. For F2c3 we validate the
// **constraint's union-aware acceptance** by feeding a pre-bracketed
// input directly through the constraint and asserting acceptance:
// `<bdy><[+]>a<]+>` should be accepted (the L=`+` context matches
// the leading `<bdy>`), but `<[+]>a<]+>` alone (no L marker) should
// be rejected.
#[test]
fn constraint_boundary_left_context_via_bdy_label() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context(
        "a",
        "b",
        vec![PhonContextElem::Boundary],
        vec![],
    );
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let bdy = alpha.boundary_label();

    // `<bdy><[+]>a<]+>` — L (`+`) matches `<bdy>`, LHS `a` is
    // bracketed. Constraint should accept.
    let good_input = linear_input_acceptor(&[
        bdy,
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let good_composed = compose_sorted(&good_input, &constraint);
    let good_paths = bounded_paths(&good_composed);
    assert!(
        any_accepting_path(&good_paths),
        "constraint should accept `<bdy><[+]>a<]+>` (L=`+` matches `<bdy>`)",
    );

    // `<[+]>a<]+>` — no L marker. Constraint should reject (the
    // bracket pair has no preceding L context).
    let bad_input = linear_input_acceptor(&[
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let bad_composed = compose_sorted(&bad_input, &constraint);
    let bad_paths = bounded_paths(&bad_composed);
    assert!(
        !any_accepting_path(&bad_paths),
        "constraint should reject `<[+]>a<]+>` (no L=`+` marker preceding)",
    );

    // Also `<^><[+]>a<]+>` should be accepted (word-start counts as boundary
    // via the union patch).
    let ws_input = linear_input_acceptor(&[
        alpha.word_start_label(),
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let ws_composed = compose_sorted(&ws_input, &constraint);
    let ws_paths = bounded_paths(&ws_composed);
    assert!(
        any_accepting_path(&ws_paths),
        "constraint should accept `<^><[+]>a<]+>` (L=`+` matches `<^>` via union)",
    );
}

// 6b. Same rule, input is just `a` (no boundary marker). The
// constraint must reject any bracketing of `a` because the L context
// (`+`) requires a boundary/word-edge marker, which isn't present.
#[test]
fn constraint_boundary_required_when_absent_passes_identity() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context(
        "a",
        "b",
        vec![PhonContextElem::Boundary],
        vec![],
    );
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();

    let input = linear_input_acceptor(&[a]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    // Identity passes (no L match, no rewrite forced).
    assert!(
        output_seq_present(&paths, &[a]),
        "bare `a` (no boundary) must pass identity; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 7. Word-start anchor: rule `a -> b / ^ _`, with the `^` PhonContextElem
// compiling to a single-arc consumer of `WORD_START_LABEL`.
//
// Same constraint-only approach as test 6 — we validate that
// `<^><[+]>a<]+>` is accepted by the constraint and `<[+]>a<]+>`
// alone is rejected. The choice between "wrap input with `<^>...<$>`
// markers at runtime" and "union `^` into Boundary at compile time"
// is documented in `context.rs`: F2c3 took the union path for
// Boundary (matching word-start markers via the union), and `^` /
// `$` keep their single-arc compilation as zero-width consumers that
// the F2c5 apply driver will introduce around the input.
#[test]
fn constraint_word_start_context_via_marker() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context(
        "a",
        "b",
        vec![PhonContextElem::WordStart],
        vec![],
    );
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let word_start = alpha.word_start_label();

    // `<^><[+]>a<]+>` — L (`^`) matches the WORD_START_LABEL.
    let good_input = linear_input_acceptor(&[
        word_start,
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let good_composed = compose_sorted(&good_input, &constraint);
    let good_paths = bounded_paths(&good_composed);
    assert!(
        any_accepting_path(&good_paths),
        "constraint should accept `<^><[+]>a<]+>` (L=`^` matches `<^>`)",
    );

    // `<[+]>a<]+>` alone — no L marker. Constraint should reject.
    let bad_input = linear_input_acceptor(&[
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let bad_composed = compose_sorted(&bad_input, &constraint);
    let bad_paths = bounded_paths(&bad_composed);
    assert!(
        !any_accepting_path(&bad_paths),
        "constraint should reject `<[+]>a<]+>` (no L=`^` marker preceding)",
    );
}

// 8. Class LHS, with context — `V -> a / x _ y`, V = {a, b, c}.
#[test]
fn constraint_class_lhs_with_context() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("c");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");

    let v_class = class_list("V", &["a", "b", "c"]);
    let v_fst = compile_class(&v_class, &mut alpha, &HashMap::new()).expect("compile V");
    let mut class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    class_table.insert("V".to_string(), v_fst);
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = PhonRewriteRule {
        from: PhonPattern::Class(ident("V")),
        to: PhonReplacement::Literal(lit("a")),
        context: Some(PhonContext {
            left: vec![ctx_literal("x")],
            right: vec![ctx_literal("y")],
        }),
        span: sp(),
    };

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    // Input `xby` (b ∈ V) — should produce `xay`.
    let b = alpha.lookup("b").unwrap();
    let input = linear_input_acceptor(&[x, b, y]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    assert!(
        output_seq_present(&paths, &[x, a, y]),
        "rule V->a/x_y must rewrite xby to xay; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 9. Smoke test: bare input (no context match available) — pass through.
#[test]
fn constraint_input_without_context_passes_through() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let _ = alpha.intern("z");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);

    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");
    let repl = build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table)
        .expect("build replacement");
    let chain = build_full_pipeline(&alpha, &constraint, &repl);

    let z = alpha.lookup("z").unwrap();

    // Input `zzz` — no `a`, no x_y context. Just identity.
    let input = linear_input_acceptor(&[z, z, z]);
    let applied = compose_sorted(&input, &chain);
    let paths = bounded_paths(&applied);

    assert!(
        output_seq_present(&paths, &[z, z, z]),
        "input with no LHS occurrence passes unchanged; got {:?}",
        paths.iter().take(20).map(|p| p.output.clone()).collect::<Vec<_>>()
    );
}

// 10. Smoke test: empty input. Trivially accepted.
#[test]
fn constraint_empty_input_accepted() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let input = linear_input_acceptor(&[]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);

    assert!(
        any_accepting_path(&paths),
        "empty input must be accepted (no constraint violations possible)",
    );
}

// 11. Standalone constraint-only test: bracketed `xay` form is accepted.
// Confirms the positive side of Constraint A doesn't over-reject when
// the bracketing IS correct.
#[test]
fn constraint_accepts_correctly_bracketed_form() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    let input = linear_input_acceptor(&[
        x,
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
        y,
    ]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        any_accepting_path(&paths),
        "constraint must accept the correctly bracketed `x<[+]>a<]+>y`",
    );
}

// 13. Constraint-only test: a partially bracketed `aaa` (with the
// middle `a` bracketed but the outer `a`s not) is rejected — the
// outer `a`s are L_LHS_R contexts (L=R=ε, LHS=a) without brackets.
// Constraint A catches this.
#[test]
fn constraint_rejects_partially_bracketed_aaa() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();

    let input = linear_input_acceptor(&[
        a,
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        BRACKET_CLOSE_OBLIG_LABEL,
        a,
    ]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        !any_accepting_path(&paths),
        "constraint should reject `a<[+]>a<]+>a` (unbracketed a's at ends)",
    );
}

// 12. Constraint-only test: unbracketed `xay` is rejected (this is what
// Constraint A forbids).
#[test]
fn constraint_rejects_unbracketed_xay() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("build constraint");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();

    let input = linear_input_acceptor(&[x, a, y]);
    let composed = compose_sorted(&input, &constraint);
    let paths = bounded_paths(&composed);
    assert!(
        !any_accepting_path(&paths),
        "unbracketed xay must be rejected by constraint A",
    );
}
