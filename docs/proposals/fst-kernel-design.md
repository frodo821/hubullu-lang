# In-tree FST kernel — design and cost

Companion to `docs/proposals/fst-morphology.md` (architecture) and `docs/proposals/rustfst-survey.md` (which recommended wrapping `rustfst`). This document designs the alternative — a minimal in-tree FST kernel — concretely enough for a build-vs-buy decision on numbers.

Trigger: `rustfst` has no mmap / zero-copy / `no_std` / WASM. Mmap matters most; a future large-lexicon analyser can't afford full deserialisation on load. **Mmap is a first-class requirement** below.

Design only. No code.

Date: 2026-05-16.

---

## 1. Scope

The morphology layer (`fst-morphology.md` §3, §4, §6) uses a narrow subset of FST machinery. Aligned with `rustfst-survey.md` §3 / §7:

**Need:**

- Mutable FST type: `add_state`, `set_start`, `set_final`, `add_arc(input_label, output_label, next_state)`. Label `0` = ε.
- Symbol tables: string ↔ integer label maps, one per side.
- Concat, union, closure (`*`, `+`, `?`, bounded `{n,m}`) — slot-grammar building blocks (§3, §F4).
- Composition (`∘`) — Kaplan-Kay rule chains, rule-over-lexicon (§F2, §4).
- ε-removal — pre-composition cleanup.
- Determinisation — unambiguous traversal.
- Minimisation — keep per-entry FSTs small.
- Inversion — swap I/O labels (free reverse-lookup).
- Reverse — flip arc direction.
- Connect / trim — drop unreachable states.
- Replace — recursion-flattening (§6.1).
- Path enumeration — forward render + reverse analyser.
- Mmap-friendly serialisation: load = `mmap` + cast header + index body. No parse, no copy on hot path.

**Don't need (design omits):**

- Weighted FSTs / arbitrary semirings. Unweighted only — no weight field on disk.
- Shortest-path / n-best.
- Lazy / dynamic FST representations.
- OpenFST format compatibility. (Big cost-saver — we own the format.)
- Live mutation after minimisation.
- Compact representations beyond the mmap layout.
- Two-level (Koskenniemi) compilation.
- Text I/O / GraphViz output. (~30 lines of ad-hoc dump for debug.)

The omitted half is most of `rustfst`'s 30 kLOC: weighted semirings (generic over `W`), lazy FSTs, OpenFST format, shortest-path, factor-weight, push, FFI shims, Python bindings, graphviz. All gone.

---

## 2. Data structures

In-memory representation, fields and rough sizes:

```rust
type Label = u32;       // 0 = ε
type StateId = u32;     // invalid sentinel = u32::MAX

struct Arc {
    input: Label,       // 4 B
    output: Label,      // 4 B
    next: StateId,      // 4 B
}                       // 12 B total, no padding

struct State {
    is_final: bool,     // 1 B (packed in real layout)
    arcs: Vec<Arc>,     // 24 B Vec header + 12 B per arc
}

struct SymbolTable {
    name_to_id: FxHashMap<String, Label>,  // ~40 B/entry + key
    id_to_name: Vec<String>,                // 24 B/entry + key
}

struct Fst {
    states: Vec<State>,
    start: Option<StateId>,
    input_symbols: Arc<SymbolTable>,
    output_symbols: Arc<SymbolTable>,
}
```

Sizing for a representative per-entry FST (Turkish verb, ~500 states, ~3 arcs per state on average = 1,500 arcs):

- Arc payload: 1500 × 12 B = **18 KB**
- State headers (Vec headers): 500 × 24 B = 12 KB
- Final-state bitset (in mmap form): 500/8 = 63 B
- **Per-FST in-memory: ~30 KB.**

Symbol tables are shared across all per-entry FSTs of one language. Proto + Turkish each have on the order of 200–500 input symbols (tag values, morpheme identities, markers) and a few hundred output symbols (phoneme/grapheme inventory). Per-language symbol-table cost: **~30–50 KB**.

