# F2c4 Strategy A — Foma `rewr_notleftmost` 4-tape construction

Design proposal for **upgrading F2c4** (longest-leftmost filter) of the
hubullu FST migration from **Strategy B** (forced-commitment + non-nesting,
~315 LOC `leftmost.rs`) to **Strategy A** (Foma's `rewr_notleftmost`
four-tape construction, Karttunen 1996 "Directed Replacement"). The
upgrade unblocks compilation of grammars that exercise `!class*`
quantifiers — full Turkish vowel harmony and similar — by replacing the
exponential complement step at the heart of Strategy B's constraint stage
with a polynomial, locally-checkable construction over a four-tape
product.

Companion to:

- `docs/proposals/f2-kaplan-kay-plan.md` — the F2 plan; this proposal
  fulfils §9 risk 6 ("per-rule FST blow-up") and the §10 plan-deferred
  comment in `src/fst/phonrule/leftmost.rs` line 41–58.
- `src/fst/phonrule/leftmost.rs` — Strategy B implementation that this
  proposal replaces.
- `src/fst/phonrule/{brackets,replacement,constraint}.rs` — F2c1/c2/c3
  outputs that this proposal **reuses** (see §3 for the survey).
- `src/fst/phonrule/{replace,rule_seq,apply}.rs` — F2c5 top-level
  pipeline that runs over the new Strategy A output unchanged.
- `src/fst/phonrule/validation_tests.rs` — the strict-100% gate Strategy
  A must continue to pass and extends.

This document is **plan only**; no code lands here. An engineer who has
read Karttunen 1996 once and is fluent in rustfst should be able to
implement Strategy A from this document plus the references in §10.

Date: 2026-05-17. Author: F2c4-upgrade planning.

---

## Section 1 — Motivation

### 1.1 The concrete failure

Full Turkish vowel harmony, as written in
`examples/turkish/profile.hu` line 92:

```text
low -> to_back_low / back !V* + !V* _
```

i.e. "rewrite a low vowel via the `to_back_low` map when there is, anywhere
to the left, a `back` vowel followed by any run of non-vowels, a morpheme
boundary, and another run of non-vowels". On a current laptop this rule's
`compile_phonrule` call:

- exceeds **10 minutes** wall-clock (no result observed; killed),
- peaks at roughly **2.4 GB** of resident memory before being killed,
- has not been observed to terminate in any non-killed run.

The F2c5 validation harness (`validation_tests.rs`) sidesteps the
problem by validating a *shape-equivalent* but smaller rule
(`val_harmony_shape_class_map_boundary`, lines 908–947) that strips the
`!V*` quantifiers (a `back + _` left context, no negated-class star).
That smaller rule compiles and validates in seconds. The full rule is
documented as a known limitation in the slot-morphology memory entry
(2026-05-17 update): *"Strategy B (forced commitment + non-nesting) の
constraint complement 構築が `!class*` 量化子で指数的に膨張"*.

The same hang appears in F4 (`build_chain_fst`) the moment the chain
references `harmony` over any non-trivial slot FST: per
`src/fst/inflection/perf_probe.rs` lines 93–100 / 148–162, the harmony
compile inside the chain probe is `#[ignore]`'d for exactly this
reason. F4's structural code is complete; its tests hang on the same
Turkish harmony compile.

### 1.2 Why Strategy B is fundamentally limited

`leftmost.rs` lines 36–58 (module doc, "Strategy choice — B") and
36–73 (the explicit Strategy-A-vs-B trade) admit the cost up front:

> A — constraint-based (Karttunen-canonical) [...] the construction is
> finicky (four-tape trick in Foma's `replace.c`, see
> `rewr_notleftmost`).
>
> B — forced commitment + non-nesting [...] dramatically simpler to get
> right.

The simplicity is real, and Strategy B passes the strict 100% gate on
every rule shape we have ever validated. The cost is structural and
unavoidable: Strategy B leans on **constraint A** in
`constraint.rs::build_obligatory_constraint` (lines 152–164), which is
built by complement (`build_bad_a` followed by `RustFstBackend::complement`).
The bad-pattern FST `BadA` for the full harmony rule is
`Σ_b* · L · LHS · R · Σ_b*` with `L = back · !V* · + · !V*` and
`R = ε`; its complement under determinise-and-flip-finals must
enumerate, for every state of the constructed DFA, "what alphabet
symbol has been seen and how far has any partial-match progressed".
With `!V*` admitting any sequence of consonants of arbitrary length on
each side of the boundary, the number of partial-match states explodes:
each consonant position must track whether a `back` has been seen,
whether the boundary has been crossed, and how much of the right-side
`!V*` has been consumed — combinatorially, **the determinised DFA has
on the order of `|Σ|^k` states for k positions of `!V*`**. The
complement preserves this and the subsequent intersection with
constraint B blows up further.

