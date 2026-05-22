//! Karttunen bracket-machinery FSTs (F2c1).
//!
//! Three small transducers that the Karttunen `@->` replace construction
//! (`docs/proposals/f2-kaplan-kay-plan.md` §3) needs for its **bracket
//! protocol**. F2c1 ships only the bracket pieces; the actual replacement
//! transducer, the obligatory-context constraint, the longest-leftmost
//! filter, and the top-level compose are F2c2..F2c5.
//!
//! ## Why brackets at all
//!
//! Karttunen's construction surrounds every *candidate* rewrite site in
//! the input with a pair of reserved bracket symbols, then runs a
//! transducer that knows "if you see `[+] A ]+` you replace; otherwise
//! pass through". Bracket introduction is **non-deterministic** (every
//! possible bracketing is generated); a downstream constraint stage
//! (F2c3) discards bracketings whose `L _ R` context is wrong; the
//! longest-leftmost filter (F2c4) picks the canonical bracketing per
//! site; and a final strip stage erases the markers so the surface is
//! clean.
//!
//! See the plan §3.1 (`Mark`), §3.4 (`Unmark`), and Beesley & Karttunen
//! 2003 chapter 3 for the readable expansion.
//!
//! ## What this module owns
//!
//! Exactly three FSTs, all single-state, all-final, self-looping:
//!
//!   * [`intro_brackets`] — Karttunen step 1's `Mark`. Identity on Σ;
//!     ε-arcs emit `<[+]>` (`BRACKET_OPEN_OBLIG_LABEL`, label 4) or
//!     `<]+>` (`BRACKET_CLOSE_OBLIG_LABEL`, label 6) at any position.
//!     Does NOT enforce balance / nesting / context — that's F2c3.
//!
//!   * [`strip_brackets`] — Karttunen step 4's `Unmark`. Identity on Σ;
//!     each bracket label is consumed and emitted as ε. Composed at the
//!     end of the chain so the surface comes out free of markers.
//!
//!   * [`identity_outside_brackets`] — the plan's "Σ\* (skipping
//!     brackets)" helper. Identity on Σ AND identity on the two bracket
//!     labels. Lets downstream stages compose a "passthrough" that
//!     scrutinises brackets without choking on them. Distinct from
//!     [`strip_brackets`] in that brackets are preserved (input ==
//!     output) rather than dropped.
//!
//! ## What this module does NOT own
//!
//!   * Replacement transducer (`Replace`, plan §3.3) — F2c2.
//!   * Obligatory constraint (`Constraint`, plan §3.2) — F2c3.
//!   * Longest-leftmost filter — F2c4.
//!   * Top-level composition + the public `compile_rewrite_rule` entry
//!     point — F2c5.
//!
//! ## Construction conventions
//!
//! All three FSTs share the same shape: **one state**, marked start and
//! final, with self-loops only. This is the minimal FST for an
//! always-final identity-style transducer. Composed with any input the
//! result is at most as large as the input (modulo bracket insertions for
//! [`intro_brackets`]); minimisation collapses trivially.
//!
//! Bracket *labels* (4 and 6) are reserved by
//! [`super::super::alphabet::PhonruleAlphabet`]; the symbol-table names
//! are `<[+]>` and `<]+>` respectively (see `alphabet.rs`).

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, EPS_LABEL};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

/// Karttunen step 1 — `Mark`: nondeterministically introduce
/// `<[+]>` / `<]+>` markers anywhere in the input, identity on Σ.
///
/// One state, both start and final. Self-loops:
///
///   * One identity arc `s -[σ:σ]-> s` per σ in Σ. Brackets are NOT in
///     Σ, so the FST never accepts brackets on the input side.
///   * One ε-emit arc `s -[ε:<[+]>]-> s` — introduces an open obligatory
///     bracket at any point, nondeterministically. May fire zero or more
///     times anywhere (a Kleene closure on its own is implicit in the
///     self-loop).
///   * One ε-emit arc `s -[ε:<]+>]-> s` — same, for the closing bracket.
///
/// Composing `intro_brackets` after any FST `F` produces an FST whose
/// outputs are: `F`'s outputs, with arbitrary `<[+]>` / `<]+>` insertions
/// at any position. The "valid bracketings only" filtering is the
/// downstream `Constraint` stage (F2c3); intro is intentionally permissive.
///
/// **Cyclicity warning.** The ε-emit self-arcs make this FST cyclic in a
/// way that yields infinitely many bracketed-output paths for any finite
/// input. Callers enumerating paths via [`super::super::backend::FstBackend::paths`]
/// must use `.take(N)` or compose with a bounded language acceptor first.
pub fn intro_brackets(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    // Identity on each Σ member.
    for label in alpha.sigma() {
        b.add_arc(s, label, label, s).expect("intro: identity arc");
    }
    // Identity on the three reserved stream markers (`<bdy>`, `<^>`,
    // `<$>`). These are NOT Σ members but they can appear in runtime
    // input streams (a `BOUNDARY` char from a multi-morpheme input,
    // word-edge markers when the apply driver wraps the input). Without
    // these arcs, any input containing a stream marker would have no
    // path through the bracket-protocol chain and the apply driver
    // would report `NoOutput`. F2c5.1 surfaced this when validating
    // against `phonrule_eval` on inputs with `\0` morpheme boundaries.
    b.add_arc(s, alpha.boundary_label(), alpha.boundary_label(), s)
        .expect("intro: <bdy> identity arc");
    b.add_arc(
        s,
        alpha.word_start_label(),
        alpha.word_start_label(),
        s,
    )
    .expect("intro: <^> identity arc");
    b.add_arc(s, alpha.word_end_label(), alpha.word_end_label(), s)
        .expect("intro: <$> identity arc");
    // ε:bracket-open and ε:bracket-close. Both are nondeterministic
    // insertions; downstream Constraint discards the wrong placements.
    b.add_arc(s, EPS_LABEL, BRACKET_OPEN_OBLIG_LABEL, s)
        .expect("intro: ε:<[+]>");
    b.add_arc(s, EPS_LABEL, BRACKET_CLOSE_OBLIG_LABEL, s)
        .expect("intro: ε:<]+>");
    b.finish().expect("intro: finish")
}

