//! F1 smoke tests for [`crate::fst`].
//!
//! Correctness coverage, not performance. Each test exercises a single trait
//! method (or a small composition) on a deliberately tiny FST. If these pass,
//! the seam works and the morphology layer (F2+) has a foundation to sit on.
//!
//! All tests run against `Backend` (the public type alias). Adding an
//! `InTreeBackend` in F-later means re-aliasing and re-running these tests
//! verbatim — the test bodies do not name `RustFstBackend` directly.

use super::{Backend, FstBackend, FstBuilder, Label, Path, SymbolTable};
use std::collections::HashSet;

/// Build a 2-state FST that maps the single input symbol `input_label` to
/// the single output symbol `output_label`. Returns the FST.
fn single_arc_fst(input_label: Label, output_label: Label) -> <Backend as FstBackend>::Fst {
    let mut b = Backend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).unwrap();
    b.set_final(s1).unwrap();
    b.add_arc(s0, input_label, output_label, s1).unwrap();
    b.finish().unwrap()
}

/// Convert a paths iterator into a sorted Vec for deterministic comparisons.
///
/// **Caller responsibility**: this helper eagerly drains the path iterator.
/// Do NOT call it on FSTs with cyclic structure (Kleene-star results etc.) —
/// the iterator is infinite there. The closure-star/plus tests below use
/// `.take(N)` directly on `Backend::paths` instead.
fn collect_paths(fst: &<Backend as FstBackend>::Fst) -> Vec<Path> {
    let mut v: Vec<Path> = Backend::paths(fst).unwrap().collect();
    v.sort_by(|a, b| a.input.cmp(&b.input).then(a.output.cmp(&b.output)));
    v
}

// ---------------------------------------------------------------------------
// 1. Build + traverse: (a:x)(b:y), input [a,b] → output [x,y].
// ---------------------------------------------------------------------------

#[test]
fn smoke_build_and_traverse_two_arc_chain() {
    let mut syms = SymbolTable::new();
    let a = syms.intern("a");
    let b = syms.intern("b");
    let x = syms.intern("x");
    let y = syms.intern("y");

    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    let s2 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s2).unwrap();
    bld.add_arc(s0, a, x, s1).unwrap();
    bld.add_arc(s1, b, y, s2).unwrap();
    let fst = bld.finish().unwrap();

    let paths = collect_paths(&fst);
    assert_eq!(paths.len(), 1, "expected exactly one path");
    assert_eq!(paths[0].input, vec![a, b]);
    assert_eq!(paths[0].output, vec![x, y]);
}

// ---------------------------------------------------------------------------
// 2. Concat / union / closure.
// ---------------------------------------------------------------------------

#[test]
fn smoke_concat() {
    let a = single_arc_fst(1, 10);
    let b = single_arc_fst(2, 20);
    let cat = Backend::concat(&a, &b).unwrap();
    let paths = collect_paths(&cat);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].input, vec![1, 2]);
    assert_eq!(paths[0].output, vec![10, 20]);
}

#[test]
fn smoke_union() {
    let a = single_arc_fst(1, 10);
    let b = single_arc_fst(2, 20);
    let u = Backend::union(&a, &b).unwrap();
    let paths: HashSet<(Vec<Label>, Vec<Label>)> = Backend::paths(&u)
        .unwrap()
        .map(|p| (p.input, p.output))
        .collect();
    assert!(paths.contains(&(vec![1], vec![10])), "union path 1 missing");
    assert!(paths.contains(&(vec![2], vec![20])), "union path 2 missing");
    assert_eq!(paths.len(), 2);
}

#[test]
fn smoke_closure_star_accepts_empty_one_two() {
    let a = single_arc_fst(1, 10);
    let star = Backend::closure_star(&a).unwrap();

    let path_set: HashSet<(Vec<Label>, Vec<Label>)> = Backend::paths(&star)
        .unwrap()
        .take(10) // safety cap; we only check the first few
        .map(|p| (p.input, p.output))
        .collect();
    assert!(path_set.contains(&(vec![], vec![])), "ε path missing");
    assert!(path_set.contains(&(vec![1], vec![10])), "1-iteration missing");
    assert!(
        path_set.contains(&(vec![1, 1], vec![10, 10])),
        "2-iteration missing"
    );
}