A handful of mitigations (more aggressive minimisation between stages;
custom complement for specific shape patterns) are conceivable but
each would only postpone the next harder rule. The 4-tape construction
*sidesteps complement entirely* and is the right structural answer.

### 1.3 Why now

1. **F4 is structurally complete but tests hang.** F4's `build_chain_fst`
   composes phonrule FSTs around slot FSTs (`compose harmony(elision(...))`).
   The composition itself is cheap; what kills it is the harmony FST
   being uncompilable. F4 cannot ship until F2c4 stops being the
   bottleneck on real grammars.
2. **Production cutover from `phonrule_eval` to the FST engine is
   blocked.** The slot-morphology memory entry calls this out:
   *"production cutover はこの性能未解決のままでは不能"*. As long as
   the FST engine cannot compile every rule in the production grammar
   (Turkish, the proto's `hu/profile.hu`), we are stuck dual-running
   the legacy evaluator forever.
3. **The validation suite is missing Turkish harmony.** F2c5's golden
   corpus reaches into Turkish *elision* (`val_turkish_elision_real_rule`,
   line 774) but cannot reach harmony because harmony doesn't compile.
   The smaller-shape variant test (`val_harmony_shape_class_map_boundary`,
   line 908) is a deliberate stand-in. Real production grammars
   deserve real validation.

The user has committed: tackle Strategy A now, before continuing F4.

---

## Section 2 — Strategy A formalism

### 2.1 The 4-tape construction at a conceptual level

Karttunen 1996 "Directed Replacement" sidesteps the
constraint-complement explosion by representing the rewrite as a
**4-tape product** in which each tape carries a different layer of
information about a candidate rewrite event. Composition over the
tape product is performed by a sequence of small, local FSTs (each
checking one well-formedness condition between adjacent tape symbols),
none of which need to materialise the universal language `Σ*`
complemented against a global match pattern.

Tape roles, intuitively:

- **Tape 1 — input.** The source string, drawn from `Σ`.
- **Tape 2 — bracket markers.** A second stream, parallel to tape 1,
  bearing one of four labels at every position: `[+]` (a leftmost
  opening bracket), `]+` (its matching close), `[?]` (a tentative open
  bracket — a position where a match *could* start), `]?` (its
  tentative close). The tentative-bracket distinction is what lets the
  construction express "leftmost-first" *locally*: at every position
  tape 2 records what bracketings are candidates without committing to
  one until later tapes constrain it.
- **Tape 3 — forbidden-position markers.** A third stream bearing a
  marker `#` at every position where a leftmost match *could have*
  started but didn't. Tape 3 is what lets the construction express
  "no earlier match was possible" without a global complement: a local
  rule says "if tape 2 has `[+]` at position `i`, tape 3 must have no
  `#` before position `i` for any other candidate match" — and that
  rule is expressible as a small FST scanning adjacent tape positions
  in a single forward sweep.
- **Tape 4 — output.** The result, drawn from `Σ ∪ RHS-alphabet`.

The construction is the composition of (roughly) five small filters
over the product alphabet `Σ × (T2-labels) × (T3-labels) × (Σ ∪ RHS)`:

1. **Bracket-introduction filter** (tape 2 vs tape 1): every contiguous
   span on tape 1 that matches LHS gets a tentative bracket pair on
   tape 2.
2. **Longest-match filter** (tape 2 internal): of all tentative spans
   sharing the same start, only the longest may be promoted to
   committed; the rest are dropped.
3. **Leftmost filter** (tape 2 vs tape 3): a committed open `[+]`
   forbids any tentative open at any later position that *overlaps*
   it (which propagates `#` markers on tape 3); and conversely, a
   committed `[+]` may not appear at a position with a `#` on tape 3.
4. **Replacement filter** (tape 4 vs tapes 1+2): inside a committed
   bracket pair, tape 4 emits RHS; outside, tape 4 echoes tape 1.
5. **Context filter** (tape 1+2 vs L/R): the committed bracket pair
   must be preceded by an L-match and followed by an R-match on
   tape 1.

Each filter is local in tape-position and finite in alphabet size, so
each is a small acceptor over the product alphabet. Their composition
is the leftmost-longest replacement transducer, no global complement
required.

### 2.2 How it avoids complement

The key move is that *"no earlier match was possible"* is expressed
as the **absence** of a forbidden-position marker on tape 3 at the
committed-open position, not as the **intersection** with `Σ*` minus
a bad-pattern language. The forbidden-position marker is propagated
forward by a small left-to-right filter that, at each position, asks
only "is there an LHS-match starting here?" — a question that has the
size of LHS itself, not of the input.

Concretely: in Strategy B's `build_bad_a` (`constraint.rs` lines 357–
383), the construction is `Σ_b* · L · LHS · R · Σ_b*` and then
complement. In Strategy A there is no `Σ_b* · L · LHS · R · Σ_b*`
ever materialised; instead, a small forward-scanning filter visits
each position and either marks tape 3 with `#` (this position
*could* be a match start but is not chosen) or leaves it blank (this
position is not a match start, or has been chosen as the leftmost
match). The "could be a match start" check is itself a small FST
of size O(|LHS| + |L| + |R|), determinised once.

