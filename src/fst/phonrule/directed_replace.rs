//! F2c4 Strategy A — **correct** Karttunen 1996 directed replacement (`@->`).
//!
//! This is the production successor to the `#[cfg(test)]` spike in
//! [`super::strategy_a_spike`]. The spike proved the *perf* claim (the
//! 4-tape / marker construction compiles in sub-millisecond time even on
//! the `!class*` rule shape that makes Strategy B's `constraint.rs`
//! complement explode), but it was deliberately **incorrect**: its caret
//! insertion was *permissive*.
//!
//! ## What this revision (task F2c4-#3) changes
//!
//! The earlier milestone folded the context into UPPER
//! (`UPPER = L · LHS · R`), bracketed the WHOLE thing, and the replace
//! stage **consumed** `L·LHS·R`. That is WRONG for real grammars because an
//! `L`/`R` symbol consumed by one match cannot license an adjacent or
//! overlapping match in the same pass — exactly the situation vowel harmony
//! produces (one back vowel licenses a whole run of targets).
//!
//! This revision implements the real `@->` shape:
//!
//!   * **Bracket only the LHS** (`UPPER = LHS`, the bracketed region is
//!     `< LHS >`).
//!   * **Context is a non-consuming licensing condition** (`Constraints(L, R)`,
//!     Karttunen 1996 §3, plan §2.3 Path B): every bracketed `< LHS >` must
//!     have an `L`-match immediately to its left and an `R`-match immediately
//!     to its right *in the surrounding Σ_b content*, **without** those L/R
//!     symbols being consumed by the bracket. One context symbol can therefore
//!     license arbitrarily many targets.
//!
//! ### The non-consuming licensing construction (regex form)
//!
//! Let `Σ_M = Σ_b ∪ {<, >}` be the bracketed alphabet at the licensing stage
//! (the caret has already been rewritten to `<` by `left_to_right`), where
//! `Σ_b = Σ ∪ {BOUNDARY, word_start, word_end}` is the input-content alphabet.
//! Let `L_t` / `R_t` be the left / right context acceptors built over Σ_b but
//! made **transparent** to the bracket markers `<`/`>` (the bracket symbols
//! are freely skipped while matching, so an `L` run reads the underlying Σ_b
//! content even when a `< LHS >` event sits inside it). Then:
//!
//! ```text
//!   LeftForbid  = ~[ ~[Σ_M* · L_t] · <  · Σ_M* ]            (every < preceded by L)
//!   RightForbid = ~[ Σ_M* · >  · ~[R_t · Σ_M*] ]            (every > followed by R)
//!   Force       = ~[ Σ_M* · L_sealed · LHS · R_sealed · Σ_M* ]  (no UNbracketed
//!                                                            licensed LHS-start)
//!   Constraints = LeftForbid ∩ RightForbid ∩ Force
//! ```
//!
//! `LeftForbid`/`RightForbid` reject brackets at unlicensed sites; `Force`
//! makes the rewrite **obligatory** wherever the context holds (it forbids a
//! licensed LHS that is *missing* its brackets — the "sealed" seams let a
//! legitimate `<`/`>` break the pattern, so only unbracketed-yet-licensed
//! starts are forbidden). Caret insertion itself is purely *permissive*; the
//! marking is pinned by these three filters plus `left_to_right`.
//!
//! ### Why this stays polynomial (the perf fix)
//!
//! The complements are over `Σ_M*·X(·Σ_M*)` products. Strategy B's blow-up is
//! NOT that shape per se — it is `determinize` over the **non-minimal, ε-laden**
//! `L`/`LHS`/`R` pieces a `!class*` quantifier produces. Each piece is therefore
//! **pre-minimised** ([`det_min`]) before entering a product: the Turkish
//! harmony `L = back !V* + !V*` minimises to **7 states**, and the products /
//! complements then stay at a few dozen states (sub-second), versus the 72 k
//! transient states / ~8 s rustfst's `determinize` produces over the raw form
//! (and the >10-minute hang Strategy B exhibits on the same rule).
//!
//! ## Construction (Karttunen 1996 Figure 11, separate `Constraints(L,R)`)
//!
//! ```text
//!   permissive_insert     Σ_b → Σ_b∪{^}, an OPTIONAL ^ at any position
//!     ∘ left_to_right         each ^ opens a bracketed LHS event `< LHS >`;
//!                             ^ outside an event is forbidden
//!     ∘ longest_match         NotInner (Karttunen §3 eq. 8): drop a `<` whose
//!                             LHS reading contains an interior `>` (a shorter
//!                             match closed while a longer continuation exists),
//!                             so the LONGEST match per start wins
//!     ∘ context_license       NON-consuming L _ R licensing (the #3 piece):
//!                             LeftForbid ∩ RightForbid ∩ Force (the Force half
//!                             is maximality-aware for variable-length runs)
//!     ∘ replace               inside <..> transduce LHS → RHS, drop brackets
//!     ∘ strip                 erase any residual markers
//! ```
//!
//! Markers: caret `^` ([`PhonruleAlphabet::caret_label`], label 8) plus the
//! two existing obligatory brackets `<`=[`BRACKET_OPEN_OBLIG_LABEL`] (4) and
//! `>`=[`BRACKET_CLOSE_OBLIG_LABEL`] (6). None are members of Σ.
//!
//! ## Scope / supported LHS shapes (task #4 = NotInner longest-match)
//!
//! [`classify_lhs`] sorts every LHS into one of three buckets:
//!
//!   * **Fixed** — single-segment LHS (a `Class` reference or a `Literal`): one
//!     match width at every start, NotInner is the identity, the seam-anchored
//!     `Force` is exact. (#2/#3 behaviour, unchanged.)
//!   * **SingleAtomRun** — a single quantified atom whose run length is
//!     unbounded with minimum ≤ 1: `a+`, `Class+`, `!Class+`, `.+`, `a*`,
//!     `a{0,}`, `a{1,}`. The longest match is the **maximal run** of the atom's
//!     segment set; NotInner prunes every shorter `< … >` sharing a start and
//!     the maximality-aware `Force` makes the longest run obligatory and
//!     leftmost. **This is the #4 deliverable** and is byte-identical to
//!     [`crate::phonrule_eval::apply_phonrule`].
//!   * **Unsupported** ([`DirectedReplaceError::UnsupportedShape`]) — shapes
//!     this milestone does not compile byte-identically and therefore *refuses*
//!     rather than miscompiling:
//!       - a **bounded** single-atom run (`a{2,3}`, `a{2,}`): the cap/floor is
//!         not the maximal run;
//!       - a **fixed multi-symbol** `Range` (`a a`): self-overlapping, needs the
//!         `NotLeftmost` filter (Karttunen eq. 10) not built here;
//!       - a **multi-atom variable** `Range` (`a b+`): longest-match
//!         obligatoriness across several atoms;
//!       - an **unequal-length alternation** (`(ab|abc)`): note `phonrule_eval`'s
//!         `Alt` is FIRST-arm-wins, not longest, so a longest-match FST would
//!         disagree with the oracle regardless;
//!       - any **syllable** LHS/context atom (`SylHead`/`SylTail`/`SylIndex`/
//!         `SylBlock`).
//!
//! RHS: multi-char `Literal`, `Null` (deletion), `Map` (per-symbol; for a
//! variable-length LHS a `Map` RHS is a no-op, matching eval). Context atoms:
//! `Literal`/`Class`/`NegClass`/`Wildcard`/`Alt` with any quantifier, plus the
//! `Boundary`/`WordStart`/`WordEnd` anchors.
//!
//! This module is **not** wired into [`super::replace::compile_rewrite_rule`]
//! yet (task #5).

use std::collections::HashMap;

use crate::ast::{
    PhonAtom, PhonContextElem, PhonPattern, PhonReplacement, PhonRewriteRule, Quantifier,
};

use super::super::alphabet::{
    PhonruleAlphabet, BRACKET_CLOSE_OBLIG_LABEL, BRACKET_OPEN_OBLIG_LABEL,
};
use super::super::backend::{FstBuilder, Label, EPS_LABEL};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

// ===========================================================================
// Public errors.
// ===========================================================================

/// Errors produced while compiling a directed-replacement transducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectedReplaceError {
    /// The rule's LHS / RHS / context shape is not (yet) supported by this
    /// milestone's UPPER/LOWER builder.
    UnsupportedShape(String),
    /// A class referenced by the rule was not found in the class table.
    UnknownClass(String),
    /// A map referenced by the rule's RHS was not found in the map table.
    UnknownMap(String),
    /// A backend FST operation failed.
    Backend(String),
}

impl std::fmt::Display for DirectedReplaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectedReplaceError::UnsupportedShape(s) => {
                write!(f, "directed-replace: unsupported rule shape: {}", s)
            }
            DirectedReplaceError::UnknownClass(s) => {
                write!(f, "directed-replace: unknown class: {}", s)
            }
            DirectedReplaceError::UnknownMap(s) => {
                write!(f, "directed-replace: unknown map: {}", s)
            }
            DirectedReplaceError::Backend(s) => {
                write!(f, "directed-replace: backend error: {}", s)
            }
        }
    }
}

impl std::error::Error for DirectedReplaceError {}

type DResult<T> = Result<T, DirectedReplaceError>;

fn be<E: std::fmt::Display>(e: E) -> DirectedReplaceError {
    DirectedReplaceError::Backend(e.to_string())
}

// ===========================================================================
// Marker label bundle.
// ===========================================================================

/// The labels the construction composes over: the user alphabet Σ plus the
/// in-stream content markers (BOUNDARY, word edges) plus the three internal
/// construction markers (`^`, `<`, `>`), none of which are Σ members.
#[derive(Clone, Debug)]
struct Markers {
    /// Σ — the user alphabet (members of `alpha.sigma()`).
    sigma: Vec<Label>,
    /// In-stream content markers: BOUNDARY (`\0`), word_start, word_end. These
    /// can appear in the runtime input and must pass through passthrough
    /// regions verbatim; contexts may also match them (`+` boundary).
    stream: Vec<Label>,
    /// Fresh caret marker (`alpha.caret_label()`, reserved label 8).
    caret: Label,
    /// Karttunen `<` — reuses the obligatory open bracket (label 4).
    open: Label,
    /// Karttunen `>` — reuses the obligatory close bracket (label 6).
    close: Label,
}

impl Markers {
    fn new(alpha: &PhonruleAlphabet) -> Self {
        Markers {
            sigma: alpha.sigma().collect(),
            stream: vec![
                alpha.boundary_label(),
                alpha.word_start_label(),
                alpha.word_end_label(),
            ],
            caret: alpha.caret_label(),
            open: BRACKET_OPEN_OBLIG_LABEL,
            close: BRACKET_CLOSE_OBLIG_LABEL,
        }
    }

    /// `Σ_b` — input-content alphabet: Σ plus the in-stream content markers.
    /// This is the alphabet contexts read and the passthrough regions copy.
    fn sigma_b(&self) -> Vec<Label> {
        let mut v = self.sigma.clone();
        v.extend_from_slice(&self.stream);
        v
    }

    /// `Σ_b ∪ {<, >}` — everything except the caret (the no-caret runs and the
    /// licensing-stage alphabet `Σ_M`).
    fn sigma_with_brackets(&self) -> Vec<Label> {
        let mut v = self.sigma_b();
        v.push(self.open);
        v.push(self.close);
        v
    }
}

// ===========================================================================
// Public entry point.
// ===========================================================================

