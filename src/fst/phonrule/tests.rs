//! F2a smoke + equivalence tests for class / map compilation.
//!
//! Two test buckets:
//!
//!   * **Structural tests** verify the compiled FST accepts / rejects the
//!     right single-symbol inputs and produces the right single-symbol
//!     outputs. Inputs are taken to the FST as one label; outputs are
//!     read off the unique accepting path.
//!
//!   * **Equivalence tests** validate the F2a compiled FST against
//!     `phonrule_eval`'s implementation of the same semantics on the same
//!     inputs. Class compilation is validated against a re-implementation
//!     of `char_in_class` (the helper is private to `phonrule_eval`, so
//!     we walk the AST the same way). Map compilation is validated by
//!     building a complete phonrule whose body rewrites the full alphabet
//!     through the map and running both `apply_phonrule` and the FST on
//!     each single-character input.

use std::collections::HashMap;

use crate::ast::{
    CharClassBody, CharClassDef, DisplayMap, PhonAtom, PhonBodyItem, PhonContext, PhonContextElem,
    PhonMapArm, PhonMapBody, PhonMapDef, PhonMapElse, PhonMapResult, PhonPattern, PhonReplacement,
    PhonRewriteRule, PhonRule, Quantifier, Span, Spanned, StringLit,
};
use crate::phonrule_eval::apply_phonrule;
use crate::span::FileId;

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{Label, Path};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::{
    compile_class, compile_class_and_complement, compile_class_complement,
    compile_context_elem, compile_context_sequence, compile_map, compile_pattern_sequence,
    neg_class_key, ContextCompileError,
};

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

fn class_union(name: &str, refs: &[&str]) -> CharClassDef {
    CharClassDef {
        name: ident(name),
        body: CharClassBody::Union(refs.iter().map(|r| ident(r)).collect()),
    }
}

// ---------------------------------------------------------------------------
// FST query helpers.
// ---------------------------------------------------------------------------

/// Collect all accepting paths from an acyclic FST. Class / map FSTs are
/// guaranteed acyclic (2-state, no back-arcs), so eager collection is safe.
fn collect_paths(fst: &RustFstWrapper) -> Vec<Path> {
    RustFstBackend::paths(fst).unwrap().collect()
}

/// Check whether a single-symbol input is accepted with output equal to
/// itself (identity acceptance — the semantics of a class membership check).
fn accepts_identity(fst: &RustFstWrapper, label: Label) -> bool {
    collect_paths(fst).iter().any(|p| {
        p.input.as_slice() == [label] && p.output.as_slice() == [label]
    })
}

/// Read the output label for a single-symbol input in a transducer. Returns
/// `None` if no accepting path consumes `input_label`. Asserts there is at
/// most one such path (a malformed map with duplicate arcs would yield more,
/// and we want to know).
fn read_output(fst: &RustFstWrapper, input_label: Label) -> Option<Label> {
    let paths = collect_paths(fst);
    let mut matching = paths
        .iter()
        .filter(|p| p.input.as_slice() == [input_label]);
    let first = matching.next()?;
    assert!(
        matching.next().is_none(),
        "expected ≤1 accepting path for input {}, got multiple",
        input_label
    );
    assert_eq!(first.output.len(), 1, "expected single-symbol output");
    Some(first.output[0])
}

// ---------------------------------------------------------------------------
// `char_in_class` re-implementation for equivalence testing.
//
// `phonrule_eval::char_in_class` is private; we replicate its logic over the
// AST so tests can call it directly. This is what plan §6.2's
// `assert_fst_matches_eval` shape requires at the class-membership level.
// ---------------------------------------------------------------------------

