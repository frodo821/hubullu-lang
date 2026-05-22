//! Backend-agnostic FST trait surface.
//!
//! This file defines the seam between hubullu's morphology layer and the
//! underlying FST kernel. The trait surface is the **minimum** the morphology
//! layer (F2: Kaplan-Kay rewrite-rule compilation; F3+: lexicon + compose +
//! per-entry specialisation) actually needs.
//!
//! Invariant: **no `rustfst::*` types may appear in this file**, in any trait
//! signature, in any public type, or in any error variant. Backend
//! implementations live in sibling modules and translate to/from their own
//! types behind the trait. A grep for `rustfst` in this file should return
//! zero matches.

use std::collections::HashMap;
use std::path::Path as FsPath;

use thiserror::Error;

/// FST label type. Same as `rustfst`'s `Label` (which is also `u32` under the
/// default `state-label-u32` feature), but a separately-declared alias so the
/// morphology layer never imports a rustfst type.
///
/// Label `0` is reserved for epsilon by convention (matching every FST library
/// since OpenFST).
pub type Label = u32;

/// FST state identifier. Same width and conventions as [`Label`].
pub type StateId = u32;

/// The reserved epsilon label. Arcs with `EPS_LABEL` on the input or output
/// side consume / emit nothing on that side.
pub const EPS_LABEL: Label = 0;

// ---------------------------------------------------------------------------
// Symbol table — backend-agnostic, concrete (not a trait).
// ---------------------------------------------------------------------------

/// Bidirectional name ↔ label map.
///
/// Symbol tables are explicitly backend-independent: morphology code builds
/// them, hands them to a backend along with label-bearing FSTs, and reads them
/// back from loaded FSTs. They are not generic over the backend.
///
/// By convention, label `0` is reserved for the epsilon symbol `"<eps>"` and
/// is inserted automatically at construction.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub name_to_id: HashMap<String, Label>,
    pub id_to_name: Vec<String>,
}

impl SymbolTable {
    /// Create a new symbol table with epsilon reserved at label 0.
    pub fn new() -> Self {
        let mut t = Self {
            name_to_id: HashMap::new(),
            id_to_name: Vec::new(),
        };
        t.name_to_id.insert("<eps>".to_string(), EPS_LABEL);
        t.id_to_name.push("<eps>".to_string());
        t
    }

    /// Intern a symbol. Returns its label, inserting it if new.
    pub fn intern(&mut self, name: &str) -> Label {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        let id = self.id_to_name.len() as Label;
        self.id_to_name.push(name.to_string());
        self.name_to_id.insert(name.to_string(), id);
        id
    }

    /// Look up a symbol by label.
    pub fn name(&self, label: Label) -> Option<&str> {
        self.id_to_name.get(label as usize).map(|s| s.as_str())
    }

    /// Look up a label by symbol name.
    pub fn label(&self, name: &str) -> Option<Label> {
        self.name_to_id.get(name).copied()
    }

    /// Number of symbols (including epsilon).
    pub fn len(&self) -> usize {
        self.id_to_name.len()
    }

    /// Whether the table is empty. Always false because epsilon is always
    /// present; provided for `clippy::len_without_is_empty`.
    pub fn is_empty(&self) -> bool {
        self.id_to_name.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Path — backend-agnostic path representation.
// ---------------------------------------------------------------------------

/// One accepting path through an FST.
///
/// Epsilon labels are stripped (consistent with `rustfst`'s `FstPath` and with
/// the morphology layer's view: forward render reads the input side, reverse
/// lookup reads the output side, neither cares about ε arcs).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    pub input: Vec<Label>,
    pub output: Vec<Label>,
}

// ---------------------------------------------------------------------------
// Error type — backend-agnostic.
// ---------------------------------------------------------------------------

/// All errors the FST kernel can produce. Backend-specific errors (e.g.
/// rustfst's `anyhow::Error`) are wrapped in [`FstError::Backend`] with their
/// string form preserved for diagnostics.
#[derive(Debug, Error)]
pub enum FstError {
    #[error("FST operation failed: {0}")]
    Backend(String),

    #[error("FST has no start state")]
    NoStart,

    #[error("invalid state id: {0}")]
    BadState(StateId),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("deserialization failed: {0}")]
    Deserialize(String),
}

/// Result type for FST operations.
pub type FstResult<T> = Result<T, FstError>;

// ---------------------------------------------------------------------------
// FstBackend — the seam.
// ---------------------------------------------------------------------------