/// Build the directed-replacement (`@->`) transducer for `rule`.
///
/// Returns a transducer reading Σ_b and writing Σ_b (all construction markers
/// are introduced, constrained, and stripped internally). Applying it once
/// (compose with a linear input acceptor, read the output side) yields the
/// obligatory, longest, leftmost-first replacement — byte-identical to
/// [`crate::phonrule_eval::apply_phonrule`] for the supported rule shapes.
///
/// `class_table` maps each referenced class name to its member symbols
/// (the membership *set* used to build `Class`/`!Class` over Σ). `map_table`
/// maps each referenced map name to its compiled symbol→symbol transducer
/// (built by [`super::map::compile_map`]); only consulted for a `Map` RHS.
pub fn build_directed_replacement(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    map_table: &HashMap<String, RustFstWrapper>,
) -> DResult<RustFstWrapper> {
    // UPPER = LHS (acceptor over Σ_b — the bracketed region is ONLY the LHS).
    // `replace_core` transduces LHS → RHS (substitution / deletion / map).
    // `left` / `right` are the bracket-transparent context acceptors over Σ_M
    // for the non-consuming licensing filter.
    let built = build_upper_and_replace_core(rule, alpha, class_table, map_table)?;
    // Snapshot markers AFTER building the pieces: that may have interned new Σ
    // symbols (RHS literals, map outputs), which must be in every passthrough
    // region's Σ_b self-loops.
    let markers = Markers::new(alpha);
    let BuiltPieces {
        upper,
        replace_core,
        lhs_brk,
        left_brk,
        right_brk,
        left_brk_sealed,
        right_brk_sealed_lead,
        shape,
    } = built;

    // 1. Permissive caret insertion — an OPTIONAL `^` at any position. The
    //    obligation (which carets are required) is enforced non-consumingly
    //    by the licensing filter's Force pattern, not here.
    let permissive = build_permissive_insert(&markers)?;

    // 2. NotLeftmost — each ^ opens a bracketed `< LHS >`; a `^` not opening an
    //    LHS is rejected (so stray carets are pruned).
    let left_to_right = build_left_to_right(&markers, &upper)?;

    // 3. NotInner — longest-match filter (Karttunen 1996 §3 eq. 8). Forbids a
    //    `<` immediately followed by an LHS-instance that contains a `>`
    //    strictly inside it (a prematurely-closed SHORTER match while a LONGER
    //    LHS continuation from the same `<` exists). `lhs_brk` is the LHS
    //    acceptor lifted to read through interior `<`/`>` markers.
    let longest = build_longest_match(&markers, &lhs_brk)?;

    // 4. Context licensing — NON-consuming `L _ R` (the F2c4-#3 piece): Forbid
    //    brackets at unlicensed sites AND Force a bracket at every licensed
    //    LHS-start. All three sub-filters are over `Σ_M*·{L,R,LHS}`,
    //    rule-local — never `Σ_b*·L·LHS·R·Σ_b*`.
    let license = build_context_license(
        &markers,
        &lhs_brk,
        &left_brk,
        &right_brk,
        &left_brk_sealed,
        &right_brk_sealed_lead,
        &shape,
    )?;

    // 5. Replace — inside <..> transduce LHS → RHS, drop the brackets.
    let replace = build_replace_inside_brackets(&markers, &replace_core)?;

    // 6. Strip any residual markers from the output.
    let strip = build_strip_all_markers(&markers)?;

    let s1 = compose_sorted(&permissive, &left_to_right)?;
    let s2 = compose_sorted(&s1, &longest)?;
    let s3 = compose_sorted(&s2, &license)?;
    let s4 = compose_sorted(&s3, &replace)?;
    let composed = compose_sorted(&s4, &strip)?;

    Ok(composed)
}

// ===========================================================================
// UPPER / replace-core / context construction.
// ===========================================================================

struct BuiltPieces {
    /// `UPPER = LHS` acceptor over Σ_b (no transparency).
    upper: RustFstWrapper,
    /// `LHS:RHS` replace-core transducer.
    replace_core: RustFstWrapper,
    /// `LHS` bracket-transparent acceptor over Σ_M (interior brackets skipped;
    /// leading/trailing brackets NOT skipped). For the licensing Force pattern.
    lhs_brk: RustFstWrapper,
    /// Left context over Σ_M, brackets transparent everywhere (for LeftForbid:
    /// "prefix before `<` ends in L").
    left_brk: RustFstWrapper,
    /// Right context over Σ_M, brackets transparent everywhere (for RightForbid).
    right_brk: RustFstWrapper,
    /// Left context over Σ_M, brackets transparent in the interior but the
    /// trailing edge sealed (no bracket skip) — for the Force pattern's
    /// `L · LHS` seam, where a `<` must break the adjacency.
    left_brk_sealed: RustFstWrapper,
    /// Right context over Σ_M, brackets transparent but the *leading* edge
    /// sealed — for the Force pattern's `LHS · R` right seam.
    right_brk_sealed_lead: RustFstWrapper,
    /// LHS length classification, selecting the obligatoriness construction.
    shape: LhsShape,
}

/// Build the pieces for the whole construction. Context acceptors are made
/// transparent to the bracket markers so an L/R run reads the underlying Σ_b
/// content through intervening `< LHS >` events; the "sealed" variants
/// suppress bracket-skip at the LHS-adjacent edge so a legitimate bracket
/// breaks the unbracketed-licensed-start pattern.
fn build_upper_and_replace_core(
    rule: &PhonRewriteRule,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    map_table: &HashMap<String, RustFstWrapper>,
) -> DResult<BuiltPieces> {
    // Classify the LHS length behaviour first; reject genuinely unsupported
    // variable-length shapes early with a clear error.
    let shape = classify_lhs(&rule.from, alpha, class_table)?;
    if let LhsShape::Unsupported(why) = &shape {
        return Err(DirectedReplaceError::UnsupportedShape(why.clone()));
    }

    // UPPER = LHS over the pure Σ_b stream (no transparency), plus replace-core.
    let lhs_acceptor = lhs_fst(&rule.from, alpha, class_table, &[], Seal::None)?;
    let replace_core = build_replace_core(&lhs_acceptor, &rule.to, alpha, map_table)?;

    // LHS, bracket-transparent interior, sealed both edges (the bracketed
    // content `< LHS >` has its own `<`/`>`; the LHS itself reads pure Σ_b but
    // may contain interior boundary skips). Sealing both edges keeps the Force
    // seam analysis precise.
    let brk = [BRACKET_OPEN_OBLIG_LABEL, BRACKET_CLOSE_OBLIG_LABEL];
    let lhs_brk = lhs_fst(&rule.from, alpha, class_table, &brk, Seal::Both)?;

    let (l, r) = match &rule.context {
        None => (Vec::new(), Vec::new()),
        Some(ctx) => (ctx.left.clone(), ctx.right.clone()),
    };

    let left_brk = context_sequence_fst(&l, alpha, class_table, &brk, Seal::None)?;
    let right_brk = context_sequence_fst(&r, alpha, class_table, &brk, Seal::None)?;
    // For the Force `L·LHS` seam, seal L's trailing edge (no bracket skip there)
    let left_brk_sealed = context_sequence_fst(&l, alpha, class_table, &brk, Seal::Trailing)?;
    // For the Force `LHS·R` seam, seal R's leading edge.
    let right_brk_sealed_lead = context_sequence_fst(&r, alpha, class_table, &brk, Seal::Leading)?;

    // Pre-minimize every acceptor that feeds a `complement` of a `Σ_M*·X`
    // product. Without this rustfst's determinize of the product explodes on
    // `!V*`-bearing contexts (see [`det_min`]); with it, each stays a handful
    // of states.
    Ok(BuiltPieces {
        upper: lhs_acceptor,
        replace_core,
        lhs_brk: det_min(&lhs_brk)?,
        left_brk: det_min(&left_brk)?,
        right_brk: det_min(&right_brk)?,
        left_brk_sealed: det_min(&left_brk_sealed)?,
        right_brk_sealed_lead: det_min(&right_brk_sealed_lead)?,
        shape,
    })
}

/// Which edge(s) of a context/LHS sequence suppress transparent-marker skips
/// (boundary skips are always kept; only the *transparent* markers are sealed).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seal {
    None,
    Leading,
    Trailing,
    Both,
}

/// Compile the LHS pattern to a **bare** identity acceptor over Σ_b
/// (no surrounding skip). `transparent` adds interior skip-stars (between
/// multi-char literal chars / between range elems) so the Force pattern can
/// read an LHS that has interior brackets; for the common single-symbol LHS
/// it has no effect. `seal` is accepted for signature uniformity but the LHS
/// is always bare at its edges (the bracketed content sits between `<`/`>`).
fn lhs_fst(
    pat: &PhonPattern,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    transparent: &[Label],
    seal: Seal,
) -> DResult<RustFstWrapper> {
    let _ = seal;
    match pat {
        PhonPattern::Literal(s) => {
            if s.node.is_empty() {
                return Err(DirectedReplaceError::UnsupportedShape(
                    "empty-LHS insertion rule (\"\" -> X)".to_string(),
                ));
            }
            Ok(literal_fst_transparent(&s.node, alpha, transparent))
        }
        PhonPattern::Class(name) => {
            let labels = class_member_labels(&name.node, alpha, class_table)?;
            single_step_over(&labels)
        }
        PhonPattern::Range(elems) => {
            // A range LHS: build the elem sequence bare (lead/trail handled by
            // the caller's surrounding context), interior transparent.
            context_sequence_fst_inner(elems, alpha, class_table, transparent, Seal::Both, false)
        }
    }
}

/// Classification of an LHS pattern by its match-length behaviour, which
/// determines how obligatoriness (the Force constraint) must be expressed.
enum LhsShape {
    /// A single fixed match length at every start (single literal, single class
    /// symbol, or a `Range` of only `Exact(1)` / fixed atoms). The seam-based
    /// Force in [`build_context_license`] is byte-identical here.
    Fixed,
    /// A single quantified atom over the segment set `set` whose length varies
    /// (`a+`, `a*`, `a{n,m}` with n<m or unbounded, `Class+`, `!Class+`). The
    /// longest match is the maximal run of `set`; obligatoriness needs a
    /// maximality guard (Force must anchor on a run not extendable by another
    /// `set` symbol).
    SingleAtomRun { set: Vec<Label> },
    /// A variable-length shape this milestone does not compile byte-identically
    /// (multi-atom variable `Range`, unequal-length alternation, syllable
    /// atoms). Returned as [`DirectedReplaceError::UnsupportedShape`].
    Unsupported(String),
}

/// Classify the LHS for obligatoriness handling. `set` labels are interned.
fn classify_lhs(
    pat: &PhonPattern,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
) -> DResult<LhsShape> {
    match pat {
        // Single class symbol — always fixed width 1.
        PhonPattern::Class(_) => Ok(LhsShape::Fixed),
        // A literal LHS. A single character is fixed width 1; a multi-character
        // literal is fixed width > 1 — but if it *self-overlaps* (a proper
        // suffix equals a proper prefix, e.g. "aa", "aba"), a new match can
        // begin strictly inside a previous one. The seam-anchored Force in
        // [`build_context_license`] would then resolve to the WRONG (non-
        // leftmost) match: that case needs the Karttunen `NotLeftmost` filter
        // (eq. 10), which this milestone does not build. Refuse rather than
        // miscompile (the caller falls back to Strategy B). A non-overlapping
        // multi-char literal (e.g. "ab") cannot self-overlap and stays Fixed.
        PhonPattern::Literal(s) => {
            if literal_self_overlaps(&s.node) {
                Ok(LhsShape::Unsupported(format!(
                    "self-overlapping multi-character literal LHS ({:?}) — a new match can begin \
                     inside a previous one; needs the NotLeftmost filter (Karttunen eq. 10), not \
                     built in this milestone",
                    s.node
                )))
            } else {
                Ok(LhsShape::Fixed)
            }
        }
        PhonPattern::Range(elems) => {
            // A single quantified atom is the canonical variable-length /
            // longest-match shape.
            if elems.len() == 1 {
                if let PhonContextElem::Atom(atom, quant) = &elems[0] {
                    let fixed = matches!(quant, Quantifier::Exact(_))
                        || matches!(quant, Quantifier::Range(n, m) if n == m);
                    // The longest-match-as-maximal-run model is exact only for an
                    // *unbounded* run whose minimum length is ≤ 1 (`+`, `*`,
                    // `{0,}`, `{1,}`): then the greedy maximal run is always a
                    // valid match. A bounded `{n,m}` (m finite, n<m) or a high
                    // `{n,}` (n≥2) caps / floors the match below the maximal run,
                    // which the run model does not capture.
                    let run_quant = matches!(
                        quant,
                        Quantifier::Plus
                            | Quantifier::Star
                            | Quantifier::AtLeast(0)
                            | Quantifier::AtLeast(1)
                    );
                    let set = atom_segment_set(atom, alpha, class_table)?;
                    return Ok(match set {
                        // 1-char-segment atom: a non-empty single-segment set.
                        Some(s) if !s.is_empty() && fixed => LhsShape::Fixed,
                        Some(s) if !s.is_empty() && run_quant => {
                            LhsShape::SingleAtomRun { set: s }
                        }
                        Some(s) if !s.is_empty() => LhsShape::Unsupported(format!(
                            "bounded variable-length single-atom LHS ({:?}) — only `+`/`*`/`{{n,}}` \
                             with n≤1 are longest-match-exact in this milestone",
                            quant
                        )),
                        // Empty set ⇒ multi-char literal (width>1): fixed only if
                        // the quantifier is fixed, else unsupported.
                        Some(_) if fixed => LhsShape::Fixed,
                        Some(_) => LhsShape::Unsupported(
                            "variable-length multi-character-literal LHS".to_string(),
                        ),
                        None => LhsShape::Unsupported(
                            "variable-length LHS over a non-simple atom (alternation / syllable)"
                                .to_string(),
                        ),
                    });
                }
            }
            // Multi-atom Range: fixed only if every elem is a fixed-width atom.
            let mut all_fixed = true;
            for e in elems {
                match e {
                    PhonContextElem::Atom(atom, quant) => {
                        let q_fixed = matches!(quant, Quantifier::Exact(_))
                            || matches!(quant, Quantifier::Range(n, m) if n == m);
                        // Alt atoms can have unequal-length arms even at Exact(1).
                        let atom_fixed = match atom {
                            PhonAtom::Alt(arms) => alt_arms_equal_width(arms),
                            PhonAtom::SylBlock(_) => false,
                            _ => true,
                        };
                        if !q_fixed || !atom_fixed {
                            all_fixed = false;
                        }
                    }
                    PhonContextElem::Boundary
                    | PhonContextElem::WordStart
                    | PhonContextElem::WordEnd => {}
                    PhonContextElem::SylHead
                    | PhonContextElem::SylTail
                    | PhonContextElem::SylIndex(_) => {
                        return Ok(LhsShape::Unsupported("syllable LHS atom".to_string()))
                    }
                }
            }
            if all_fixed {
                // A fixed-width *multi-symbol* Range (e.g. `a a`) can begin a
                // new match strictly inside a previous one (`aaa` admits `aa` at
                // both offsets 0 and 1). Resolving that to the LEFTMOST match
                // requires the Karttunen `NotLeftmost` filter (eq. 10), which
                // this milestone does not build — the single-segment LHS that #2
                // /#3 support cannot self-overlap, so the gap was invisible until
                // now. Marked unsupported rather than miscompiled.
                Ok(LhsShape::Unsupported(
                    "fixed multi-symbol LHS Range (self-overlapping, e.g. `a a`) — needs the \
                     NotLeftmost filter (Karttunen eq. 10), not built in this milestone"
                        .to_string(),
                ))
            } else {
                Ok(LhsShape::Unsupported(
                    "multi-atom variable-length LHS Range (e.g. `a b+`, unequal alternation) — \
                     longest-match obligatoriness across multiple atoms is task #4-followup"
                        .to_string(),
                ))
            }
        }
    }
}

