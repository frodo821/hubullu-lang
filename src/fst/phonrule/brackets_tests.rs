//! Tests for the F2c1 bracket-machinery FSTs.
//!
//! Per the F2c1 brief: round-trip / property tests on
//! [`intro_brackets`], [`strip_brackets`], and
//! [`identity_outside_brackets`] in isolation. Full validation against
//! `phonrule_eval` is F2c5 territory; here we just verify each FST's
//! behaviour against direct expectation.
//!
//! ## Test fixture — Σ = {a, b, c}
//!
//! Every test interns "a", "b", "c" as Σ members. The brackets occupy
//! reserved labels 4 (`<[+]>`) and 6 (`<]+>`); these are NOT in Σ.
//!
//! ## Path-enumeration strategy on cyclic FSTs
//!
//! [`intro_brackets`] has ε-emit self-loops for both bracket labels,
//! producing **infinitely many accepting paths** for any finite input.
//! Naïve `paths(...).collect()` would hang. We use two strategies:
//!
//!   * **Compose-then-enumerate with a bounded take**. For a finite
//!     input acceptor `I`, `compose(I, intro_brackets)` still produces
//!     an infinite path set (ε-loops survive composition), so we
//!     `.take(N)` and check the desired output is present in the prefix.
//!     `N` is chosen large enough to cover all bracketings up to a few
//!     marker insertions on a 3-symbol input.
//!
//!   * **Forward simulation**. For [`strip_brackets`] and
//!     [`identity_outside_brackets`], which have NO ε-emit arcs, paths
//!     are bounded by the input length — composing with a finite input
//!     acceptor yields a finite output FST and we can enumerate exhaustively.
//!
//! Brief tradeoff: the test for "intro produces identity path" is
//! satisfied by composing a 3-symbol input acceptor and looking for the
//! original 3-symbol output in the first N paths. The cyclic-FST warning
//! from F1's `paths()` doc applies — we document the `.take(N)` choice
//! locally and never call `.collect()` on a cyclic iterator.

use std::collections::HashSet;

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, Label, Path};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::brackets::{identity_outside_brackets, intro_brackets, strip_brackets};

// ---------------------------------------------------------------------------
// Fixture builders.
// ---------------------------------------------------------------------------

/// Build the standard test alphabet with Σ = {a, b, c}.
fn fixture_alpha() -> (PhonruleAlphabet, Label, Label, Label) {
    let mut alpha = PhonruleAlphabet::empty();
    let a = alpha.intern("a");
    let b = alpha.intern("b");
    let c = alpha.intern("c");
    (alpha, a, b, c)
}

/// Build a linear input acceptor that accepts exactly the sequence
/// `labels`, identity input=output.
///
/// (N+1)-state chain, used to bound enumerations: composing this with a
/// cyclic FST anchors the input side to a single string while leaving
/// the output side free.
fn linear_input_acceptor(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let mut prev = b.add_state();
    b.set_start(prev).expect("linear: set_start");
    for &l in labels {
        let next = b.add_state();
        b.add_arc(prev, l, l, next).expect("linear: add_arc");
        prev = next;
    }
    b.set_final(prev).expect("linear: set_final");
    b.finish().expect("linear: finish")
}

/// Bounded enumeration cap for cyclic FSTs.
///
/// `intro_brackets` composed with a 3-symbol input still has ε-emit
/// self-loops, so `paths()` is infinite. We `.take(BOUNDED_PATHS)` and
/// look for desired outputs in the prefix. 4096 is generous — the BFS
/// path-iterator enumerates short paths first, so the bare-input
/// identity path and any near-zero-bracket-insertion path appear well
/// inside this cap.
const BOUNDED_PATHS: usize = 4096;

/// Collect up to `BOUNDED_PATHS` paths from a possibly-cyclic FST. Always
/// safe to call (never panics, never hangs).
fn bounded_paths(fst: &RustFstWrapper) -> Vec<Path> {
    RustFstBackend::paths(fst)
        .expect("paths iter")
        .take(BOUNDED_PATHS)
        .collect()
}

/// Whether any path in `paths` has the given output label sequence.
fn output_seq_present(paths: &[Path], expected: &[Label]) -> bool {
    paths.iter().any(|p| p.output.as_slice() == expected)
}

/// Whether any path in `paths` has the given input AND output label
/// sequences.
fn io_pair_present(paths: &[Path], input: &[Label], output: &[Label]) -> bool {
    paths
        .iter()
        .any(|p| p.input.as_slice() == input && p.output.as_slice() == output)
}

// ---------------------------------------------------------------------------
// 1. intro_brackets — identity path is preserved.
// ---------------------------------------------------------------------------