fn eval_char_in_class(ch: &str, class_name: &str, classes: &[CharClassDef]) -> bool {
    for cls in classes {
        if cls.name.node == class_name {
            return match &cls.body {
                CharClassBody::List(members) => members.iter().any(|m| m.node == ch),
                CharClassBody::Union(refs) => {
                    refs.iter().any(|r| eval_char_in_class(ch, &r.node, classes))
                }
            };
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 1. Alphabet sanity.
// ---------------------------------------------------------------------------

#[test]
fn alphabet_intern_assigns_user_labels_above_reserved() {
    let mut a = PhonruleAlphabet::empty();
    let e = a.intern("e");
    let i = a.intern("i");
    let a2 = a.intern("a");
    assert!(e >= 16 && i >= 16 && a2 >= 16);
    assert_eq!(a.lookup("e"), Some(e));
    assert!(e != i && i != a2 && e != a2);
}

// ---------------------------------------------------------------------------
// 2. Class — List membership.
// ---------------------------------------------------------------------------

#[test]
fn class_list_accepts_members_rejects_non_members() {
    let mut alpha = PhonruleAlphabet::empty();
    // Pre-intern the full alphabet so non-members have labels too.
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let front = class_list("front", &["e", "i"]);
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_class(&front, &mut alpha, &table).unwrap();

    let l_e = alpha.lookup("e").unwrap();
    let l_i = alpha.lookup("i").unwrap();
    let l_a = alpha.lookup("a").unwrap();
    let l_o = alpha.lookup("o").unwrap();
    let l_u = alpha.lookup("u").unwrap();

    assert!(accepts_identity(&fst, l_e), "'e' should be in `front`");
    assert!(accepts_identity(&fst, l_i), "'i' should be in `front`");
    assert!(!accepts_identity(&fst, l_a));
    assert!(!accepts_identity(&fst, l_o));
    assert!(!accepts_identity(&fst, l_u));
}

#[test]
fn class_list_equivalence_with_eval() {
    let classes = vec![class_list("front", &["e", "i"])];
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_class(&classes[0], &mut alpha, &table).unwrap();

    for s in ["a", "e", "i", "o", "u"] {
        let l = alpha.lookup(s).unwrap();
        let fst_accepts = accepts_identity(&fst, l);
        let eval_accepts = eval_char_in_class(s, "front", &classes);
        assert_eq!(
            fst_accepts, eval_accepts,
            "FST vs eval disagree on '{}' in `front`: fst={} eval={}",
            s, fst_accepts, eval_accepts
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Class — Union.
// ---------------------------------------------------------------------------

#[test]
fn class_union_accepts_members_of_both() {
    let front = class_list("front", &["e", "i"]);
    let back = class_list("back", &["a", "o", "u"]);
    let v = class_union("V", &["front", "back"]);
    let classes = vec![front.clone(), back.clone(), v.clone()];

    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u", "x"] {
        alpha.intern(s);
    }
    let mut table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst_front = compile_class(&front, &mut alpha, &table).unwrap();
    table.insert("front".to_string(), fst_front);
    let fst_back = compile_class(&back, &mut alpha, &table).unwrap();
    table.insert("back".to_string(), fst_back);
    let fst_v = compile_class(&v, &mut alpha, &table).unwrap();

    for s in ["a", "e", "i", "o", "u", "x"] {
        let l = alpha.lookup(s).unwrap();
        let fst_accepts = accepts_identity(&fst_v, l);
        let eval_accepts = eval_char_in_class(s, "V", &classes);
        assert_eq!(
            fst_accepts, eval_accepts,
            "V membership mismatch on '{}'",
            s
        );
    }
}

#[test]
fn class_union_forward_reference_is_rejected() {
    // Union references a class not yet in the table.
    let v = class_union("V", &["front"]);
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_class(&v, &mut alpha, &table).expect_err("forward ref must error");
    let msg = err.to_string();
    assert!(msg.contains("front"), "error should name missing class: {}", msg);
    assert!(msg.contains('V'), "error should name referrer: {}", msg);
}

// ---------------------------------------------------------------------------
// 4. Class complement over Σ.
// ---------------------------------------------------------------------------

#[test]
fn class_complement_over_alphabet() {
    // front = {e, i}; Σ = {a, e, i, o, u}; !front should accept a, o, u.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let front = class_list("front", &["e", "i"]);
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let comp = compile_class_complement(&front, &mut alpha, &table).unwrap();

    for s in ["a", "o", "u"] {
        let l = alpha.lookup(s).unwrap();
        assert!(accepts_identity(&comp, l), "'{}' should be in !front", s);
    }
    for s in ["e", "i"] {
        let l = alpha.lookup(s).unwrap();
        assert!(!accepts_identity(&comp, l), "'{}' must NOT be in !front", s);
    }
}

// ---------------------------------------------------------------------------
// 5. Map — without `else_arm`.
// ---------------------------------------------------------------------------

fn make_map(name: &str, arms: &[(&str, &str)], else_arm: Option<PhonMapElse>) -> PhonMapDef {
    PhonMapDef {
        name: ident(name),
        param: ident("c"),
        body: PhonMapBody::Match {
            arms: arms
                .iter()
                .map(|(f, t)| PhonMapArm {
                    from: lit(f),
                    to: PhonMapResult::Literal(lit(t)),
                })
                .collect(),
            else_arm,
        },
    }
}

#[test]
fn map_without_else_uses_eval_identity_fallback() {
    // Per phonrule_eval.rs:544-545, a map with no else_arm returns the input
    // unchanged on non-listed characters. The FST must agree.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["e", "i", "o"] {
        alpha.intern(s);
    }
    alpha.intern("a");
    alpha.intern("ı");
    let m = make_map("m", &[("e", "a"), ("i", "ı")], None);
    let fst = compile_map(&m, &mut alpha);

    let l_e = alpha.lookup("e").unwrap();
    let l_i = alpha.lookup("i").unwrap();
    let l_o = alpha.lookup("o").unwrap();
    let l_a = alpha.lookup("a").unwrap();
    let l_idot = alpha.lookup("ı").unwrap();

    assert_eq!(read_output(&fst, l_e), Some(l_a), "'e' -> 'a'");
    assert_eq!(read_output(&fst, l_i), Some(l_idot), "'i' -> 'ı'");
    // No else_arm — non-listed inputs fall through to identity per eval.
    assert_eq!(read_output(&fst, l_o), Some(l_o), "'o' -> 'o' (identity fallback)");
}

// ---------------------------------------------------------------------------
// 6. Map — with `else -> c` (Var: identity).
// ---------------------------------------------------------------------------

#[test]
fn map_with_var_else_is_identity_on_uncovered() {
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["e", "i", "o", "a", "ı"] {
        alpha.intern(s);
    }
    let m = make_map(
        "m",
        &[("e", "a"), ("i", "ı")],
        Some(PhonMapElse::Var(ident("c"))),
    );
    let fst = compile_map(&m, &mut alpha);

    let l_o = alpha.lookup("o").unwrap();
    assert_eq!(read_output(&fst, l_o), Some(l_o));
}

// ---------------------------------------------------------------------------
// 7. Map — with `else -> Literal(X)` (collapses uncovered inputs).
// ---------------------------------------------------------------------------

#[test]
fn map_with_literal_else_collapses_uncovered_to_literal() {
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["e", "i", "o", "a", "x"] {
        alpha.intern(s);
    }
    let m = make_map(
        "m",
        &[("e", "a")],
        Some(PhonMapElse::Literal(lit("x"))),
    );
    let fst = compile_map(&m, &mut alpha);

    let l_e = alpha.lookup("e").unwrap();
    let l_i = alpha.lookup("i").unwrap();
    let l_o = alpha.lookup("o").unwrap();
    let l_x = alpha.lookup("x").unwrap();
    let l_a = alpha.lookup("a").unwrap();

    assert_eq!(read_output(&fst, l_e), Some(l_a));
    // Both "i" and "o" are uncovered → both map to "x".
    assert_eq!(read_output(&fst, l_i), Some(l_x));
    assert_eq!(read_output(&fst, l_o), Some(l_x));
}

// ---------------------------------------------------------------------------
// 8. Map — first-arm-wins on duplicate `from`.
// ---------------------------------------------------------------------------

#[test]
fn map_duplicate_from_first_arm_wins() {
    // Eval `apply_map` returns on the first matching arm; later arms with the
    // same `from` are dead code. The FST must mirror this — no parallel
    // arcs on the same input.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["e", "a", "b", "o"] {
        alpha.intern(s);
    }
    let m = make_map("m", &[("e", "a"), ("e", "b")], None);
    let fst = compile_map(&m, &mut alpha);
    let l_e = alpha.lookup("e").unwrap();
    let l_a = alpha.lookup("a").unwrap();
    assert_eq!(read_output(&fst, l_e), Some(l_a), "first arm wins → 'e' -> 'a'");
}

// ---------------------------------------------------------------------------
// 9. End-to-end equivalence: build a phonrule that rewrites every char via
//    a map, run both `apply_phonrule` and the FST, compare per input.
//
//    The rewrite rule is `C -> map / _` where `C` is a class that lists
//    the full single-character alphabet. Eval walks each char and applies
//    the map; we apply our map FST to each char's label. Outputs must
//    agree on every input character.
// ---------------------------------------------------------------------------

fn make_phonrule_for_map_test(map: PhonMapDef, alphabet_chars: &[&str]) -> PhonRule {
    // class C = [<every single-char alphabet member>]
    let c_class = class_list("C", alphabet_chars);
    // rewrite: C -> map_name / _
    let rule = PhonRewriteRule {
        from: PhonPattern::Class(ident("C")),
        to: PhonReplacement::Map(ident(&map.name.node)),
        context: Some(PhonContext {
            left: vec![],
            right: vec![],
        }),
        span: sp(),
    };
    PhonRule {
        name: ident("test_rule"),
        display: DisplayMap::new(),
        derived_from: None,
        syllable: None,
        classes: vec![c_class],
        maps: vec![map],
        body: vec![PhonBodyItem::Rewrite(rule)],
        span: sp(),
    }
}

#[test]
fn map_equivalence_with_eval_under_phonrule() {
    let alphabet_chars = ["a", "e", "i", "o", "u"];

    // Map: e -> a, i -> ı, no else_arm. Eval per char:
    //   "e" -> "a", "i" -> "ı", "a" -> "a", "o" -> "o", "u" -> "u".
    let map = make_map("m", &[("e", "a"), ("i", "ı")], None);
    let phonrule = make_phonrule_for_map_test(map.clone(), &alphabet_chars);

    // Prepare FST alphabet and map.
    let mut alpha = PhonruleAlphabet::empty();
    for s in &alphabet_chars {
        alpha.intern(s);
    }
    // The map output may introduce "ı" outside the input alphabet — intern
    // it so the alphabet contains every label we expect to compare against.
    alpha.intern("ı");
    let fst = compile_map(&map, &mut alpha);

    for ch in alphabet_chars.iter().copied() {
        let eval_out = apply_phonrule(ch, &phonrule);
        let in_label = alpha.lookup(ch).unwrap();
        let fst_out_label = read_output(&fst, in_label).unwrap_or_else(|| {
            panic!("FST has no path for input '{}' (label {})", ch, in_label)
        });
        let fst_out_str = alpha.label_to_str(fst_out_label).unwrap();
        assert_eq!(
            fst_out_str, eval_out,
            "FST vs eval disagree on '{}': fst='{}' eval='{}'",
            ch, fst_out_str, eval_out
        );
    }
}

#[test]
fn map_equivalence_with_else_var() {
    let alphabet_chars = ["a", "e", "i", "o"];
    let map = make_map(
        "m",
        &[("e", "a")],
        Some(PhonMapElse::Var(ident("c"))),
    );
    let phonrule = make_phonrule_for_map_test(map.clone(), &alphabet_chars);

    let mut alpha = PhonruleAlphabet::empty();
    for s in &alphabet_chars {
        alpha.intern(s);
    }
    let fst = compile_map(&map, &mut alpha);

    for ch in alphabet_chars.iter().copied() {
        let eval_out = apply_phonrule(ch, &phonrule);
        let in_label = alpha.lookup(ch).unwrap();
        let fst_out_label = read_output(&fst, in_label).unwrap();
        let fst_out_str = alpha.label_to_str(fst_out_label).unwrap();
        assert_eq!(fst_out_str, eval_out, "mismatch on '{}'", ch);
    }
}

// Note: there is no equivalent `map_equivalence_with_else_literal` test.
// `apply_phonrule` iterates a rewrite rule to convergence
// (`phonrule_eval.rs:164-178`). An `else -> Literal("x")` map applied
// inside a rewrite `C -> m / _` where any output is still in C will
// iterate multiple steps — but our single-step map FST returns the
// first-iteration result. Constructing a phonrule that converges in
// exactly one step requires excluding all map outputs from C, which
// then prevents eval from rewriting at all for those inputs. The
// single-step FST semantics is fully covered by the structural test
// `map_with_literal_else_collapses_uncovered_to_literal` above
// (matching eval's `apply_map` line-for-line). End-to-end iterative
// validation is F2b / F2c territory once the rewrite-rule FST and the
// iterate-to-convergence apply driver land.

// ===========================================================================
// F2b — Context / pattern elem compilation.
//
// These tests verify each `PhonContextElem` variant compiles to an acceptor
// over `PhonruleAlphabet` whose accepted-language matches the eval's
// per-elem matching behaviour (`phonrule_eval::match_seq` / `consume_atom`).
// Per the brief, F2b validates each piece in isolation as an acceptor; the
// full Karttunen-pipeline integration is F2c territory.
// ===========================================================================

// ---------------------------------------------------------------------------
// AST helpers for context elems.
// ---------------------------------------------------------------------------

fn ctx_class(name: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Class(ident(name)), Quantifier::Exact(1))
}

fn ctx_neg_class(name: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::NegClass(ident(name)), Quantifier::Exact(1))
}

fn ctx_literal(s: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Literal(lit(s)), Quantifier::Exact(1))
}