A language `.huc` with ~300 entries → 300 × 30 KB = **~9 MB FST data + ~50 KB symbol tables**. Comfortable for both heap and mmap.

The mmap form (§4) drops the `Vec` headers and uses fixed-width offset tables instead, costing the same or less than the in-memory form.

---

## 3. Algorithm sketches

**ε-removal.** For each state `s`, compute ε-closure `E(s)`; copy each `t ∈ E(s)`'s non-ε out-arcs to `s`; mark `s` final iff any `t ∈ E(s)` is; drop ε-arcs. Reference: Mohri (2002). Hard: in transducers, input-ε with non-ε output is not removable without changing the path-set. We limit to symmetric ε (both sides) — sufficient for cleanup; asymmetric ε lives in the composition filter. ~250 LOC.

**Composition with ε-filter.** Product construction: states of `A ∘ B` are pairs `(a, b)`; arc `(a,b) → (a',b')` exists when `A: a →[i:k] a'` and `B: b →[k:o] b'`. Wrinkle: when both sides emit ε, naive product over-counts duplicate ε-paths. Remedy: Mohri's (1997) three-state ε-filter, tracking which side last took an ε-step to forbid non-canonical orderings. References: Mohri (1997); Allauzen et al. (2007) OpenFst paper §3.3. Hard: the filter is easy to get wrong (dropping legitimate paths, mishandling final states). Even mature implementations ship bugs here — `rustfst`'s `determinize` (#288) and `optimize` panic are in the same family. This is the single largest correctness risk. 500–700 LOC.

**Determinisation.** Acceptor case = Rabin-Scott subset construction. Transducer case = subsets paired with residual output strings (Mohri 1997). Hard: termination not guaranteed for arbitrary transducers — only for functional / twins-property ones. Forward is functional by construction (we reject ambiguous chains at compile time, §6.6); reverse may be genuinely ambiguous and should be determinised as an acceptor on the output side only. ~350 LOC, plus a non-determinisable-input guard.

**Minimisation (Hopcroft).** Partition-refinement: start with `{final, non-final}`; refine until `δ(s, α)` and `δ(t, α)` land in the same block for all `α`. Reference: Hopcroft (1971). For FSTs, treat `(input, output)` pairs as composite alphabet. Hard: subtle off-by-ones in the splitter queue, unreachable-state handling, missed splits. ~450 LOC. Moore (1956) is simpler (~300 LOC, O(n²)) and good enough at our scale — implement first as a verification reference.

**Replace.** Splice non-terminal-labelled arcs into a root FST per `NT_i → FST_i`. Reference: Allauzen & Riley (2012). Implements §6.1 Option A flattening — clitic paradigms inline up to a depth bound (default 8). Hard: clean cycle-exceeds-bound errors; ordering when an `NT` appears inside another `NT`'s FST. ~350 LOC.

**Mmap serialisation.** Emit header + body laid out for `cast_at_offset` + arithmetic; no parsing. Reference: `BurntSushi/fst`'s [FORMAT.md](https://github.com/BurntSushi/fst/blob/master/FORMAT.md) and `rkyv`'s archive layout. Discipline: fixed-width records, explicit endianness (LE only), magic + version + checksum, alignment-aware offsets so `&[Arc]` slices cast directly. Hard: `#[repr(C)]` with proper alignment to avoid UB; offset-table symbol layout so lookups don't allocate. ~400–500 LOC for emit + load + validate.

**Path traversal.** Worklist of `(state, position, output_so_far)`; ambiguity = branches. Reverse traversal = forward on the inverted FST. Hard: ε-loops; cap path length (default `surface_len × 4`) and error on overflow. ~250 LOC.

---

## 4. Mmap binary format — concrete sketch

Hubullu FST on-disk layout, designed for **mmap + cast + walk** without parsing. All multi-byte fields are little-endian. Magic number identifies us; version field lets us hard-fail on incompatible files (no migration — recompile from `.hu`).

```
Offset  Size      Field                    Notes
------  --------  -----------------------  -------------------------------------------
0x00    8 B       magic                    "HUFST\0\0\0"
0x08    2 B       version_major: u16       bump = breaking; refuse to load on mismatch
0x0A    2 B       version_minor: u16       bump = additive; load if major matches
0x0C    4 B       flags: u32               bit 0 = has_checksum; rest reserved
0x10    4 B       state_count: u32
0x14    4 B       arc_count: u32
0x18    4 B       input_symbol_count: u32
0x1C    4 B       output_symbol_count: u32
0x20    4 B       start_state: u32         u32::MAX = no start
0x24    4 B       reserved: u32            for future feature bits / alignment
0x28    8 B       final_bitset_offset: u64
0x30    8 B       state_offsets_offset: u64
0x38    8 B       arcs_offset: u64
0x40    8 B       input_symtab_offset: u64
0x48    8 B       output_symtab_offset: u64
0x50    4 B       checksum: u32 (CRC32C)   covers bytes 0x00 .. end-of-symtabs
0x54    12 B      padding                  align body to 64 B
0x60    ...       BODY                     sections in offset-table order
```

### Body sections

**Final-state bitset.** `ceil(state_count / 8)` bytes; bit `i` set iff state `i` is final. Aligned to 8 B.

**State offsets table.** `state_count × u32` byte offsets *into the arcs section* indicating where state `i`'s arc list begins. Aligned to 4 B. State `i`'s arc list length is `state_offsets[i+1] - state_offsets[i]` divided by `sizeof(Arc)` (`= 12`). A sentinel `state_offsets[state_count]` is written so the last state's length is computable uniformly.

**Arcs section.** Packed `Arc` records: `[input: u32][output: u32][next: u32]`, 12 B per arc, no padding. State `i`'s arcs occupy `&arcs[state_offsets[i] .. state_offsets[i+1]]` (byte offsets); this slice can be cast to `&[Arc]` directly given the file is aligned. The arc array is sorted by `(input, output)` per state to enable binary search during traversal.

**Symbol tables.** Two of them (input, output). Each table is:

```
[entry_count: u32]
[string_offsets: entry_count × u32]   // byte offsets into the string blob
[string_blob: packed UTF-8, no separators]
[blob_length: u32]                    // for bounds checks
```

Looking up symbol `i`'s name: `&blob[string_offsets[i] .. string_offsets[i+1]]`. (A sentinel offset is appended so the last string's length is uniform.)