/// The segment-label set a *simple* atom matches in one step, or `None` if the
/// atom is not a simple single-segment matcher (Alt / SylBlock).
fn atom_segment_set(
    atom: &PhonAtom,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
) -> DResult<Option<Vec<Label>>> {
    Ok(match atom {
        PhonAtom::Class(name) => Some(class_member_labels(&name.node, alpha, class_table)?),
        PhonAtom::NegClass(name) => {
            let members = class_member_labels(&name.node, alpha, class_table)?;
            Some(alpha.sigma().filter(|l| !members.contains(l)).collect())
        }
        PhonAtom::Wildcard => Some(alpha.sigma().collect()),
        PhonAtom::Literal(lit) => {
            // A single-character literal is a one-segment matcher; a multi-char
            // literal is fixed width > 1 (handled by the Fixed path, not a run).
            if lit.node.chars().count() == 1 {
                Some(vec![alpha.intern(&lit.node)])
            } else {
                Some(vec![]) // signal "simple but width>1" → treated Fixed via caller
            }
        }
        PhonAtom::Alt(_) | PhonAtom::SylBlock(_) => None,
    })
}

/// True iff the literal string `s` self-overlaps: some proper non-empty suffix
/// equals the proper prefix of the same length. Such literals admit a match
/// starting strictly inside a previous one (`"aa"` matches at offsets 0 and 1
/// of `"aaa"`; `"aba"` matches at offsets 0 and 2 of `"ababa"`), which the
/// seam-based Force cannot resolve to the leftmost — those need `NotLeftmost`.
///
/// A single character (or empty) literal cannot self-overlap (no proper
/// non-empty suffix). Operates on chars (Σ symbols are length-1 strings in v1).
fn literal_self_overlaps(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    // Proper border lengths: 1 ..= n-1. If any border exists, the string
    // self-overlaps (a suffix of that length equals the prefix of that length).
    for k in 1..n {
        if chars[..k] == chars[n - k..] {
            return true;
        }
    }
    false
}

/// True iff every alternation arm matches the same fixed width (so the Alt is
/// length-unambiguous and the seam-based Force stays correct).
fn alt_arms_equal_width(arms: &[PhonContextElem]) -> bool {
    let width = |e: &PhonContextElem| -> Option<usize> {
        match e {
            PhonContextElem::Atom(PhonAtom::Literal(l), Quantifier::Exact(1)) => {
                Some(l.node.chars().count())
            }
            PhonContextElem::Atom(_, Quantifier::Exact(1)) => Some(1),
            _ => None,
        }
    };
    let mut iter = arms.iter();
    let Some(first) = iter.next().and_then(width) else {
        return false;
    };
    for a in iter {
        match width(a) {
            Some(w) if w == first => {}
            _ => return false,
        }
    }
    true
}

/// Build the `LHS:RHS` replace-core transducer.
///
/// `Null` → input-only (delete the LHS). `Literal` → input-only LHS followed
/// by output-only RHS emit. `Map` → compose the LHS acceptor with the map's
/// Kleene star (per-symbol map), mirroring `replacement::compile_lhs_to_rhs`.
fn build_replace_core(
    lhs_acceptor: &RustFstWrapper,
    rhs: &PhonReplacement,
    alpha: &mut PhonruleAlphabet,
    map_table: &HashMap<String, RustFstWrapper>,
) -> DResult<RustFstWrapper> {
    match rhs {
        PhonReplacement::Null => Ok(make_input_only(lhs_acceptor, alpha)),
        PhonReplacement::Literal(lit) => {
            let lhs_in = make_input_only(lhs_acceptor, alpha);
            let rhs_out = output_only_emitter(&lit.node, alpha);
            RustFstBackend::concat(&lhs_in, &rhs_out).map_err(be)
        }
        PhonReplacement::Map(map_ident) => {
            let map_fst = map_table
                .get(&map_ident.node)
                .ok_or_else(|| DirectedReplaceError::UnknownMap(map_ident.node.clone()))?;
            let map_star = RustFstBackend::closure_star(map_fst).map_err(be)?;
            let lhs_sorted = RustFstBackend::arc_sort_output(lhs_acceptor).map_err(be)?;
            let map_sorted = RustFstBackend::arc_sort_input(&map_star).map_err(be)?;
            RustFstBackend::compose(&lhs_sorted, &map_sorted).map_err(be)
        }
    }
}

// ---------------------------------------------------------------------------
// Context-sequence compilation (over Σ_b, BOUNDARY-transparent like eval).
// ---------------------------------------------------------------------------

/// Compile a context element sequence to an acceptor over Σ_b, reproducing
/// `phonrule_eval`'s matcher semantics:
///
///   * `\0` (BOUNDARY) is transparently skipped before each consuming Atom
///     (`segment_at` / `consume_literal` in `phonrule_eval`).
///   * a `Boundary` (`+`) elem matches the BOUNDARY label OR a word edge.
///   * each quantified atom wraps its single-position acceptor.
///
/// This mirrors `super::context::compile_context_sequence` but takes class
/// **member lists** (the `Vec<String>` table this module is called with)
/// rather than pre-compiled class FSTs, and lives here so the directed-replace
/// construction stays self-contained.
///
/// `transparent` is a set of marker labels (`<`/`>` at the licensing stage,
/// empty for the pure-stream LHS) that may be freely interspersed anywhere in
/// the match without being constrained. They are woven in via skip-stars at
/// every concatenation seam and folded into every closure so that a context
/// run reads the underlying Σ_b content through any intervening bracketed
/// `< LHS >` events. With `transparent = &[]` this reduces to the plain Σ_b
/// matcher (only BOUNDARY is skipped, exactly as `phonrule_eval` does).
fn context_sequence_fst(
    elems: &[PhonContextElem],
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    transparent: &[Label],
    seal: Seal,
) -> DResult<RustFstWrapper> {
    context_sequence_fst_inner(elems, alpha, class_table, transparent, seal, true)
}

/// Inner builder. `surround` adds leading/trailing skip-stars (true for
/// contexts; false for a bare LHS range). `seal` suppresses the *transparent*
/// markers (not BOUNDARY) at the leading and/or trailing edge.
fn context_sequence_fst_inner(
    elems: &[PhonContextElem],
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    transparent: &[Label],
    seal: Seal,
    surround: bool,
) -> DResult<RustFstWrapper> {
    let bdy = alpha.boundary_label();
    // Interior skip = BOUNDARY + transparent markers (freely skippable).
    let mut bm: Vec<Label> = vec![bdy];
    bm.extend_from_slice(transparent);
    let interior_skip = star_over(&bm)?;
    // Edge skip honouring `seal`. A *sealed* edge must still skip the brackets
    // that legitimately surround PRIOR / FOLLOWING events, but NOT the seam
    // bracket of the current target:
    //   * sealed trailing (the `L · LHS` seam): the seam is the current
    //     target's OPEN `<`; prior events end in CLOSE `>`. So skip bdy + `>`
    //     but not `<` — a `<` immediately before the LHS breaks the pattern.
    //   * sealed leading (the `LHS · R` seam): the seam is the current
    //     target's CLOSE `>`; following events start with OPEN `<`. So skip
    //     bdy + `<` but not `>`.
    let lead_sealed = matches!(seal, Seal::Leading | Seal::Both);
    let trail_sealed = matches!(seal, Seal::Trailing | Seal::Both);
    let trail_seal_labels: Vec<Label> = vec![bdy, BRACKET_CLOSE_OBLIG_LABEL];
    let lead_seal_labels: Vec<Label> = vec![bdy, BRACKET_OPEN_OBLIG_LABEL];
    // First-element leading seam when surround is off: the seam is the event's
    // OPEN `<`, so allow BOUNDARY + prior CLOSE `>`, never the open bracket.
    let lead_first_seal_labels: Vec<Label> = vec![bdy, BRACKET_CLOSE_OBLIG_LABEL];
    let lead_skip = if lead_sealed {
        star_over(&lead_seal_labels)?
    } else {
        interior_skip.clone()
    };
    let trail_skip = if trail_sealed {
        star_over(&trail_seal_labels)?
    } else {
        interior_skip.clone()
    };

    if elems.is_empty() {
        // Empty context: `{ε}` content with a BOUNDARY absorber (a `\0`
        // between the last real symbol and the LHS edge is tolerated). The skip
        // must honour the requested seal so an empty *sealed* context cannot
        // absorb the seam bracket: a leading-sealed empty R context is the
        // `LHS · R` right seam, whose left neighbour is the event's CLOSE `>`,
        // so it uses `lead_skip` (skips bdy + `<`, NOT `>`); a trailing-sealed
        // empty L context is the `L · LHS` left seam (right neighbour the OPEN
        // `<`) and uses `trail_skip` (skips bdy + `>`, NOT `<`).
        if !surround {
            return epsilon_acceptor();
        }
        return Ok(if lead_sealed {
            lead_skip
        } else {
            trail_skip
        });
    }

    let mut acc: Option<RustFstWrapper> = if surround {
        Some(lead_skip.clone())
    } else {
        None
    };
    let last = elems.len() - 1;
    // The LHS (built with `surround == false`) needs its quantified atoms in
    // *base-first* form so a run has clean, marker-free edges (cannot absorb the
    // event's own `<`/`>`); a *context* (`surround == true`) keeps the
    // skip-first form so its leading `*`/`?` can read through intervening
    // bracketed events.
    let base_first = !surround;
    for (i, elem) in elems.iter().enumerate() {
        let piece = context_elem_fst(elem, alpha, class_table, transparent, base_first)?;
        // The pre-skip before the LAST element is sealed when trailing-sealed:
        // a zero-width last `*`/`?` atom must not let its pre-skip swallow the
        // seam bracket (`<`) just before the LHS. Likewise its closure now
        // ends on a consumed symbol (skip-before-atom), so a non-empty run is
        // also clean at the seam.
        let seal_this_pre = trail_sealed && i == last;
        // The pre-skip before the FIRST element is sealed when leading-sealed
        // and there is no surround lead_skip to own the seam: a leading `*`/`?`
        // atom (or its closure) must not let its pre-skip swallow the seam
        // bracket — for the LHS used by NotInner / the Force pattern, the seam
        // is the event's OPEN `<`. Allow prior CLOSE `>` and BOUNDARY, not `<`.
        let seal_first_pre = lead_sealed && !surround && i == 0;
        let atom_pre = if seal_this_pre {
            star_over(&trail_seal_labels)?
        } else if seal_first_pre {
            star_over(&lead_first_seal_labels)?
        } else {
            interior_skip.clone()
        };
        let anchor_pre = if seal_this_pre {
            // sealed trailing anchor: allow prior CLOSE brackets, not OPEN.
            Some(star_over(&[BRACKET_CLOSE_OBLIG_LABEL])?)
        } else if seal_first_pre {
            Some(star_over(&[BRACKET_CLOSE_OBLIG_LABEL])?)
        } else if !transparent.is_empty() {
            Some(star_over(transparent)?)
        } else {
            None
        };
        // BOUNDARY-skip before each consuming Atom (eval skips `\0` there);
        // not before a `+`/anchor. For the first elem under surround the
        // lead_skip already provided the leading skip.
        let pre = if matches!(elem, PhonContextElem::Atom(_, _)) {
            Some(atom_pre)
        } else {
            anchor_pre
        };
        if let Some(pre) = pre {
            // Skip the leading seam for the first elem if surround already led.
            if !(surround && i == 0) {
                acc = Some(match acc {
                    None => pre,
                    Some(p) => RustFstBackend::concat(&p, &pre).map_err(be)?,
                });
            } else {
                // first elem under surround: lead_skip already in acc; add the
                // BOUNDARY-skip for an Atom only if lead is sealed (so `\0`
                // before the first atom is still tolerated).
                if lead_sealed && matches!(elem, PhonContextElem::Atom(_, _)) {
                    let bskip = star_over(&[bdy])?;
                    acc = Some(match acc {
                        None => bskip,
                        Some(p) => RustFstBackend::concat(&p, &bskip).map_err(be)?,
                    });
                }
            }
        }
        acc = Some(match acc {
            None => piece,
            Some(p) => RustFstBackend::concat(&p, &piece).map_err(be)?,
        });
    }
    let acc = acc.expect("non-empty or surround");
    if surround {
        RustFstBackend::concat(&acc, &trail_skip).map_err(be)
    } else {
        Ok(acc)
    }
}

