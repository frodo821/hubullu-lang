//! `CharClassDef` → acceptor FST (F2a step 2).
//!
//! Per the F2 plan §2 table row for `CharClassDef`: a class is an identity
//! transducer accepting exactly one symbol per accepting path, where the
//! accepted set is the class's member set.
//!
//! Reference semantics (matches `phonrule_eval::char_in_class`,
//! `src/phonrule_eval.rs:501`):
//!
//!   * `CharClassBody::List(members)` — the accepted set is the literal
//!     symbols listed.
//!   * `CharClassBody::Union(refs)` — the accepted set is the union of the
//!     referenced classes' accepted sets, resolved recursively. Cycles are
//!     rejected; undefined references are rejected.
//!
//! F2a only consumes phonrule-local classes; F2b will extend resolution to
//! fall through to the phoneme inventory (see `phonrule_eval.rs:514-518`).
//! The hook is open here: callers that want phoneme-inventory fallback can
//! pre-register a class with the relevant inventory members and pass it in
//! via `class_table`.

use std::collections::{HashMap, HashSet};

use crate::ast::{CharClassBody, CharClassDef};

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBackend, FstBuilder, FstResult, Label};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};

/// Compile a single `CharClassDef` to an identity-acceptor FST.
///
/// The result is a 2-state FST: start → final with one arc per class member
/// (input label == output label). Empty classes produce a 2-state FST with
/// no arcs (an unreachable final state — accepts nothing). This matches
/// `phonrule_eval`'s behaviour: an empty class's `char_in_class` is always
/// `false`.
///
/// `class_table` provides previously-compiled classes by name, used when the
/// body is a `Union` of other class refs. Forward references (a `Union`
/// member not yet in `class_table`) produce a [`ClassCompileError::UnknownClass`]
/// error — callers must compile classes in dependency order. (A topological
/// sort over `Union` references is straightforward and could be added later
/// if convenient; F2a keeps the contract minimal.)
///
/// Member symbols are interned into `alpha`. New literals extend the runtime
/// alphabet Σ (per plan §5 "alphabet drift" — F2a silently includes; no
/// warning policy yet).
pub fn compile_class(
    class: &CharClassDef,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ClassCompileError> {
    let members = resolve_members(class, alpha, class_table)?;
    Ok(build_identity_acceptor(&members))
}

/// Compile the complement of a class over the current Σ.
///
/// Result accepts exactly those symbols in `alpha.sigma()` that are NOT
/// members of `class`. This is the FST analogue of `phonrule_eval`'s
/// `PhonAtom::NegClass` branch (`phonrule_eval.rs:768-779`), modulo the
/// boundary-transparency that the eval layer applies on top of class
/// membership (boundary handling is F2b context-elem compilation, not class
/// compilation).
///
/// **F2a placement.** The full `!class` context-elem support is F2b; this
/// helper lands now because the alphabet-handle dependency is the same and
/// keeping the construction next to `compile_class` is clearer than
/// splitting it across phases. F2b will call it from context-elem
/// compilation.
///
/// Note: the complement is taken over Σ at the time of the call. If new
/// symbols are interned later (e.g. by other rules), they will NOT be in
/// this complement acceptor's language. Callers building a full phonrule FST
/// must therefore compile classes / class complements **after** all
/// alphabet-extending operations.
pub fn compile_class_complement(
    class: &CharClassDef,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<RustFstWrapper, ClassCompileError> {
    let in_class = resolve_members(class, alpha, class_table)?;
    let in_class_set: HashSet<Label> = in_class.iter().copied().collect();
    let complement: Vec<Label> = alpha
        .sigma()
        .filter(|l| !in_class_set.contains(l))
        .collect();
    Ok(build_identity_acceptor(&complement))
}

// ---------------------------------------------------------------------------
// Internals.
// ---------------------------------------------------------------------------

/// Resolve a class to its terminal label set.
///
/// `List` arms intern their members and add them directly.
/// `Union` arms recursively resolve the referenced classes via `class_table`;
/// **forward references are an error** — callers compile in dependency
/// order.
fn resolve_members(
    class: &CharClassDef,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> Result<Vec<Label>, ClassCompileError> {
    let mut members: Vec<Label> = Vec::new();
    let mut seen: HashSet<Label> = HashSet::new();
    let push = |members: &mut Vec<Label>, seen: &mut HashSet<Label>, l: Label| {
        if seen.insert(l) {
            members.push(l);
        }
    };

    match &class.body {
        CharClassBody::List(lits) => {
            for lit in lits {
                let l = alpha.intern(&lit.node);
                push(&mut members, &mut seen, l);
            }
        }
        CharClassBody::Union(refs) => {
            for r in refs {
                let referenced = class_table.get(&r.node).ok_or_else(|| {
                    ClassCompileError::UnknownClass {
                        referrer: class.name.node.clone(),
                        target: r.node.clone(),
                    }
                })?;
                for l in input_labels_of(referenced) {
                    push(&mut members, &mut seen, l);
                }
            }
        }
    }
    Ok(members)
}

/// Extract the set of input labels reachable in an identity acceptor.
///
/// Class acceptors built by [`build_identity_acceptor`] are acyclic 2-state
/// FSTs with one arc per accepted symbol; [`RustFstBackend::paths`] yields
/// one path per arc, finite, with `path.input[0]` equal to the arc's input
/// label. We go through the trait surface here rather than reaching into
/// `rustfst::*` types — same seam discipline as the rest of `src/fst/`.
fn input_labels_of(fst: &RustFstWrapper) -> Vec<Label> {
    let mut labels: Vec<Label> = Vec::new();
    let mut seen: HashSet<Label> = HashSet::new();
    for path in RustFstBackend::paths(fst).expect("paths over identity acceptor") {
        if let Some(&first) = path.input.first() {
            if seen.insert(first) {
                labels.push(first);
            }
        }
    }
    labels
}

/// Build a 2-state identity acceptor over the given labels.
///
/// State `s0` is start, `s1` is final; one arc `s0 -[l:l]-> s1` per label.
/// An empty `labels` slice yields an FST with no accepting path (Ø
/// language) — `s1` is final but unreachable.
fn build_identity_acceptor(labels: &[Label]) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    for &l in labels {
        b.add_arc(s0, l, l, s1).expect("add_arc");
    }
    b.finish().expect("finish")
}

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Errors produced by class compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassCompileError {
    /// A `Union` arm referred to a class not present in `class_table`.
    /// Callers must compile classes in dependency order, or the referenced
    /// class is undefined.
    UnknownClass { referrer: String, target: String },
}

impl std::fmt::Display for ClassCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClassCompileError::UnknownClass { referrer, target } => write!(
                f,
                "class '{}' references undefined or not-yet-compiled class '{}'",
                referrer, target
            ),
        }
    }
}

impl std::error::Error for ClassCompileError {}

/// Map `ClassCompileError` to `FstError::Backend` for callers that need to
/// surface class errors through the backend-agnostic error channel.
impl From<ClassCompileError> for super::super::backend::FstError {
    fn from(e: ClassCompileError) -> Self {
        super::super::backend::FstError::Backend(e.to_string())
    }
}

/// `FstResult` flavour for class compilation that erases the concrete error
/// type, for situations where callers want a single error channel.
pub fn compile_class_to_fst_result(
    class: &CharClassDef,
    alpha: &mut PhonruleAlphabet,
    class_table: &HashMap<String, RustFstWrapper>,
) -> FstResult<RustFstWrapper> {
    compile_class(class, alpha, class_table).map_err(Into::into)
}