/// The FST kernel facade.
///
/// Every operation morphology needs is defined here. The morphology layer
/// works in terms of `<B as FstBackend>::Fst`, never a concrete backend type.
///
/// Operations are conceptually pure: they take `&Self::Fst` and return a new
/// `Self::Fst`. Backends may use mutable representations internally, but the
/// surface is value-in / value-out. This lets backends choose their own
/// internal mutation strategy without changing the trait.
///
/// ### Naming
///
/// Where the underlying OpenFST convention conflicts with hubullu's terms, the
/// trait method takes hubullu's name (e.g. `closure_star` not `closure(Star)`,
/// `eps_remove` not `rm_epsilon`). The backend translates.
pub trait FstBackend {
    /// Opaque FST handle. Backends wrap their own immutable / mutable FST
    /// types in this. Must be `Clone` because most FST ops conceptually
    /// produce a new FST; cloning a handle is required when the same input
    /// FST feeds multiple downstream ops.
    type Fst: Clone;

    /// Mutable FST builder. Separated from `Self::Fst` so the immutable type
    /// can be made `ConstFst`-like later if a backend wants that distinction.
    type Builder: FstBuilder<Backend = Self>;

    // ----- builder entry point -----

    /// Create a new empty FST builder.
    fn builder() -> Self::Builder;

    // ----- combinators (composition building blocks) -----