/// Compile one [`PhonContextElem`] to an acceptor over Σ_b∪`transparent`.
fn context_elem_fst(
    elem: &PhonContextElem,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    transparent: &[Label],
    base_first: bool,
) -> DResult<RustFstWrapper> {
    match elem {
        PhonContextElem::Boundary => Ok(boundary_union_fst(alpha)),
        PhonContextElem::WordStart => single_step_over(&[alpha.word_start_label()]),
        PhonContextElem::WordEnd => single_step_over(&[alpha.word_end_label()]),
        PhonContextElem::SylHead => Err(DirectedReplaceError::UnsupportedShape(
            "%syl<head>% context".to_string(),
        )),
        PhonContextElem::SylTail => Err(DirectedReplaceError::UnsupportedShape(
            "%syl<tail>% context".to_string(),
        )),
        PhonContextElem::SylIndex(_) => Err(DirectedReplaceError::UnsupportedShape(
            "%syl<#N>% context".to_string(),
        )),
        PhonContextElem::Atom(atom, quant) => {
            let base = atom_fst(atom, alpha, class_table, transparent, base_first)?;
            // BOUNDARY transparency before/within the atom (eval skips `\0`
            // before each consuming atom); plus the transparent markers.
            apply_quantifier(base, *quant, alpha, transparent, base_first)
        }
    }
}

/// Compile a [`PhonAtom`] to a single-position acceptor over Σ_b.
fn atom_fst(
    atom: &PhonAtom,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
    transparent: &[Label],
    base_first: bool,
) -> DResult<RustFstWrapper> {
    match atom {
        PhonAtom::Class(name) => {
            let labels = class_member_labels(&name.node, alpha, class_table)?;
            single_step_over(&labels)
        }
        PhonAtom::NegClass(name) => {
            let members = class_member_labels(&name.node, alpha, class_table)?;
            let neg: Vec<Label> = alpha
                .sigma()
                .filter(|l| !members.contains(l))
                .collect();
            single_step_over(&neg)
        }
        PhonAtom::Literal(lit) => Ok(literal_fst(&lit.node, alpha)),
        PhonAtom::Wildcard => {
            let sigma: Vec<Label> = alpha.sigma().collect();
            single_step_over(&sigma)
        }
        PhonAtom::Alt(alts) => {
            if alts.is_empty() {
                return single_step_over(&[]); // empty language
            }
            let mut iter = alts.iter();
            let first =
                context_elem_fst(iter.next().unwrap(), alpha, class_table, transparent, base_first)?;
            let mut acc = first;
            for alt in iter {
                let next = context_elem_fst(alt, alpha, class_table, transparent, base_first)?;
                acc = RustFstBackend::union(&acc, &next).map_err(be)?;
            }
            Ok(acc)
        }
        PhonAtom::SylBlock(_) => Err(DirectedReplaceError::UnsupportedShape(
            "%syl[ ... ]% atom".to_string(),
        )),
    }
}

/// Resolve a class name to its Σ member labels (interning members so the
/// labels are stable). Errors if the class is absent from `class_table`.
fn class_member_labels(
    name: &str,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, Vec<String>>,
) -> DResult<Vec<Label>> {
    let members = class_table
        .get(name)
        .ok_or_else(|| DirectedReplaceError::UnknownClass(name.to_string()))?;
    Ok(members.iter().map(|m| alpha.intern(m)).collect())
}

/// Wrap a single-position acceptor with its quantifier.
///
/// For multi-occurrence quantifiers the repetition unit is `base ·
/// bdy_marker_skip` so that BOUNDARY (`\0`, skipped before each consuming
/// atom by eval) and the transparent bracket markers may appear *between*
/// repetitions of a `*`/`+`/`{n,m}` run. `Exact(1)` is the un-quantified
/// (v1) form and needs no inter-copy skip.
fn apply_quantifier(
    base: RustFstWrapper,
    quant: Quantifier,
    alpha: &PhonruleAlphabet,
    transparent: &[Label],
    base_first: bool,
) -> DResult<RustFstWrapper> {
    if !base_first {
        return apply_quantifier_skip_first(base, quant, alpha, transparent);
    }
    // Inter-copy skip = BOUNDARY + transparent markers.
    let mut bm: Vec<Label> = vec![alpha.boundary_label()];
    bm.extend_from_slice(transparent);
    let inter = star_over(&bm)?;
    // Repetition is built **base-first**: the first occurrence is a bare `base`
    // and every *subsequent* occurrence is `inter · base` (the inter-copy skip
    // sits strictly BETWEEN copies). This means a multi-occurrence run neither
    // begins with a skip nor ends with one: the leading edge is a consumed base
    // (so it cannot absorb a preceding seam bracket `<`) and the trailing edge
    // is a consumed base (so it cannot absorb a following seam bracket `>`),
    // while the transparent markers still pass *between* copies. The same lifted
    // acceptor is therefore safe at both seams — used by the Force pattern (no
    // unbracketed start) and by NotInner (interior `>` detection) alike.
    //
    // `tail = (inter · base)`, the unit for occurrences 2..n.
    let tail = RustFstBackend::concat(&inter, &base).map_err(be)?;
    // `run(min,max)` = base-first run of [min,max] occurrences (max=None ⇒ ∞).
    let run = |min: u32, max: Option<u32>| -> DResult<RustFstWrapper> {
        if min == 0 && max == Some(0) {
            return epsilon_acceptor();
        }
        // At least one occurrence: base · (inter·base){min-1,max-1}.
        let lo = min.saturating_sub(1);
        let tail_rep = match max {
            None => RustFstBackend::closure_star(&tail).map_err(be)?,
            Some(hi) => RustFstBackend::closure_bounded(&tail, lo, hi - 1).map_err(be)?,
        };
        let tail_rep = if max.is_none() && lo > 0 {
            // base-first with min>1 and unbounded max: (inter·base){lo,} =
            // (inter·base){lo} · (inter·base)*  — closure_bounded(lo,lo) then *.
            let head = RustFstBackend::closure_bounded(&tail, lo, lo).map_err(be)?;
            let star = RustFstBackend::closure_star(&tail).map_err(be)?;
            RustFstBackend::concat(&head, &star).map_err(be)?
        } else {
            tail_rep
        };
        let one_or_more = RustFstBackend::concat(&base, &tail_rep).map_err(be)?;
        if min == 0 {
            RustFstBackend::closure_optional(&one_or_more).map_err(be)
        } else {
            Ok(one_or_more)
        }
    };

    let wrapped = match quant {
        Quantifier::Exact(1) => base,
        Quantifier::Exact(0) => epsilon_acceptor()?,
        Quantifier::Exact(n) => run(n, Some(n))?,
        Quantifier::Star => run(0, None)?,
        Quantifier::Plus => run(1, None)?,
        Quantifier::Question => run(0, Some(1))?,
        Quantifier::AtLeast(n) => run(n, None)?,
        Quantifier::Range(n, m) => run(n, Some(m))?,
    };
    Ok(wrapped)
}

/// Skip-first quantifier wrapping (the repetition unit is `inter · base`), used
/// for **contexts** (`surround == true`): the leading inter-copy skip lets a
/// context's first `*`/`?` occurrence read through intervening bracketed events
/// (a `< LHS >` to its right that it must see past). Putting the skip *before*
/// each base also means the closure ends on a consumed base, so a trailing
/// `*`/`+` context atom (e.g. `!V*`) doesn't leak a transparent bracket past the
/// run — keeping the Force pattern's `L · LHS` seam clean.
fn apply_quantifier_skip_first(
    base: RustFstWrapper,
    quant: Quantifier,
    alpha: &PhonruleAlphabet,
    transparent: &[Label],
) -> DResult<RustFstWrapper> {
    let mut bm: Vec<Label> = vec![alpha.boundary_label()];
    bm.extend_from_slice(transparent);
    let inter = star_over(&bm)?;
    let unit = || RustFstBackend::concat(&inter, &base).map_err(be);
    let wrapped = match quant {
        Quantifier::Exact(1) => base,
        Quantifier::Exact(0) => epsilon_acceptor()?,
        Quantifier::Exact(n) => RustFstBackend::closure_bounded(&unit()?, n, n).map_err(be)?,
        Quantifier::Star => RustFstBackend::closure_star(&unit()?).map_err(be)?,
        Quantifier::Plus => RustFstBackend::closure_plus(&unit()?).map_err(be)?,
        Quantifier::Question => RustFstBackend::closure_optional(&base).map_err(be)?,
        Quantifier::AtLeast(n) => {
            let star = RustFstBackend::closure_star(&unit()?).map_err(be)?;
            if n == 0 {
                star
            } else {
                let head = RustFstBackend::closure_bounded(&unit()?, n, n).map_err(be)?;
                RustFstBackend::concat(&head, &star).map_err(be)?
            }
        }
        Quantifier::Range(n, m) => RustFstBackend::closure_bounded(&unit()?, n, m).map_err(be)?,
    };
    Ok(wrapped)
}

// ===========================================================================
// 1. Permissive caret insertion.
// ===========================================================================

/// `permissive`: Σ_b:Σ_b identity self-loops + an ε:^ self-loop (1 state).
/// Inserts an OPTIONAL `^` at any position; the licensing Force pattern is
/// what makes the chosen marking obligatory (and the licensing Forbid pattern
/// what keeps it from over-marking).
fn build_permissive_insert(markers: &Markers) -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).map_err(be)?;
    b.set_final(s).map_err(be)?;
    for &l in &markers.sigma_b() {
        b.add_arc(s, l, l, s).map_err(be)?;
    }
    b.add_arc(s, EPS_LABEL, markers.caret, s).map_err(be)?;
    b.finish().map_err(be)
}

// ===========================================================================
// 2. NotLeftmost (LeftToRight).
// ===========================================================================

/// `left_to_right`: `[~$[%^] [%^:%< LHS 0:%>]]* ~$[%^]`.
///
/// Reads the marked tape (Σ_b∪{^}); allows arbitrary no-caret runs (over
/// Σ_b∪{<,>}) interleaved with bracketed events `^:< · LHS · ε:>`. Any caret
/// outside a bracketed event is rejected. Brackets `<`/`>` are emitted around
/// the matched LHS so the licensing + replace stages can find them.
fn build_left_to_right(markers: &Markers, upper: &RustFstWrapper) -> DResult<RustFstWrapper> {
    let no_caret = sigma_star_over(&markers.sigma_with_brackets())?;

    let caret_to_open = single_arc(markers.caret, markers.open)?;
    let close_insert = single_arc(EPS_LABEL, markers.close)?;
    let cat1 = RustFstBackend::concat(&caret_to_open, upper).map_err(be)?;
    let bracket_event = RustFstBackend::concat(&cat1, &close_insert).map_err(be)?;

    let cell = RustFstBackend::concat(&no_caret, &bracket_event).map_err(be)?;
    let cells_star = RustFstBackend::closure_star(&cell).map_err(be)?;
    RustFstBackend::concat(&cells_star, &no_caret).map_err(be)
}

// ===========================================================================
// 3. NotInner (LongestMatch) — Karttunen 1996 §3 eq. (8).
// ===========================================================================

