//! `PhonContextElem` → acceptor FST (F2b step 4).
//!
//! Per the F2 plan §7 step 4 ("Context elem compilation") and §2.1 rows
//! 7–13: each [`PhonContextElem`] becomes a small **acceptor** (identity
//! transducer where input == output on every arc) over the
//! [`PhonruleAlphabet`]. F2c will assemble these per-position acceptors
//! into the Karttunen `Mark`/`Constraint`/`Replace`/`Unmark` chain; F2b
//! delivers only the building blocks.
//!
//! ## Brief
//!
//! Reference:
//! - `docs/proposals/f2-kaplan-kay-plan.md` §2.4 (anchor protocol),
//!   §3 (Karttunen construction these pieces feed into),
//!   §7 step 4, §10 non-goals (syllable contexts).
//! - `src/phonrule_eval.rs` — `match_seq` (line 614), `match_atom_quant`
//!   (line 691), `consume_atom` (line 751), `consume_one_elem`
//!   (line 912). These are the semantics F2b reproduces.
//!
//! ## Reading order
//!
//! 1. [`compile_context_elem`] — the single-elem compiler; one [`RustFstWrapper`]
//!    per `PhonContextElem`.
//! 2. [`compile_context_sequence`] — the public entry point; concats elem
//!    acceptors in order.
//! 3. [`compile_pattern_sequence`] — public entry point for LHS patterns.
//!    Same shape as `compile_context_sequence`; see below.
//!
//! ## Markers (`+`, `^`, `$`) — what F2b emits vs what eval does
//!
//! ### `^` / `$` — word-edge anchors
//!
//! In `phonrule_eval`, `^` matches "cursor == 0" and `$` matches
//! "cursor >= chars.len()" (lines 655–660). These are **zero-width**
//! position predicates on the raw character array.
//!
//! In the FST, position predicates aren't directly representable.
//! Standard Karttunen-style trick: turn the position into a
//! **symbol** that the caller introduces at the appropriate edge of
//! the input stream before applying the constraint, then strips after.
//!
//! F2b emits a single-arc acceptor that consumes
//! [`PhonruleAlphabet::word_start_label`] (resp. `word_end_label`).
//! It does not consume any "real" symbol; it is meaningful only when
//! the caller (F2c) has actually prepended `WORD_START` / appended
//! `WORD_END` to the input. The Karttunen construction wraps the rule
//! transducer in `prepend_marker ∘ rule ∘ strip_marker`, so the markers
//! are present at constraint-eval time and gone at output time.
//!
//! This is the standard symbol-stream-marker trick; the plan §2.4
//! describes it in detail.
//!
//! ### `+` — boundary
//!
//! In eval `+` is *both* a literal boundary marker (`\0`,
//! `phonrule_eval.rs:21`) inserted between morphemes by the compose
//! chain, **and** a synonym for the word edge (lines 635–653). At a
//! given direction, eval's `Boundary` succeeds when:
//!
//!  * the immediate stream char is `BOUNDARY` and is consumed, OR
//!  * the cursor is at the edge.
//!
//! Per F2c3 (the obligatory-constraint stage), `Boundary` now compiles
//! to the **union of three single-arc acceptors**:
//!
//!   `boundary_label ∪ word_start_label ∪ word_end_label`
//!
//! reproducing the eval's "boundary OR edge" semantics
//! (`phonrule_eval.rs:638-653`). The Karttunen apply-driver (F2c5) is
//! still expected to:
//!
//!   1. Insert `BOUNDARY_LABEL` between slot fills via the compose
//!      chain (mirroring `evaluate_compose` in `inflection_eval.rs`).
//!   2. Wrap the input in `WORD_START_LABEL ... WORD_END_LABEL` before
//!      applying the rule transducer (and strip them after).
//!
//! With those two conditions, the union acceptor matches `+` wherever
//! the eval would: at an in-stream boundary marker, at the start of
//! the wrapped input, or at the end. The union approach is local —
//! the alternative ("wrap input with a `<bdy>` marker at the edges too,
//! so a single-arc consumer of BOUNDARY_LABEL is sufficient") would
//! force every FST consumer to know about the bracketing convention.
//!
//! ### History
//!
//! F2b originally emitted a single-arc consumer of `BOUNDARY_LABEL`
//! and deferred the union to F2c. F2c3 took the union path because
//! the obligatory-constraint construction inspects the L/R context
//! acceptors directly: a union there is local and simpler than
//! requiring the runtime apply driver to insert extra edge markers.
//!
//! ### `+` vs literal "+"
//!
//! The boundary marker `+` in phonrule source is the AST variant
//! [`PhonContextElem::Boundary`]. A literal `"+"` character in a rewrite
//! pattern would be `PhonAtom::Literal(StringLit("+"))`, a *different*
//! AST variant. They compile to different labels — `BOUNDARY_LABEL`
//! (label 1, reserved) and a user-interned label for the `'+'`
//! character (≥ 16). The label spaces are disjoint so the two cannot
//! be confused at the FST level even though they share a glyph in the
//! source language.
//!
//! ## Quantifiers
//!
//! `PhonContextElem::Atom(atom, quant)` carries a [`Quantifier`]. F2b
//! compiles the atom to a single-position acceptor, then wraps it per
//! the quantifier using [`FstBackend`]'s `closure_*` /
//! `closure_bounded`. `Quantifier::Exact(1)` is a no-op (the un-quantified
//! v1 form).
//!
//! ## Non-goals (F2b)
//!
//! Per the F2 plan §10 non-goal 5, **syllable-aware contexts**
//! (`SylHead` / `SylTail` / `SylIndex`, and the
//! [`PhonAtom::SylBlock`] atom) are not compiled. F2b rejects them
//! with [`ContextCompileError::SyllableElemUnsupported`]. F2c / a
//! follow-up sub-project will compile syllabification to FSTs.