### Format guarantees

- **Endianness**: little-endian only; loader asserts.
- **Alignment**: file page-aligned via `mmap`. Body starts at `0x60` (64 B aligned); arcs and offsets are 4 B aligned. Loader refuses on mismatch.
- **Versioning**: `version_major` bump = breaking, refuse to load. `version_minor` = additive (optional sections behind flag bits). v1.0.0.
- **Checksum**: optional CRC32C over all bytes excl. checksum field. `flags` bit 0 indicates presence; off by default for streamed writes, on for distribution.
- **Random access**: state `i` arcs = two `u32` reads from offsets + slice into arcs. O(1), no alloc, no decode.
- **Traversal**: step from `s` on label `l` = binary search in `state_arcs[s]` for `(l, _)` — O(log k); typical k < 10.

### Lookup sketch

```
struct MmapFst<'a> {
    header: &'a Header,                  // cast from buffer[0..]
    final_bitset: &'a [u8],
    state_offsets: &'a [u32],
    arcs: &'a [Arc],                     // requires alignment
    input_strings: SymtabView<'a>,
    output_strings: SymtabView<'a>,
}

fn state_arcs(&self, s: StateId) -> &[Arc] {
    let lo = self.state_offsets[s as usize] as usize / size_of::<Arc>();
    let hi = self.state_offsets[s as usize + 1] as usize / size_of::<Arc>();
    &self.arcs[lo..hi]
}
```

No copy. No parse. Loading a 100 MB language `.huc` is `mmap()` syscall + header validation = single-digit milliseconds.

---

## 5. Module breakdown with LOC estimates