The total construction cost is polynomial in the grammar size and
linear in `|Σ|` rather than exponential, because no step crosses the
"determinise a constraint over `Σ*`" cliff.

### 2.3 Where Karttunen 1996 specifies which sub-construction

Karttunen 1996 §3 defines the **directed replacement operator**
`@->` in terms of a sequence of named auxiliary constructions:

- `Intro(X)` — the "introducer", which nondeterministically marks
  every LHS-match span (~ analogous to F2c1's `intro_brackets`, but
  with the four-bracket vocabulary above and operating over the tape
  product).
- `NotInner(X)` — the longest-match filter; §3 equation (8).
- `NotLeftmost(X)` — the leftmost-first filter; §3 equation (10).
  **This is the genuinely novel piece** Strategy A buys us.
- `Replace(X, Y)` — the replacement transducer (~ F2c2's
  `replacement.rs`, lifted to the product alphabet).
- `Constraints(L, R)` — the context filter (~ F2c3's `constraint.rs`,
  but only the *positive* half; the negative-by-complement half is
  unnecessary because `NotLeftmost` already guarantees uniqueness).

Foma's `replace.c` implements all five as the function family
`rewr_intro` / `rewr_notinner` / `rewr_notleftmost` /
`rewr_replace` / `rewr_constraints`. The orchestrator
`rewr_compile` (in the same file) chains them in the canonical
order. **The function to read first is `rewr_notleftmost` — it is
~80 lines of C, mutable-state-heavy but algorithmically the heart
of the construction.**

---

## Section 3 — What changes from F2c4 (Strategy B)

### 3.1 Files replaced

`src/fst/phonrule/leftmost.rs` is **fully replaced** (~315 LOC →
~500 LOC). The module's public surface stays — `build_longest_leftmost_filter`
remains the entry point with the same signature — but the internals
are entirely new. The old function name is misleading under Strategy
A (the filter does more than longest-leftmost; it's the whole
`NotInner ∘ NotLeftmost` composition). Recommend renaming to
`build_directed_replacement_filter` with a `pub use` alias for the
old name for one transitional release.

`src/fst/phonrule/leftmost_tests.rs` — the integration tests are kept
in shape but the test corpus is broadened (see §4).

### 3.2 Files reused **as-is**

- `brackets.rs` (F2c1). The bracket-pair vocabulary is the same
  (`<[+]>`, `<]+>`), and `intro_brackets` / `strip_brackets` /
  `identity_outside_brackets` are unchanged. **Caveat**: Strategy A's
  4-tape construction internally uses a wider bracket vocabulary
  (four labels: committed open/close + tentative open/close). The
  extra two labels are *internal to the leftmost.rs module's tape
  encoding*; they are encoded into rustfst's 2-tape representation
  (see §3.5) and never escape `leftmost.rs`'s output. From the
  surrounding pipeline's perspective the post-leftmost FST still
  speaks the F2c1 bracket protocol.