#[test]
fn intro_brackets_preserves_identity_path() {
    // Composing intro_brackets with a fixed input "abc" should produce,
    // among many bracketed outputs, the bare "abc" (zero bracket
    // insertions). This is the "identity passthrough is there" test.
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &intro).expect("compose");

    let paths = bounded_paths(&composed);
    assert!(
        output_seq_present(&paths, &[a, b, c]),
        "intro_brackets should yield bare identity output [a,b,c] among its paths"
    );
}

// ---------------------------------------------------------------------------
// 2. intro_brackets — nondeterministically inserts brackets.
// ---------------------------------------------------------------------------

#[test]
fn intro_brackets_can_insert_open_at_start() {
    // Output "<[+]>abc" should be among the paths: one ε-emit of <[+]>
    // before the three identity arcs.
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &intro).expect("compose");
    let paths = bounded_paths(&composed);

    let want = [BRACKET_OPEN_OBLIG_LABEL, a, b, c];
    assert!(
        output_seq_present(&paths, &want),
        "intro_brackets should produce output '<[+]>abc' among its paths"
    );
}

#[test]
fn intro_brackets_can_insert_close_at_end() {
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &intro).expect("compose");
    let paths = bounded_paths(&composed);

    let want = [a, b, c, BRACKET_CLOSE_OBLIG_LABEL];
    assert!(
        output_seq_present(&paths, &want),
        "intro_brackets should produce output 'abc<]+>' among its paths"
    );
}

#[test]
fn intro_brackets_can_wrap_input_in_brackets() {
    // "<[+]>abc<]+>": one open at front, one close at end.
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &intro).expect("compose");
    let paths = bounded_paths(&composed);

    let want = [
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        b,
        c,
        BRACKET_CLOSE_OBLIG_LABEL,
    ];
    assert!(
        output_seq_present(&paths, &want),
        "intro_brackets should produce output '<[+]>abc<]+>' among its paths"
    );
}

#[test]
fn intro_brackets_can_insert_pair_inside_input() {
    // "a<[+]><]+>bc": an open and close bracket between 'a' and 'b'.
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &intro).expect("compose");
    let paths = bounded_paths(&composed);

    let want = [
        a,
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
        b,
        c,
    ];
    assert!(
        output_seq_present(&paths, &want),
        "intro_brackets should produce output 'a<[+]><]+>bc' among its paths"
    );
}

// ---------------------------------------------------------------------------
// 3. strip_brackets — removes both bracket types.
// ---------------------------------------------------------------------------

#[test]
fn strip_brackets_drops_open_and_close_markers() {
    // Input "<[+]>abc<]+>" → output "abc".
    let (alpha, a, b, c) = fixture_alpha();
    let strip = strip_brackets(&alpha);
    // Build an input acceptor for the bracketed sequence. strip_brackets
    // accepts brackets on its input side; this is a finite acyclic
    // composition so enumeration is bounded.
    let input = linear_input_acceptor(&[
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        b,
        c,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let composed = RustFstBackend::compose(&input, &strip).expect("compose");
    let paths = bounded_paths(&composed);

    // Exactly one accepting path; output is the bracket-free "abc".
    assert_eq!(paths.len(), 1, "strip on linear input should yield 1 path");
    assert_eq!(paths[0].output.as_slice(), &[a, b, c]);
}

#[test]
fn strip_brackets_is_identity_on_plain_sigma() {
    // Input "abc" (no brackets) → output "abc" (identity).
    let (alpha, a, b, c) = fixture_alpha();
    let strip = strip_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &strip).expect("compose");
    let paths = bounded_paths(&composed);

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].input.as_slice(), &[a, b, c]);
    assert_eq!(paths[0].output.as_slice(), &[a, b, c]);
}

#[test]
fn strip_brackets_handles_only_brackets() {
    // Input "<[+]><]+>" → output "" (everything stripped).
    let (alpha, _a, _b, _c) = fixture_alpha();
    let strip = strip_brackets(&alpha);
    let input = linear_input_acceptor(&[
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
    ]);
    let composed = RustFstBackend::compose(&input, &strip).expect("compose");
    let paths = bounded_paths(&composed);

    assert_eq!(paths.len(), 1);
    let empty: &[Label] = &[];
    assert_eq!(paths[0].output.as_slice(), empty);
}

// ---------------------------------------------------------------------------
// 4. Round-trip: intro ∘ strip on bracket-free input.
// ---------------------------------------------------------------------------