fn ctx_wildcard() -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Wildcard, Quantifier::Exact(1))
}

fn ctx_alt(alts: Vec<PhonContextElem>) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Alt(alts), Quantifier::Exact(1))
}

fn ctx_quant(atom: PhonAtom, q: Quantifier) -> PhonContextElem {
    PhonContextElem::Atom(atom, q)
}

// ---------------------------------------------------------------------------
// FST acceptance helpers.
// ---------------------------------------------------------------------------

/// Whether `fst` accepts exactly the path of labels `seq` (identity I/O on
/// each step). For acyclic FSTs we collect all paths; for Kleene-star
/// outputs we cap the iterator to avoid hanging.
fn accepts_seq(fst: &RustFstWrapper, seq: &[Label]) -> bool {
    // Cap at a generous bound for cyclic FSTs (Star/Plus). We only need
    // to know whether the specific sequence is in the language; lazily
    // matching it against a bounded slice of paths is fine for the
    // sequence lengths in these tests (≤ 6 symbols).
    RustFstBackend::paths(fst).unwrap().take(2048).any(|p| {
        p.input.as_slice() == seq && p.output.as_slice() == seq
    })
}

/// Whether `fst` accepts the empty input (ε).
fn accepts_empty(fst: &RustFstWrapper) -> bool {
    RustFstBackend::paths(fst).unwrap().take(2048).any(|p| {
        p.input.is_empty() && p.output.is_empty()
    })
}