- `replacement.rs` (F2c2). Unchanged. The replacement transducer
  operates on inputs already bracketed by Strategy A's output. The
  module's outside-passthrough on bracket labels (replacement.rs lines
  442–456) is harmless because Strategy A's output has no stray
  brackets to pass through (Strategy A is stricter about bracket
  uniqueness than Strategy B's constraint stage).

### 3.3 Files reused **with care** — `constraint.rs` (F2c3)

This is the survey decision flagged in the task. There are two paths:

**Path A — keep `constraint.rs` as a parallel filter.** Strategy A's
`NotLeftmost` enforces leftmost-first; Strategy A's `Constraints(L, R)`
sub-step enforces context licensing. The current `constraint.rs`
*also* enforces context licensing, just via a different (positive +
complement) construction. If both are kept, we are double-enforcing
the same property — wasteful but harmless if minimisation collapses
the redundancy after composition. **Cost**: every per-rule compile
runs the constraint complement, which is the slow part on Turkish
harmony. This path defeats the entire purpose of Strategy A.

**Path B — replace `constraint.rs`'s role with Strategy A's
`Constraints(L, R)` sub-step.** Strategy A's context filter is built
*inside* `leftmost.rs` over the tape product. The standalone
`constraint.rs` is retired from the per-rule compilation chain. F2c5's
`replace.rs::compile_rewrite_rule` (lines 144–197) is edited to drop
the `let constraint = ...` step and the `compose_sorted(&intro, &constraint)`
step; the new pipeline becomes:

```text
intro_brackets ∘ replacement ∘ leftmost ∘ strip_brackets
```

The constraint module stays in-tree for two reasons: (i) F2c3's
construction is a useful debugging / reference implementation when
diagnosing Strategy A bugs; (ii) the smaller-shape variant tests
that pass under Strategy B should keep passing under Strategy B (we
keep Strategy B available as a compile-time option — see §8). The
module's tests stay; it just stops being on the default compile path.

**Recommendation**: Path B. Doing Path A means Strategy A's perf win
is invisible (the slow stage still runs). Path B is the structurally
clean answer Karttunen 1996 prescribes — the directed-replacement
construction is self-contained and includes its own context filter.

The cost of Path B is roughly +50 LOC in `leftmost.rs` (the context
filter sub-step over the tape product) and –1 composition step in
`replace.rs`. The cost of Path A is "Strategy A is pointless".

### 3.4 Files reused as-is — F2c5

- `replace.rs` — edit one line per Path B above (drop the constraint
  composition). The rest of the module is reusable.
- `rule_seq.rs` — unchanged. It composes per-rule FSTs without
  caring what's inside each one.
- `apply.rs` — unchanged. The apply driver is engine-agnostic: it
  composes input ∘ rule and enumerates paths.
- `validation_tests.rs` — unchanged structurally; the corpus is
  extended (§4).

### 3.5 Trait surface

The `FstBackend` trait (`src/fst/backend.rs`) already has every
primitive Strategy A needs:

- `concat`, `union`, `closure_star/plus/optional/bounded` — for the
  per-tape filter pieces.
- `compose` — for tape-product composition.
- `intersect` — for combining filters on the same tape pair.
- `complement` — *not used* in Strategy A's main path, but kept
  available for Strategy B fallback (§8).
- `arc_sort_input` / `arc_sort_output` — for the compose discipline.
- `determinize` / `minimize` / `eps_remove` — for opportunistic
  normalisation between stages.
- `paths` — for the integration tests.

**One potential gap**: Strategy A's tape-product representation may
benefit from a multi-tape composition primitive (sometimes called
"3-way compose" or "intersect-compose") that rustfst does not directly
expose. The workaround is to encode the product alphabet into 2-tape
labels (a label tuple becomes a single composite label, interned in
the symbol table). This is what `replace.c` does internally; rustfst
has no objection. **Conclusion: no trait extension needed.** If
profiling later shows the composite-label decoding is a hotspot, we
can add a `compose_3way` method to `FstBackend` then.

A second potential gap: rustfst's `compose` requires the right-hand
operand input-sorted (per `arc_sort_input` doc, backend.rs lines
250–262). Strategy A's tape-product composition will frequently
intersect three or more filters before consuming the result; ensure
each intermediate stage is re-sorted. This is a discipline question,
not a trait gap.

---

## Section 4 — What stays from validation infrastructure

`validation_tests.rs` is the entire F2c5 validation gate. It runs
unchanged against Strategy A — the harness compiles a phonrule with
`compile_phonrule` and applies with `apply_phonrule_fst`, asserting
byte-identical output against `phonrule_eval`. The harness is engine-
agnostic; it does not care that `leftmost.rs` has been rewritten.

What changes is the **corpus**. Specifically, the following tests
should be **added** to `validation_tests.rs` once Strategy A lands:

1. **Full Turkish vowel harmony, low vowels.** Mirror of
   `examples/turkish/profile.hu` line 92 (`low -> to_back_low / back !V* + !V* _`)
   driven over the actual Turkish vowel inventory + a small consonant
   set. Currently absent from validation because Strategy B can't
   compile it.
2. **Full Turkish vowel harmony, high vowels (4-way).** Three rules
   from `profile.hu` lines 105–107
   (`high -> to_back_unrounded_high / back_unrounded !V* + !V* _` and
   its `back_rounded` / `front_rounded` siblings). Same shape as (1)
   but exercises map RHS with multiple arms.
3. **The proto's phonrules.** Currently the a-priori project's
   `hu/profile.hu` rules are intended for proto validation but aren't
   in the FST corpus yet. After Strategy A lands, they go in.
4. **Fuzz over Turkish.** The existing `val_turkish_elision_fuzz_500_inputs`
   pattern (line 828) extended to the harmony rules — 500 random
   inputs over the Turkish vowel + consonant alphabet, asserting
   byte-identical output.

The smaller-shape variants (`val_harmony_shape_class_map_boundary`,
line 908) **stay** as fast smoke tests — they exercise the same
construction shape with fewer states, and they run in seconds.
Strategy A's full Turkish tests will be the gold-standard but slow
(seconds-to-minutes, not the >10-minute hang Strategy B exhibited).

The fuzz harness (`fuzz_inputs` + the LCG, lines 337–380) is reused
verbatim; only the seed strings and alphabets change.

---

## Section 5 — Per-step implementation breakdown

Sub-steps in implementation order. LOC estimates are for the
*production-quality* implementation including doc comments and
defensive guards; spike-quality versions are smaller.

| # | Sub-step                                                                 | LOC est | Days | Risk   |
|---|--------------------------------------------------------------------------|---------|------|--------|
| 1 | Read Karttunen 1996 + Foma `replace.c` `rewr_*` family with notes        | —       | 3    | low    |
| 2 | Design the in-memory tape encoding (labels-as-product, intern scheme)    | 150     | 2    | medium |
| 3 | "Bracket pair with content" sub-FST over the product alphabet            | 150     | 2    | medium |
| 4 | `NotLeftmost` — "no earlier match was possible" forbidden-position FST   | 250     | 5    | **HIGH** |
| 5 | `NotInner` — longest-match-per-position constraint                       | 150     | 3    | medium |
| 6 | Compose the 4-tape construction (intro ∘ NotInner ∘ NotLeftmost ∘ ctx)   | 100     | 2    | medium |
| 7 | Project to 2-tape (input+output, dropping bracket & forbidden tapes)     |  80     | 1    | low    |
| 8 | Validation — re-run F2c5 suite + add Turkish harmony / elision corpus    | 150     | 3    | low    |
| 9 | F4 test fixture cleanup — remove smaller-shape variant, use full Turkish |  50     | 1    | low    |

**Totals.** LOC: ~1080 (plus ~300 LOC of inline tests not double-counted)
→ total file footprint for `leftmost.rs` rewrite: ~1400 LOC.
Working days: **22 days = ~4.4 working weeks** for one engineer fluent
in Rust + rustfst who has read Karttunen 1996 once.

**Honest framing on the estimate.** The 2–3 weeks figure from the
original perf-gap discussion (slot-morphology memory entry, F2 line
72) was rough — it costed the algorithm in the abstract. This plan
costs it including the tape encoding into rustfst's 2-tape FST,
the `NotLeftmost` construction (the genuinely novel and hardest
piece), and the validation corpus extension. 4 weeks is honest; 3
weeks if the spike (§7) goes well.

**Critical path.** Step 4 (`NotLeftmost`) is the spike. Steps 1–3 land
in week 1; step 4 occupies most of week 2; steps 5–7 land in week 3;
steps 8–9 in week 4. Buffer is intentionally thin — if step 4 slips,
revisit (§7 spike result is the gate, not implementation length).

**Bail-out checkpoint.** If at end of week 2 step 4 is still red on a
toy `a -> b / x _ y` rule, do not push further. Either escalate or
revert to Strategy B + a custom-complement-for-`!class*`-shape
optimisation (which is its own multi-week project but with a known
shape).

---

## Section 6 — Risks

### 6.1 Tape encoding correctness

Strategy A's 4-tape construction encoded into rustfst's 2-tape FST is
the fiddliest piece of infrastructure. Each tape symbol becomes part
of a composite label; the symbol table grows by roughly
`|Σ| × |T2| × |T3| × |Σ ∪ RHS|`, which is large but bounded. A clean
abstraction (a `TapeProductLabel(t1: Label, t2: Label, t3: Label, t4: Label)`
encoded into a `u32` via bit-packing, or a hash-interned `(t1,t2,t3,t4)`
tuple) is essential — without one, off-by-one bugs in the encoding /
decoding will silently break every downstream filter.

**Mitigation**: implement the tape encoding as the very first sub-step
(§5 step 2), with exhaustive unit tests round-tripping encode → decode
for every combination of representative tape labels. Do *not* layer
filters on top until the encoding has its own green tests.

### 6.2 Foma's actual implementation is in mutable C

`replace.c` is ~2000 lines of imperative C with heavy mutable state
(global arena allocator, mutable tape arrays, in-place rewrites). The
algorithm is portable but the *code* is not directly translatable to
rustfst's immutable-FST style. Translating naively yields code that is
*much* slower (every "modify a tape" becomes a full FST rebuild) and
much harder to verify.

**Mitigation**: prefer reading Mohri & Sproat 1996 alongside Karttunen
1996 — Mohri's exposition is more abstract and weight-aware (we don't
need the weights, but the abstraction maps cleanly to immutable FSTs).
Use `replace.c` as a correctness oracle (small inputs, observe its
output), not as a port target.

