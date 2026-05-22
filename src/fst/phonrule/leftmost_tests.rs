//! Integration round-trip tests for the F2c4 longest-leftmost filter.
//!
//! These tests assemble the **full bracketed-string pipeline** with the
//! leftmost filter slotted between Replace and Strip:
//!
//! ```text
//!   input ∘ intro_brackets ∘ constraint ∘ replacement ∘ leftmost ∘ strip_brackets
//! ```
//!
//! and assert that each (rule, input) produces **exactly one** accepting
//! output, matching `phonrule_eval`'s answer. This is the canonical
//! correctness gate for F2c4: the filter must eliminate the residual
//! ambiguity F2c3 left.
//!
//! ## Discipline
//!
//! Each test:
//!
//!   1. Builds the rule + alphabet + class table.
//!   2. Compiles constraint, replacement, leftmost.
//!   3. Composes the full pipeline (sigma-sorted at each step).
//!   4. Builds the input acceptor and composes with the pipeline.
//!   5. Enumerates accepting paths (bounded), deduplicates outputs.
//!   6. Asserts exactly one unique output and that it matches the
//!      expected canonical answer.
//!
//! The dedup step is necessary because rustfst's `paths()` enumerates
//! distinct **paths** through the FST, not distinct outputs — two paths
//! producing the same output are both yielded. For F2c4 the contract is
//! "exactly one **output**", which is what we assert. F2c5's
//! `determinize + minimize` will compress duplicate paths.
//!
//! ## Why `compose` over `intersect`
//!
//! The `constraint ∩ leftmost` step in the brief's pipeline notation is
//! a set intersection of two acceptors over the bracketed alphabet, but
//! the two acceptors live at **different points** in the chain:
//!
//!   - `constraint` operates on the **pre-Replace** bracketed input.
//!   - `leftmost` operates on the **post-Replace** bracketed string.
//!
//! So in practice the pipeline composes them sequentially with Replace
//! in between, not as an FSA intersection. The brief's `∩` notation was
//! abstract; the concrete composition is `... ∘ constraint ∘ replace ∘
//! leftmost ∘ ...`. Tests below match the concrete order.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    CharClassBody, CharClassDef, PhonAtom, PhonContext, PhonContextElem, PhonMapArm,
    PhonMapBody, PhonMapDef, PhonMapElse, PhonMapResult, PhonPattern, PhonReplacement,
    PhonRewriteRule, Quantifier, Span, Spanned, StringLit,
};
use crate::span::FileId;

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBuilder, Label, Path};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::brackets::{intro_brackets, strip_brackets};
use super::class::compile_class;
use super::constraint::build_obligatory_constraint;
use super::leftmost::build_longest_leftmost_filter;
use super::map::compile_map;
use super::replacement::build_replacement_transducer;

// ---------------------------------------------------------------------------
// AST helpers (mirror constraint_tests / replacement_tests style).
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

fn rule_no_context(from: &str, to: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Literal(lit(to)),
        context: None,
        span: sp(),
    }
}

fn rule_to_null(from: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Null,
        context: None,
        span: sp(),
    }
}

fn rule_to_map(from: &str, map_name: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Map(ident(map_name)),
        context: None,
        span: sp(),
    }
}

// ---------------------------------------------------------------------------
// Pipeline helpers.
// ---------------------------------------------------------------------------

/// Build a linear identity-IO acceptor over `labels`.
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

/// Arc-sort both operands and compose. Mirrors the discipline used in
/// `constraint_tests::compose_sorted`.
fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> RustFstWrapper {
    let a_sorted = RustFstBackend::arc_sort_output(a).expect("arc_sort_output");
    let b_sorted = RustFstBackend::arc_sort_input(b).expect("arc_sort_input");
    RustFstBackend::compose(&a_sorted, &b_sorted).expect("compose")
}