/// `NotInner`: the longest-match filter. After `left_to_right`, every event on
/// the tape is a well-formed `< LHS >` (the caret has been rewritten to `<`),
/// so this filter operates over `Σ_M = Σ_b ∪ {<, >}`.
///
/// When the LHS can match more than one length from the same start (a
/// quantified atom like `a+`, or an unequal-length alternation `(ab|abc)`),
/// `left_to_right` admits *every* candidate bracketing that shares a start `<`
/// (e.g. for `aaa` under `a+`: `<a>aa`, `<aa>a`, `<aaa>`). NotInner keeps only
/// the LONGEST: it forbids a `<` immediately followed by an LHS-instance that
/// contains a `>` *strictly inside* it.
///
/// Karttunen 1996 §3 eq. (8): `NotInner = ~$[ < [UPPER'' & $[>]] ]`, where
/// `UPPER''` is the LHS acceptor lifted to permit the internal markers `<`/`>`
/// interspersed between its symbols. A `>` strictly inside such an instance
/// means a SHORTER match was closed (its `>` placed early) while the symbols
/// continue to form a longer LHS from the same `<` — the shorter placement is
/// dropped, leaving the longest.
///
/// Concretely:
///
/// ```text
///   has_close  = Σ_M* · > · Σ_M*                       (contains a `>`)
///   lhs_inner  = lhs_brk ∩ has_close                   (an LHS reading that has
///                                                        an interior `>`)
///   bad        = Σ_M* · < · lhs_inner · Σ_M*
///   NotInner   = ~bad
/// ```
///
/// `lhs_brk` is the LHS acceptor over Σ_M with interior `<`/`>` transparency
/// and **both edges sealed** (no leading/trailing marker skip) — so a candidate
/// reading cannot absorb the LHS's own opening `<` or its final closing `>`;
/// only a `>` *between two LHS-consuming symbols* counts. Each operand is
/// pre-minimised (`lhs_brk` via [`det_min`]) and every product stays at a
/// handful of states, so the filter is sub-second.
///
/// For a fixed-width LHS (single literal / single class symbol / fixed-shape
/// `Range`) `lhs_brk` admits only one length per start, so `lhs_inner` is empty
/// and `NotInner` reduces to the identity over Σ_M — preserving the prior
/// (vacuous) behaviour on those shapes.
fn build_longest_match(markers: &Markers, lhs_brk: &RustFstWrapper) -> DResult<RustFstWrapper> {
    let sigma_m = markers.sigma_with_brackets();
    let sigma_m_star = sigma_star_over(&sigma_m)?;
    let close_step = sigma_step_over(&[markers.close])?;
    let open_step = sigma_step_over(&[markers.open])?;

    // has_close = Σ_M* · > · Σ_M* (the reading contains at least one `>`).
    let hc1 = RustFstBackend::concat(&sigma_m_star, &close_step).map_err(be)?;
    let has_close = RustFstBackend::concat(&hc1, &sigma_m_star).map_err(be)?;

    // lhs_inner = lhs_brk ∩ has_close — an LHS reading (interior markers
    // permitted, edges sealed) that has a `>` strictly inside it. Pre-minimise
    // both operands of the intersection.
    let lhs_min = det_min(lhs_brk)?;
    let has_close_min = det_min(&has_close)?;
    let lhs_inner = RustFstBackend::intersect(&lhs_min, &has_close_min).map_err(be)?;
    let lhs_inner = det_min(&lhs_inner)?;

    // If lhs_inner is empty (fixed-width LHS), NotInner is the identity. The
    // complement of an empty bad pattern is Σ_M* anyway, so we can skip the
    // product entirely — it is the common case and keeps the chain tiny.
    if RustFstBackend::paths(&lhs_inner)
        .map_err(be)?
        .next()
        .is_none()
    {
        return sigma_star_over(&sigma_m);
    }

    // bad = Σ_M* · < · lhs_inner · Σ_M*
    let b1 = RustFstBackend::concat(&sigma_m_star, &open_step).map_err(be)?;
    let b2 = RustFstBackend::concat(&b1, &lhs_inner).map_err(be)?;
    let bad = RustFstBackend::concat(&b2, &sigma_m_star).map_err(be)?;

    RustFstBackend::complement(&bad, &sigma_m).map_err(be)
}

// ===========================================================================
// 4. Context licensing — NON-consuming `L _ R` (the F2c4-#3 piece).
// ===========================================================================

/// Build the non-consuming context-licensing filter (`Constraints(L, R)`),
/// the heart of F2c4-#3.
///
/// Operates on the bracketed tape over `Σ_M = Σ_b ∪ {<, >}` (the caret has
/// already been rewritten to `<` by `left_to_right`). It is the intersection
/// of three forbid-based sub-filters. Every complement's operand is a `Σ_M*·X`
/// (or `X·Σ_M*`, or `Σ_M*·X·Σ_M*`) product whose `X ∈ {L, R, L·LHS·R}` pieces
/// have been pre-minimised by [`det_min`] in `build_upper_and_replace_core`.
///
/// This is what keeps the construction polynomial. Strategy B's blow-up is NOT
/// the `Σ*·…·Σ*` *shape* per se — it is `determinize` over the **non-minimal,
/// ε-laden** `L`/`LHS`/`R` pieces (the `!class*` quantifier expands to a large
/// transient subset construction). With each piece reduced to its minimal DFA
/// first (the Turkish harmony `L` minimises to 7 states), the products and
/// their complements stay at a few dozen states and compile in well under a
/// second — the rule Strategy B could not compile in 10 minutes.
///
/// 1. **LeftForbid** — `~[ ~[Σ_M*·L] · < · Σ_M* ]`: every `<` is immediately
///    preceded by an L-match. `L` is bracket-transparent, so the match reads
///    the Σ_b content through earlier `< LHS >` events; the same L symbol can
///    license many adjacent `<`s (the multi-target property).
/// 2. **RightForbid** — `~[ Σ_M* · > · ~[R·Σ_M*] ]`: every `>` is immediately
///    followed by an R-match.
/// 3. **Force** — `~[ Σ_M* · L_sealed · LHS · R_sealedlead · Σ_M* ]`: there is
///    NO *unbracketed* licensed LHS-start. The sealed seams mean a legitimate
///    `<` (between L and LHS) or `>` (between LHS and R) breaks the pattern, so
///    only an LHS that is licensed *and missing its brackets* is forbidden —
///    making the rewrite obligatory wherever the context holds.
fn build_context_license(
    markers: &Markers,
    lhs_brk: &RustFstWrapper,
    left_brk: &RustFstWrapper,
    right_brk: &RustFstWrapper,
    left_brk_sealed: &RustFstWrapper,
    right_brk_sealed_lead: &RustFstWrapper,
    shape: &LhsShape,
) -> DResult<RustFstWrapper> {
    let sigma_m = markers.sigma_with_brackets();
    let sigma_m_star = sigma_star_over(&sigma_m)?;
    let open_step = sigma_step_over(&[markers.open])?;
    let close_step = sigma_step_over(&[markers.close])?;

    // --- 1. LeftForbid: every `<` preceded by an L-match. ---
    let good_pref = RustFstBackend::concat(&sigma_m_star, left_brk).map_err(be)?;
    let not_good_pref = RustFstBackend::complement(&good_pref, &sigma_m).map_err(be)?;
    let bl1 = RustFstBackend::concat(&not_good_pref, &open_step).map_err(be)?;
    let bad_left = RustFstBackend::concat(&bl1, &sigma_m_star).map_err(be)?;
    let left_license = RustFstBackend::complement(&bad_left, &sigma_m).map_err(be)?;

    // --- 2. RightForbid: every `>` followed by an R-match. ---
    let good_suf = RustFstBackend::concat(right_brk, &sigma_m_star).map_err(be)?;
    let not_good_suf = RustFstBackend::complement(&good_suf, &sigma_m).map_err(be)?;
    let br1 = RustFstBackend::concat(&sigma_m_star, &close_step).map_err(be)?;
    let bad_right = RustFstBackend::concat(&br1, &not_good_suf).map_err(be)?;
    let right_license = RustFstBackend::complement(&bad_right, &sigma_m).map_err(be)?;

    // --- 3. Force: no UNBRACKETED licensed LHS-start. ---
    // bad_force = prefix_noopen · L_sealed · LHS · R_sealedlead · suffix_noclose
    //
    // The leading `Σ_M*` is replaced by `prefix_noopen` (ε, or any Σ_M* run
    // ending in a non-`<` symbol) and the trailing `Σ_M*` by `suffix_noclose`
    // (ε, or any run starting with a non-`>` symbol). This pins the seam: the
    // symbol *immediately* before the L·LHS region may not be a `<`, and the
    // symbol immediately after the LHS·R region may not be a `>`. So a genuine
    // `< LHS >` event (whose `<` sits exactly at the seam) is NOT counted as an
    // unbracketed start. With a *non-empty* L this was already guaranteed
    // (`L_sealed` ends on a real L symbol); the guard makes it hold for the
    // empty-context case too, where `L_sealed`/`R_sealedlead` match ε and the
    // bare `Σ_M*` would otherwise absorb the event's own `<`/`>`.
    let nonopen: Vec<Label> = sigma_m.iter().copied().filter(|&l| l != markers.open).collect();
    let nonclose: Vec<Label> = sigma_m.iter().copied().filter(|&l| l != markers.close).collect();
    let prefix_noopen = {
        let tail = RustFstBackend::concat(&sigma_m_star, &sigma_step_over(&nonopen)?).map_err(be)?;
        det_min(&RustFstBackend::union(&epsilon_acceptor()?, &tail).map_err(be)?)?
    };
    let suffix_noclose = {
        let head = RustFstBackend::concat(&sigma_step_over(&nonclose)?, &sigma_m_star).map_err(be)?;
        det_min(&RustFstBackend::union(&epsilon_acceptor()?, &head).map_err(be)?)?
    };

    let force = match shape {
        LhsShape::SingleAtomRun { set } => {
            // Variable-length single-atom run (`a+`, `Class+`, `a{n,m}` …): the
            // longest match is the MAXIMAL run of `set`. Obligatoriness forbids
            // an unbracketed maximal run, anchored so the run START is not
            // bracketed (no `<` immediately before) and the run is MAXIMAL
            // (cannot be extended by another `set` symbol). The run is lifted
            // through interior `<`/`>` so its full extent is seen even across
            // already-bracketed inner candidates that NotInner left in place.
            //
            //   bad_force_run = prefix_pre · L_sealed · run_lifted
            //                   · R_sealedlead · not_continue
            //
            // where `prefix_pre` ends in a symbol that is neither `<` nor a
            // `set` member (so the run's left edge is a genuine, unbracketed,
            // maximal-left start), and `not_continue` forbids the run being
            // followed (through skippable markers/boundary) by another `set`
            // symbol.
            let set_step = sigma_step_over(set)?;
            let mut interlift_labels = vec![markers.open, markers.close, markers.stream[0]]; // <, >, BOUNDARY
            interlift_labels.dedup();
            let interlift = star_over(&interlift_labels)?;
            // run_lifted = set · (interlift · set)*  (base-first; leading clean).
            let cont = RustFstBackend::concat(&interlift, &set_step).map_err(be)?;
            let cont_star = RustFstBackend::closure_star(&cont).map_err(be)?;
            let run_lifted = RustFstBackend::concat(&set_step, &cont_star).map_err(be)?;
            // prefix_pre = ε ∪ Σ_M* · (Σ_M \ {<} \ set)
            let pre_set: Vec<Label> = sigma_m
                .iter()
                .copied()
                .filter(|l| *l != markers.open && !set.contains(l))
                .collect();
            let prefix_pre = {
                let tail = RustFstBackend::concat(&sigma_m_star, &sigma_step_over(&pre_set)?)
                    .map_err(be)?;
                det_min(&RustFstBackend::union(&epsilon_acceptor()?, &tail).map_err(be)?)?
            };
            // not_continue = ~[ interlift · set · Σ_M* ]: the remainder after the
            // run may not begin (through skippable markers / boundary) with
            // another `set` symbol, i.e. the run is MAXIMAL. (No `suffix_noclose`
            // guard here: a trailing `>` does NOT exempt the run — a run that is
            // only *partially* bracketed, e.g. the bare `a` in `a<a>`, is still
            // an unbracketed start and must be forced.)
            let cont_rest = RustFstBackend::concat(&cont, &sigma_m_star).map_err(be)?;
            let not_continue = RustFstBackend::complement(&det_min(&cont_rest)?, &sigma_m)
                .map_err(be)?;

            let f1 = RustFstBackend::concat(&prefix_pre, left_brk_sealed).map_err(be)?;
            let f2 = RustFstBackend::concat(&f1, &run_lifted).map_err(be)?;
            let f3 = RustFstBackend::concat(&f2, right_brk_sealed_lead).map_err(be)?;
            let bad_force = RustFstBackend::concat(&f3, &not_continue).map_err(be)?;
            let bad_force = det_min(&bad_force)?;
            RustFstBackend::complement(&bad_force, &sigma_m).map_err(be)?
        }
        _ => {
            // Fixed-width LHS (single literal / class / fixed Range): the
            // seam-anchored Force is exact.
            let f1 = RustFstBackend::concat(&prefix_noopen, left_brk_sealed).map_err(be)?;
            let f2 = RustFstBackend::concat(&f1, lhs_brk).map_err(be)?;
            let f3 = RustFstBackend::concat(&f2, right_brk_sealed_lead).map_err(be)?;
            let bad_force = RustFstBackend::concat(&f3, &suffix_noclose).map_err(be)?;
            let bad_force = det_min(&bad_force)?;
            RustFstBackend::complement(&bad_force, &sigma_m).map_err(be)?
        }
    };

    let lr = RustFstBackend::intersect(&left_license, &right_license).map_err(be)?;
    RustFstBackend::intersect(&lr, &force).map_err(be)
}