### 6.3 Validation regression

Strategy A's output on any rule Strategy B already handles must be
byte-identical to Strategy B's output (both are computing the same
formal language — leftmost-longest replacement — so any disagreement
is a bug in one or the other). If Strategy A disagrees with Strategy
B on a previously-passing rule:

- the disagreement may be a Strategy A bug — most likely, given
  Strategy A is fresh code and Strategy B has been validated against
  `phonrule_eval` on 100 golden + 3000 fuzz inputs;
- or a Strategy B bug that the fuzz corpus happened not to expose
  (less likely but possible — Strategy B's `LHS ⊆ RHS` carve-out at
  `leftmost.rs` lines 95–105 is a known edge case).

**Mitigation**: run *both* engines side-by-side during the Strategy A
bring-up, diff their outputs on the full F2c5 corpus, treat any
disagreement as a P0 bug. The slot-morphology memory entry notes that
F2c5 already operates this way against `phonrule_eval`; we layer one
more comparison.

### 6.4 Perf gain less than expected

If Strategy A still has exponential cases on real grammars — for
example if our `NotLeftmost` implementation accidentally re-introduces
a complement step we missed — we will have spent 3-4 weeks for no
production gain. The risk is real because the published algorithm
sketches assume an idealised FST model; rustfst's particular state /
arc representation may introduce O(|Σ|) factors that compound.