/// Bounded cap for enumerating paths on possibly-cyclic compositions.
///
/// The cyclic ε-emit arcs in `intro_brackets` mean that without
/// bounding, paths() can enumerate forever. 4096 is the same generous
/// cap used by `constraint_tests` / `replacement_tests`; the F2c4 chain
/// is tighter so most tests see <100 distinct outputs.
const BOUNDED_PATHS: usize = 4096;

fn bounded_paths(fst: &RustFstWrapper) -> Vec<Path> {
    RustFstBackend::paths(fst)
        .expect("paths iter")
        .take(BOUNDED_PATHS)
        .collect()
}

/// Collect the unique output sequences from `paths`, preserving
/// first-seen order.
///
/// `paths()` enumerates accepting **paths** of the FST, not distinct
/// outputs. For F2c4 we care about output uniqueness, so we dedupe by
/// output label sequence.
fn unique_outputs(paths: &[Path]) -> Vec<Vec<Label>> {
    let mut seen: HashSet<Vec<Label>> = HashSet::new();
    let mut out: Vec<Vec<Label>> = Vec::new();
    for p in paths {
        let key = p.output.clone();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

/// Build the full F2c4 pipeline:
///
/// ```text
///   intro_brackets ∘ constraint ∘ replacement ∘ leftmost ∘ strip_brackets
/// ```
fn build_full_pipeline(
    alpha: &PhonruleAlphabet,
    constraint: &RustFstWrapper,
    repl: &RustFstWrapper,
    leftmost: &RustFstWrapper,
) -> RustFstWrapper {
    let intro = intro_brackets(alpha);
    let strip = strip_brackets(alpha);
    let s1 = compose_sorted(&intro, constraint);
    let s2 = compose_sorted(&s1, repl);
    let s3 = compose_sorted(&s2, leftmost);
    compose_sorted(&s3, &strip)
}

/// Apply the pipeline to an input and return the unique outputs.
fn run_pipeline(
    alpha: &PhonruleAlphabet,
    constraint: &RustFstWrapper,
    repl: &RustFstWrapper,
    leftmost: &RustFstWrapper,
    input: &RustFstWrapper,
) -> Vec<Vec<Label>> {
    let chain = build_full_pipeline(alpha, constraint, repl, leftmost);
    let applied = compose_sorted(input, &chain);
    unique_outputs(&bounded_paths(&applied))
}

/// Assert exactly one unique output equal to `expected`.
///
/// Renders a readable failure including all observed outputs (decoded
/// through the symbol table) so test failures point at the residual
/// ambiguity directly.
#[track_caller]
fn assert_single_output(
    alpha: &PhonruleAlphabet,
    outputs: &[Vec<Label>],
    expected: &[Label],
) {
    let decode = |lbls: &[Label]| -> String {
        lbls.iter()
            .map(|l| alpha.label_to_str(*l).unwrap_or("?").to_string())
            .collect::<Vec<_>>()
            .join("")
    };
    if outputs.len() == 1 && outputs[0].as_slice() == expected {
        return;
    }
    let exp_s = decode(expected);
    let observed: Vec<String> = outputs.iter().map(|o| decode(o)).collect();
    panic!(
        "expected exactly one output '{}' ({:?}); observed {} unique outputs: {:?}",
        exp_s,
        expected,
        outputs.len(),
        observed
    );
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

// 1. `a -> b`, input `aaa` → exactly `bbb`.
#[test]
fn leftmost_no_context_aaa_to_bbb() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let input = linear_input_acceptor(&[a, a, a]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[b, b, b]);
}

// 2. `a -> b / x _ y`, input `xay` → exactly `xby`.
#[test]
fn leftmost_xay_to_xby() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();
    let input = linear_input_acceptor(&[x, a, y]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, b, y]);
    let _ = a;
}

// 3. `a -> b / x _ y`, input `xayxay` → exactly `xbyxby`.
#[test]
fn leftmost_xayxay_to_xbyxby() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();
    let input = linear_input_acceptor(&[x, a, y, x, a, y]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, b, y, x, b, y]);
    let _ = a;
}

// 4. `a -> b / x _ y`, input `xa` (no R) → exactly `xa`.
#[test]
fn leftmost_no_r_match_passes_unchanged() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let input = linear_input_acceptor(&[x, a]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, a]);
}