// ---------------------------------------------------------------------------
// Boundary / WordStart / WordEnd — single-arc consumers of reserved markers.
// ---------------------------------------------------------------------------

#[test]
fn ctx_boundary_accepts_boundary_or_word_edges() {
    // F2c3 reconciliation: `+` now compiles to the union
    // (boundary | word_start | word_end), matching
    // `phonrule_eval::match_seq`'s "boundary OR edge" semantics
    // (`phonrule_eval.rs:638-653`). Pre-F2c3 this acceptor only
    // accepted BOUNDARY_LABEL; documented here so a future bisect
    // knows the change is intentional.
    let mut alpha = PhonruleAlphabet::empty();
    let a = alpha.intern("a");
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&PhonContextElem::Boundary, &mut alpha, &table).unwrap();
    assert!(accepts_seq(&fst, &[alpha.boundary_label()]), "+ accepts <bdy>");
    assert!(
        accepts_seq(&fst, &[alpha.word_start_label()]),
        "+ accepts <^> (union semantics — F2c3)"
    );
    assert!(
        accepts_seq(&fst, &[alpha.word_end_label()]),
        "+ accepts <$> (union semantics — F2c3)"
    );
    assert!(!accepts_seq(&fst, &[a]), "+ must reject a phoneme symbol");
    assert!(!accepts_empty(&fst), "+ consumes one symbol; ε rejected");
}