**Mitigation**: see §7 — spike first on a toy rule with `!class*`
before committing to the full rewrite. If the spike is qualitatively
better than Strategy B (linear-in-alphabet vs exponential-in-quantifier
on the same toy), commit; otherwise revisit.

### 6.5 Symbol-table bloat from tape product encoding

If we encode the 4-tape product as bit-packed `u32` labels, every
distinct `(t1, t2, t3, t4)` tuple becomes a symbol-table entry. For
even modest alphabets this is `|Σ|² × |T2| × |T3|` entries —
potentially tens of thousands. rustfst's `SymbolTable` handles this
size, but FST serialisation (`.huc`, F7) will balloon proportionally.

**Mitigation**: do not intern composite labels into the *user-visible*
symbol table. Strategy A's tape-product labels live in a
*construction-local* symbol table that is **projected away** in §5
step 7 before the output FST is returned. The user-visible symbol
table sees only `Σ` plus the two F2c1 brackets, as before.

---

## Section 7 — Spike plan (recommended first step)

Before committing to the full Strategy A rewrite, do a **2-day spike**:

**Goal**: implement the *minimum* leftmost-first filter (Strategy A's
`NotLeftmost`, the genuinely novel piece) on the simplest possible
rule, and measure compile time / state count against Strategy B on
the same rule with a `!class*` quantifier added.

**Spike rule**: `a -> b / x _ y` with one variant `a -> b / x !V* _ y`
where `V = {a, e, i}` (small alphabet — 5 symbols including b, x, y).

**Spike scope**:
- Implement only `Intro` + `NotLeftmost` over a hand-encoded
  product alphabet (do not bother with `NotInner` or `Constraints`
  for the spike; pick a rule that doesn't need them).
- Encode the tapes as a tuple `(input_label, bracket_label)` interned
  as composite labels (skip tape 3 for the spike if `a -> b / x _ y`
  has no leftmost ambiguity in the first variant).
- Compose `Intro ∘ NotLeftmost` and project to 2-tape.
- Time the compile + count states.

**Comparison**:
- Strategy B's compile time on the same rule: should be ~ms for the
  base rule, ~seconds for the `!V*` variant (will scale poorly).
- Strategy A spike compile time on the same rule: should be ~ms for
  both variants, ideally same order of magnitude.

**Decision rule**:
- **If Strategy A's spike compile is qualitatively better** (linear-in-
  alphabet rather than exponential-in-quantifier-count, i.e. the
  `!V*` variant compiles in roughly the same time as the base), commit
  to the full Strategy A rewrite per §5.