/// Karttunen step 4 — `Unmark`: remove `<[+]>` / `<]+>` markers from the
/// output, identity on Σ.
///
/// One state, both start and final. Self-loops:
///
///   * One identity arc `s -[σ:σ]-> s` per σ in Σ (the surface symbols
///     pass through unchanged).
///   * One arc `s -[<[+]>:ε]-> s` — consumes an open bracket on input,
///     emits nothing.
///   * One arc `s -[<]+>:ε]-> s` — consumes a close bracket on input,
///     emits nothing.
///
/// Composing `F ∘ strip_brackets` where `F` produces bracket-bearing
/// outputs gives an FST whose outputs are `F`'s outputs minus the
/// brackets. This is the cleanup that runs last in the Karttunen chain.
///
/// Symbols NOT in Σ and not brackets are simply not accepted (no arc):
/// `strip_brackets` is a "drop the brackets and otherwise be identity"
/// transducer over `Σ ∪ {<[+]>, <]+>}`, nothing more.
pub fn strip_brackets(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    // Identity on each Σ member.
    for label in alpha.sigma() {
        b.add_arc(s, label, label, s).expect("strip: identity arc");
    }
    // Identity on the three reserved stream markers; same rationale as
    // [`intro_brackets`]. The apply driver decides which markers to
    // strip on string-decode (typically `<^>` / `<$>` go away;
    // `<bdy>` round-trips as `\0` to match eval).
    b.add_arc(s, alpha.boundary_label(), alpha.boundary_label(), s)
        .expect("strip: <bdy> identity arc");
    b.add_arc(
        s,
        alpha.word_start_label(),
        alpha.word_start_label(),
        s,
    )
    .expect("strip: <^> identity arc");
    b.add_arc(s, alpha.word_end_label(), alpha.word_end_label(), s)
        .expect("strip: <$> identity arc");
    // Bracket inputs are consumed, ε emitted.
    b.add_arc(s, BRACKET_OPEN_OBLIG_LABEL, EPS_LABEL, s)
        .expect("strip: <[+]>:ε");
    b.add_arc(s, BRACKET_CLOSE_OBLIG_LABEL, EPS_LABEL, s)
        .expect("strip: <]+>:ε");
    b.finish().expect("strip: finish")
}

/// "Σ\* skipping brackets" — identity on Σ AND identity on the two
/// bracket labels.
///
/// One state, both start and final. Self-loops:
///
///   * One identity arc `s -[σ:σ]-> s` per σ in Σ.
///   * One identity arc `s -[<[+]>:<[+]>]-> s`.
///   * One identity arc `s -[<]+>:<]+>]-> s`.
///
/// This is the plan §3 "Σ\* (skipping brackets)" passthrough. It looks
/// like [`strip_brackets`] but **keeps** the brackets on the output side;
/// downstream stages (`Constraint` in F2c3, `Replace` in F2c2) want to
/// see brackets to act on them, not have them erased.
///
/// The naming convention is Karttunen's (Beesley & Karttunen 2003
/// chapter 3): "outside brackets" means "in the region between bracket
/// pairs we leave the symbol alone, in the same way we leave Σ alone
/// elsewhere". Brackets themselves are part of that 'identity passthrough'
/// — the function does not look at whether we're inside or outside any
/// bracket pair (no state machinery to track that). It is the brace stage
/// the rest of the construction composes around.
pub fn identity_outside_brackets(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    // Identity on each Σ member.
    for label in alpha.sigma() {
        b.add_arc(s, label, label, s)
            .expect("identity-outside: σ self-arc");
    }
    // Identity on the three reserved stream markers; same rationale as
    // [`intro_brackets`] / [`strip_brackets`].
    b.add_arc(s, alpha.boundary_label(), alpha.boundary_label(), s)
        .expect("identity-outside: <bdy> self-arc");
    b.add_arc(
        s,
        alpha.word_start_label(),
        alpha.word_start_label(),
        s,
    )
    .expect("identity-outside: <^> self-arc");
    b.add_arc(s, alpha.word_end_label(), alpha.word_end_label(), s)
        .expect("identity-outside: <$> self-arc");
    // Identity on each bracket label.
    b.add_arc(
        s,
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_OPEN_OBLIG_LABEL,
        s,
    )
    .expect("identity-outside: <[+]> self-arc");
    b.add_arc(
        s,
        BRACKET_CLOSE_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
        s,
    )
    .expect("identity-outside: <]+> self-arc");
    b.finish().expect("identity-outside: finish")
}
