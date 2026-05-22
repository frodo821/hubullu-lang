# `rustfst` survey — wrap vs roll our own

Informs open question #1 of `docs/proposals/fst-morphology.md` (§8.1). The proposal tentatively recommends "wrap `rustfst`"; this survey tests that recommendation against the actual state of the crate.

Date: 2026-05-16.

---

## 1. Summary / TL;DR

**Wrap `rustfst`.** Confidence: high.

`rustfst` is a mature, actively maintained, dual-licensed Rust port of OpenFST that ships every primitive the proposal needs (concat, union, closure, composition, determinisation, minimisation, invert, project, reverse, connect, rm_epsilon, shortest-path) plus a `TrivialWeight` semiring that gives us unweighted FSTs cleanly. It has a real `ConstFst` for read-only deployment, OpenFST-compatible binary I/O, lightweight dependencies, no known critical correctness bugs, and roughly the right level of abstraction for us to build Kaplan-Kay rewrite-rule compilation on top. The two notable gaps — no memory-mapped loading, and no Kaplan-Kay layer — were already understood in the proposal. Rolling our own FST kernel would cost an estimated 4-6 engineer-weeks of work that buys us nothing the morphology layer cares about.

The only real surprise (mildly negative): no published MSRV in `Cargo.toml` despite the README citing 1.51; no `no_std` or WASM support; one open issue (#288) notes `determinize` diverges from OpenFST in some cases — likely benign for our use but worth a regression test when we cut over from `phonrule_eval`.

---

## 2. `rustfst` at a glance

| field | value |
|---|---|
| crate name | `rustfst` |
| latest version | **1.3.1** (April 2026) per lib.rs; crates.io GitHub releases show 1.2.6 in July 2025 |
| repo | https://github.com/garvys-org/rustfst |
| stars | 183 |
| open issues | 23 |
| commits on main | 1,874 |
| licence | Apache-2.0 OR MIT (dual) |
| MSRV | README says rustc >= 1.51.0; **not set in `Cargo.toml`** |
| monthly downloads | ~21k (lib.rs ranks #68 in Algorithms) |
| reverse deps | 6-7 direct |
| maturity | Production-grade per README; no explicit alpha/beta; v1.x stable since April 2023 |
| code size | ~30 kLOC Rust |
| language split | Rust 81% / Python 11% / C++ 7% (FFI + Python bindings) |
| dependencies | `anyhow`, `bimap`, `bitflags`, `generic-array`, `itertools`, `nom`, `num-traits`, `ordered-float`, `rand`, `serde`, `superslice` — all lightweight |

Maintainer activity: 1.0.0 (Apr 2023), 1.1.1 (Aug 2023), 1.2.6 (Jul 2025), 1.3.1 (Apr 2026). The cadence is slow but consistent — this is a stable library, not abandonware, but also not in heavy flux. The lab pattern is "ship it, then bug-fix releases."

### Stated use cases
README: speech recognition, machine translation, OCR, pattern matching, string processing, information extraction. No published morphology user, but XFST/HFST patterns map directly onto the algorithm set.

### Sibling crates in the ecosystem
- `rustfst-ffi` — C ABI for the same library
- `rustfst-python` — Python bindings (active, on PyPI)

We use neither.

---

## 3. Coverage of needed operations

For each operation the proposal needs, status in `rustfst` plus notes:

| operation | `rustfst` status | notes |
|---|---|---|
| state/arc model | ✓ `VectorFst<W>` (mutable) and `ConstFst<W>` (immutable) | `Tr` (transition) carries input label, output label, weight, destination state. State IDs `u32`, label IDs `u32`. |
| input vs output labels | ✓ first-class on every `Tr` | Exactly the transducer model we need. |
| epsilon transitions | ✓ label `0`, symbol `<eps>` (note: not OpenFST's `<epsilon>`) | Different from OpenFST string but mechanically the same. |
| symbol tables | ✓ `SymbolTable`, `Arc`-shared | Bidirectional `Symbol ↔ Label` mapping. |
| weight semiring: boolean / unweighted | ✓ `BooleanWeight` and `TrivialWeight` | `TrivialWeight` (added in 1.2.x) is a unit semiring — the cleanest match for our unweighted use. `BooleanWeight` also implements `WeaklyDivisibleSemiring` since 1.2.x, which matters for minimisation. |
| concatenation | ✓ `algorithms::concat` | Module-level function. |
| union | ✓ `algorithms::union` | |
| closure (Kleene `*`, `+`, optional) | ✓ `algorithms::closure` | Star + plus per OpenFST convention. Optional (`?`) is `union(fst, epsilon_acceptor)` — trivial to wrap. |
| composition `∘` | ✓ `algorithms::compose` | The headline feature. API ergonomics are flagged as awkward (issue #235: "type arguments to compose are horrendous") — we'll hide that behind our wrapper. |
| determinisation | ✓ `determinize`, `determinize_with_config` | Issue #288 notes behaviour diverges from OpenFST in some cases — investigate before relying on it for correctness-critical paths. |
| minimisation | ✓ `minimize`, `minimize_with_config`, `acceptor_minimize` | Hopcroft-style per the source. |
| inversion (swap I/O) | ✓ `invert` | Needed for reverse lookup. |
| projection (to acceptor on one side) | ✓ `project` with `ProjectType::{Input, Output}` | |
| reverse (flip arc direction) | ✓ `reverse` | Needed for some reverse-traversal constructions. |
| shortest-path / n-best | ✓ `shortest_path`, `shortest_path_with_config`, `shortest_distance` | Not strictly needed for unweighted morphology, but useful for ranking ambiguous analyses later. |
| connect / trim | ✓ `connect` | |
| ε-removal | ✓ `rm_epsilon` | |
| replace (lazy replacement) | ✓ `algorithms::replace` (module) | Useful for the recursion-flattening step in §6.1 of the proposal. |
| weight pushing | ✓ `push`, `push_with_config` | Not needed for boolean semiring; useful if we ever go weighted. |
| factor weight | ✓ `algorithms::factor_weight` | Not needed for our use. |
| `optimize` (det + min) | ✓ general-purpose convenience | Note one community-reported bug: a panic in `decode` under specific input shapes traceable to PR #166. Verify with our actual FSTs. |
| binary I/O (OpenFST-compatible) | ✓ `read`, `write` on `VectorFst`/`ConstFst` via `SerializableFst` trait | Compatible with OpenFST. |
| text I/O (OpenFST text format) | ✓ `read_text`, `write_text` | Useful for debug snapshots. |
| GraphViz output | ✓ via `SerializableFst` | Useful for proposal §A appendix-style debugging. |
| `ConstFst → VectorFst` conversion | ✓ `From` impl | Roundtrip is clean. |

**Out-of-the-box coverage of the proposal's needs: essentially complete at the primitive level.**

What we still build sits one level up.

---

## 4. What we'd still build

These are explicitly absent from `rustfst` and we'd own them regardless of choice:

1. **Kaplan-Kay rewrite-rule compilation** (proposal §3, §F2). `rustfst` provides no rule-compilation layer; this is the proposal's estimated 4-week chunk. Roughly 1500-2500 LOC. Mohri's 1996 paper ("An Efficient Compiler for Weighted Rewrite Rules") and Karttunen's "Replace Operator" (1995) are the operative algorithms. Must handle: literal LHS/RHS, alternation classes, ranges, quantifiers, syllable-aware contexts, iteration-to-convergence.
2. **Higher-level grammar combinators**: XFST-style operations such as `compile-replace`, `ignore`, `restriction`. We need at minimum a `restrict_by_label_set` helper to implement slot filters (§5 of fst-morphology). Estimated 300-500 LOC of constructions composed from `rustfst` primitives.
3. **Slot-grammar compiler** (proposal §F4). Turning `ComposeExpr`/`SlotDef` into FSTs by combining `concat`, `closure`, `compose`, and our `restrict`. Estimated 800-1200 LOC.
4. **Lexicon FST builder** (proposal §F3). Turning the morpheme inventory into a tagged-path acceptor. Estimated 200-400 LOC.
5. **Per-entry FST specialisation** (proposal §F5). Binding stems into the chain FST. Small — 100-200 LOC.
6. **Circumfix / infix coupling constructions** (proposal §6.3, §6.4). Synchronisation symbols + marker insertion FSTs. ~200 LOC each, plus tests.
7. **Recursion flattening pass** (proposal §6.1). A topo sort over entry refs and inline-expansion with a depth bound. ~300 LOC.
8. **`.huc` envelope format** (proposal §4). Postcard wrapper around `rustfst` binary blobs + AST + metadata. ~500-800 LOC.
9. **Analyser API** (`hubullu analyze <surface>`, proposal §F6). Reverse traversal entry point. ~200 LOC + CLI plumbing.
10. **Two-level operators**, if we ever want them. Not in scope per proposal §3.

`rustfst` won't help with any of (1)-(9). It will, however, give us the math to build them all from.

---

## 5. Trade-off summary

### Option A — Wrap `rustfst`

**Pro:**
- Every FST primitive we need is implemented, tested, benchmarked.
- Composition + minimisation are the hardest algorithms to get right; the cost of re-implementing them correctly is high (Hopcroft minimisation alone is a multi-week project to ship without bugs in subtle inputs).
- `ConstFst` is a real read-only representation we can target for the on-disk `.huc` blob.
- OpenFST-compatible binary format means our compiled FSTs interoperate with the wider FST tooling ecosystem — useful for debugging and validation against XFST/Foma reference outputs.
- Dependencies are sober (`anyhow`, `nom`, `serde`, lightweight numeric utilities) — no surprise transitive bloat.
- Apache-2.0 OR MIT licence matches hubullu's licensing posture.
- Maintained by a real org with active commits.

**Con:**
- API ergonomics: `compose` has unwieldy type signatures (issue #235). We mitigate by hiding behind a `hubullu::fst` facade.
- One open determinisation behaviour-divergence issue (#288) and one community-reported `optimize` panic. Both need regression tests against our actual FSTs before we trust the optimisation pipeline. Neither is a deal-breaker.
- No `no_std`, no WASM, no zero-copy / memory-mapped loading. We pay full deserialisation cost on `.huc` load. For hubullu's scale (entries in the hundreds-to-low-thousands) this is irrelevant; for the future Phase F9-bis large-lexicon case, we may need to add an mmap layer ourselves or upstream one.
- MSRV is unspecified in `Cargo.toml` — we pin a Rust version in our own toolchain.
- `TrivialWeight` is recent (1.2.x); we depend on >=1.2.6.
- Adds ~30 kLOC of transitive Rust code to hubullu's tree (compile-time cost: real but acceptable).

### Option B — Roll our own

**Pro:**
- Full control over the data model — we could make labels carry our `MorphemeId` newtypes natively rather than mapping through `u32`.
- Zero-copy `.huc` loading is buildable from day one if we design for it.
- Smaller binary footprint by ~1MB.
- No dependency on a third-party crate's release cadence.

**Con:**
- Composition + minimisation are non-trivial to ship correctly. A minimum-viable FST library that gives us `concat`, `union`, `closure`, `compose`, `determinize`, `minimize`, `invert`, `project`, `connect`, `rm_epsilon` is realistically **3000-5000 LOC and 4-6 engineer-weeks** for an engineer not previously steeped in finite-state algorithms. Hopcroft minimisation alone is ~500 LOC of subtle code; composition with epsilon filters is another ~600.
- We'd still need all of §4 items (1)-(9) on top.
- Risk is loaded entirely on the engine layer that the morphology layer is supposed to take for granted.
- No OpenFST interop for debugging — we couldn't compare our FSTs to reference XFST/Foma outputs side-by-side.

The "roll our own" budget would let us deliver F1 (FST infrastructure) in proposal terms, but it pushes the F2 (Kaplan-Kay) work later by an equivalent amount, with no morphology-layer benefit to show for the detour.

---

## 6. Recommendation

**Wrap `rustfst`.** Specifically, depend on `rustfst >= 1.2.6` (for `TrivialWeight`) — when `1.3.x` stabilises we can move to it.

Reasoning, not hedged:

1. **The proposal's hard problem is Kaplan-Kay rule compilation, not FST primitives.** Spending 4-6 weeks rebuilding primitives that exist, are tested, and are benchmarked-faster-than-OpenFST is a misallocation. That time should go directly into §F2 (rewrite-rule compilation), which is genuinely novel work for us.
2. **`TrivialWeight` plus `BooleanWeight` cleanly cover "unweighted morphology."** We don't get pushed into a weighted semiring we don't need. The proposal's §9 non-goal "not implementing weighted FSTs" is respected.
3. **`ConstFst` plus OpenFST binary format gives us a working on-disk story** without us designing one. Wrap each `ConstFst` blob in a postcard envelope per proposal §4; done.
4. **The known issues are bounded and testable.** Determinisation divergence (#288): write a regression test comparing `determinize`'d FST behaviour against an explicit reference FST for our actual phonrule cases — if we hit it, we either avoid `determinize` in those cases or contribute a fix upstream. `optimize` panic: we can sidestep by calling `determinize` then `minimize` explicitly. Neither blocks F1.
5. **Licence and ecosystem signals are healthy.** Apache/MIT dual, ~21k monthly downloads, 1.8k commits, recent releases, real reverse-dependency users. Not a research toy.
6. **If `rustfst` ever falters,** the wrapper layer (§7 below) gives us a swap-out boundary. We are not married to it; we are leveraging it.

The proposal's tentative recommendation holds. Promote it from tentative to firm.

---

## 7. Wrapper API sketch

The wrapper lives at `hubullu::fst` and is the only module in the codebase that mentions `rustfst` types. Everything else uses our types.

```rust
// hubullu/src/fst/mod.rs

use rustfst::semirings::BooleanWeight;
use rustfst::fst_impls::{VectorFst, ConstFst};

/// Our label type. Wraps u32 internally; carries phantom for input vs output.
pub struct Label<Side>(u32, PhantomData<Side>);
pub struct Input; pub struct Output;
pub type ILabel = Label<Input>;
pub type OLabel = Label<Output>;

/// Symbol table indexed by side.
pub struct Alphabet { input: SymbolTable, output: SymbolTable }
impl Alphabet {
    pub fn intern_input(&mut self, s: &str) -> ILabel;
    pub fn intern_output(&mut self, s: &str) -> OLabel;
    pub fn lookup_input(&self, l: ILabel) -> Option<&str>;
    pub fn lookup_output(&self, l: OLabel) -> Option<&str>;
}

/// Mutable FST under construction.
pub struct FstBuilder { /* wraps VectorFst<BooleanWeight> */ }
impl FstBuilder {
    pub fn new() -> Self;
    pub fn add_state(&mut self) -> StateId;
    pub fn set_start(&mut self, s: StateId);
    pub fn set_final(&mut self, s: StateId);
    pub fn add_arc(&mut self, from: StateId, to: StateId, ilabel: ILabel, olabel: OLabel);
    pub fn add_epsilon_arc(&mut self, from: StateId, to: StateId);
    pub fn finish(self) -> Fst;
}

/// Immutable FST. The currency of the compile pipeline.
pub struct Fst { /* wraps ConstFst<BooleanWeight> + Alphabet */ }

impl Fst {
    // -- combinators (each returns a new Fst; pure) --
    pub fn concat(&self, other: &Fst) -> Fst;
    pub fn union(&self, other: &Fst) -> Fst;
    pub fn closure_star(&self) -> Fst;
    pub fn closure_plus(&self) -> Fst;
    pub fn optional(&self) -> Fst;
    pub fn bounded_repeat(&self, n: u32, m: u32) -> Fst;
    pub fn compose(&self, other: &Fst) -> Result<Fst>;
    pub fn invert(&self) -> Fst;            // swap input/output
    pub fn project_input(&self) -> Fst;      // -> acceptor on input side
    pub fn project_output(&self) -> Fst;     // -> acceptor on output side
    pub fn reverse(&self) -> Fst;            // flip arc direction

    // -- optimisation --
    pub fn determinize(&self) -> Result<Fst>;
    pub fn minimize(&self) -> Fst;
    pub fn rm_epsilon(&self) -> Fst;
    pub fn connect(&self) -> Fst;
    pub fn optimize(&self) -> Fst;           // det + min, with our own pipeline (not rustfst::optimize)

    // -- traversal --
    pub fn forward(&self, input: &[ILabel]) -> impl Iterator<Item = Vec<OLabel>>;
    pub fn analyse(&self, surface: &[OLabel]) -> impl Iterator<Item = Vec<ILabel>>;
        // analyse = invert + forward, hidden from caller
    pub fn paths(&self) -> impl Iterator<Item = (Vec<ILabel>, Vec<OLabel>)>;

    // -- I/O --
    pub fn to_bytes(&self) -> Vec<u8>;       // rustfst binary, alphabet-stripped
    pub fn from_bytes(bytes: &[u8], alpha: Arc<Alphabet>) -> Result<Self>;

    // -- introspection --
    pub fn num_states(&self) -> usize;
    pub fn num_arcs(&self) -> usize;
    pub fn alphabet(&self) -> &Alphabet;
}

// Marker symbols used by morphology layers (boundary, circumfix sync, infix slot).
// Reserved labels in the input alphabet.
pub mod markers {
    pub const BOUNDARY: ILabel = /* reserved label 1 */;
    pub const CIRCUMFIX_SYNC_BASE: u32 = /* reserved range */;
    pub const INFIX_POS_BASE: u32 = /* reserved range */;
}

// Rewrite-rule compilation lives one level up, built ON Fst, NOT in this module.
// hubullu::fst::rewrite (proposal §F2) is a separate file.
```

Key design points:
- The wrapper hides `rustfst`'s awkward composition type signatures (issue #235) behind a uniform `Fst -> Fst` API.
- We commit to `BooleanWeight` (or `TrivialWeight`) at the wrapper level. The rest of the codebase sees an unweighted transducer.
- `ConstFst` is the storage form; `VectorFst` is hidden inside `FstBuilder`. `optimize`/`minimize` convert through.
- `Alphabet` is owned per-FST or shared via `Arc`; the wrapper hides `rustfst::SymbolTable` entirely.
- The `analyse` method is just `invert(); forward()` but presenting it as a first-class operation matches proposal §5's framing.
- Determinisation returns `Result` (a guard against the #288 divergence — we can swap it for a custom implementation if needed without breaking callers).

The `hubullu::fst::rewrite` module (Kaplan-Kay compilation, proposal §F2) sits on top of this and is the place to spend our complexity budget.

---

## 8. Risks and follow-ups

1. **Verify `determinize` against issue #288** before relying on it in F2. Write a regression test using a worked-example phonrule with known FST shape.
2. **Verify `optimize` panic** by exercising it on a representative chain × phonrule composition. If it panics, use explicit `determinize` + `minimize` pipeline.
3. **Pin MSRV ourselves.** `rustfst` doesn't pin theirs; we'll set hubullu's `rust-version` to the highest version `rustfst 1.2.6+` actually requires (probably 1.65+ given `bitflags 2.5`, `itertools 0.14`, `ordered-float 5`).
4. **Track `1.3.x` stabilisation.** `1.3.1` is recent (April 2026); skim the changelog when it lands fully before adopting.
5. **Watch for upstream `no_std`/WASM PRs.** Not blocking, but if we ever want browser-side traversal it becomes relevant.
6. **Document the marker-symbol reservation scheme** in `hubullu::fst::markers` early — getting circumfix and infix synchronisation right depends on it (proposal §6.3, §6.4).

---

## 9. References

- `rustfst` on crates.io: https://crates.io/crates/rustfst
- `rustfst` on docs.rs: https://docs.rs/rustfst/latest/rustfst/
- `rustfst` on GitHub: https://github.com/garvys-org/rustfst
- `rustfst` on lib.rs (stats): https://lib.rs/crates/rustfst
- BurntSushi's `fst` (read-only automata, the alternative for non-transducer needs): https://github.com/BurntSushi/fst
- Mohri, M. (1996). "An Efficient Compiler for Weighted Rewrite Rules." https://aclanthology.org/P96-1031.pdf
- Karttunen, L. (1995). "The Replace Operator." https://aclanthology.org/P95-1003.pdf
- Companion proposal: `docs/proposals/fst-morphology.md` (§3, §4, §6, §8.1)