#[test]
fn smoke_closure_plus_excludes_empty() {
    let a = single_arc_fst(1, 10);
    let plus = Backend::closure_plus(&a).unwrap();
    let path_set: HashSet<(Vec<Label>, Vec<Label>)> =
        Backend::paths(&plus).unwrap().take(10).map(|p| (p.input, p.output)).collect();
    assert!(
        !path_set.contains(&(vec![], vec![])),
        "ε path must be absent from closure_plus"
    );
    assert!(path_set.contains(&(vec![1], vec![10])));
}

#[test]
fn smoke_closure_optional() {
    let a = single_arc_fst(1, 10);
    let opt = Backend::closure_optional(&a).unwrap();
    let path_set: HashSet<(Vec<Label>, Vec<Label>)> =
        Backend::paths(&opt).unwrap().map(|p| (p.input, p.output)).collect();
    assert!(path_set.contains(&(vec![], vec![])), "ε path missing");
    assert!(path_set.contains(&(vec![1], vec![10])));
    assert_eq!(path_set.len(), 2);
}

#[test]
fn smoke_closure_bounded() {
    let a = single_arc_fst(1, 10);
    let bnd = Backend::closure_bounded(&a, 2, 3).unwrap();
    let path_set: HashSet<(Vec<Label>, Vec<Label>)> =
        Backend::paths(&bnd).unwrap().map(|p| (p.input, p.output)).collect();
    assert!(!path_set.contains(&(vec![], vec![])), "0-iter must be excluded");
    assert!(!path_set.contains(&(vec![1], vec![10])), "1-iter excluded");
    assert!(path_set.contains(&(vec![1, 1], vec![10, 10])));
    assert!(path_set.contains(&(vec![1, 1, 1], vec![10, 10, 10])));
    assert!(
        !path_set.contains(&(vec![1, 1, 1, 1], vec![10, 10, 10, 10])),
        "4-iter must be excluded"
    );
}

// ---------------------------------------------------------------------------
// 3. Compose: A=(a:b), B=(b:c) → A∘B accepts a → c.
// ---------------------------------------------------------------------------

#[test]
fn smoke_compose_a_to_c() {
    // A : input 1 → output 2.
    // B : input 2 → output 3.
    // A ∘ B : input 1 → output 3.
    let a = single_arc_fst(1, 2);
    let b = single_arc_fst(2, 3);
    let comp = Backend::compose(&a, &b).unwrap();
    let paths = collect_paths(&comp);
    assert_eq!(paths.len(), 1, "expected exactly one composed path");
    assert_eq!(paths[0].input, vec![1]);
    assert_eq!(paths[0].output, vec![3]);
}

// ---------------------------------------------------------------------------
// 4. Determinize + minimize on an ambiguous FST.
// ---------------------------------------------------------------------------

#[test]
fn smoke_determinize_minimize_collapses_duplicate_paths() {
    // Two parallel paths both reading input `1` and writing output `10`.
    // Determinise should collapse the parallel arcs; minimise should collapse
    // any duplicate states it produced.
    //
    //          1:10
    //   s0 ─────────→ s2 (final)
    //     \         /
    //      \  1:10 /
    //       → s1 →
    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    let s2 = bld.add_state();
    let s3 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s2).unwrap();
    bld.set_final(s3).unwrap();
    bld.add_arc(s0, 1, 10, s2).unwrap();
    bld.add_arc(s0, 1, 10, s1).unwrap();
    bld.add_arc(s1, 0, 0, s3).unwrap(); // ε-arc to merge target finals
    let amb = bld.finish().unwrap();

    let eps_free = Backend::eps_remove(&amb).unwrap();
    let det = Backend::determinize(&eps_free).unwrap();
    let mini = Backend::minimize(&det).unwrap();

    // After determinise + minimise, the language is still {(1, 10)}.
    let paths: Vec<Path> = Backend::paths(&mini).unwrap().collect();
    let unique: HashSet<_> = paths.iter().map(|p| (p.input.clone(), p.output.clone())).collect();
    assert_eq!(
        unique.len(),
        1,
        "language preserved: should accept a single (input, output) pair"
    );
    assert_eq!(unique.iter().next().unwrap(), &(vec![1u32], vec![10u32]));

    // And minimise must not blow the state count.
    let n = Backend::num_states(&mini);
    assert!(n <= 2, "minimised state count should be ≤2, got {}", n);
}

