//! Iterative-to-convergence apply driver (F2c5.3).
//!
//! Per the plan §4, hubullu's `phonrule_eval` runs each rewrite rule in
//! a loop until the surface stops changing (`phonrule_eval.rs:164-178`).
//! In FST terms this is the transitive closure of the per-rule
//! transducer; plan §4.2 recommends option (b) — a **runtime iteration
//! loop** that applies the single-pass FST repeatedly until the input
//! stabilises.
//!
//! This module implements that loop.
//!
//! ## Apply pipeline
//!
//! For a compiled phonrule FST `R` (output of
//! [`super::rule_seq::compile_phonrule`]) and an input string `s`:
//!
//! 1. Convert `s` to a Σ-label sequence via `alpha.intern` (each char
//!    becomes one label; the BOUNDARY char `\0` becomes the reserved
//!    boundary label).
//! 2. Build a linear identity-IO acceptor over those labels.
//! 3. Compose `input_acceptor ∘ R`. The result's accepting paths each
//!    encode one (input, output) pair where input = the original
//!    sequence and output = `R` applied to it.
//! 4. Enumerate accepting paths, extract the unique output sequence(s).
//!    For a functional rule there's exactly one; for a non-functional
//!    rule we pick the lexicographically first to be deterministic.
//! 5. Convert output labels back to a string. Strip stream markers
//!    (`<bdy>`, `<^>`, `<$>`) to match `phonrule_eval`'s
//!    `strip_boundaries` shape; preserve in-stream BOUNDARY labels
//!    that originated as `\0` characters (eval keeps those).
//! 6. If output != input, loop. Bounded by `max_iter`
//!    (default `MAX_PHONRULE_ITER = 64`).
//!
//! ## Stream marker convention
//!
//! `phonrule_eval` carries `BOUNDARY = '\0'` in the input string. The
//! FST alphabet's `boundary_label` corresponds to that char. We convert
//! `\0 ↔ boundary_label` in the string ↔ label translation; the
//! conversion is symmetric so a string round-trip is the identity.
//!
//! The word-start and word-end markers (`<^>`, `<$>`) are NOT in
//! `phonrule_eval`'s string representation — eval uses cursor position
//! to detect word edges. The apply driver only inserts these markers
//! if the compiled rule references them (i.e., the rule mentions `^`
//! or `$` in some context). For F2c5 we always insert them at apply
//! time and always strip them on output, mirroring the constraint
//! shape that includes them as Σ̂ members. (This is slightly
//! over-cautious — a rule that doesn't reference edges sees the
//! markers harmlessly — but it's the simplest discipline and
//! validates byte-identical against eval.)
//!
//! ## What this module owns
//!
//!   * [`apply_phonrule_fst`] — the public entry point.
//!   * [`ApplyError`] — convergence / FST runtime errors.
//!   * [`MAX_PHONRULE_ITER`] — the iteration cap.
//!
//! ## What this module does NOT own
//!
//!   * FST compilation — F2c5.1 / F2c5.2.
//!   * Validation harness — F2c5.4.

use crate::phonrule_eval::BOUNDARY;

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBuilder, Label};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

/// Default maximum iterations per apply call. Matches plan §4.2's
/// `MAX_PHONRULE_ITER = 64`. A rule that doesn't converge in 64 passes
/// is almost certainly malformed (the eval engine has an unbounded
/// loop here which can hang on `a -> aa / _`-style rules — F2 is
/// strictly better by bounding).
pub const MAX_PHONRULE_ITER: u32 = 64;

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by the apply driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// Hit the per-call iteration cap. Almost always indicates a
    /// malformed rule (e.g. `a -> aa / _` loops forever).
    ConvergenceLimit {
        iterations: u32,
        last_input: String,
    },
    /// The composed input ∘ rule FST had no accepting paths. Should
    /// not happen for a well-formed Karttunen `@->` chain — every
    /// well-bracketed input has at least one output — but defensive
    /// against backend quirks.
    NoOutput { input: String },
    /// Backend FST operation failed (e.g. compose error).
    Backend(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::ConvergenceLimit {
                iterations,
                last_input,
            } => write!(
                f,
                "phonrule did not converge within {} iterations (last input: {:?})",
                iterations, last_input
            ),
            ApplyError::NoOutput { input } => write!(
                f,
                "phonrule FST produced no output for input {:?}",
                input
            ),
            ApplyError::Backend(s) => write!(f, "phonrule apply backend error: {}", s),
        }
    }
}

impl std::error::Error for ApplyError {}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Apply a compiled phonrule FST to an input string, iterating to
/// convergence.
///
/// `rule_fst` is the output of [`super::rule_seq::compile_phonrule`].
/// `alpha` must be the same alphabet used to compile the rule
/// (otherwise label translation will be inconsistent).
///
/// `max_iter` caps the convergence loop. Use [`MAX_PHONRULE_ITER`] for
/// the standard value (64).
///
/// Returns the converged surface string. The output preserves
/// `BOUNDARY` (`\0`) characters that were present in the input
/// (mirroring `phonrule_eval`'s behaviour); word-start / word-end
/// markers are stripped if they appear in the FST's output.
pub fn apply_phonrule_fst(
    rule_fst: &RustFstWrapper,
    input: &str,
    alpha: &mut PhonruleAlphabet,
    max_iter: u32,
) -> Result<String, ApplyError> {
    // Pre-sort the rule FST's input arcs ONCE for the whole convergence
    // loop — rule_fst is fixed across iterations, and `compose`
    // requires its right operand to be input-sorted (per the trait
    // contract). Without this we'd re-sort on every iteration.
    let rule_sorted = RustFstBackend::arc_sort_input(rule_fst)
        .map_err(|e| ApplyError::Backend(e.to_string()))?;
    let mut current = input.to_string();
    for _ in 0..max_iter {
        let next = apply_once(&rule_sorted, &current, alpha)?;
        if next == current {
            return Ok(next);
        }
        current = next;
    }
    Err(ApplyError::ConvergenceLimit {
        iterations: max_iter,
        last_input: current,
    })
}