#[test]
fn ctx_word_start_accepts_word_start_label_only() {
    let mut alpha = PhonruleAlphabet::empty();
    let a = alpha.intern("a");
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&PhonContextElem::WordStart, &mut alpha, &table).unwrap();
    assert!(accepts_seq(&fst, &[alpha.word_start_label()]));
    assert!(!accepts_seq(&fst, &[a]));
    assert!(!accepts_seq(&fst, &[alpha.boundary_label()]));
    assert!(!accepts_seq(&fst, &[alpha.word_end_label()]));
}

#[test]
fn ctx_word_end_accepts_word_end_label_only() {
    let mut alpha = PhonruleAlphabet::empty();
    let a = alpha.intern("a");
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&PhonContextElem::WordEnd, &mut alpha, &table).unwrap();
    assert!(accepts_seq(&fst, &[alpha.word_end_label()]));
    assert!(!accepts_seq(&fst, &[a]));
    assert!(!accepts_seq(&fst, &[alpha.boundary_label()]));
    assert!(!accepts_seq(&fst, &[alpha.word_start_label()]));
}

// ---------------------------------------------------------------------------
// Literal — multi-char path acceptor.
// ---------------------------------------------------------------------------

#[test]
fn ctx_literal_ab_accepts_a_then_b_only() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&ctx_literal("ab"), &mut alpha, &table).unwrap();
    let a = alpha.lookup("a").expect("'a' interned");
    let b = alpha.lookup("b").expect("'b' interned");
    // Positive.
    assert!(accepts_seq(&fst, &[a, b]), "literal 'ab' accepts [a, b]");
    // Negatives.
    assert!(!accepts_seq(&fst, &[a]), "'ab' rejects [a] (prefix only)");
    assert!(!accepts_seq(&fst, &[b]), "'ab' rejects [b] (suffix only)");
    assert!(!accepts_seq(&fst, &[a, b, a]), "'ab' rejects [a, b, a] (trailing)");
    assert!(!accepts_empty(&fst), "non-empty literal rejects ε");
}