use std::collections::HashMap;

use crate::ast::{PhonAtom, PhonContextElem, Quantifier};

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBackend, FstBuilder, Label};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::class::{compile_class, compile_class_complement, ClassCompileError};
use crate::ast::CharClassDef;

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by context-elem / pattern compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextCompileError {
    /// A `Class` / `NegClass` referenced a name not in `class_table`.
    /// Callers compile classes in dependency order first, then pass the
    /// populated table to context compilation.
    UnknownClass { name: String },
    /// Class-level compile error bubbled up from
    /// [`compile_class`] / [`compile_class_complement`].
    Class(ClassCompileError),
    /// A syllable-aware element appeared in a context or pattern. F2b
    /// does not compile these (plan §10 non-goal 5); callers must fall
    /// back to `phonrule_eval` for any phonrule whose contexts mention
    /// `%syl<head>%`, `%syl<tail>%`, `%syl<#N>%`, or `%syl[ ... ]%`.
    SyllableElemUnsupported { kind: &'static str },
}

impl std::fmt::Display for ContextCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextCompileError::UnknownClass { name } => {
                write!(f, "context elem references undefined or not-yet-compiled class '{}'", name)
            }
            ContextCompileError::Class(e) => write!(f, "{}", e),
            ContextCompileError::SyllableElemUnsupported { kind } => write!(
                f,
                "syllable-aware context element '{}' is not yet supported in the FST engine \
                 (plan §10 non-goal 5; use phonrule_eval as fallback)",
                kind
            ),
        }
    }
}

impl std::error::Error for ContextCompileError {}

impl From<ClassCompileError> for ContextCompileError {
    fn from(e: ClassCompileError) -> Self {
        ContextCompileError::Class(e)
    }
}

// ---------------------------------------------------------------------------
// Public entry points.
// ---------------------------------------------------------------------------