| Module | Purpose | LOC | Risk |
|---|---|---|---|
| `fst::core` | `Fst`, `State`, `Arc`, builder, mutation primitives | 250 | low |
| `fst::symtab` | `SymbolTable` + interning + serialisation hooks | 150 | low |
| `fst::ops::basic` | `concat`, `union`, `closure_star/plus/optional`, `bounded_repeat` | 200 | low |
| `fst::ops::epsilon` | `rm_epsilon` for symmetric-ε transducers | 250 | medium |
| `fst::ops::composition` | classical product construction + Mohri 3-state ε-filter | 600 | **high** |
| `fst::ops::determinize` | subset construction with output-residual; functional-input guard | 350 | medium |
| `fst::ops::minimize` | Hopcroft partition refinement (or Moore as v1) | 450 | medium |
| `fst::ops::invert_reverse_trim` | invert (label swap), reverse (arc flip), connect (reachability) | 150 | low |
| `fst::ops::replace` | non-terminal expansion with depth-bound cycle check | 350 | medium |
| `fst::traversal` | forward + reverse path iterators; ε-loop guard | 250 | low |
| `fst::serialize::emit` | write mmap binary format from an `Fst` | 300 | medium |
| `fst::serialize::load` | mmap loader, header/alignment/checksum validation, `MmapFst` view | 250 | low |
| `fst::debug` | tiny text-dump + dot output for tests/diagnosis | 100 | low |
| tests (kernel) | per-op unit tests | 600 | low |
| tests (ported from rustfst) | cross-validation on golden FSTs | 800–1200 | low |
| **Total** | | **5050–5450** | |

This is somewhat higher than the survey's "3000–5000 LOC, 4–6 weeks" estimate because: (a) the survey didn't include mmap serialisation, which adds ~550 LOC; (b) the survey didn't budget test-porting weight; (c) `replace` was rolled into the "stuff on top of `rustfst`" bucket in the survey, but in the in-tree design it sits inside the kernel.

The Mohri-1996 paper on weighted rewrite-rule compilation cited in the survey is **not** in this kernel — it stays one layer up (`fst::rewrite`, ~2000 LOC), unchanged from the survey's accounting. Both options pay it.

---

## 6. Engineer-week estimate

Per section. Assumes a single Rust-fluent engineer who has read Mohri (1997) and Hopcroft (1971) once and has not previously implemented FST algorithms. Includes test-writing inline (kernel unit tests; cross-validation tests counted separately at the end).

| Section | Weeks |
|---|---|
| Core + symtab + basic ops (concat, union, closure, bounded repeat) | 1.0 |
| ε-removal | 0.5 |
| Composition with ε-filter | **2.5** |
| Determinise + minimise | 2.0 |
| Invert / reverse / trim / replace | 1.0 |
| Path traversal (forward + reverse) | 0.5 |
| Mmap serialise + load + validate | 1.5 |
| Test porting + cross-validation against `rustfst` | 1.5 |
| Documentation / integration buffer | 0.5 |
| **Total** | **10.5 weeks** |