#[test]
fn ctx_literal_single_char_accepts_only_that_char() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&ctx_literal("x"), &mut alpha, &table).unwrap();
    let x = alpha.lookup("x").unwrap();
    let y = alpha.intern("y");
    assert!(accepts_seq(&fst, &[x]));
    assert!(!accepts_seq(&fst, &[y]));
}

#[test]
fn ctx_literal_empty_is_epsilon_acceptor() {
    // Eval's `consume_literal` on "" exits its for-loop immediately and
    // returns `Some(cursor)` — the literal consumes no chars. Match by
    // accepting ε.
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&ctx_literal(""), &mut alpha, &table).unwrap();
    assert!(accepts_empty(&fst));
}

// ---------------------------------------------------------------------------
// Alternation — union of compiled alternative acceptors.
// ---------------------------------------------------------------------------

#[test]
fn ctx_alternation_accepts_each_alt_rejects_outsider() {
    // ( a | b ): single-position acceptor of either "a" or "b".
    let mut alpha = PhonruleAlphabet::empty();
    let _ = alpha.intern("c"); // ensure 'c' has a label even though not in alt
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(
        &ctx_alt(vec![ctx_literal("a"), ctx_literal("b")]),
        &mut alpha,
        &table,
    )
    .unwrap();
    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let c = alpha.lookup("c").unwrap();
    assert!(accepts_seq(&fst, &[a]), "(a|b) accepts [a]");
    assert!(accepts_seq(&fst, &[b]), "(a|b) accepts [b]");
    assert!(!accepts_seq(&fst, &[c]), "(a|b) rejects [c]");
    assert!(!accepts_empty(&fst), "(a|b) of literal atoms is non-ε");
}

// ---------------------------------------------------------------------------
// Star — Kleene star of a class acceptor.
// ---------------------------------------------------------------------------

#[test]
fn ctx_star_of_class_accepts_empty_and_repeats() {
    // V* over class V = {e, i}.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let v_class = class_list("V", &["e", "i"]);
    let mut table: HashMap<String, RustFstWrapper> = HashMap::new();
    let v_fst = compile_class(&v_class, &mut alpha, &table).unwrap();
    table.insert("V".to_string(), v_fst);

    // Build V* as `Atom(Class("V"), Star)`.
    let fst = compile_context_elem(
        &ctx_quant(PhonAtom::Class(ident("V")), Quantifier::Star),
        &mut alpha,
        &table,
    )
    .unwrap();
    let e = alpha.lookup("e").unwrap();
    let i = alpha.lookup("i").unwrap();
    let a = alpha.lookup("a").unwrap();

    assert!(accepts_empty(&fst), "V* accepts ε");
    assert!(accepts_seq(&fst, &[e]), "V* accepts [e]");
    assert!(accepts_seq(&fst, &[i]), "V* accepts [i]");
    assert!(accepts_seq(&fst, &[e, i, e]), "V* accepts [e, i, e]");
    assert!(!accepts_seq(&fst, &[a]), "V* rejects [a] (non-member)");
    assert!(!accepts_seq(&fst, &[e, a]), "V* rejects [e, a] (has non-member)");
}