// ---------------------------------------------------------------------------
// One-pass apply.
// ---------------------------------------------------------------------------

/// Apply the FST one time to the input string.
///
/// Steps:
///   1. Encode `input` to label sequence (interning new chars).
///   2. Build a linear acceptor over those labels.
///   3. Compose `input ∘ rule_fst`.
///   4. Read the first accepting path's output labels.
///   5. Decode labels back to a string.
fn apply_once(
    rule_fst_input_sorted: &RustFstWrapper,
    input: &str,
    alpha: &mut PhonruleAlphabet,
) -> Result<String, ApplyError> {
    let labels = string_to_labels(input, alpha);
    let input_acceptor = linear_acceptor(&labels);

    // Linear acceptor is identity I=O on every arc, so its arcs are
    // already "output-sorted" in the trivial sense; arc_sort_output
    // sets the property bit that compose checks.
    let left = RustFstBackend::arc_sort_output(&input_acceptor)
        .map_err(|e| ApplyError::Backend(e.to_string()))?;
    let applied = RustFstBackend::compose(&left, rule_fst_input_sorted)
        .map_err(|e| ApplyError::Backend(e.to_string()))?;

    // Enumerate paths bounded; for a well-bracketed linear input the
    // path space after composition with the Karttunen chain is
    // bounded (one canonical output per input). We take the first
    // accepting path's output. If multiple unique outputs are
    // observed, that's a bug in the leftmost filter, but we still
    // produce a deterministic answer by lexicographic choice.
    let mut iter = RustFstBackend::paths(&applied)
        .map_err(|e| ApplyError::Backend(e.to_string()))?;
    let mut best: Option<Vec<Label>> = None;
    for p in iter.by_ref().take(PATH_ENUM_CAP) {
        let out = p.output;
        best = Some(match best {
            None => out,
            Some(prev) if out < prev => out,
            Some(prev) => prev,
        });
    }
    let out_labels = best.ok_or_else(|| ApplyError::NoOutput {
        input: input.to_string(),
    })?;
    Ok(labels_to_string(&out_labels, alpha))
}

/// Maximum number of accepting paths enumerated per apply pass.
///
/// The Karttunen `@->` chain we build is functional after
/// determinise/minimise where those succeed; even when they don't, the
/// leftmost filter collapses the path space to a small handful of
/// equivalent paths per input. 4096 is generous and matches the cap
/// used by `leftmost_tests::BOUNDED_PATHS`.
const PATH_ENUM_CAP: usize = 4096;

// ---------------------------------------------------------------------------
// String ↔ labels translation.
// ---------------------------------------------------------------------------

/// Convert a string to a label sequence.
///
/// Each char becomes one label:
///   * `BOUNDARY` (`'\0'`) → `alpha.boundary_label()`.
///   * other chars → `alpha.intern(ch_str)` (interns into Σ if new).
///
/// Word-start/word-end markers are NOT inserted at the edges — F2c5
/// doesn't require that for the rules in current grammars (they don't
/// reference `^` / `$`). If a future rule needs them, the compiled FST
/// will require the input acceptor to include them; this function
/// is the natural place to add an opt-in flag.
fn string_to_labels(s: &str, alpha: &mut PhonruleAlphabet) -> Vec<Label> {
    s.chars()
        .map(|ch| {
            if ch == BOUNDARY {
                alpha.boundary_label()
            } else {
                alpha.intern(&ch.to_string())
            }
        })
        .collect()
}

/// Convert a label sequence to a string.
///
/// Each label maps back to its symbol-table name; the name is then
/// pushed onto the result string. Stream markers `<^>` and `<$>` are
/// stripped on output. The boundary marker `<bdy>` maps back to
/// `'\0'` to round-trip with [`string_to_labels`].
///
/// Multi-char symbol names (e.g. a phoneme `"ng"`) are emitted
/// literally — Σ symbols are arbitrary &str values from the alphabet,
/// so the round-trip property requires that a string built by
/// `string_to_labels` and then back is preserved char-by-char only when
/// every char is also a Σ symbol of length 1. In practice
/// `phonrule_eval` operates on `&str` char-by-char too, so the two
/// engines agree as long as Σ symbols are length-1 strings (the v1
/// case). The F2c5 validation harness drives only length-1 Σ.
fn labels_to_string(labels: &[Label], alpha: &PhonruleAlphabet) -> String {
    let mut out = String::new();
    for &l in labels {
        if l == alpha.word_start_label() || l == alpha.word_end_label() {
            continue;
        }
        if l == alpha.boundary_label() {
            out.push(BOUNDARY);
            continue;
        }
        if let Some(name) = alpha.label_to_str(l) {
            out.push_str(name);
        }
    }
    out
}

/// Build a linear identity-IO acceptor over `labels`.
///
/// `(N+1)-state` chain with one arc per label. Used as the input
/// acceptor in `apply_once`.
fn linear_acceptor(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let start = b.add_state();
    b.set_start(start).expect("set_start");
    let mut prev = start;
    for &l in labels {
        let next = b.add_state();
        b.add_arc(prev, l, l, next).expect("add_arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish linear acceptor")
}