    /// Concatenation: accept `L(a) · L(b)`.
    fn concat(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst>;

    /// Union: accept `L(a) ∪ L(b)`.
    fn union(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst>;

    /// Kleene star: accept `L(a)*`.
    fn closure_star(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Kleene plus: accept `L(a)+`.
    fn closure_plus(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Optional (`?`): accept `L(a) ∪ {ε}`.
    fn closure_optional(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Bounded repeat `{n,m}`: accept any concatenation of `n..=m` copies of
    /// `L(a)`. Implementation may unroll to alternation.
    fn closure_bounded(a: &Self::Fst, n: u32, m: u32) -> FstResult<Self::Fst>;

    // ----- set operations (F2c3 prerequisites) -----

    /// Acceptor complement over an explicit alphabet `Σ_b`.
    ///
    /// Returns an FST accepting exactly `Σ_b* \ L(a)`. The complement is
    /// computed by determinising `a` (after epsilon removal), completing
    /// the resulting DFA over `Σ_b` with a fresh dead/sink state for
    /// missing transitions, and swapping final / non-final.
    ///
    /// **`a` must be an acceptor over Σ_b** — every arc has
    /// `input_label == output_label`, and every arc label is a member of
    /// `Σ_b`. Behaviour on a non-acceptor or out-of-alphabet arc is
    /// unspecified (the F2c3 caller always builds acceptors from Σ_b
    /// pieces, so the precondition is easy to honour).
    ///
    /// `Σ_b` is explicit because the morphology layer's notion of
    /// alphabet is the live `PhonruleAlphabet` plus reserved markers —
    /// it is not derivable from the FST itself (the FST may
    /// under-mention members it could have transitioned on but does not).
    fn complement(a: &Self::Fst, alphabet: &[Label]) -> FstResult<Self::Fst>;

    /// Acceptor intersection: `L(a) ∩ L(b)`.
    ///
    /// Both operands must be acceptors (input==output on every arc).
    /// Implemented as composition (`a ∘ b` where both sides are
    /// identity transducers gives the intersection, since each path
    /// requires both sides to match the same label). The backend
    /// applies the standard arc-sort discipline internally.
    fn intersect(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst>;

    // ----- composition (the critical op) -----

    /// FST composition `a ∘ b`. Per rustfst-survey §1 / §5, this is the
    /// most-used and hardest-to-get-right operation. Backends MUST NOT route
    /// this through rustfst's `optimize` wrapper (issue with PR #166); call
    /// `determinize` + `minimize` explicitly if optimisation is wanted.
    fn compose(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst>;

    // ----- normalisation -----

    /// Determinise. May fail or diverge on non-functional transducers; per
    /// rustfst issue #288 the behaviour differs from OpenFST in some edge
    /// cases — morphology callers should ensure functional input where it
    /// matters (proposal §6.6).
    fn determinize(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Minimise. Backends may require determinised input.
    fn minimize(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Remove epsilon transitions (pre-composition cleanup).
    fn eps_remove(a: &Self::Fst) -> FstResult<Self::Fst>;

    // ----- arc sorting (composition prerequisite) -----

    /// Sort outgoing arcs of every state by **input label**.
    ///
    /// rustfst's `compose(a, b)` requires `b` (the right operand) to be
    /// **input-sorted** — the right side's `SortedMatcher` walks its arcs
    /// in I-label order to find matches against the left side's output
    /// labels. Freshly built FSTs from [`FstBuilder`] do not have their
    /// `I_LABEL_SORTED` property bit set, so a direct `compose(a, b)`
    /// fails with a property-bit error even when arcs happen to be in
    /// the right order. Call `arc_sort_input(b)` before composition.
    ///
    /// The operation preserves language: it only reorders arcs leaving
    /// each state; it does not add, remove, or relabel them.
    fn arc_sort_input(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Sort outgoing arcs of every state by **output label**.
    ///
    /// rustfst's `compose(a, b)` requires `a` (the left operand) to be
    /// **output-sorted** — the left side's `SortedMatcher` walks its arcs
    /// in O-label order to find matches against the right side's input
    /// labels. See [`arc_sort_input`](Self::arc_sort_input) for the
    /// freshly-built-FST caveat; the same applies here. Call
    /// `arc_sort_output(a)` before composition.
    ///
    /// The operation preserves language: it only reorders arcs leaving
    /// each state; it does not add, remove, or relabel them.
    fn arc_sort_output(a: &Self::Fst) -> FstResult<Self::Fst>;

    // ----- label transformations -----

    /// Swap input and output labels on every arc. Used for reverse lookup:
    /// `analyse(surface) = forward(invert(fst), surface)`.
    fn invert(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Reverse arc direction. Final states become reachable from a new
    /// start state and vice versa. Useful for some traversal constructions.
    fn reverse(a: &Self::Fst) -> FstResult<Self::Fst>;

    /// Remove unreachable states (trim).
    fn connect(a: &Self::Fst) -> FstResult<Self::Fst>;

    // ----- recursion flattening -----

    /// Replace: in `root`, substitute every arc labelled `label` with the
    /// corresponding FST. Used by F8 to flatten clitic-with-paradigm
    /// recursion at compile time, with a depth bound enforced one layer up.
    ///
    /// `label_to_fst` maps the placeholder label to the FST to splice in.
    /// `root_label` is the conventional label identifying `root` itself in
    /// the replacement table (rustfst's `replace` requires the root to also
    /// be in the table indexed by some label).
    fn replace(
        root: &Self::Fst,
        root_label: Label,
        label_to_fst: &[(Label, Self::Fst)],
    ) -> FstResult<Self::Fst>;

    // ----- traversal -----

    /// Iterate over all accepting paths.
    ///
    /// For forward render, the caller projects each `Path.output`; for reverse
    /// lookup, the caller first calls [`invert`](Self::invert) and then reads
    /// `Path.output` (which now holds the analysis side).
    ///
    /// Backends should return paths with epsilon labels stripped on both
    /// sides; the morphology layer doesn't care about ε arcs.
    ///
    /// Returns a boxed iterator because rustfst's path iterator has a
    /// lifetime tied to the FST; the wrapper materialises eagerly to give
    /// the caller a clean owned iterator. This is acceptable at our scale
    /// (per-entry FSTs <1000 states, hundreds of paths max). If we ever
    /// need streaming, we can revisit.
    fn paths(fst: &Self::Fst) -> FstResult<Box<dyn Iterator<Item = Path> + '_>>;

    // ----- serialisation -----

    /// Serialise to bytes. The on-disk format is backend-specific for F1
    /// (rustfst's native binary). The `.huc` envelope (postcard + this blob)
    /// is F7 work; see `docs/proposals/fst-morphology.md` §4.
    fn serialize(fst: &Self::Fst) -> FstResult<Vec<u8>>;

    /// Deserialise from bytes. Must round-trip with [`serialize`](Self::serialize).
    fn deserialize(bytes: &[u8]) -> FstResult<Self::Fst>;

    /// Load an FST in a way that lets it share storage with the file on disk.
    ///
    /// **F1 status**: the rustfst-backed implementation just reads the file
    /// into memory and deserialises — there is no actual zero-copy. The trait
    /// API supports zero-copy; the in-tree kernel (Option C) is the path that
    /// would actually deliver it. See `fst-kernel-design.md` §4. Callers must
    /// not assume the returned FST aliases the file.
    fn mmap_load(path: &FsPath) -> FstResult<Self::Fst>;

    // ----- introspection (small utility surface) -----

    /// Number of states. Useful for tests and diagnostics.
    fn num_states(fst: &Self::Fst) -> usize;
}

/// Mutable FST under construction. Yielded by [`FstBackend::builder`].
///
/// The builder API is small and imperative because every FST library on Earth
/// gives you imperative state/arc construction. Combinators live on
/// [`FstBackend`].
pub trait FstBuilder {
    type Backend: FstBackend<Builder = Self>;

    /// Add a new state, returning its id.
    fn add_state(&mut self) -> StateId;

    /// Set the start state. Required before [`finish`](Self::finish).
    fn set_start(&mut self, s: StateId) -> FstResult<()>;

    /// Mark a state as final.
    fn set_final(&mut self, s: StateId) -> FstResult<()>;

    /// Add an arc from `from` to `to` reading `input_label`, writing
    /// `output_label`. Either label may be [`EPS_LABEL`].
    fn add_arc(
        &mut self,
        from: StateId,
        input_label: Label,
        output_label: Label,
        to: StateId,
    ) -> FstResult<()>;

    /// Finish construction and return the immutable FST.
    fn finish(self) -> FstResult<<Self::Backend as FstBackend>::Fst>;
}