// ---------------------------------------------------------------------------
// Class — delegates to F2a's `compile_class` via `class_table`.
// ---------------------------------------------------------------------------

#[test]
fn ctx_class_regression_reuses_f2a_compiled_class() {
    // front = {e, i}. The context elem `Class("front")` must return the
    // same acceptor F2a built — same accept set.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let front = class_list("front", &["e", "i"]);
    let mut table: HashMap<String, RustFstWrapper> = HashMap::new();
    let front_fst = compile_class(&front, &mut alpha, &table).unwrap();
    table.insert("front".to_string(), front_fst);

    let fst = compile_context_elem(&ctx_class("front"), &mut alpha, &table).unwrap();
    let e = alpha.lookup("e").unwrap();
    let a = alpha.lookup("a").unwrap();
    assert!(accepts_seq(&fst, &[e]));
    assert!(!accepts_seq(&fst, &[a]));
}

#[test]
fn ctx_unknown_class_errors() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_context_elem(&ctx_class("nonexistent"), &mut alpha, &table)
        .expect_err("unknown class must error");
    match err {
        ContextCompileError::UnknownClass { name } => {
            assert_eq!(name, "nonexistent");
        }
        other => panic!("expected UnknownClass, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// NegClass — looked up via the !<name> key produced by
// `compile_class_and_complement`.
// ---------------------------------------------------------------------------

#[test]
fn ctx_neg_class_uses_precompiled_complement() {
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "e", "i", "o", "u"] {
        alpha.intern(s);
    }
    let front = class_list("front", &["e", "i"]);
    let mut table: HashMap<String, RustFstWrapper> = HashMap::new();
    // Use the helper that produces both forms.
    let (pos_key, pos, neg_key, neg) =
        compile_class_and_complement(&front, &mut alpha, &table).unwrap();
    assert_eq!(pos_key, "front");
    assert_eq!(neg_key, "!front");
    table.insert(pos_key, pos);
    table.insert(neg_key, neg);

    let fst = compile_context_elem(&ctx_neg_class("front"), &mut alpha, &table).unwrap();
    let e = alpha.lookup("e").unwrap();
    let a = alpha.lookup("a").unwrap();
    assert!(!accepts_seq(&fst, &[e]), "!front rejects 'e'");
    assert!(accepts_seq(&fst, &[a]), "!front accepts 'a'");
}

#[test]
fn ctx_neg_class_without_precompile_errors() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_context_elem(&ctx_neg_class("front"), &mut alpha, &table)
        .expect_err("neg class without complement entry must error");
    match err {
        ContextCompileError::UnknownClass { name } => {
            assert_eq!(name, neg_class_key("front"));
        }
        other => panic!("expected UnknownClass for '!front', got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Wildcard — accepts any single Σ member.
// ---------------------------------------------------------------------------

#[test]
fn ctx_wildcard_accepts_any_sigma_member() {
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "b", "c"] {
        alpha.intern(s);
    }
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_elem(&ctx_wildcard(), &mut alpha, &table).unwrap();
    for s in ["a", "b", "c"] {
        let l = alpha.lookup(s).unwrap();
        assert!(accepts_seq(&fst, &[l]), "'.' accepts '{}'", s);
    }
    // The wildcard explicitly excludes reserved markers (boundary,
    // word-edge): they are not in Σ.
    assert!(!accepts_seq(&fst, &[alpha.boundary_label()]));
    assert!(!accepts_seq(&fst, &[alpha.word_start_label()]));
}

// ---------------------------------------------------------------------------
// Syllable elems — F2 non-goal, must error cleanly.
// ---------------------------------------------------------------------------

#[test]
fn ctx_syl_head_is_unsupported() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_context_elem(&PhonContextElem::SylHead, &mut alpha, &table).unwrap_err();
    match err {
        ContextCompileError::SyllableElemUnsupported { kind } => {
            assert_eq!(kind, "%syl<head>%");
        }
        other => panic!("expected SyllableElemUnsupported, got {:?}", other),
    }
}

#[test]
fn ctx_syl_tail_is_unsupported() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_context_elem(&PhonContextElem::SylTail, &mut alpha, &table).unwrap_err();
    assert!(matches!(err, ContextCompileError::SyllableElemUnsupported { .. }));
}