// 5. `a -> null`, input `aaa` → exactly empty string.
#[test]
fn leftmost_null_rhs_deletes_all() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_to_null("a");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let input = linear_input_acceptor(&[a, a, a]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[]);
}

// 6. Class LHS `V -> a / x _ y` with V = {a, b, c}, input `xby` → exactly
// `xay`.
#[test]
fn leftmost_class_lhs_with_context() {
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
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();
    let input = linear_input_acceptor(&[x, b, y]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, a, y]);
}

// 7. No LHS occurrence: rule `a -> b`, input `zzz` → exactly `zzz`.
#[test]
fn leftmost_input_without_lhs_passes_through() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("z");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let z = alpha.lookup("z").unwrap();
    let input = linear_input_acceptor(&[z, z, z]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[z, z, z]);
}

// 8. Empty input: any rule on empty string → exactly empty string.
#[test]
fn leftmost_empty_input_passes_through() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let input = linear_input_acceptor(&[]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[]);
}

// 9. Rule `a -> to_x` where to_x maps `a → x`, else identity. Input `aaa`
// → exactly `xxx`. Exercises the Map RHS code path of the leftmost
// filter (`build_any_single_sigma_acceptor`).
#[test]
fn leftmost_map_rhs_aaa_to_xxx() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();

    // Build the map: to_x = c -> match { "a" -> "x", else -> c }
    let map_def = PhonMapDef {
        name: ident("to_x"),
        param: ident("c"),
        body: PhonMapBody::Match {
            arms: vec![PhonMapArm {
                from: lit("a"),
                to: PhonMapResult::Literal(lit("x")),
            }],
            else_arm: Some(PhonMapElse::Var(ident("c"))),
        },
    };
    let map_fst = compile_map(&map_def, &mut alpha);
    let mut map_table: HashMap<String, RustFstWrapper> = HashMap::new();
    map_table.insert("to_x".to_string(), map_fst);

    let rule = rule_to_map("a", "to_x");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let x = alpha.lookup("x").unwrap();
    let input = linear_input_acceptor(&[a, a, a]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, x, x]);
}

// 10. `ab -> xy`, input `abab` → exactly `xyxy`. Two non-overlapping
// occurrences of a multi-char LHS.
#[test]
fn leftmost_multi_char_lhs_abab_to_xyxy() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("ab", "xy");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();
    let input = linear_input_acceptor(&[a, b, a, b]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, y, x, y]);
}

// 11. `a -> b` with single occurrence: input `cac` → exactly `cbc`.
// Sanity that surrounding characters are preserved.
#[test]
fn leftmost_single_occurrence_in_context() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("b");
    let _ = alpha.intern("c");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_no_context("a", "b");
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let c = alpha.lookup("c").unwrap();
    let input = linear_input_acceptor(&[c, a, c]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[c, b, c]);
}

// 12. `a -> b / x _ y`, input `xayay` (second `a` has no preceding `x`).
// Only the first `a` should rewrite → exactly `xbyay`.
#[test]
fn leftmost_context_selective_rewrite() {
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("a");
    let _ = alpha.intern("x");
    let _ = alpha.intern("y");
    let class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

    let rule = rule_with_context("a", "b", vec![ctx_literal("x")], vec![ctx_literal("y")]);
    let constraint =
        build_obligatory_constraint(&rule, &mut alpha, &class_table).expect("constraint");
    let repl =
        build_replacement_transducer(&rule, &mut alpha, &class_table, &map_table).expect("repl");
    let leftmost =
        build_longest_leftmost_filter(&rule, &mut alpha, &class_table, &map_table).expect("leftmost");

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.lookup("y").unwrap();
    let input = linear_input_acceptor(&[x, a, y, a, y]);
    let outputs = run_pipeline(&alpha, &constraint, &repl, &leftmost, &input);
    assert_single_output(&alpha, &outputs, &[x, b, y, a, y]);
}