- **If Strategy A's spike compile is no better than Strategy B**, do
  not commit to the rewrite. Revisit: investigate whether the spike's
  encoding is suboptimal, or whether a custom complement for the
  specific `!class*` shape might be cheaper than the general 4-tape
  construction.

The spike must be performed *before* §5 step 2 (the production tape
encoding) so the spike's quick-and-dirty encoding doesn't have to
satisfy production constraints. Time-box: 2 days. If the spike isn't
running by end of day 2, that itself is a signal — the algorithm is
not fitting rustfst's model easily.

---

## Section 8 — Open decisions (RESOLVED 2026-05-17)

**Resolved decisions:**
- **8.1 Foma usage**: **clean-room re-implementation** from Karttunen 1996 + Mohri & Sproat 1996. Foma's `replace.c` is used only as a small-input black-box correctness oracle. No license question.
- **8.2 Strategy B retention**: **fallback only** (not Option c's per-rule dispatcher). Default is A; B is available behind a feature flag for emergency escape. No per-rule heuristic — keeps the dispatching surface small.
- **8.3 F4 fixtures**: not formally resolved, but follow Option (b) by default (small fast variant in default tests + full Turkish in slow/opt-in integration test).

### Original open decisions (reference)

### 8.1 Strategy A in-tree vs port from Foma

**Foma's source is dual-licensed** Apache-2.0 / GPL-2.0+ depending
on the file; `replace.c` specifically needs to be checked at the
file header. The plan §3 line 69 claims "(Apache-2.0 compatible)";
verify before any direct porting. Hubullu is MIT-licensed (per
`Cargo.toml` line 5). Apache-2.0 → MIT is a one-way compatibility
direction and the standard "include the Apache notice" applies; GPL
→ MIT is **not** compatible without re-licensing hubullu, which is
out of scope.

**Recommendation**: **re-implement from Karttunen 1996 + Mohri & Sproat
1996 from scratch (clean-room).** Use `replace.c` as a correctness
oracle (run small inputs through Foma, compare outputs), not as a port
target. This sidesteps the licensing question entirely and is also
the faster path to a Rust-idiomatic implementation.

**Open question for the user**: confirm clean-room re-implementation
is preferred, or specifically allow Apache-2.0 portions of `replace.c`
to be included with attribution.

### 8.2 Whether to keep Strategy B as a compile-time option

Strategy B is faster than Strategy A on simple rules (no `!class*`,
no overlapping LHS). For the vast majority of phonrules in current
and future grammars, Strategy B is the right answer.

**Option (a)**: Keep Strategy B available as a compile-time choice via
a flag (`--engine=strategy-b` or per-rule heuristic — "if the rule
has no `!class*` quantifier, use B; otherwise A"). Feature parity at
the cost of code duplication and a divergence-bug surface.

**Option (b)**: Drop Strategy B once Strategy A passes the full F2c5
gate (including the new Turkish harmony tests). One engine, simpler.
But every simple rule now pays Strategy A's overhead — likely 2-5×
slower compile on rules that don't need the 4-tape machinery.

**Option (c)**: Keep Strategy B as a compile-time choice, default to A.
Adds a per-rule heuristic and a flag, but production tuning has the
escape hatch.

**Recommendation**: **Option (c)**. The heuristic is cheap (check the
AST for `NegClass` atoms with `Star`/`Plus`/`Range(_, m>1)` quantifiers).
Strategy B is already written and validated; throwing it away would
forfeit ~315 LOC of working code for marginal simplicity.

**Open question for the user**: confirm Option (c), or push back if a
one-engine policy is preferred for maintainability.

### 8.3 F4 test fixture choice

F4's `build_chain_fst` tests currently use the smaller-shape variant
of harmony (`val_harmony_shape_class_map_boundary`). Once Strategy A
lands:

**Option (a)**: F4 tests switch to the full Turkish harmony rule from
`examples/turkish/profile.hu`. Highest validation value; slowest test.

**Option (b)**: F4 tests use both — the smaller-shape variant as the
fast test (every test run), the full rule as a slow integration test
(opt-in via `cargo test --features slow-tests` or `#[ignore]` + an
explicit run target). Best of both.

**Option (c)**: F4 tests use only the smaller-shape variant; full
Turkish lives in `validation_tests.rs` and isn't exercised by F4
tests directly. Fastest tests; lowest F4 coverage.

**Recommendation**: **Option (b)**. The slow test catches Strategy A
regressions early; the fast test keeps the dev loop tight.

**Open question for the user**: confirm Option (b), or pick a / c.

---

## Section 9 — Non-goals

1. **Not changing `@->` semantics.** Strategy A computes the same
   formal language as Strategy B — obligatory, longest-match,
   leftmost-first replacement. Any output difference is a bug.
2. **Not changing the `FstBackend` trait surface.** Strategy A is
   implementable with the existing primitives (concat / union /
   closure / compose / intersect / arc_sort / determinize / minimize /
   eps_remove). If profiling later motivates a `compose_3way` or
   similar, that is a separate proposal.
3. **Not modifying `phonrule_eval`.** The evaluator is the validation
   oracle; it must stay byte-identical to current behaviour. Any
   evaluator change invalidates the strict 100% gate.
4. **Not solving syllable-aware contexts** (`%syl<head>%`,
   `%syl<tail>%`, `%syl<#N>%`, `%syl[...]%`). Plan §10 non-goal 5
   stands. Strategy A inherits the same `UnsupportedLhsShape` error
   path for syllable elements.
5. **Not modifying F4 (`build_chain_fst`) structurally.** F4's hang
   is a downstream symptom of F2c4's perf cliff; fixing F2c4 unblocks
   F4 with no F4 code changes needed.
6. **Not modifying F2c1 / F2c2 / F2c3 / F2c5 modules** other than
   the one-line composition-order edit in `replace.rs` per Path B
   (§3.3).
7. **Not introducing weighted FSTs.** Karttunen 1996 has a weighted
   extension (and Mohri & Sproat 1996 is the canonical weighted
   reference); we use only the unweighted construction.
8. **Not preserving backward compatibility of `leftmost.rs`'s internal
   API.** The module's *public* entry `build_longest_leftmost_filter`
   keeps its signature; internal helpers (`build_rhs_acceptor`,
   `harvest_map_outputs`, etc.) are replaced wholesale. Pre-1.0 per
   `fst-morphology.md` §8 decision 5.

---

## Section 10 — References

### Papers

- **Karttunen, L. (1996). "Directed Replacement."**
  Proceedings of ACL 1996, pp. 108–115.
  https://aclanthology.org/P96-1016.pdf — the canonical formal
  description of the `@->` operator and the 4-tape construction.
  Read §3 carefully; equations (8) and (10) define `NotInner` and
  `NotLeftmost` respectively.
- **Karttunen, L. (1995). "The Replace Operator."**
  Proceedings of ACL 1995, pp. 16–23.
  https://aclanthology.org/P95-1003.pdf — the earlier (single-tape)
  formulation that F2 currently implements. Useful as a baseline to
  understand what Strategy A is improving on.
- **Mohri, M. & Sproat, R. (1996). "An Efficient Compiler for
  Weighted Rewrite Rules."** ACL 1996, pp. 231–238.
  https://aclanthology.org/P96-1031.pdf — alternative formulation
  with more abstract / cleaner exposition. Useful when Karttunen
  1996's notation is dense.
- **Beesley, K. R. & Karttunen, L. (2003). *Finite State Morphology*.**
  CSLI Publications. Chapter on `Replace by Default` (Chapter 3) is
  the book-length expansion of Karttunen 1995/1996. Read alongside
  the papers when the formal notation compresses.

### Source

- **Foma `replace.c`.** https://github.com/mhulden/foma/blob/master/foma/replace.c
  — reference C implementation of the directed-replacement operator.
  Functions to study: `rewr_intro`, `rewr_notinner`, `rewr_notleftmost`,
  `rewr_replace`, `rewr_constraints`, and the orchestrator
  `rewr_compile`. **License**: verify per file header before any
  direct porting (see §8.1).

### In-tree

- `docs/proposals/f2-kaplan-kay-plan.md` — F2 master plan. §3 (Karttunen
  construction), §9 risk 6 (anticipates this upgrade).
- `docs/proposals/fst-morphology.md` — architecture proposal. §3 / §6 /
  §7 for context on where F2c4 sits in the larger picture.
- `docs/proposals/rustfst-survey.md` — what rustfst provides; relevant
  to §3.5 (no trait extension needed) and §6 (rustfst's known sharp
  edges).
- `src/fst/phonrule/leftmost.rs` — Strategy B implementation (to be
  replaced).
- `src/fst/phonrule/leftmost_tests.rs` — integration test pattern
  (to be reused with extended corpus).
- `src/fst/phonrule/validation_tests.rs` — strict 100% gate (to be
  extended with Turkish harmony).
- `examples/turkish/profile.hu` lines 73–116 — the harmony + elision
  rules that motivate this upgrade.

End of plan.