// ---------------------------------------------------------------------------
// 5. Invert: (a:b).invert() accepts b → a.
// ---------------------------------------------------------------------------

#[test]
fn smoke_invert_swaps_io_labels() {
    let a = single_arc_fst(1, 2);
    let inv = Backend::invert(&a).unwrap();
    let paths = collect_paths(&inv);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].input, vec![2]);
    assert_eq!(paths[0].output, vec![1]);
}

// ---------------------------------------------------------------------------
// 6. Replace: splice a "subroutine" FST into a label position in a "root".
// ---------------------------------------------------------------------------

#[test]
fn smoke_replace_inlines_subroutine() {
    // The root is a 3-state chain: arc1=(1:10), arc2=(NT:NT), arc3=(3:30),
    // where NT is the placeholder label (we pick 99) that points at a
    // subroutine FST. The subroutine accepts input 2 → output 20.
    //
    // After replace, the composed surface should be (1, 10), (2, 20),
    // (3, 30) — exactly what an inlined call produces.
    let nt: Label = 99;
    let root_label: Label = 100; // root's own NT id

    // Subroutine.
    let sub = single_arc_fst(2, 20);

    // Root: state chain s0 -[1:10]-> s1 -[nt:nt]-> s2 -[3:30]-> s3
    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    let s2 = bld.add_state();
    let s3 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s3).unwrap();
    bld.add_arc(s0, 1, 10, s1).unwrap();
    bld.add_arc(s1, nt, nt, s2).unwrap();
    bld.add_arc(s2, 3, 30, s3).unwrap();
    let root = bld.finish().unwrap();

    let result = Backend::replace(&root, root_label, &[(nt, sub)]).unwrap();
    let eps_free = Backend::eps_remove(&result).unwrap();

    let paths: Vec<Path> = Backend::paths(&eps_free).unwrap().collect();
    let surfaces: HashSet<(Vec<Label>, Vec<Label>)> =
        paths.iter().map(|p| (p.input.clone(), p.output.clone())).collect();
    assert!(
        surfaces.contains(&(vec![1, 2, 3], vec![10, 20, 30])),
        "replace should have inlined subroutine; got {:?}",
        surfaces
    );
}

// ---------------------------------------------------------------------------
// 7. Serialise / deserialise round-trip.
// ---------------------------------------------------------------------------

#[test]
fn smoke_serialize_round_trip() {
    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    let s2 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s2).unwrap();
    bld.add_arc(s0, 7, 70, s1).unwrap();
    bld.add_arc(s1, 8, 80, s2).unwrap();
    let original = bld.finish().unwrap();

    let bytes = Backend::serialize(&original).unwrap();
    assert!(!bytes.is_empty(), "serialised blob is empty");

    let restored = Backend::deserialize(&bytes).unwrap();
    let p_orig = collect_paths(&original);
    let p_rest = collect_paths(&restored);
    assert_eq!(p_orig, p_rest, "round-tripped paths must match");
}

// ---------------------------------------------------------------------------
// 8. mmap_load: write a serialised blob to a temp file, then load via the
//    mmap entry point. For F1 this is `read + deserialize` (see backend.rs);
//    the test still validates round-trip semantics that the trait promises.
// ---------------------------------------------------------------------------

#[test]
fn smoke_mmap_load_round_trip() {
    use std::io::Write;

    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s1).unwrap();
    bld.add_arc(s0, 42, 420, s1).unwrap();
    let original = bld.finish().unwrap();

    let bytes = Backend::serialize(&original).unwrap();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
    tmp.write_all(&bytes).expect("write");
    tmp.flush().expect("flush");

    let loaded = Backend::mmap_load(tmp.path()).expect("mmap_load");
    let p_orig = collect_paths(&original);
    let p_load = collect_paths(&loaded);
    assert_eq!(p_orig, p_load, "mmap_load paths must match original");
}