/// Compile a context elem sequence into a single concatenated acceptor.
///
/// Each elem becomes a small acceptor via [`compile_context_elem`]; the
/// pieces are then concatenated left-to-right with [`FstBackend::concat`].
///
/// An empty sequence produces a one-state ε-acceptor (start == final, no
/// arcs). This is the language `{ε}` — matches at any position without
/// consuming, mirroring eval's "no elements left, success" base case
/// (`phonrule_eval.rs:629-631`).
///
/// `class_table` must already contain every class the sequence references
/// (classes are compiled before contexts in F2's compilation order; F2a's
/// `compile_class` is the producer). Forward refs surface as
/// [`ContextCompileError::UnknownClass`].
///
/// Member symbols of `Literal` / `Alt` / `Atom(Literal)` are interned
/// into `alpha`; new symbols extend Σ.
pub fn compile_context_sequence(
    elems: &[PhonContextElem],
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ContextCompileError> {
    // BOUNDARY transparency: per `phonrule_eval`'s context matcher,
    // `\0` (BOUNDARY) chars are transparently skipped:
    //
    //   * Before each Atom (via `segment_at` /
    //     `consume_literal`, `phonrule_eval.rs:1059, 1086`).
    //   * NOT before a `Boundary` (`+`) elem — that elem checks the
    //     current cursor char directly (`phonrule_eval.rs:638`).
    //   * NOT before zero-width anchors (`^`, `$`, syllable elems).
    //
    // We replicate this by:
    //
    //   * Prefixing each Atom with `BOUNDARY*` (allows zero or more
    //     boundary chars to be transparently skipped before the
    //     atom matches).
    //   * Prefixing Boundary/anchor elems with no skip.
    //   * Appending a trailing `BOUNDARY*` to the whole sequence so
    //     BDY chars between L's last match and LHS-start (or LHS-end
    //     and R's first match) are absorbed.
    //
    // Empty sequence still emits a `BOUNDARY*` so the constraint
    // can absorb BDY chars at the L/R boundary positions.
    let bdy_star = build_boundary_star(alpha);
    if elems.is_empty() {
        return Ok(bdy_star);
    }
    let mut acc: Option<RustFstWrapper> = None;
    for elem in elems {
        let next = compile_context_elem(elem, alpha, class_table)?;
        let wrapped = if elem_skips_bdy_before(elem) {
            RustFstBackend::concat(&bdy_star, &next)
                .expect("concat BDY* · elem")
        } else {
            next
        };
        acc = Some(match acc {
            None => wrapped,
            Some(prev) => RustFstBackend::concat(&prev, &wrapped)
                .expect("concat acc · wrapped"),
        });
    }
    // Append trailing BOUNDARY* to absorb BDY chars between the
    // last consumed context elem and the LHS edge.
    let acc = acc.expect("at least one elem");
    Ok(RustFstBackend::concat(&acc, &bdy_star).expect("concat acc · BDY*"))
}

/// Whether the elem's matcher skips BDY chars before consuming.
///
/// True for Atoms (class / literal / wildcard / alternation);
/// false for Boundary and zero-width anchors. Mirrors
/// `phonrule_eval::match_seq` (`phonrule_eval.rs:633-685`).
fn elem_skips_bdy_before(elem: &PhonContextElem) -> bool {
    matches!(elem, PhonContextElem::Atom(_, _))
}

/// Compile a pattern elem sequence (the LHS `A` in `A -> B / L _ R`) into
/// a single concatenated acceptor.
///
/// In the AST, the LHS of a rewrite rule is [`crate::ast::PhonPattern`],
/// whose `Range` variant carries a `Vec<PhonContextElem>` (see
/// `ast.rs:470`). `PhonContextElem` is therefore the unified element
/// shape; this function is a thin wrapper over
/// [`compile_context_sequence`] with the same semantics.
///
/// Eval allows the same elem set on the LHS (anchors `+`/`^`/`$` are
/// admitted "rare, but the grammar admits them inside `%syl[...]%`
/// blocks / alternations" — `phonrule_eval.rs:1021-1037`), so this
/// function does not restrict the variant set. Syllable elems are
/// rejected via the same error channel.
///
/// (The `PhonPattern::Class` / `PhonPattern::Literal` variants used by
/// v1 single-segment LHS are handled one layer up — F2c will wrap them
/// as singleton sequences before calling this function.)
pub fn compile_pattern_sequence(
    elems: &[PhonContextElem],
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ContextCompileError> {
    compile_context_sequence(elems, alpha, class_table)
}

// ---------------------------------------------------------------------------
// Single-elem compilation.
// ---------------------------------------------------------------------------

/// Compile one [`PhonContextElem`] to a small acceptor.
///
/// Per the brief: zero-width anchors (`^`, `$`) and `+` become
/// single-arc consumers of their reserved label; `Atom(...)` dispatches
/// to per-atom compilation and then wraps in the quantifier; syllable
/// elems error out.
pub fn compile_context_elem(
    elem: &PhonContextElem,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ContextCompileError> {
    match elem {
        PhonContextElem::Boundary => Ok(boundary_union_acceptor(alpha)),
        PhonContextElem::WordStart => Ok(single_label_acceptor(alpha.word_start_label())),
        PhonContextElem::WordEnd => Ok(single_label_acceptor(alpha.word_end_label())),
        PhonContextElem::SylHead => Err(ContextCompileError::SyllableElemUnsupported {
            kind: "%syl<head>%",
        }),
        PhonContextElem::SylTail => Err(ContextCompileError::SyllableElemUnsupported {
            kind: "%syl<tail>%",
        }),
        PhonContextElem::SylIndex(_) => Err(ContextCompileError::SyllableElemUnsupported {
            kind: "%syl<#N>%",
        }),
        PhonContextElem::Atom(atom, quant) => {
            let base = compile_atom(atom, alpha, class_table)?;
            apply_quantifier(base, *quant)
        }
    }
}

// ---------------------------------------------------------------------------
// Atom compilation.
// ---------------------------------------------------------------------------

/// Compile a [`PhonAtom`] to a single-position acceptor (un-quantified).
fn compile_atom(
    atom: &PhonAtom,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ContextCompileError> {
    match atom {
        // Class / NegClass: delegate to F2a. The class FST is reused
        // directly when already compiled (most common path); if absent
        // from `class_table`, that's a hard error. We do NOT
        // recompile the CharClassDef ourselves here because we don't
        // have the AST node — class_table is canonical.
        //
        // Note: for `NegClass` we cannot rebuild the complement from
        // the cached identity acceptor without the original member set
        // (the complement depends on Σ at compile time, see F2a's
        // class-complement caveat). The fix: callers pre-register the
        // *complement* acceptor under a name like "!<class>" alongside
        // the positive class. F2a's `compile_class_complement` is the
        // producer; F2c's pipeline will populate both forms. For F2b
        // tests we do exactly that.
        PhonAtom::Class(ident) => class_table
            .get(&ident.node)
            .cloned()
            .ok_or(ContextCompileError::UnknownClass {
                name: ident.node.clone(),
            }),
        PhonAtom::NegClass(ident) => {
            let key = neg_class_key(&ident.node);
            class_table.get(&key).cloned().ok_or({
                ContextCompileError::UnknownClass { name: key }
            })
        }
        PhonAtom::Literal(lit) => Ok(compile_literal(&lit.node, alpha)),
        PhonAtom::Wildcard => Ok(compile_wildcard(alpha)),
        PhonAtom::Alt(alts) => compile_alternation(alts, alpha, class_table),
        // SylBlock is a syllable-aware atom: F2 non-goal.
        PhonAtom::SylBlock(_) => Err(ContextCompileError::SyllableElemUnsupported {
            kind: "%syl[ ... ]%",
        }),
    }
}

/// Build the canonical `class_table` key for a negated class.
///
/// F2b assumes callers pre-register class complements under this
/// scheme. F2c will codify this in the phonrule-compile driver.
pub fn neg_class_key(class_name: &str) -> String {
    format!("!{}", class_name)
}

/// Convenience helper for callers: compile a class and its complement
/// at once, returning both keyed appropriately for `class_table`.
///
/// This is the standard pre-population step for context compilation.
/// F2b tests use it; F2c's compile driver will too.
pub fn compile_class_and_complement(
    class: &CharClassDef,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<(String, RustFstWrapper, String, RustFstWrapper), ContextCompileError> {
    let pos = compile_class(class, alpha, class_table)?;
    let neg = compile_class_complement(class, alpha, class_table)?;
    Ok((
        class.name.node.clone(),
        pos,
        neg_class_key(&class.name.node),
        neg,
    ))
}

// ---------------------------------------------------------------------------
// Per-shape acceptor builders.
// ---------------------------------------------------------------------------

/// Build a 2-state acceptor accepting **any one** of
/// `boundary_label`, `word_start_label`, `word_end_label`.
///
/// This is the F2c3 reconciliation of `PhonContextElem::Boundary` with
/// `phonrule_eval::match_seq` (`phonrule_eval.rs:635-653`): eval accepts
/// `+` at either an in-stream BOUNDARY symbol *or* at the word edges.
/// One state pair with three parallel arcs (one per reserved label) is
/// the FST analogue of that three-way disjunction.
fn boundary_union_acceptor(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    b.add_arc(s0, alpha.boundary_label(), alpha.boundary_label(), s1)
        .expect("boundary arc");
    b.add_arc(s0, alpha.word_start_label(), alpha.word_start_label(), s1)
        .expect("word_start arc");
    b.add_arc(s0, alpha.word_end_label(), alpha.word_end_label(), s1)
        .expect("word_end arc");
    b.finish().expect("boundary_union_acceptor finish")
}

/// Build a 2-state acceptor with a single identity arc on `label`.
///
/// This is the FST analogue of "consume exactly one symbol equal to
/// `label`". Used for `WordStart` / `WordEnd` and as the inner cell of
/// `compile_wildcard` (which lays one per Σ member). `Boundary` no
/// longer uses this — see [`boundary_union_acceptor`].
fn single_label_acceptor(label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    b.add_arc(s0, label, label, s1).expect("add_arc");
    b.finish().expect("finish")
}

/// Build a one-state acceptor accepting only ε (start == final, no arcs).
///
/// The language is `{ε}`. Used as:
///   * the identity element for context-sequence concatenation when the
///     sequence is empty,
///   * inside `closure_optional` (rustfst already does this internally).
fn epsilon_acceptor() -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    b.finish().expect("finish")
}

/// Build a path acceptor for a literal string.
///
/// The literal is decomposed **per Unicode char** (matching
/// `phonrule_eval::consume_literal`, line 1094: `lit.chars().collect()`).
/// For an N-char literal we lay an (N+1)-state chain with one arc per
/// char, identity I/O. Each char is interned into `alpha` so its label
/// is a stable Σ member.
///
/// Empty literal is the ε-acceptor (matches eval's "0-char loop ends
/// immediately, return Some(cursor)" behaviour).
fn compile_literal(lit: &str, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
    let chars: Vec<char> = lit.chars().collect();
    if chars.is_empty() {
        return epsilon_acceptor();
    }
    // Each char becomes one arc. Between adjacent chars we add a
    // self-loop on the BOUNDARY label so that `\0` morpheme markers
    // are transparently skipped when matching a multi-char literal
    // (mirrors `phonrule_eval`'s `consume_literal`,
    // `phonrule_eval.rs:1086-1119`).
    let bdy = alpha.boundary_label();
    let mut b = RustFstBackend::builder();
    let mut prev = b.add_state();
    let start = prev;
    b.set_start(start).expect("set_start");
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 {
            // Insert a BOUNDARY-skipping self-loop at the previous
            // state, so 0 or more `\0` may appear between this char
            // and the next.
            b.add_arc(prev, bdy, bdy, prev)
                .expect("literal: BOUNDARY-skip self-loop");
        }
        let next = b.add_state();
        let ch_str = ch.to_string();
        let label = alpha.intern(&ch_str);
        b.add_arc(prev, label, label, next).expect("add_arc");
        prev = next;
    }
    b.set_final(prev).expect("set_final");
    b.finish().expect("finish")
}

/// Build a single-position wildcard acceptor — accepts any one symbol
/// from Σ.
///
/// Σ is taken at call time (per F2a's class-complement caveat). The
/// resulting acceptor is a 2-state FST with one arc per current Σ
/// member. Symbols interned later will not be members of this acceptor's
/// language.
fn compile_wildcard(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    for label in alpha.sigma() {
        b.add_arc(s0, label, label, s1).expect("add_arc");
    }
    b.finish().expect("finish")
}

/// Compile an alternation `(a | b | ...)`: union of its alternative
/// acceptors.
///
/// Each alternative is itself a full [`PhonContextElem`] (eval allows
/// anchors / quantified atoms inside alternations); we compile each
/// recursively and `union` them pairwise. An empty alternation is the
/// empty language (Ø: a 2-state FST with no arcs and an unreachable
/// final state), matching eval's "no alternative matches → fail"
/// fallthrough.
fn compile_alternation(
    alts: &[PhonContextElem],
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ContextCompileError> {
    if alts.is_empty() {
        return Ok(empty_language_acceptor());
    }
    let mut iter = alts.iter();
    let first = iter.next().unwrap();
    let mut acc = compile_context_elem(first, alpha, class_table)?;
    for alt in iter {
        let next = compile_context_elem(alt, alpha, class_table)?;
        acc = RustFstBackend::union(&acc, &next)
            .expect("union of two well-formed acceptors cannot fail");
    }
    Ok(acc)
}

/// Build `BOUNDARY*` — a one-state acceptor with a self-loop on the
/// boundary label only.
///
/// Used by [`compile_context_sequence`] to insert BOUNDARY-transparency
/// between context atoms (mirrors `phonrule_eval`'s `\0`-skipping
/// behaviour, `phonrule_eval.rs:1057-1080`).
fn build_boundary_star(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    b.add_arc(s, alpha.boundary_label(), alpha.boundary_label(), s)
        .expect("BOUNDARY* self-loop");
    b.finish().expect("BOUNDARY* finish")
}

/// 2-state FST with no arcs — accepts no input (`L = Ø`).
fn empty_language_acceptor() -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    // No arcs: `s1` is unreachable from `s0`.
    b.finish().expect("finish")
}

/// Wrap an acceptor with its quantifier.
///
/// Maps [`Quantifier`] to [`FstBackend`]'s closure / bounded
/// operations. `Quantifier::Exact(1)` is a no-op (the un-quantified v1
/// form). `Quantifier::Exact(0)` collapses to the ε-acceptor (the
/// brief's "Star accepts empty" case generalises).
fn apply_quantifier(
    base: RustFstWrapper,
    quant: Quantifier,
) -> Result<RustFstWrapper, ContextCompileError> {
    let wrapped = match quant {
        Quantifier::Exact(1) => base,
        Quantifier::Exact(0) => epsilon_acceptor(),
        Quantifier::Exact(n) => RustFstBackend::closure_bounded(&base, n, n)
            .expect("closure_bounded {n,n} on a well-formed acceptor"),
        Quantifier::Star => RustFstBackend::closure_star(&base).expect("closure_star"),
        Quantifier::Plus => RustFstBackend::closure_plus(&base).expect("closure_plus"),
        Quantifier::Question => {
            RustFstBackend::closure_optional(&base).expect("closure_optional")
        }
        Quantifier::AtLeast(n) => {
            // {n,} = base^n · base*
            let plus = RustFstBackend::closure_star(&base).expect("closure_star");
            if n == 0 {
                plus
            } else {
                let head = RustFstBackend::closure_bounded(&base, n, n)
                    .expect("closure_bounded {n,n}");
                RustFstBackend::concat(&head, &plus).expect("concat n with star")
            }
        }
        Quantifier::Range(n, m) => RustFstBackend::closure_bounded(&base, n, m)
            .expect("closure_bounded {n,m}"),
    };
    Ok(wrapped)
}

// ---------------------------------------------------------------------------
// FstError bridge.
// ---------------------------------------------------------------------------

impl From<ContextCompileError> for super::super::backend::FstError {
    fn from(e: ContextCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}