#[test]
fn intro_then_strip_preserves_identity_path() {
    // Property: for any input x, `intro_brackets ∘ strip_brackets`
    // applied to x must include `x` among its accepting outputs (the
    // identity-path round-trip). intro nondeterministically adds
    // brackets; strip nondeterministically (well: deterministically per
    // input position) removes them; the join contains the no-op path.
    //
    // Implementation note: we verify the **semantic** round-trip without
    // composing the two cyclic self-loop FSTs directly. rustfst's
    // `compose` checks arc-sort property bits and rejects pairs where
    // neither side is explicitly sorted (error: "1st argument cannot
    // match on output labels and 2nd argument cannot match on input
    // labels (sort?)"). Adding arc-sort to [`FstBackend`] is outside
    // F2c1 scope; the production rule-compile chain in F2c5 will do
    // that and exercise the actual compose. For F2c1 we verify the
    // mathematical relation by:
    //
    //   1. Composing `input ∘ intro` (one-side-bounded, this compose
    //      always works) to get the bracketed outputs of intro on a
    //      specific finite input.
    //   2. Simulating strip's semantics on each bracketed output path
    //      (drop labels 4 and 6) and asserting the original input is
    //      among the resulting strip-outputs.
    //
    // This proves "for some bracketing path, strip recovers identity",
    // which is the round-trip relation the F2c5 compose will rely on.
    let (alpha, a, b, c) = fixture_alpha();
    let intro = intro_brackets(&alpha);

    let input = linear_input_acceptor(&[a, b, c]);
    let bracketed = RustFstBackend::compose(&input, &intro).expect("input ∘ intro");
    let paths = bounded_paths(&bracketed);

    let bracket_set: HashSet<Label> = [BRACKET_OPEN_OBLIG_LABEL, BRACKET_CLOSE_OBLIG_LABEL]
        .iter()
        .copied()
        .collect();

    // Strip semantics: drop bracket labels from output, keep others.
    let strip_path_output = |p: &Path| -> Vec<Label> {
        p.output
            .iter()
            .copied()
            .filter(|l| !bracket_set.contains(l))
            .collect()
    };

    let recovers_identity = paths
        .iter()
        .any(|p| strip_path_output(p).as_slice() == &[a, b, c]);

    assert!(
        recovers_identity,
        "intro then strip must recover the bare input [a,b,c]; \
         saw {} bracketed paths but none stripped to identity",
        paths.len()
    );

    // Also: every bracketed output should strip to *some* substring of
    // the original (well, exactly [a,b,c] since intro is identity on Σ
    // and only inserts brackets — it cannot remove or substitute Σ
    // symbols). This is a stronger sanity check.
    for p in &paths {
        let stripped = strip_path_output(p);
        assert_eq!(
            stripped.as_slice(),
            &[a, b, c],
            "intro preserves Σ symbols verbatim — every bracketed output \
             must strip back to the input; got {:?} from path {:?}",
            stripped,
            p
        );
    }
}

// ---------------------------------------------------------------------------
// 5. identity_outside_brackets — passes brackets through unchanged.
// ---------------------------------------------------------------------------

#[test]
fn identity_outside_brackets_preserves_brackets() {
    // Input "<[+]>abc<]+>" → output "<[+]>abc<]+>" (full identity).
    let (alpha, a, b, c) = fixture_alpha();
    let id = identity_outside_brackets(&alpha);
    let seq = [
        BRACKET_OPEN_OBLIG_LABEL,
        a,
        b,
        c,
        BRACKET_CLOSE_OBLIG_LABEL,
    ];
    let input = linear_input_acceptor(&seq);
    let composed = RustFstBackend::compose(&input, &id).expect("compose");
    let paths = bounded_paths(&composed);

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].input.as_slice(), &seq);
    assert_eq!(paths[0].output.as_slice(), &seq);
}

#[test]
fn identity_outside_brackets_is_identity_on_plain_sigma() {
    // Input "abc" → output "abc".
    let (alpha, a, b, c) = fixture_alpha();
    let id = identity_outside_brackets(&alpha);
    let input = linear_input_acceptor(&[a, b, c]);
    let composed = RustFstBackend::compose(&input, &id).expect("compose");
    let paths = bounded_paths(&composed);

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].input.as_slice(), &[a, b, c]);
    assert_eq!(paths[0].output.as_slice(), &[a, b, c]);
}

// ---------------------------------------------------------------------------
// 6. Structural sanity — each FST has the expected single-state shape.
// ---------------------------------------------------------------------------

#[test]
fn all_three_fsts_are_single_state() {
    let (alpha, _a, _b, _c) = fixture_alpha();
    assert_eq!(RustFstBackend::num_states(&intro_brackets(&alpha)), 1);
    assert_eq!(RustFstBackend::num_states(&strip_brackets(&alpha)), 1);
    assert_eq!(
        RustFstBackend::num_states(&identity_outside_brackets(&alpha)),
        1
    );
}