// ===========================================================================
// 5. Replace.
// ===========================================================================

/// `replace`: identity outside `<..>`; inside, drop the brackets and apply the
/// replace-core transducer (`LHS:RHS`) to the bracketed content.
fn build_replace_inside_brackets(
    markers: &Markers,
    replace_core: &RustFstWrapper,
) -> DResult<RustFstWrapper> {
    let outside_one = sigma_step_over(&markers.sigma_b())?;

    let open_drop = single_arc(markers.open, EPS_LABEL)?;
    let close_drop = single_arc(markers.close, EPS_LABEL)?;
    let cat1 = RustFstBackend::concat(&open_drop, replace_core).map_err(be)?;
    let bracket_replace = RustFstBackend::concat(&cat1, &close_drop).map_err(be)?;

    let alt = RustFstBackend::union(&outside_one, &bracket_replace).map_err(be)?;
    RustFstBackend::closure_star(&alt).map_err(be)
}

// ===========================================================================
// 6. Strip.
// ===========================================================================

/// `strip`: identity on Σ_b, marker:ε on each of `^`, `<`, `>` (defensive).
fn build_strip_all_markers(markers: &Markers) -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).map_err(be)?;
    b.set_final(s).map_err(be)?;
    for &l in &markers.sigma_b() {
        b.add_arc(s, l, l, s).map_err(be)?;
    }
    for marker in [markers.caret, markers.open, markers.close] {
        b.add_arc(s, marker, EPS_LABEL, s).map_err(be)?;
    }
    b.finish().map_err(be)
}

// ===========================================================================
// Small FST builders.
// ===========================================================================

fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> DResult<RustFstWrapper> {
    let a_sorted = RustFstBackend::arc_sort_output(a).map_err(be)?;
    let b_sorted = RustFstBackend::arc_sort_input(b).map_err(be)?;
    RustFstBackend::compose(&a_sorted, &b_sorted).map_err(be)
}

/// `eps_remove · determinize · minimize` — collapse an acceptor to its minimal
/// DFA. CRUCIAL before feeding a context/LHS acceptor into a `complement` of a
/// `Σ_M*·L` (resp. `R·Σ_M*`) product: the context acceptors are built from
/// many `concat`/`closure`/`union` steps and carry redundant ε structure, and
/// rustfst's `determinize` of the `Σ_M*·L` product over the non-minimal form
/// explodes (observed: 72 k transient states / ~8 s on the Turkish harmony L),
/// even though the minimal DFA is tiny (7 states). Pre-minimizing the operand
/// keeps the subsequent determinize/complement at a few dozen states / ms —
/// the difference between sub-second and "Strategy B can't compile it".
fn det_min(a: &RustFstWrapper) -> DResult<RustFstWrapper> {
    let e = RustFstBackend::eps_remove(a).map_err(be)?;
    let d = RustFstBackend::determinize(&e).map_err(be)?;
    RustFstBackend::minimize(&d).map_err(be)
}

/// Kleene-star acceptor over `labels` (one-state self-loops). Alias of
/// [`sigma_star_over`] used where the "zero or more of these markers" reading
/// is clearer.
fn star_over(labels: &[Label]) -> DResult<RustFstWrapper> {
    sigma_star_over(labels)
}

/// Identity self-loop acceptor over `labels` (`Σ*` shape).
fn sigma_star_over(labels: &[Label]) -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).map_err(be)?;
    b.set_final(s).map_err(be)?;
    for &l in labels {
        b.add_arc(s, l, l, s).map_err(be)?;
    }
    b.finish().map_err(be)
}

/// Single step over `labels` (one of any, no closure).
fn sigma_step_over(labels: &[Label]) -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).map_err(be)?;
    b.set_final(s1).map_err(be)?;
    for &l in labels {
        b.add_arc(s0, l, l, s1).map_err(be)?;
    }
    b.finish().map_err(be)
}

/// 2-state identity acceptor over `labels` (alias of [`sigma_step_over`] used
/// where the "single symbol from this set" reading is clearer).
fn single_step_over(labels: &[Label]) -> DResult<RustFstWrapper> {
    sigma_step_over(labels)
}

/// One-state ε-acceptor (`{ε}`).
fn epsilon_acceptor() -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).map_err(be)?;
    b.set_final(s).map_err(be)?;
    b.finish().map_err(be)
}

/// 2-state acceptor accepting any one of boundary / word-start / word-end —
/// the `+` boundary union (mirrors `phonrule_eval::match_seq` Boundary).
fn boundary_union_fst(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    for l in [
        alpha.boundary_label(),
        alpha.word_start_label(),
        alpha.word_end_label(),
    ] {
        b.add_arc(s0, l, l, s1).expect("boundary union arc");
    }
    b.finish().expect("boundary_union finish")
}

/// Linear identity acceptor for a literal string (one Σ arc per char, with
/// BOUNDARY-skip self-loops between chars — mirrors eval's `consume_literal`).
fn literal_fst(s: &str, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
    literal_fst_transparent(s, alpha, &[])
}

/// Like [`literal_fst`] but additionally self-loops on each `transparent`
/// marker between chars (interior bracket-transparency for the Force pattern).
fn literal_fst_transparent(
    s: &str,
    alpha: &mut PhonruleAlphabet,
    transparent: &[Label],
) -> RustFstWrapper {
    let chars: Vec<char> = s.chars().collect();
    let bdy = alpha.boundary_label();
    let mut b = RustFstBackend::builder();
    let start = b.add_state();
    b.set_start(start).expect("set_start");
    let mut prev = start;
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 {
            b.add_arc(prev, bdy, bdy, prev).expect("literal BDY-skip");
            for &m in transparent {
                b.add_arc(prev, m, m, prev).expect("literal marker-skip");
            }
        }
        let next = b.add_state();
        let label = alpha.intern(&ch.to_string());
        b.add_arc(prev, label, label, next).expect("literal arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish literal")
}

/// Project an identity acceptor to input-only (`σ:σ` → `σ:ε`) via composition
/// with a Σ_b/bracket eraser.
fn make_input_only(acceptor: &RustFstWrapper, alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let eraser = build_eraser(alpha);
    let lhs_sorted = RustFstBackend::arc_sort_output(acceptor).expect("sort lhs");
    let eraser_sorted = RustFstBackend::arc_sort_input(&eraser).expect("sort eraser");
    RustFstBackend::compose(&lhs_sorted, &eraser_sorted).expect("compose eraser")
}

/// One-state `σ:ε` eraser over Σ_b and the two brackets (defensive).
fn build_eraser(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    for label in alpha.sigma() {
        b.add_arc(s, label, EPS_LABEL, s).expect("eraser σ:ε");
    }
    for marker in [
        alpha.boundary_label(),
        alpha.word_start_label(),
        alpha.word_end_label(),
        BRACKET_OPEN_OBLIG_LABEL,
        BRACKET_CLOSE_OBLIG_LABEL,
    ] {
        b.add_arc(s, marker, EPS_LABEL, s).expect("eraser marker:ε");
    }
    b.finish().expect("eraser finish")
}

/// One-state-chain output-only emitter for a literal string (`ε:chᵢ`).
fn output_only_emitter(lit: &str, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
    let chars: Vec<char> = lit.chars().collect();
    let mut b = RustFstBackend::builder();
    let start = b.add_state();
    b.set_start(start).expect("set_start");
    let mut prev = start;
    for ch in &chars {
        let next = b.add_state();
        let label = alpha.intern(&ch.to_string());
        b.add_arc(prev, EPS_LABEL, label, next).expect("emit arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish emitter")
}

/// Two-state transducer with one arc `input:output`.
fn single_arc(input: Label, output: Label) -> DResult<RustFstWrapper> {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).map_err(be)?;
    b.set_final(s1).map_err(be)?;
    b.add_arc(s0, input, output, s1).map_err(be)?;
    b.finish().map_err(be)
}

/// Linear identity acceptor for a label sequence. Test-only (the apply
/// harness builds linear input acceptors); production code uses the
/// transparent / literal builders above.
#[cfg(test)]
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
    b.finish().expect("finish linear")
}

