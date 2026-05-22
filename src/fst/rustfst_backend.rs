//! `rustfst`-backed implementation of [`FstBackend`].
//!
//! This is the **only** file in the codebase that names `rustfst::*` types.
//! Every method translates between hubullu's backend-agnostic types ([`Label`],
//! [`Path`], etc.) and rustfst's internal types (`VectorFst`, `Tr`,
//! `TropicalWeight`, `FstPath`). If a `rustfst::*` import appears anywhere
//! else under `src/`, the seam is broken — see the grep check enforced in
//! `tests/fst_seam_lint.rs` (F1) and the CI lint.
//!
//! ## Semiring choice
//!
//! We use [`TropicalWeight`] with `one() == 0.0` to represent unweighted
//! transducers. The survey (`docs/proposals/rustfst-survey.md` §2) recommended
//! `TrivialWeight` or `BooleanWeight`, but in rustfst 1.3.1 neither implements
//! `WeightQuantize`, which `determinize` and `minimize` both require. The
//! morphology layer never sees the semiring (it lives entirely below the
//! trait surface), so this choice is purely an implementation detail.

use std::path::Path as FsPath;

use rustfst::algorithms::compose::compose as rfst_compose;
use rustfst::algorithms::concat::concat as rfst_concat;
use rustfst::algorithms::closure::{closure as rfst_closure, ClosureType};
use rustfst::algorithms::determinize::determinize as rfst_determinize;
use rustfst::algorithms::replace::replace as rfst_replace;
use rustfst::algorithms::rm_epsilon::rm_epsilon as rfst_rm_epsilon;
use rustfst::algorithms::tr_compares::{ILabelCompare, OLabelCompare};
use rustfst::algorithms::tr_sort as rfst_tr_sort;
use rustfst::algorithms::union::union as rfst_union;
use rustfst::algorithms::{
    connect as rfst_connect, invert as rfst_invert, minimize as rfst_minimize,
    reverse as rfst_reverse,
};
use rustfst::fst_impls::VectorFst;
use rustfst::fst_traits::{
    CoreFst, ExpandedFst, Fst as RustFstTrait, MutableFst, SerializableFst,
};
use rustfst::semirings::{Semiring, TropicalWeight};
use rustfst::{Label as RfstLabel, StateId as RfstStateId, Tr};

use super::backend::{FstBackend, FstBuilder, FstError, FstResult, Label, Path, StateId};

/// Convert a rustfst error (`anyhow::Error`) into our [`FstError`].
fn wrap_err<E: std::fmt::Display>(e: E) -> FstError {
    FstError::Backend(e.to_string())
}

/// The concrete FST type behind the trait. A simple newtype around rustfst's
/// `VectorFst<TropicalWeight>` so the trait's `Self::Fst = RustFstWrapper`
/// rather than `Self::Fst = VectorFst<TropicalWeight>`; this prevents rustfst
/// types from appearing in the trait's method signatures.
#[derive(Clone, Debug)]
pub struct RustFstWrapper {
    pub(crate) inner: VectorFst<TropicalWeight>,
}

impl RustFstWrapper {
    /// Construct from a rustfst FST. Internal-only — morphology code never
    /// builds these directly; it goes through the builder.
    pub(crate) fn new(inner: VectorFst<TropicalWeight>) -> Self {
        Self { inner }
    }
}

/// Backend marker type.
pub struct RustFstBackend;

/// Builder for [`RustFstBackend`]: thin wrapper around `VectorFst`.
pub struct RustFstBuilder {
    inner: VectorFst<TropicalWeight>,
}

impl FstBuilder for RustFstBuilder {
    type Backend = RustFstBackend;

    fn add_state(&mut self) -> StateId {
        self.inner.add_state() as StateId
    }

    fn set_start(&mut self, s: StateId) -> FstResult<()> {
        self.inner
            .set_start(s as RfstStateId)
            .map_err(wrap_err)
    }

    fn set_final(&mut self, s: StateId) -> FstResult<()> {
        self.inner
            .set_final(s as RfstStateId, TropicalWeight::one())
            .map_err(wrap_err)
    }

