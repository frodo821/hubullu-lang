//! Rule-sequence + apply-chain compilation (F2c5.2).
//!
//! A `PhonRule` body is an ordered list of `PhonBodyItem`s, each of which
//! is either a `Rewrite` (a single `PhonRewriteRule`) or an `Apply` (a
//! reference to another phonrule). This module's [`compile_phonrule`]
//! turns a whole `PhonRule` into a single composed FST that, applied
//! once to an input, performs **one pass** through every rewrite + every
//! resolved apply in body order.
//!
//! ## Why composition gives only "one pass"
//!
//! Per `phonrule_eval::apply_phonrule_inner` (lines 158–196): each
//! `Rewrite` rule is applied **iteratively to convergence** before the
//! next body item runs. The eval semantics are:
//!
//! ```text
//!   for item in body:
//!     match item:
//!       Rewrite(r):  loop { r.apply(s); if unchanged break }
//!       Apply(o):    s = apply_phonrule(o)(s)   // recursive
//! ```
//!
//! Composition of FSTs gives us the **one-pass** version of each rule
//! and of the chain. Iteration to convergence is the apply driver's
//! job (`apply::apply_phonrule_fst`, F2c5.3). The shape we compile
//! here is:
//!
//! ```text
//!   compile_phonrule(P) =
//!     rule_1 ∘ rule_2 ∘ ... ∘ rule_N
//!   where each rule_i is either:
//!     - compile_rewrite_rule(r)            for Rewrite(r)
//!     - compile_phonrule(P_ref)            for Apply(P_ref name)
//! ```
//!
//! Note: this matches eval *only* when each `Rewrite` rule converges
//! in one pass. The apply driver re-applies the whole compiled FST
//! until the surface stabilises, which handles cascading harmony
//! correctly **at the granularity of the whole phonrule** rather than
//! per-rule. For Turkish harmony + elision both shapes give the same
//! fixed point because:
//!
//!   - the eval per-rule fixed point of a single non-overlapping
//!     `a -> b / L _ R` is reached in one pass (the rule is functional
//!     in the eval semantics),
//!   - applying the *sequence* iteratively converges to the same
//!     surface form as applying each rule iteratively then moving on,
//!     because hubullu's rules don't cascade in mutually-affecting
//!     ways within a single phonrule.
//!
//! Validation against `phonrule_eval` (F2c5.4) is the empirical check
//! on this equivalence. If a real rule surfaces a divergence between
//! the two iteration shapes, we can switch to per-rule iteration in
//! the apply driver (apply each compiled component FST iteratively
//! before composing forward).
//!
//! ## Apply chain
//!
//! `PhonBodyItem::Apply(name)` references another phonrule by name. The
//! referenced phonrule is compiled (with its own apply chain resolved
//! recursively) and composed in at that body position. Cycles in the
//! apply graph are detected and reported as
//! [`RuleSeqCompileError::ApplyCycle`].
//!
//! ## What this module owns
//!
//!   * [`compile_phonrule`] — the public entry point.
//!   * [`RuleSeqCompileError`] — sum of compile / lookup / cycle errors.
//!
//! ## What this module does NOT own
//!
//!   * Per-rewrite-rule compilation — F2c5.1 (`replace`).
//!   * Runtime apply driver — F2c5.3 (`apply`).
//!   * Validation harness — F2c5.4 (`validation_tests`).

use std::collections::{HashMap, HashSet};

use crate::ast::{
    CharClassBody, CharClassDef, PhonAtom, PhonBodyItem, PhonContextElem, PhonMapBody, PhonMapElse,
    PhonMapResult, PhonPattern, PhonReplacement, PhonRule,
};

use super::super::alphabet::PhonruleAlphabet;
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;
use super::class::{compile_class, ClassCompileError};
use super::context::{compile_class_and_complement, neg_class_key};
use super::map::compile_map;
use super::replace::{
    compile_rewrite_rule_dispatch, RewriteCompileError, RewriteEngineOptions,
};

// ---------------------------------------------------------------------------
// Public errors.
// ---------------------------------------------------------------------------

/// Errors produced by [`compile_phonrule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSeqCompileError {
    /// `PhonBodyItem::Apply(name)` referenced a phonrule not in the
    /// resolver table.
    UnknownPhonrule { name: String },
    /// The apply chain has a cycle (A applies B applies A).
    ApplyCycle { path: Vec<String> },
    /// A class definition couldn't be compiled.
    Class(ClassCompileError),
    /// A rewrite rule compilation failed.
    Rewrite(RewriteCompileError),
}