// ===========================================================================
// Tests.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{PhonContext, Spanned};
    use crate::phonrule_eval::apply_phonrule;
    use crate::span::FileId;
    use std::time::Instant;

    use crate::ast::{
        CharClassBody, CharClassDef, DisplayMap, PhonBodyItem, PhonMapArm, PhonMapBody, PhonMapDef,
        PhonMapElse, PhonMapResult, PhonRule, Span,
    };
    use crate::fst::phonrule::map::compile_map;

    fn sp() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }
    fn id(s: &str) -> Spanned<String> {
        Spanned::new(s.to_string(), sp())
    }
    fn lit(s: &str) -> Spanned<String> {
        Spanned::new(s.to_string(), sp())
    }

    /// `a -> b / x _ y`.
    fn toy_base_rule() -> PhonRewriteRule {
        PhonRewriteRule {
            from: PhonPattern::Literal(lit("a")),
            to: PhonReplacement::Literal(lit("b")),
            context: Some(PhonContext {
                left: vec![PhonContextElem::Atom(
                    PhonAtom::Literal(lit("x")),
                    Quantifier::Exact(1),
                )],
                right: vec![PhonContextElem::Atom(
                    PhonAtom::Literal(lit("y")),
                    Quantifier::Exact(1),
                )],
            }),
            span: sp(),
        }
    }

    /// `a -> b / x !V* _ y` with V = {a, e, i}.
    fn toy_negclass_rule() -> PhonRewriteRule {
        PhonRewriteRule {
            from: PhonPattern::Literal(lit("a")),
            to: PhonReplacement::Literal(lit("b")),
            context: Some(PhonContext {
                left: vec![
                    PhonContextElem::Atom(PhonAtom::Literal(lit("x")), Quantifier::Exact(1)),
                    PhonContextElem::Atom(PhonAtom::NegClass(id("V")), Quantifier::Star),
                ],
                right: vec![PhonContextElem::Atom(
                    PhonAtom::Literal(lit("y")),
                    Quantifier::Exact(1),
                )],
            }),
            span: sp(),
        }
    }

    fn as_phonrule(rule: PhonRewriteRule, classes: Vec<CharClassDef>, maps: Vec<PhonMapDef>) -> PhonRule {
        PhonRule {
            name: id("toy"),
            display: DisplayMap::default(),
            derived_from: None,
            syllable: None,
            classes,
            maps,
            body: vec![PhonBodyItem::Rewrite(rule)],
            span: sp(),
        }
    }

    fn class_list(name: &str, members: &[&str]) -> CharClassDef {
        CharClassDef {
            name: id(name),
            body: CharClassBody::List(members.iter().map(|m| lit(m)).collect()),
        }
    }

    /// Pre-intern every char in `corpus` into Σ (closed-alphabet discipline).
    fn alpha_for(corpus: &[&str]) -> PhonruleAlphabet {
        let mut alpha = PhonruleAlphabet::empty();
        for s in corpus {
            for ch in s.chars() {
                if ch == crate::phonrule_eval::BOUNDARY {
                    continue;
                }
                alpha.intern(&ch.to_string());
            }
        }
        alpha
    }

    /// Encode a string to labels: `\0` → boundary label, else intern.
    fn string_to_labels(s: &str, alpha: &mut PhonruleAlphabet) -> Vec<Label> {
        s.chars()
            .map(|ch| {
                if ch == crate::phonrule_eval::BOUNDARY {
                    alpha.boundary_label()
                } else {
                    alpha.intern(&ch.to_string())
                }
            })
            .collect()
    }

    fn labels_to_string(labels: &[Label], alpha: &PhonruleAlphabet) -> String {
        let mut s = String::new();
        for &l in labels {
            if l == alpha.word_start_label() || l == alpha.word_end_label() {
                continue;
            }
            if l == alpha.boundary_label() {
                s.push(crate::phonrule_eval::BOUNDARY);
                continue;
            }
            if let Some(name) = alpha.label_to_str(l) {
                s.push_str(name);
            }
        }
        s
    }

    fn apply_fst_one_pass(
        fst_input_sorted: &RustFstWrapper,
        input: &str,
        alpha: &mut PhonruleAlphabet,
    ) -> String {
        let labels = string_to_labels(input, alpha);
        let acceptor = linear(&labels);
        let left = RustFstBackend::arc_sort_output(&acceptor).expect("sort left");
        let applied = RustFstBackend::compose(&left, fst_input_sorted).expect("compose apply");
        let mut best: Option<Vec<Label>> = None;
        for p in RustFstBackend::paths(&applied).expect("paths").take(4096) {
            best = Some(match best {
                None => p.output,
                Some(prev) if p.output < prev => p.output,
                Some(prev) => prev,
            });
        }
        let out = best.unwrap_or_else(|| panic!("no accepting path for input {:?}", input));
        labels_to_string(&out, alpha)
    }

    /// Apply iteratively to convergence (matching `phonrule_eval`'s loop).
    fn apply_fst(fst: &RustFstWrapper, input: &str, alpha: &mut PhonruleAlphabet) -> String {
        let sorted = RustFstBackend::arc_sort_input(fst).expect("sort rule");
        let mut current = input.to_string();
        for _ in 0..64 {
            let next = apply_fst_one_pass(&sorted, &current, alpha);
            if next == current {
                return next;
            }
            current = next;
        }
        current
    }

    fn class_table_v() -> HashMap<String, Vec<String>> {
        let mut t = HashMap::new();
        t.insert(
            "V".to_string(),
            vec!["a".to_string(), "e".to_string(), "i".to_string()],
        );
        t
    }

    const CORPUS: &[&str] = &[
        "", "xy", "xay", "xby", "xaay", "zxayz", "xaya", "axy", "xayxay",
        "a", "x", "y", "xxayy", "yax", "zzz", "xayay",
    ];

    #[test]
    fn base_rule_byte_identical_to_eval() {
        let rule = toy_base_rule();
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let mut alpha = alpha_for(CORPUS);
        let class_table: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

        let fst = build_directed_replacement(&rule, &mut alpha, &class_table, &map_table)
            .expect("build base rule");

        let mut failures = Vec::new();
        for &input in CORPUS {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input, expected, actual));
            }
        }
        assert!(failures.is_empty(), "base rule disagreed with phonrule_eval: {:?}", failures);
    }

    #[test]
    fn negclass_rule_byte_identical_to_eval() {
        let rule = toy_negclass_rule();
        let phonrule = as_phonrule(rule.clone(), vec![class_list("V", &["a", "e", "i"])], vec![]);
        let corpus: Vec<&str> = CORPUS
            .iter()
            .copied()
            .chain(["xcay", "xccay", "xeay", "xcby", "xcaay", "zxcayz"])
            .collect();
        let mut alpha = alpha_for(&corpus);
        let class_table = class_table_v();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

        let fst = build_directed_replacement(&rule, &mut alpha, &class_table, &map_table)
            .expect("build negclass rule");

        let mut failures = Vec::new();
        for &input in &corpus {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.to_string(), expected, actual));
            }
        }
        assert!(failures.is_empty(), "negclass rule disagreed with phonrule_eval: {:?}", failures);
    }

    #[test]
    fn xay_yields_only_xby_not_passthrough() {
        let rule = toy_base_rule();
        let mut alpha = alpha_for(&["xay", "xby"]);
        let class_table: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &class_table, &map_table)
            .expect("build");

        let labels: Vec<Label> = "xay".chars().map(|c| alpha.intern(&c.to_string())).collect();
        let acceptor = linear(&labels);
        let left = RustFstBackend::arc_sort_output(&acceptor).expect("sort");
        let right = RustFstBackend::arc_sort_input(&fst).expect("sort");
        let applied = RustFstBackend::compose(&left, &right).expect("compose");
        let mut outputs = std::collections::HashSet::new();
        for p in RustFstBackend::paths(&applied).expect("paths").take(4096) {
            let s: String = p.output.iter().filter_map(|&l| alpha.label_to_str(l)).collect();
            outputs.insert(s);
        }
        assert_eq!(
            outputs,
            ["xby".to_string()].into_iter().collect(),
            "xay must produce ONLY xby (no passthrough); got {:?}",
            outputs
        );
    }

    #[test]
    fn compile_is_subsecond_both_variants() {
        let class_table = class_table_v();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();

        let t0 = Instant::now();
        let mut alpha_b = alpha_for(CORPUS);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let _ = build_directed_replacement(&toy_base_rule(), &mut alpha_b, &empty, &map_table)
            .expect("base");
        let base_elapsed = t0.elapsed();

        let t1 = Instant::now();
        let mut alpha_n = alpha_for(CORPUS);
        for c in ["c", "d", "f", "g", "e", "i"] {
            alpha_n.intern(c);
        }
        let _ = build_directed_replacement(&toy_negclass_rule(), &mut alpha_n, &class_table, &map_table)
            .expect("negclass");
        let neg_elapsed = t1.elapsed();

        assert!(base_elapsed.as_millis() < 1000, "base compile not sub-second: {:?}", base_elapsed);
        assert!(neg_elapsed.as_millis() < 1000, "negclass compile not sub-second: {:?}", neg_elapsed);
        eprintln!("directed_replace compile times: base={:?} negclass={:?}", base_elapsed, neg_elapsed);
    }

    // -----------------------------------------------------------------------
    // Deterministic fuzz.
    // -----------------------------------------------------------------------

    struct Lcg {
        state: u64,
    }
    impl Lcg {
        fn new(seed: &str) -> Self {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in seed.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            Self { state: h.max(1) }
        }
        fn next_u64(&mut self) -> u64 {
            self.state = self
                .state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.state
        }
    }

    fn fuzz_inputs(seed: &str, chars: &[char], count: usize, max_len: usize) -> Vec<String> {
        let mut rng = Lcg::new(seed);
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let len = (rng.next_u64() as usize) % (max_len + 1);
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                s.push(chars[(rng.next_u64() as usize) % chars.len()]);
            }
            out.push(s);
        }
        out
    }

    #[test]
    fn base_rule_fuzz_byte_identical() {
        let rule = toy_base_rule();
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let chars = ['x', 'y', 'a', 'b', 'z'];
        let inputs = fuzz_inputs("dr-base", &chars, 600, 8);

        let mut alpha = alpha_for(&["x", "y", "a", "b", "z"]);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table)
            .expect("build base");

        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "base fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    #[test]
    fn negclass_rule_fuzz_byte_identical() {
        let rule = toy_negclass_rule();
        let phonrule = as_phonrule(rule.clone(), vec![class_list("V", &["a", "e", "i"])], vec![]);
        let chars = ['x', 'y', 'a', 'b', 'c', 'd', 'e', 'i'];
        let inputs = fuzz_inputs("dr-negclass", &chars, 600, 8);

        let mut alpha = alpha_for(&["x", "y", "a", "b", "c", "d", "e", "i"]);
        let class_table = class_table_v();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &class_table, &map_table)
            .expect("build negclass");

        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "negclass fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    // -----------------------------------------------------------------------
    // F2c4-#3: distinguishing multi-target-licensing cases.
    //
    // These are the cases where CONSUMING context (the old fold-into-UPPER
    // construction) gets it WRONG: ONE context char must license MULTIPLE
    // adjacent targets in a single pass.
    // -----------------------------------------------------------------------

    /// `a -> b / x .* _` — a single `x` plus any run licenses EVERY following
    /// `a` in a single pass (the left context `x .*` matches the whole prefix
    /// for each target). Consuming context would consume the `x` on the first
    /// match and fail to license the second.
    fn multi_license_rule() -> PhonRewriteRule {
        PhonRewriteRule {
            from: PhonPattern::Literal(lit("a")),
            to: PhonReplacement::Literal(lit("b")),
            context: Some(PhonContext {
                left: vec![
                    PhonContextElem::Atom(PhonAtom::Literal(lit("x")), Quantifier::Exact(1)),
                    PhonContextElem::Atom(PhonAtom::Wildcard, Quantifier::Star),
                ],
                right: vec![],
            }),
            span: sp(),
        }
    }

    #[test]
    fn multi_target_left_license_byte_identical() {
        let rule = multi_license_rule();
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let corpus: Vec<&str> = vec![
            "", "x", "a", "xa", "xaa", "xaaa", "xaba", "xyaa", "axa", "xaxa",
            "zxaa", "xzaza", "aax",
        ];
        let mut alpha = alpha_for(&corpus);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table)
            .expect("build multi-license");

        let mut failures = Vec::new();
        for &input in &corpus {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.to_string(), expected, actual));
            }
        }
        assert!(failures.is_empty(), "multi-license disagreed: {:?}", failures);

        // Direct assertion of the distinguishing case: `xaa` → BOTH a's
        // licensed by the single x → `xbb` (one pass).
        let mut a2 = alpha_for(&["xaa", "xbb"]);
        let f2 = build_directed_replacement(&rule, &mut a2, &empty, &map_table).unwrap();
        let sorted = RustFstBackend::arc_sort_input(&f2).unwrap();
        assert_eq!(apply_fst_one_pass(&sorted, "xaa", &mut a2), "xbb");
    }

    #[test]
    fn multi_target_left_license_fuzz() {
        let rule = multi_license_rule();
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let chars = ['x', 'a', 'b', 'z'];
        let inputs = fuzz_inputs("dr-multi", &chars, 600, 8);
        let mut alpha = alpha_for(&["x", "a", "b", "z"]);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table)
            .expect("build multi");

        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "multi fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    // -----------------------------------------------------------------------
    // F2c4-#3 MILESTONE: real Turkish vowel-harmony low-vowel rule.
    //   low -> to_back_low / back !V* + !V* _
    // -----------------------------------------------------------------------

    /// Vowel + consonant inventory for the milestone test (the real Turkish
    /// vowels plus a small consonant set to drive the `!V*` runs).
    const TR_VOWELS: &[&str] = &["a", "e", "ı", "i", "o", "ö", "u", "ü"];
    const TR_CONS: &[&str] = &["k", "l", "r", "t", "n"];

    fn turkish_harmony_rule() -> PhonRewriteRule {
        PhonRewriteRule {
            from: PhonPattern::Class(id("low")),
            to: PhonReplacement::Map(id("to_back_low")),
            context: Some(PhonContext {
                left: vec![
                    PhonContextElem::Atom(PhonAtom::Class(id("back")), Quantifier::Exact(1)),
                    PhonContextElem::Atom(PhonAtom::NegClass(id("V")), Quantifier::Star),
                    PhonContextElem::Boundary,
                    PhonContextElem::Atom(PhonAtom::NegClass(id("V")), Quantifier::Star),
                ],
                right: vec![],
            }),
            span: sp(),
        }
    }

    fn turkish_classes() -> Vec<CharClassDef> {
        vec![
            class_list("front", &["e", "i", "ö", "ü"]),
            class_list("back", &["a", "ı", "o", "u"]),
            CharClassDef {
                name: id("V"),
                body: CharClassBody::Union(vec![id("front"), id("back")]),
            },
            class_list("low", &["e", "a", "ö", "o"]),
        ]
    }

    fn turkish_map() -> PhonMapDef {
        PhonMapDef {
            name: id("to_back_low"),
            param: id("c"),
            body: PhonMapBody::Match {
                arms: vec![
                    PhonMapArm { from: lit("e"), to: PhonMapResult::Literal(lit("a")) },
                    PhonMapArm { from: lit("ö"), to: PhonMapResult::Literal(lit("a")) },
                ],
                else_arm: Some(PhonMapElse::Var(id("c"))),
            },
        }
    }

    /// Build the class member table (V resolved to front ∪ back) for the
    /// directed-replace entry point.
    fn turkish_class_table() -> HashMap<String, Vec<String>> {
        let mut t = HashMap::new();
        t.insert("front".to_string(), vec!["e", "i", "ö", "ü"].into_iter().map(String::from).collect());
        t.insert("back".to_string(), vec!["a", "ı", "o", "u"].into_iter().map(String::from).collect());
        t.insert(
            "V".to_string(),
            vec!["e", "i", "ö", "ü", "a", "ı", "o", "u"].into_iter().map(String::from).collect(),
        );
        t.insert("low".to_string(), vec!["e", "a", "ö", "o"].into_iter().map(String::from).collect());
        t
    }

    fn turkish_alpha() -> PhonruleAlphabet {
        let mut alpha = PhonruleAlphabet::empty();
        for v in TR_VOWELS {
            alpha.intern(v);
        }
        for c in TR_CONS {
            alpha.intern(c);
        }
        alpha
    }

    fn build_turkish_fst(alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
        let map_def = turkish_map();
        let mut map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        // Intern the map's outputs into Σ before compiling the map (the map
        // builder snapshots Σ for its else arcs).
        let map_fst = compile_map(&map_def, alpha);
        map_table.insert("to_back_low".to_string(), map_fst);
        let class_table = turkish_class_table();
        build_directed_replacement(&turkish_harmony_rule(), alpha, &class_table, &map_table)
            .expect("build turkish harmony")
    }

    #[test]
    fn turkish_harmony_compiles_subsecond() {
        let mut alpha = turkish_alpha();
        let t0 = Instant::now();
        let _fst = build_turkish_fst(&mut alpha);
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "turkish harmony compile not sub-second: {:?} (Strategy B could not do it in 10 min)",
            elapsed
        );
        eprintln!("turkish harmony compile time: {:?}", elapsed);
    }

    #[test]
    fn turkish_harmony_byte_identical_curated() {
        let mut alpha = turkish_alpha();
        let fst = build_turkish_fst(&mut alpha);
        let phonrule = as_phonrule(turkish_harmony_rule(), turkish_classes(), vec![turkish_map()]);

        let b = "\0";
        let corpus: Vec<String> = vec![
            "".into(),
            "a".into(),
            format!("yol{}ler", b).replace('y', "k"),   // back o, +, low e → a
            format!("o{}e", b),                          // back o, +, low e → "o\0a"
            format!("a{}e", b),                          // back a, +, low e → "a\0a"
            format!("e{}e", b),                          // front e, no back → unchanged
            format!("a{}ö", b),                          // back a, +, low ö → "a\0a"
            format!("a{}i", b),                          // i is high, not low → unchanged
            format!("o{}len{}e", b, b),                  // run over two morphemes
            format!("ak{}le", b),                        // back a, !V*=k, +, !V*=l, low e → a
            format!("a{}klle", b),                       // back a, +, !V*=kll, low e → a
            format!("o{}e{}e", b, b),                     // cascade across boundaries
            format!("e{}a{}e", b, b),                    // front then back, only after back licensed
        ];

        let mut failures = Vec::new();
        for input in &corpus {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(failures.is_empty(), "turkish curated disagreed: {:?}", failures);
    }

    #[test]
    fn turkish_harmony_fuzz_byte_identical() {
        let mut alpha = turkish_alpha();
        let fst = build_turkish_fst(&mut alpha);
        let phonrule = as_phonrule(turkish_harmony_rule(), turkish_classes(), vec![turkish_map()]);

        // Alphabet for fuzz: vowels, consonants, and boundary.
        let chars: Vec<char> = TR_VOWELS
            .iter()
            .chain(TR_CONS.iter())
            .map(|s| s.chars().next().unwrap())
            .chain(['\0'])
            .collect();
        let inputs = fuzz_inputs("dr-turkish", &chars, 600, 9);

        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "turkish fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    // -----------------------------------------------------------------------
    // F2c4-#4 MILESTONE: NotInner longest-match on AMBIGUOUS-LENGTH LHS.
    //
    // Each LHS below matches more than one length from the same start, so the
    // NotInner filter (Karttunen §3 eq. 8) must pick the LONGEST. Every case is
    // asserted byte-identical to `phonrule_eval`.
    // -----------------------------------------------------------------------

    /// Build a `Range`-LHS rewrite rule: `from_elems -> to / context`.
    fn range_rule(
        from_elems: Vec<PhonContextElem>,
        to: PhonReplacement,
        context: Option<PhonContext>,
    ) -> PhonRewriteRule {
        PhonRewriteRule {
            from: PhonPattern::Range(from_elems),
            to,
            context,
            span: sp(),
        }
    }

    fn atom_lit(s: &str, q: Quantifier) -> PhonContextElem {
        PhonContextElem::Atom(PhonAtom::Literal(lit(s)), q)
    }

    /// Run a corpus byte-identical against `phonrule_eval`, collecting failures.
    fn check_corpus(
        rule: &PhonRewriteRule,
        classes: Vec<CharClassDef>,
        class_table: &HashMap<String, Vec<String>>,
        corpus: &[&str],
        seed_chars: &[&str],
    ) {
        let phonrule = as_phonrule(rule.clone(), classes, vec![]);
        let mut alpha = alpha_for(seed_chars);
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(rule, &mut alpha, class_table, &map_table)
            .expect("build range rule");
        let mut failures = Vec::new();
        for &input in corpus {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.to_string(), expected, actual));
            }
        }
        assert!(failures.is_empty(), "range rule disagreed with eval: {:?}", failures);
    }

    /// `a+ -> x`: input `aaa` must rewrite the WHOLE run to a single `x`
    /// (longest), NOT `xaa` (shortest) or `xxx` (greedy-shortest repeated).
    #[test]
    fn a_plus_to_x_longest() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Plus)],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let corpus = &[
            "", "a", "aa", "aaa", "aaaa", "b", "ab", "ba", "aba", "baab",
            "aabaaa", "abba", "aaab",
        ];
        check_corpus(&rule, vec![], &empty, corpus, &["a", "b", "x"]);

        // Direct longest assertion.
        let mut alpha = alpha_for(&["a", "b", "x"]);
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap();
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        assert_eq!(apply_fst(&fst, "aaa", &mut alpha), "x");
        assert_eq!(apply_phonrule("aaa", &phonrule), "x");
    }

    /// `a+ -> x / b _`: longest match plus a left context.
    #[test]
    fn a_plus_to_x_with_left_context() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Plus)],
            PhonReplacement::Literal(lit("x")),
            Some(PhonContext {
                left: vec![atom_lit("b", Quantifier::Exact(1))],
                right: vec![],
            }),
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let corpus = &[
            "", "a", "aa", "baa", "baaa", "aab", "baab", "bbaa", "ab", "ba",
            "baaab", "aaa", "bab",
        ];
        check_corpus(&rule, vec![], &empty, corpus, &["a", "b", "x"]);
    }

    /// `(ab|abc) -> x`: unequal-length alternation. **Out of scope** for this
    /// milestone — and note `phonrule_eval`'s `Alt` is FIRST-alternative-wins,
    /// not longest (it would map `abc` → `xc` via the length-2 `ab` arm), so a
    /// longest-match FST would *disagree* with the oracle here regardless.
    /// Compiling it returns `UnsupportedShape` rather than miscompiling.
    #[test]
    fn unequal_alternation_unsupported() {
        let rule = range_rule(
            vec![PhonContextElem::Atom(
                PhonAtom::Alt(vec![atom_lit("ab", Quantifier::Exact(1)), atom_lit("abc", Quantifier::Exact(1))]),
                Quantifier::Exact(1),
            )],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let mut alpha = alpha_for(&["a", "b", "c", "x", "z"]);
        let err = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap_err();
        assert!(matches!(err, DirectedReplaceError::UnsupportedShape(_)), "got {:?}", err);
    }

    /// `aa -> x` overlap: a fixed-width 2-symbol LHS can self-overlap (`aaa`
    /// admits `aa` at offsets 0 and 1), which needs the `NotLeftmost` filter
    /// this milestone does not build. **Out of scope** → `UnsupportedShape`.
    #[test]
    fn aa_overlap_unsupported() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Exact(1)), atom_lit("a", Quantifier::Exact(1))],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let mut alpha = alpha_for(&["a", "b", "x"]);
        let err = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap_err();
        assert!(matches!(err, DirectedReplaceError::UnsupportedShape(_)), "got {:?}", err);
    }

    /// `literal_self_overlaps` recognises borders correctly.
    #[test]
    fn self_overlap_predicate() {
        // No proper border ⇒ no overlap.
        assert!(!literal_self_overlaps(""));
        assert!(!literal_self_overlaps("a"));
        assert!(!literal_self_overlaps("ab"));
        assert!(!literal_self_overlaps("abc"));
        assert!(!literal_self_overlaps("xy"));
        // Has a proper border ⇒ overlaps.
        assert!(literal_self_overlaps("aa"));
        assert!(literal_self_overlaps("aaa"));
        assert!(literal_self_overlaps("aba"));
        assert!(literal_self_overlaps("abab")); // border "ab"
        assert!(literal_self_overlaps("abcab")); // border "ab"
    }

    /// `ab -> xy`: a NON-overlapping multi-char literal LHS stays supported by
    /// Strategy A and is byte-identical to eval. (Mirrors the production
    /// `val_multi_char_lhs_ab_to_xy` validation case at the engine level.)
    #[test]
    fn nonoverlapping_multichar_literal_supported_byte_identical() {
        let rule = PhonRewriteRule {
            from: PhonPattern::Literal(lit("ab")),
            to: PhonReplacement::Literal(lit("xy")),
            context: None,
            span: sp(),
        };
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let corpus = &[
            "", "a", "b", "ab", "ba", "abab", "aab", "abb", "ababab", "aabb", "zzz",
        ];
        let mut alpha = alpha_for(&["a", "b", "x", "y", "z"]);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        // Must compile (supported), and be byte-identical.
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table)
            .expect("ab -> xy must be supported by Strategy A");
        let mut failures = Vec::new();
        for &input in corpus {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.to_string(), expected, actual));
            }
        }
        assert!(failures.is_empty(), "ab -> xy disagreed with eval: {:?}", failures);
    }

    /// `aa -> x` and `aba -> x`: self-overlapping multi-char LITERAL LHS. These
    /// would MISCOMPILE as `Fixed` (no NotLeftmost filter) so `classify_lhs`
    /// must return `UnsupportedShape` — letting `compile_rewrite_rule` fall back
    /// to Strategy B. Verify the rejection here; the byte-identical fallback is
    /// validated through the production path in `replace.rs` / validation_tests.
    #[test]
    fn self_overlapping_multichar_literal_unsupported() {
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        for word in ["aa", "aba"] {
            let rule = PhonRewriteRule {
                from: PhonPattern::Literal(lit(word)),
                to: PhonReplacement::Literal(lit("x")),
                context: None,
                span: sp(),
            };
            let mut alpha = alpha_for(&["a", "b", "x"]);
            let err = build_directed_replacement(&rule, &mut alpha, &empty, &map_table)
                .unwrap_err();
            assert!(
                matches!(err, DirectedReplaceError::UnsupportedShape(_)),
                "self-overlapping literal {:?} must be UnsupportedShape, got {:?}",
                word,
                err
            );
        }
    }

    /// `a{2,3} -> x`: a *bounded* single-atom run caps/floors the match below
    /// the maximal run, which the run model does not capture. **Out of scope**
    /// → `UnsupportedShape` (only `+`/`*`/`{n,}` with n≤1 are run-exact).
    #[test]
    fn bounded_range_unsupported() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Range(2, 3))],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let mut alpha = alpha_for(&["a", "b", "x"]);
        let err = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap_err();
        assert!(matches!(err, DirectedReplaceError::UnsupportedShape(_)), "got {:?}", err);
    }

    /// `a+ -> "" ` (deletion of a whole run, variable length).
    #[test]
    fn a_plus_deletion_longest() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Plus)],
            PhonReplacement::Null,
            None,
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let corpus = &["", "a", "aa", "aaa", "bab", "baab", "aba", "abba"];
        check_corpus(&rule, vec![], &empty, corpus, &["a", "b"]);
    }

    /// Class-quantified ambiguous LHS: `V+ -> x` with `V = {a,e,i}`. A run of
    /// vowels (mixed) collapses to a single `x` (longest).
    #[test]
    fn class_plus_longest() {
        let rule = range_rule(
            vec![PhonContextElem::Atom(PhonAtom::Class(id("V")), Quantifier::Plus)],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let class_table = class_table_v();
        let corpus = &[
            "", "a", "ae", "aei", "b", "ab", "bae", "baeib", "aeb", "bbaei",
        ];
        check_corpus(&rule, vec![class_list("V", &["a", "e", "i"])], &class_table, corpus, &["a", "e", "i", "b", "x"]);
    }

    /// 600-input deterministic fuzz for `a+ -> x` over a small alphabet,
    /// byte-identical to eval.
    #[test]
    fn a_plus_fuzz_byte_identical() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Plus)],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let chars = ['a', 'b', 'x'];
        let inputs = fuzz_inputs("dr-aplus", &chars, 600, 9);
        let mut alpha = alpha_for(&["a", "b", "x"]);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap();
        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "a+ fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    /// `a{1,} -> x` (= `a+` written as `AtLeast(1)`): a second 600-input fuzz
    /// over the unbounded-run path, byte-identical to eval.
    #[test]
    fn atleast_run_fuzz_byte_identical() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::AtLeast(1))],
            PhonReplacement::Literal(lit("x")),
            None,
        );
        let phonrule = as_phonrule(rule.clone(), vec![], vec![]);
        let chars = ['a', 'b', 'x', 'z'];
        let inputs = fuzz_inputs("dr-atleast", &chars, 600, 9);
        let mut alpha = alpha_for(&["a", "b", "x", "z"]);
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let fst = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap();
        let mut failures = Vec::new();
        for input in &inputs {
            let expected = apply_phonrule(input, &phonrule);
            let actual = apply_fst(&fst, input, &mut alpha);
            if expected != actual {
                failures.push((input.clone(), expected, actual));
            }
        }
        assert!(
            failures.is_empty(),
            "atleast fuzz: {} disagreements, first few: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    /// NotInner compile stays sub-second on an ambiguous-length LHS.
    #[test]
    fn notinner_compile_subsecond() {
        let rule = range_rule(
            vec![atom_lit("a", Quantifier::Plus)],
            PhonReplacement::Literal(lit("x")),
            Some(PhonContext {
                left: vec![atom_lit("b", Quantifier::Exact(1))],
                right: vec![],
            }),
        );
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        let map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        let mut alpha = alpha_for(&["a", "b", "x"]);
        let t0 = Instant::now();
        let _ = build_directed_replacement(&rule, &mut alpha, &empty, &map_table).unwrap();
        let elapsed = t0.elapsed();
        assert!(elapsed.as_millis() < 1000, "NotInner compile not sub-second: {:?}", elapsed);
        eprintln!("NotInner (a+ -> x / b _) compile time: {:?}", elapsed);
    }
}