    fn add_arc(
        &mut self,
        from: StateId,
        input_label: Label,
        output_label: Label,
        to: StateId,
    ) -> FstResult<()> {
        self.inner
            .add_tr(
                from as RfstStateId,
                Tr::new(
                    input_label as RfstLabel,
                    output_label as RfstLabel,
                    TropicalWeight::one(),
                    to as RfstStateId,
                ),
            )
            .map_err(wrap_err)
    }

    fn finish(self) -> FstResult<RustFstWrapper> {
        Ok(RustFstWrapper::new(self.inner))
    }
}

impl FstBackend for RustFstBackend {
    type Fst = RustFstWrapper;
    type Builder = RustFstBuilder;

    fn builder() -> Self::Builder {
        RustFstBuilder {
            inner: VectorFst::<TropicalWeight>::new(),
        }
    }

    fn concat(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_concat(&mut out, &b.inner).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn union(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_union(&mut out, &b.inner).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn closure_star(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_closure(&mut out, ClosureType::ClosureStar);
        Ok(RustFstWrapper::new(out))
    }

    fn closure_plus(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_closure(&mut out, ClosureType::ClosurePlus);
        Ok(RustFstWrapper::new(out))
    }

    fn closure_optional(a: &Self::Fst) -> FstResult<Self::Fst> {
        // Optional = union with an ε-acceptor (a one-state FST that is both
        // start and final). rustfst doesn't ship `optional` directly, but the
        // construction is trivial.
        let mut eps_fst = VectorFst::<TropicalWeight>::new();
        let s = eps_fst.add_state();
        eps_fst.set_start(s).map_err(wrap_err)?;
        eps_fst
            .set_final(s, TropicalWeight::one())
            .map_err(wrap_err)?;

        let mut out = a.inner.clone();
        rfst_union(&mut out, &eps_fst).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn closure_bounded(a: &Self::Fst, n: u32, m: u32) -> FstResult<Self::Fst> {
        // {n,m} unrolls to alternation: X^n | X^(n+1) | ... | X^m.
        // For X^k we concat k copies of a. The empty alternation case
        // (n > m) is an error; n == m == 0 yields an ε-acceptor.
        if n > m {
            return Err(FstError::Backend(format!(
                "closure_bounded: n ({}) > m ({})",
                n, m
            )));
        }

        // Helper: ε-acceptor (single-state start+final FST).
        let mk_eps = || -> FstResult<VectorFst<TropicalWeight>> {
            let mut e = VectorFst::<TropicalWeight>::new();
            let s = e.add_state();
            e.set_start(s).map_err(wrap_err)?;
            e.set_final(s, TropicalWeight::one()).map_err(wrap_err)?;
            Ok(e)
        };

        // Helper: a^k = concat(a, a, ..., a) k times. k=0 = ε.
        let pow = |k: u32| -> FstResult<VectorFst<TropicalWeight>> {
            if k == 0 {
                return mk_eps();
            }
            let mut acc = a.inner.clone();
            for _ in 1..k {
                rfst_concat(&mut acc, &a.inner).map_err(wrap_err)?;
            }
            Ok(acc)
        };

        let mut acc = pow(n)?;
        for k in (n + 1)..=m {
            let next = pow(k)?;
            rfst_union(&mut acc, &next).map_err(wrap_err)?;
        }
        Ok(RustFstWrapper::new(acc))
    }

    fn compose(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst> {
        // Per rustfst-survey §6 issue #235: the type signature of compose is
        // awkward, requiring full type annotation on the result. We pin
        // VectorFst<TropicalWeight> here so the morphology layer never has
        // to think about it. Per the survey we do NOT call the convenience
        // `optimize` wrapper (PR #166 issue); callers wanting optimisation
        // call determinize + minimize explicitly.
        let out: VectorFst<TropicalWeight> =
            rfst_compose(a.inner.clone(), b.inner.clone()).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn complement(a: &Self::Fst, alphabet: &[Label]) -> FstResult<Self::Fst> {
        // Plan: eps-remove, determinise, complete over `alphabet`, swap
        // final / non-final. F2c3 calls this on acceptors over the
        // bracket-augmented alphabet `Σ_b = Σ ∪ {<[+]>, <]+>}`.
        //
        // The "complete" step is the one rustfst doesn't ship. We add
        // a fresh dead/sink state to the determinised FST; for every
        // existing state (including the dead state itself), any
        // alphabet symbol that has no outgoing arc gets a fresh arc
        // pointing to the dead state. The dead state is non-final.
        // After completion, "final ↔ non-final" swap yields complement.
        //
        // The empty-language case (no start state) becomes Σ_b*: one
        // accepting state with self-loops on every alphabet symbol.

        // Step 1: eps-remove + determinise. We use the existing
        // backend methods to keep behaviour consistent.
        let no_eps = Self::eps_remove(a)?;
        let det = Self::determinize(&no_eps)?;
        let mut inner = det.inner;

        // Edge case: empty FST (no start) — complement is Σ_b*.
        if inner.start().is_none() {
            let mut sigma_star = VectorFst::<TropicalWeight>::new();
            let s = sigma_star.add_state();
            sigma_star.set_start(s).map_err(wrap_err)?;
            sigma_star
                .set_final(s, TropicalWeight::one())
                .map_err(wrap_err)?;
            for &l in alphabet {
                sigma_star
                    .add_tr(
                        s,
                        Tr::new(
                            l as RfstLabel,
                            l as RfstLabel,
                            TropicalWeight::one(),
                            s,
                        ),
                    )
                    .map_err(wrap_err)?;
            }
            return Ok(RustFstWrapper::new(sigma_star));
        }

        // Step 2: add a dead/sink state.
        let dead = inner.add_state();

        // Step 3: complete every state (including the dead state)
        // over `alphabet`. For each state, gather the input labels
        // already present on outgoing arcs; for every alphabet symbol
        // not present, add an arc to `dead`.
        let n_states = inner.num_states();
        for s in 0..(n_states as RfstStateId) {
            let trs = inner.get_trs(s).map_err(wrap_err)?;
            // Collect the input labels of existing outgoing arcs.
            // (Acceptors have ilabel==olabel; we key on ilabel for
            // determinism per the precondition.)
            let mut present: std::collections::HashSet<RfstLabel> =
                std::collections::HashSet::new();
            for tr in trs.iter() {
                present.insert(tr.ilabel);
            }
            drop(trs);
            for &l in alphabet {
                if !present.contains(&(l as RfstLabel)) {
                    inner
                        .add_tr(
                            s,
                            Tr::new(
                                l as RfstLabel,
                                l as RfstLabel,
                                TropicalWeight::one(),
                                dead,
                            ),
                        )
                        .map_err(wrap_err)?;
                }
            }
        }

        // Step 4: swap final / non-final. Gather original final flags
        // first (before mutating).
        let n_states_total = inner.num_states();
        let mut was_final = vec![false; n_states_total];
        for s in 0..(n_states_total as RfstStateId) {
            was_final[s as usize] = inner
                .final_weight(s)
                .map_err(wrap_err)?
                .is_some();
        }
        // Now flip: clear all finals, set non-finals as final.
        for s in 0..(n_states_total as RfstStateId) {
            if was_final[s as usize] {
                inner.delete_final_weight(s).map_err(wrap_err)?;
            } else {
                inner
                    .set_final(s, TropicalWeight::one())
                    .map_err(wrap_err)?;
            }
        }

        Ok(RustFstWrapper::new(inner))
    }

    fn intersect(a: &Self::Fst, b: &Self::Fst) -> FstResult<Self::Fst> {
        // Intersection of acceptors = composition. Both operands are
        // identity transducers (input==output per arc), so composing
        // them produces an FST whose accepting paths are exactly the
        // strings accepted by both. Standard arc-sort discipline
        // applies internally so callers don't have to remember it.
        let left = Self::arc_sort_output(a)?;
        let right = Self::arc_sort_input(b)?;
        Self::compose(&left, &right)
    }

    fn determinize(a: &Self::Fst) -> FstResult<Self::Fst> {
        let out: VectorFst<TropicalWeight> = rfst_determinize(&a.inner).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn minimize(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_minimize(&mut out).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn eps_remove(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_rm_epsilon(&mut out).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn arc_sort_input(a: &Self::Fst) -> FstResult<Self::Fst> {
        // rustfst's `tr_sort` is in-place on a mutable FST and sets the
        // I_LABEL_SORTED / O_LABEL_SORTED property bit so that downstream
        // `compose` knows the sort holds (the matcher checks the bit, not
        // the actual arc order — F2c1 surfaced this). The comparator
        // ordering is selected by the `TrCompare` type parameter.
        let mut out = a.inner.clone();
        rfst_tr_sort(&mut out, ILabelCompare {});
        Ok(RustFstWrapper::new(out))
    }

    fn arc_sort_output(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_tr_sort(&mut out, OLabelCompare {});
        Ok(RustFstWrapper::new(out))
    }

    fn invert(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_invert(&mut out);
        Ok(RustFstWrapper::new(out))
    }

    fn reverse(a: &Self::Fst) -> FstResult<Self::Fst> {
        let out: VectorFst<TropicalWeight> = rfst_reverse(&a.inner).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn connect(a: &Self::Fst) -> FstResult<Self::Fst> {
        let mut out = a.inner.clone();
        rfst_connect(&mut out).map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn replace(
        root: &Self::Fst,
        root_label: Label,
        label_to_fst: &[(Label, Self::Fst)],
    ) -> FstResult<Self::Fst> {
        // rustfst's replace API takes Vec<(Label, B)> where B: Borrow<F>,
        // and the root must appear in the list at root_label.
        let mut entries: Vec<(RfstLabel, &VectorFst<TropicalWeight>)> = Vec::new();
        entries.push((root_label as RfstLabel, &root.inner));
        for (lbl, fst) in label_to_fst {
            entries.push((*lbl as RfstLabel, &fst.inner));
        }
        let out: VectorFst<TropicalWeight> =
            rfst_replace::<TropicalWeight, VectorFst<TropicalWeight>, VectorFst<TropicalWeight>, _>(
                entries,
                root_label as RfstLabel,
                /* epsilon_on_replace */ true,
            )
            .map_err(wrap_err)?;
        Ok(RustFstWrapper::new(out))
    }

    fn paths(fst: &Self::Fst) -> FstResult<Box<dyn Iterator<Item = Path> + '_>> {
        // rustfst's `paths_iter` returns a `PathsIterator<'a, W, F>` whose
        // items are `FstPath<W>` (per `src/fst_path.rs`). Each `FstPath` has
        // `ilabels: Vec<Label>` and `olabels: Vec<Label>` with ε already
        // stripped (see `FstPath::add_to_path` source). We just rebrand to
        // our `Path` type and box the result.
        //
        // **Important — laziness matters**: PathsIterator does BFS and on
        // a cyclic FST (e.g. Kleene-star result) it yields infinitely many
        // paths. The iterator is exposed lazily so callers can `take(n)` /
        // `filter` etc. without forcing the whole language. An eager
        // `.collect()` here would hang for any non-acyclic FST.
        let iter = fst.inner.paths_iter().map(|p| Path {
            input: p.ilabels.iter().map(|&l| l as Label).collect(),
            output: p.olabels.iter().map(|&l| l as Label).collect(),
        });
        Ok(Box::new(iter))
    }

    fn serialize(fst: &Self::Fst) -> FstResult<Vec<u8>> {
        // rustfst's SerializableFst has `store(Write)` and `load(&[u8])`.
        let mut buf: Vec<u8> = Vec::new();
        fst.inner.store(&mut buf).map_err(wrap_err)?;
        Ok(buf)
    }

    fn deserialize(bytes: &[u8]) -> FstResult<Self::Fst> {
        let inner: VectorFst<TropicalWeight> =
            VectorFst::<TropicalWeight>::load(bytes).map_err(|e| FstError::Deserialize(e.to_string()))?;
        Ok(RustFstWrapper::new(inner))
    }

    fn mmap_load(path: &FsPath) -> FstResult<Self::Fst> {
        // F-later: true zero-copy when we own the format (Option C in
        // fst-kernel-design.md §4). rustfst's binary loader requires a fully
        // materialised buffer, so this is a `read + deserialize` shim today.
        // The trait surface is unchanged from the future mmap design, so
        // upgrading is a private implementation change.
        let bytes = std::fs::read(path)?;
        Self::deserialize(&bytes)
    }

    fn num_states(fst: &Self::Fst) -> usize {
        fst.inner.num_states()
    }
}