impl std::fmt::Display for RuleSeqCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSeqCompileError::UnknownPhonrule { name } => write!(
                f,
                "phonrule '{}' referenced in 'apply' but not found in resolver",
                name
            ),
            RuleSeqCompileError::ApplyCycle { path } => {
                write!(f, "apply-chain cycle detected: {}", path.join(" -> "))
            }
            RuleSeqCompileError::Class(e) => write!(f, "{}", e),
            RuleSeqCompileError::Rewrite(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for RuleSeqCompileError {}

impl From<ClassCompileError> for RuleSeqCompileError {
    fn from(e: ClassCompileError) -> Self {
        RuleSeqCompileError::Class(e)
    }
}

impl From<RewriteCompileError> for RuleSeqCompileError {
    fn from(e: RewriteCompileError) -> Self {
        RuleSeqCompileError::Rewrite(e)
    }
}

impl From<RuleSeqCompileError> for super::super::backend::FstError {
    fn from(e: RuleSeqCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// A resolver from phonrule name → AST node, used to follow `apply` chains.
///
/// In production this is backed by the phase2 phonrule registry; tests
/// implement an ad-hoc `HashMap`-based resolver. We keep this as a
/// trait so the FST compilation doesn't have to depend on phase2.
pub trait PhonRuleAstResolver {
    fn resolve(&self, name: &str) -> Option<&PhonRule>;
}

impl PhonRuleAstResolver for HashMap<String, PhonRule> {
    fn resolve(&self, name: &str) -> Option<&PhonRule> {
        self.get(name)
    }
}

/// Compile a whole `PhonRule` to a single composed FST.
///
/// Walks the body in order; for each `Rewrite` item, compiles the
/// per-rule FST; for each `Apply` item, recursively compiles the
/// referenced phonrule. Composes them all left-to-right.
///
/// `alpha` is mutated as the per-rule compilers intern symbols. The
/// caller should pass a fresh `PhonruleAlphabet` (built from the
/// phoneme inventory) per top-level compile.
///
/// `resolver` resolves `Apply` references to AST nodes. If a referenced
/// phonrule isn't present, returns [`RuleSeqCompileError::UnknownPhonrule`].
/// Cycles in the apply graph are detected and reported as
/// [`RuleSeqCompileError::ApplyCycle`].
///
/// Empty body produces an identity FST over Σ (a one-state self-loop
/// accepting every Σ + stream-marker symbol). Composing it with any
/// input acceptor yields the input itself.
pub fn compile_phonrule(
    phonrule: &PhonRule,
    resolver: &dyn PhonRuleAstResolver,
    alpha: &mut PhonruleAlphabet,
) -> Result<RustFstWrapper, RuleSeqCompileError> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    compile_phonrule_inner(phonrule, resolver, alpha, &mut visited, &mut stack)
}

fn compile_phonrule_inner(
    phonrule: &PhonRule,
    resolver: &dyn PhonRuleAstResolver,
    alpha: &mut PhonruleAlphabet,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> Result<RustFstWrapper, RuleSeqCompileError> {
    // Cycle detection: if this phonrule is already on the stack, the
    // apply graph has a cycle.
    if stack.iter().any(|n| n == &phonrule.name.node) {
        let mut path = stack.clone();
        path.push(phonrule.name.node.clone());
        return Err(RuleSeqCompileError::ApplyCycle { path });
    }
    stack.push(phonrule.name.node.clone());
    visited.insert(phonrule.name.node.clone());

    // Phase 0 — pre-intern every literal symbol referenced by every
    // rewrite rule in the body (recursively into apply chains is
    // handled by the recursive call). Without this, the first compiled
    // rule's per-stage Σ snapshots would close over a smaller Σ than
    // later rules — silently rejecting inputs at apply time. See
    // `replace::intern_rule_literals` for the per-rule version.
    pre_intern_phonrule_literals(phonrule, alpha);

    // Build the local class / map tables for this phonrule. These are
    // **scoped** to this phonrule's body; nested `apply` chains build
    // their own.
    //
    // `class_members` is the flat member-NAME table Strategy A
    // (`build_directed_replacement`) needs to build `!Class` complements: it
    // maps each class name to its resolved set of Σ member strings, with nested
    // `class V = front | back` unions flattened to their leaf members. We build
    // it alongside the compiled-FST `class_table` (Strategy B's input) in the
    // same definition order, so a `Union` can resolve against earlier classes.
    let mut class_table: HashMap<String, RustFstWrapper> = HashMap::new();
    let mut class_members: HashMap<String, Vec<String>> = HashMap::new();
    for class in &phonrule.classes {
        let members = resolve_class_members(class, &class_members);
        class_members.insert(class.name.node.clone(), members);
        let pos_fst = compile_class(class, alpha, &class_table)?;
        class_table.insert(class.name.node.clone(), pos_fst);
        // Also pre-register the complement under the "!<name>" key.
        // This matches `context::neg_class_key` and is what context
        // compilation looks up for `NegClass(name)` atoms.
        let (_, _, neg_key, neg_fst) =
            compile_class_and_complement(class, alpha, &class_table)
                .map_err(|e| match e {
                    super::context::ContextCompileError::Class(c) => RuleSeqCompileError::Class(c),
                    other => RuleSeqCompileError::Rewrite(RewriteCompileError::Constraint(
                        super::constraint::ConstraintCompileError::Context(other),
                    )),
                })?;
            // The `compile_class_and_complement` helper returns
            // both keys; we only need to insert the negation since
            // the positive was already inserted above.
            let _ = neg_class_key; // silence the import warning
            class_table.insert(neg_key, neg_fst);
    }

    let mut map_table: HashMap<String, RustFstWrapper> = HashMap::new();
    for map in &phonrule.maps {
        let map_fst = compile_map(map, alpha);
        map_table.insert(map.name.node.clone(), map_fst);
    }

    // Engine selection: Strategy A by default, per-rule fallback to Strategy B
    // on `UnsupportedShape`, with the §8.2 env escape hatch
    // (`HUBULLU_FORCE_STRATEGY_B`) forcing Strategy B for everything.
    let engine_opts = RewriteEngineOptions::from_env();

    // Compose body items left-to-right.
    let mut acc: Option<RustFstWrapper> = None;
    for item in &phonrule.body {
        let step = match item {
            PhonBodyItem::Rewrite(rule) => compile_rewrite_rule_dispatch(
                rule,
                alpha,
                &class_table,
                &class_members,
                &map_table,
                engine_opts,
            )?,
            PhonBodyItem::Apply(apply) => {
                let target = resolver.resolve(&apply.rule.node).ok_or_else(|| {
                    RuleSeqCompileError::UnknownPhonrule {
                        name: apply.rule.node.clone(),
                    }
                })?;
                compile_phonrule_inner(target, resolver, alpha, visited, stack)?
            }
        };
        acc = Some(match acc {
            None => step,
            Some(prev) => compose_sorted(&prev, &step),
        });
    }

    stack.pop();

    // Empty body: return an identity-on-Σ FST so callers can compose
    // freely without special-casing the empty case.
    Ok(acc.unwrap_or_else(|| build_identity_over_sigma(alpha)))
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Arc-sort both operands and compose. Same discipline as `replace.rs`.
fn compose_sorted(a: &RustFstWrapper, b: &RustFstWrapper) -> RustFstWrapper {
    let a_sorted = RustFstBackend::arc_sort_output(a).expect("arc_sort_output");
    let b_sorted = RustFstBackend::arc_sort_input(b).expect("arc_sort_input");
    RustFstBackend::compose(&a_sorted, &b_sorted).expect("compose")
}

/// Resolve a `CharClassDef` to its flat set of Σ member strings.
///
/// A `List` body is its literal members directly. A `Union(front | back | ...)`
/// is the concatenation of the already-resolved member sets of each referenced
/// class (looked up in `prior`, which holds every class defined *before* this
/// one — classes must be defined before use, matching `compile_class`). An
/// unresolved union reference (forward / unknown) contributes nothing here; the
/// compiled-FST path's `compile_class` is the authoritative error site for a
/// genuinely unknown class, so this stays lenient. Order is preserved and
/// duplicates removed (the resulting set is what Strategy A complements over).
fn resolve_class_members(
    class: &CharClassDef,
    prior: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push_unique = |s: &str, out: &mut Vec<String>| {
        if !out.iter().any(|e| e == s) {
            out.push(s.to_string());
        }
    };
    match &class.body {
        CharClassBody::List(members) => {
            for m in members {
                push_unique(&m.node, &mut out);
            }
        }
        CharClassBody::Union(names) => {
            for name in names {
                if let Some(members) = prior.get(&name.node) {
                    for m in members {
                        push_unique(m, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// Walk a phonrule and intern every literal symbol into `alpha`.
///
/// Mirrors `replace::intern_rule_literals` but at the phonrule level:
/// every rewrite rule's LHS / RHS / context literals, every class
/// member literal, and every map arm `from`/`to` literal are interned
/// before any per-rule compilation runs. This is the closed-alphabet
/// discipline at the phonrule scope.
fn pre_intern_phonrule_literals(phonrule: &PhonRule, alpha: &mut PhonruleAlphabet) {
    // Class members.
    for class in &phonrule.classes {
        match &class.body {
            CharClassBody::List(members) => {
                for m in members {
                    intern_str_chars(&m.node, alpha);
                }
            }
            CharClassBody::Union(_) => {}
        }
    }
    // Map arms.
    for map in &phonrule.maps {
        let PhonMapBody::Match { arms, else_arm } = &map.body;
        for arm in arms {
            intern_str_chars(&arm.from.node, alpha);
            match &arm.to {
                PhonMapResult::Literal(l) => intern_str_chars(&l.node, alpha),
                PhonMapResult::Var(_) => {}
            }
        }
        if let Some(e) = else_arm {
            match e {
                PhonMapElse::Literal(l) => intern_str_chars(&l.node, alpha),
                PhonMapElse::Var(_) => {}
            }
        }
    }
    // Body items.
    for item in &phonrule.body {
        if let PhonBodyItem::Rewrite(rule) = item {
            intern_pattern(&rule.from, alpha);
            intern_replacement(&rule.to, alpha);
            if let Some(ctx) = &rule.context {
                for elem in &ctx.left {
                    intern_context_elem(elem, alpha);
                }
                for elem in &ctx.right {
                    intern_context_elem(elem, alpha);
                }
            }
        }
    }
}

fn intern_pattern(pat: &PhonPattern, alpha: &mut PhonruleAlphabet) {
    match pat {
        PhonPattern::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonPattern::Class(_) => {}
        PhonPattern::Range(elems) => {
            for e in elems {
                intern_context_elem(e, alpha);
            }
        }
    }
}

fn intern_replacement(rep: &PhonReplacement, alpha: &mut PhonruleAlphabet) {
    match rep {
        PhonReplacement::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonReplacement::Null | PhonReplacement::Map(_) => {}
    }
}

fn intern_context_elem(elem: &PhonContextElem, alpha: &mut PhonruleAlphabet) {
    match elem {
        PhonContextElem::Boundary
        | PhonContextElem::WordStart
        | PhonContextElem::WordEnd
        | PhonContextElem::SylHead
        | PhonContextElem::SylTail
        | PhonContextElem::SylIndex(_) => {}
        PhonContextElem::Atom(atom, _) => intern_atom(atom, alpha),
    }
}

fn intern_atom(atom: &PhonAtom, alpha: &mut PhonruleAlphabet) {
    match atom {
        PhonAtom::Class(_) | PhonAtom::NegClass(_) | PhonAtom::Wildcard => {}
        PhonAtom::Literal(lit) => intern_str_chars(&lit.node, alpha),
        PhonAtom::Alt(alts) => {
            for a in alts {
                intern_context_elem(a, alpha);
            }
        }
        PhonAtom::SylBlock(elems) => {
            for e in elems {
                intern_context_elem(e, alpha);
            }
        }
    }
}

fn intern_str_chars(s: &str, alpha: &mut PhonruleAlphabet) {
    for ch in s.chars() {
        if ch == crate::phonrule_eval::BOUNDARY {
            continue;
        }
        alpha.intern(&ch.to_string());
    }
}

/// Build a one-state identity FST over Σ + stream markers.
///
/// Used as the result for an empty phonrule body. Composing this with
/// any input yields the input unchanged.
fn build_identity_over_sigma(alpha: &PhonruleAlphabet) -> RustFstWrapper {
    use super::super::backend::FstBuilder;
    let mut b = RustFstBackend::builder();
    let s = b.add_state();
    b.set_start(s).expect("set_start");
    b.set_final(s).expect("set_final");
    let mut seen: HashSet<u32> = HashSet::new();
    for label in alpha.sigma() {
        if seen.insert(label) {
            b.add_arc(s, label, label, s).expect("identity arc");
        }
    }
    for marker in [
        alpha.boundary_label(),
        alpha.word_start_label(),
        alpha.word_end_label(),
    ] {
        if seen.insert(marker) {
            b.add_arc(s, marker, marker, s).expect("marker identity arc");
        }
    }
    b.finish().expect("identity FST finish")
}