Range: **8–11 weeks**, composition being the dominant variable. If it takes 4 weeks instead of 2.5 (plausible given the ε-filter's reputation), total goes to 12. If `replace` needs pushdown semantics for non-trivial cycles, add another week.

Honest framing: the user's intuition *"使う部分だけならそんなに重くはならない"* is partly right, partly wrong. LOC savings from omitting weighted semirings, lazy FSTs, OpenFST format, shortest-path, push are **real and large** (~20 of 30 kLOC dropped → ~5 kLOC). But engineer-week savings on the operations we *keep* are smaller, because composition, determinisation, and minimisation are heavyweight regardless of semiring. Mohri's ε-filter is the same state count whether arcs carry `1.0` or nothing.

The kernel is **not** ~4 weeks. It is ~10.

---

## 7. Side-by-side comparison

Three options, end-to-end.

| Aspect | A. Wrap `rustfst` | B. `rustfst` + custom mmap layer | C. In-tree kernel |
|---|---|---|---|
| Engineer-weeks (kernel) | 0 | 0 | 8–11 |
| Engineer-weeks (mmap) | 0 (no mmap) | 1.5–2 | included |
| Engineer-weeks (rewrite-rule, on top) | 4 | 4 | 4 |
| **Total to first end-to-end render** | **~4 wk** | **~5.5 wk** | **~14 wk** |
| LOC we own (kernel layer) | ~500 (wrapper) | ~1500 (wrapper + emit + load) | ~5000 (full kernel) |
| LOC we own (everything on top, same in all 3) | ~3500 | ~3500 | ~3500 |
| External dependency (Rust crates) | `rustfst` ~30 kLOC | `rustfst` ~30 kLOC | none |
| Mmap / zero-copy load | no | **yes** | **yes** |
| Multi-process FST sharing (one mmap, many readers) | no | yes | yes |
| `no_std` / WASM | no | no (rustfst pulls `std`) | yes (with care) |
| OpenFST binary interop (debug against XFST/Foma) | yes | yes | no |
| Maintenance burden | low (track upstream) | medium (track upstream + own format) | **high** (we own every bug) |
| Algorithm correctness | inherited from `rustfst`; known issues #288 / `optimize` panic, both bounded | inherited from `rustfst`; same | **must validate ourselves** against external reference |
| Composition rule-compile readiness (i.e. when can `fst::rewrite` start?) | now | now | week 8–11 |
| Dictionary scale ceiling (realistic) | ~10 MB FST-in-heap | ~1 GB FST-on-disk | ~1 GB FST-on-disk |
| Risk of "subtle bug discovered at month 6" | low (rustfst is exercised by speech/MT users) | low–medium | **medium–high** |
| Ability to fix bugs without upstream | low (PR + wait) | low | high |
| Compile-time cost added to `cargo build` | ~30 kLOC of deps | ~30 kLOC of deps | none |

**The decisive cell**: row "Total to first end-to-end render." A is 4 weeks; B is 5.5 weeks; C is 14 weeks. The 8.5-week delta between B and C is the price of owning the kernel.

**The other decisive cell**: row "Mmap / zero-copy load." A cannot deliver it. B and C can. If mmap is non-negotiable, A is out and the comparison is B vs C only.

---

## 8. Borrowing `rustfst`'s tests

`rustfst-tests-data` (in-repo) ships golden FST inputs + expected outputs derived from OpenFST. Each case: `(input.fst.txt, operation, expected_output.fst.txt)`.

Categories worth porting (~5–10 families):

1. **Composition correctness** — synthetic FST pairs, ε-bearing transducers. Most valuable category — directly exercises the Mohri ε-filter.
2. **Determinisation correctness** — including the #288 divergence cases; pick OpenFST as ground truth, document either way.
3. **Minimisation** — idempotency (`minimize(minimize(F)) == minimize(F)`), language preservation, monotone arc count.
4. **ε-removal** — language preservation, no ε-arcs in result (symmetric-ε sense).
5. **Invert / reverse round-trips** — `invert(invert(F)) == F`; `reverse(reverse(F))` equivalent.
6. **Connect / trim** — unreachable states removed, reachable preserved.
7. **Replace** — flattening produces expected FST; cycle detection fires.
8. **Closure** — `*`, `+`, `?`, bounded repeat on hand-written FSTs.
9. **Serialisation round-trip** — mmap write→read, structural equality. (Ours, not borrowed.)
10. **Path enumeration** — forward + reverse paths match expected enumeration.

Bulk: ~800–1200 LOC test code + 1–5 MB of golden `.fst.txt` files.

**Licensing**: `rustfst` is **Apache-2.0 OR MIT** (survey §2); test data inherits. Include `LICENSE-APACHE` + `LICENSE-MIT` + a `NOTICE` crediting `rustfst`. Standard OSS hygiene; no blockers.

Flow: build input FST in our representation, run our op, compare against expected output FST. Mismatches indicate bugs in us, `rustfst`, or OpenFST — triage per case.

---

## 9. Risks specific to the in-tree path

In roughly decreasing severity:

**1. Composition correctness.** Mohri's ε-filter has a long tail of subtle bugs — OpenFST, Foma, HFST have all shipped composition bugs in their history; `rustfst`'s #288 is in the same family. Building from scratch puts us on the hook with no upstream. **Mitigation**: test porting (§8); cross-validation against `rustfst` on every release; keep `rustfst` as a `dev-dependencies` for differential testing.

**2. Minimisation correctness.** Hopcroft has many easy-to-get-subtly-wrong details (splitter queue, out-degree handling, ε-interaction). **Mitigation**: implement Moore (O(n²)) first as reference; switch to Hopcroft only after Moore-equivalent runs green on 100% of the corpus.

**3. Mmap format stability.** v1 will reveal mistakes (alignment, symtab layout, missing flags). **Mitigation**: major + minor versioning (§4); refuse incompatible majors with a clear "recompile" error; treat `.huc` as a build artefact, never distribution. Matches `fst-morphology.md` §8 decision 5.

**4. Scope creep.** Someone (us, in 6 months) will want weighted FSTs for ranking analyses, or shortest-path. The "just add a weight field" temptation is corrosive. **Mitigation**: explicit non-goals in module docs; `Arc` is `#[non_exhaustive]` so adding a field is a deliberate breaking change.

**5. Performance regressions vs `rustfst`.** Our impl probably won't be faster initially; composition on large FSTs could be slower. **Mitigation**: per-entry FSTs are <1000 states — a 2× slowdown is invisible at compile time. Only the union-FST analyser (Phase F9-bis) would care.

**6. Opportunity cost.** 8–11 weeks not spent on Kaplan-Kay (the actual hard problem with morphology payoff). Other in-flight work (proto §3.12/§3.13, phonrule extensions) is paused that long with one developer.

---

## 10. Recommendation

**Pick B: `rustfst` + custom mmap layer.** Confidence: high.

Single-issue reasoning. The mmap requirement is real — without it, the future large-lexicon analyser breaks down. But mmap is *additive* and lives outside `rustfst`: wrap `ConstFst` in an emit-to-mmap-format pass and a zero-copy `MmapFst` view. The `rustfst-survey.md` §7 wrapper changes minimally — `Fst` internally holds either a `ConstFst<BooleanWeight>` (built / freshly composed) or a `MmapFst<'static>` view (loaded), and traversal works on both. Build + optimise = `rustfst`; load = ours.

Cost B over A: **1.5–2 weeks**, ~500 LOC. Cost C over A: **8–11 weeks**, ~4500 LOC, ongoing maintenance, non-trivial correctness validation.

**Single number driving the recommendation: C costs 8–11 weeks vs B costs 1.5–2 weeks for the same scale ceiling and the same mmap capability.** The 7–9-week delta buys `no_std` / WASM (not a stated goal), independence from `rustfst`'s release cadence (slow but stable), ~30 kLOC fewer transitive deps, and the pleasure of owning the algorithms. None of those justify a 7–9-week delay to Kaplan-Kay (the actual hard problem).

If `rustfst` ever falters — abandonment, licence change, an unfixable correctness bug — the wrapper boundary `hubullu::fst` is the seam where C swaps in. Until then, B strictly dominates C on every axis except fun.

The user's framing was *"とりあえず設計引いて、どれくらいのコストがかかるかを確定させよう"* — "draw the design first and pin down the cost." Pinned: **8–11 weeks**. The decision is the user's; my reading of cost/benefit says C isn't worth it.

If C is chosen anyway — e.g. `no_std`/WASM becomes a hard requirement, or owning the algorithms is itself a goal — this document is the design to build to. §5's LOC estimates are honest, §4's mmap format is implementable, §8's test-porting is the safety net.

---

## Appendix A — Build order (if C is chosen)

Most useful incremental gates:

1. **Week 1**: `fst::core` + `symtab` + `ops::basic` + `debug`. Gate: hand-written 5-state FST, concat-with-self, walk paths.
2. **Week 1.5**: `ops::epsilon` + `ops::invert_reverse_trim`. Gate: ε-removal + invert round-trips clean.
3. **Weeks 2–4.5**: `ops::composition`. Gate: ported `rustfst` composition golden tests pass.
4. **Weeks 4.5–6.5**: `ops::determinize` + `ops::minimize`. Gate: golden tests pass; Moore == Hopcroft on corpus.
5. **Weeks 6.5–7**: `traversal`. Gate: forward + reverse paths on hand-written and golden FSTs.
6. **Weeks 7–8**: `ops::replace`. Gate: 3-level grammar flattens; cycle detection fires.
7. **Weeks 8–9.5**: `serialize::emit` + `serialize::load`. Gate: large-FST round-trip; loaded FST passes traversal tests.
8. **Weeks 9.5–11**: Cross-validation, bug fixes, docs, morphology-layer integration.

Composition is the spike. **If week 4.5 arrives and composition is still red on >10% of golden tests, bail to B.**

## Appendix B — What stays the same across B and C

Both produce a `hubullu::fst` module with the `rustfst-survey.md` §7 external API. The morphology layer (`fst::rewrite`, slot-grammar compiler, lexicon builder, per-entry specialiser, circumfix/infix coupling, recursion flattening, analyser CLI) is identical. The `.huc` envelope format (postcard + per-entry FST blobs) is identical. The F1–F8 phase numbering is identical. Only `hubullu::fst` internals change.

This is the seam guarantee: B↔C is reversible at the cost of rewriting one module. Pick B now; revisit if assumptions change.

### How to swap rustfst for an in-tree kernel

After F1 (committed 2026-05-16) the seam is concrete: `src/fst/backend.rs` defines the `FstBackend` trait, and `src/fst/rustfst_backend.rs` is the only file in the codebase that names `rustfst::*` types. Swapping to Option C is a three-step change:

1. **Implement `FstBackend` for `InTreeBackend`.** Create `src/fst/intree_backend.rs` defining `pub struct InTreeBackend;` plus `impl FstBackend for InTreeBackend` covering every trait method. The actual kernel code (state/arc model, composition, determinise, minimise, replace, serialise, traversal — §3 above) sits in `src/fst/intree/` submodules; the backend impl translates to/from `Path`, `Label`, `StateId`, `FstError` exactly as `RustFstBackend` does today.

2. **Flip the type alias.** In `src/fst/mod.rs`, change

   ```rust
   pub type Backend = RustFstBackend;
   ```

   to

   ```rust
   pub type Backend = InTreeBackend;
   ```

   That is the one-line swap. Add `pub mod intree_backend;` next to `pub mod rustfst_backend;` and re-export the new backend.

3. **`cargo build`.** The compiler is the enforcement mechanism. Because no morphology code references `RustFstBackend`, `VectorFst`, `TropicalWeight`, or any other concrete rustfst type — only `Backend`, `FstBackend`, `Path`, `Label`, `StateId`, `SymbolTable`, `FstError` — nothing above `src/fst/` needs to change. If a rustfst type had leaked above the trait, the build would fail at the leak site, and the fix is to push the leak below the trait into a new method.

The invariant is enforced by manual grep today (`grep -rn rustfst src/ --include='*.rs' | grep -v '^src/fst/'` should match only docstring mentions). After F1 lands, the F1 smoke tests in `src/fst/tests.rs` double as a swap-readiness check: they refer to FSTs only through `Backend` and `FstBackend`, so they run unchanged against any conforming backend. Re-running them against `InTreeBackend` is the acceptance criterion for the swap.

What does NOT need to change at swap time: the `.huc` envelope format (it wraps backend-emitted bytes opaquely), the morphology layer (`fst::rewrite`, slot-grammar compiler, etc.), the F-numbered phase plan above F1, CLI surfaces, LSP plumbing, tests outside `src/fst/`. The cost of the swap is exactly the cost in §6: 8–11 engineer-weeks to implement the kernel itself, plus a few hours of integration (one type alias, one new module re-export, the grep, the smoke-test rerun).

If during F2 implementation we discover the trait surface is missing an operation, we add it as a new trait method, implement it in `RustFstBackend` first, and the swap-future-`InTreeBackend` inherits the requirement. The trait grows additively over the F-phases; it is not frozen at F1.