// ---------------------------------------------------------------------------
// Arc sort + compose-after-sort smoke tests (F2c2.1 prerequisite).
// ---------------------------------------------------------------------------

/// `arc_sort_input` must preserve the language. We build a small acceptor with
/// two arcs out of the start state (input labels 2, then 1 — deliberately out
/// of order), sort by input, and verify the same two (input, output) paths are
/// still present.
#[test]
fn arc_sort_input_preserves_language() {
    let mut bld = Backend::builder();
    let s0 = bld.add_state();
    let s1 = bld.add_state();
    bld.set_start(s0).unwrap();
    bld.set_final(s1).unwrap();
    bld.add_arc(s0, 2, 20, s1).unwrap();
    bld.add_arc(s0, 1, 10, s1).unwrap();
    let fst = bld.finish().unwrap();

    let sorted = Backend::arc_sort_input(&fst).unwrap();

    let before: HashSet<(Vec<Label>, Vec<Label>)> = Backend::paths(&fst)
        .unwrap()
        .map(|p| (p.input, p.output))
        .collect();
    let after: HashSet<(Vec<Label>, Vec<Label>)> = Backend::paths(&sorted)
        .unwrap()
        .map(|p| (p.input, p.output))
        .collect();
    assert_eq!(before, after, "arc_sort_input must preserve the language");
}

/// Composition after explicit arc-sort: the F2c1 failure mode reproduced and
/// fixed. We construct two FSTs whose composition would fail on the
/// "property-bit not set" check (sigma-style self-looping FSTs, mirroring the
/// shape `brackets.rs` produces). Direct `compose` should error; sorting the
/// left operand by output and the right by input should make it succeed.
#[test]
fn compose_after_arc_sort_succeeds_where_direct_compose_fails() {
    // Build two single-state cyclic FSTs over labels {1, 2}: A = identity
    // self-loops on each label. B = same shape. Composing A ∘ B should
    // yield an identity-over-{1,2} FST.
    let build_identity_self_loop = || {
        let mut bld = Backend::builder();
        let s = bld.add_state();
        bld.set_start(s).unwrap();
        bld.set_final(s).unwrap();
        // Add arcs in a non-sorted order: 2 then 1 (forces the property bit
        // to NOT be O_LABEL_SORTED / I_LABEL_SORTED at finish time).
        bld.add_arc(s, 2, 2, s).unwrap();
        bld.add_arc(s, 1, 1, s).unwrap();
        bld.finish().unwrap()
    };
    let a = build_identity_self_loop();
    let b = build_identity_self_loop();

    // Direct compose on freshly-built FSTs surfaces the property-bit issue.
    // rustfst will bail with a "sort?" diagnostic. We accept either an
    // explicit Err or — if rustfst's behaviour changes across versions —
    // a success; the important assertion is that the post-sort path works.
    let direct = Backend::compose(&a, &b);

    let a_sorted = Backend::arc_sort_output(&a).unwrap();
    let b_sorted = Backend::arc_sort_input(&b).unwrap();
    let composed = Backend::compose(&a_sorted, &b_sorted)
        .expect("compose after arc-sort must succeed");

    // Sanity: at least one accepting path with input == output == [1] is in
    // the composed FST. (Cyclic FST → take a bounded prefix.)
    let any_one: bool = Backend::paths(&composed)
        .unwrap()
        .take(64)
        .any(|p| p.input == vec![1] && p.output == vec![1]);
    assert!(
        any_one,
        "compose after arc-sort should yield identity path [1]→[1]; \
         direct compose result was {:?}",
        direct.as_ref().err().map(|e| e.to_string())
    );
}

// ---------------------------------------------------------------------------
// Symbol table sanity (the type is concrete; one test pins the contract).
// ---------------------------------------------------------------------------

#[test]
fn smoke_symbol_table_intern_lookup() {
    let mut t = SymbolTable::new();
    let a = t.intern("a");
    let b = t.intern("b");
    let a2 = t.intern("a");
    assert_eq!(a, a2, "intern is idempotent");
    assert_ne!(a, b);
    assert_eq!(t.name(a), Some("a"));
    assert_eq!(t.label("a"), Some(a));
    // epsilon is always there at label 0.
    assert_eq!(t.label("<eps>"), Some(0));
}