#[test]
fn ctx_syl_block_atom_is_unsupported() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let elem = PhonContextElem::Atom(PhonAtom::SylBlock(vec![]), Quantifier::Exact(1));
    let err = compile_context_elem(&elem, &mut alpha, &table).unwrap_err();
    assert!(matches!(err, ContextCompileError::SyllableElemUnsupported { .. }));
}

// ---------------------------------------------------------------------------
// Sequence concat — [Class("L"), Boundary, Class("R")].
// ---------------------------------------------------------------------------

#[test]
fn ctx_sequence_l_boundary_r_accepts_exact_path() {
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "b", "c", "d"] {
        alpha.intern(s);
    }
    let l_class = class_list("L", &["a", "b"]);
    let r_class = class_list("R", &["c", "d"]);
    let mut table: HashMap<String, RustFstWrapper> = HashMap::new();
    table.insert("L".to_string(), compile_class(&l_class, &mut alpha, &table).unwrap());
    table.insert("R".to_string(), compile_class(&r_class, &mut alpha, &table).unwrap());

    let seq = vec![
        ctx_class("L"),
        PhonContextElem::Boundary,
        ctx_class("R"),
    ];
    let fst = compile_context_sequence(&seq, &mut alpha, &table).unwrap();

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    let c = alpha.lookup("c").unwrap();
    let d = alpha.lookup("d").unwrap();
    let bdy = alpha.boundary_label();

    // Positives: every (L_member, bdy, R_member) triple.
    assert!(accepts_seq(&fst, &[a, bdy, c]));
    assert!(accepts_seq(&fst, &[a, bdy, d]));
    assert!(accepts_seq(&fst, &[b, bdy, c]));
    assert!(accepts_seq(&fst, &[b, bdy, d]));

    // Negatives: wrong members, wrong order, missing boundary, extra symbols.
    assert!(!accepts_seq(&fst, &[c, bdy, a]), "wrong sides");
    assert!(!accepts_seq(&fst, &[a, c]), "missing boundary");
    assert!(!accepts_seq(&fst, &[a, bdy]), "missing right");
    assert!(!accepts_seq(&fst, &[bdy, c]), "missing left");
    assert!(!accepts_seq(&fst, &[a, bdy, c, d]), "trailing junk");
    assert!(!accepts_empty(&fst), "non-empty sequence rejects ε");
}

#[test]
fn ctx_empty_sequence_is_epsilon() {
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let fst = compile_context_sequence(&[], &mut alpha, &table).unwrap();
    assert!(accepts_empty(&fst), "empty context sequence accepts ε");
}

// ---------------------------------------------------------------------------
// Pattern sequence — structurally identical to context sequence.
// ---------------------------------------------------------------------------

#[test]
fn ctx_pattern_sequence_matches_context_sequence_semantics() {
    // The PhonPattern::Range LHS reuses PhonContextElem; the pattern
    // entry-point should produce the same FST as the context entry point
    // for the same input. Spot-check by accepting equivalent strings.
    let mut alpha = PhonruleAlphabet::empty();
    for s in ["a", "b"] {
        alpha.intern(s);
    }
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let seq = vec![ctx_literal("a"), ctx_literal("b")];

    let ctx_fst = compile_context_sequence(&seq, &mut alpha, &table).unwrap();
    let pat_fst = compile_pattern_sequence(&seq, &mut alpha, &table).unwrap();

    let a = alpha.lookup("a").unwrap();
    let b = alpha.lookup("b").unwrap();
    assert!(accepts_seq(&ctx_fst, &[a, b]));
    assert!(accepts_seq(&pat_fst, &[a, b]));
    assert!(!accepts_seq(&pat_fst, &[a]));
    assert!(!accepts_seq(&pat_fst, &[b]));
}

#[test]
fn ctx_pattern_sequence_rejects_syllable_elems() {
    // The plan §10 non-goal applies to LHS patterns too. Pattern
    // compilation must surface the same SyllableElemUnsupported error
    // when a `SylHead` / `SylTail` / `SylBlock` appears on the LHS.
    let mut alpha = PhonruleAlphabet::empty();
    let table: HashMap<String, RustFstWrapper> = HashMap::new();
    let err = compile_pattern_sequence(
        &[PhonContextElem::SylHead],
        &mut alpha,
        &table,
    )
    .unwrap_err();
    assert!(matches!(err, ContextCompileError::SyllableElemUnsupported { .. }));
}
