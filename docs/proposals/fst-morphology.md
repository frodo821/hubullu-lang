# Proposal: FST-based morphology engine

Migrate hubullu's morphology engine to a **finite-state transducer (FST)** core, unifying compile, search, and storage under a single primitive (FST traversal). The current AST-walker + SQLite-cache split is replaced by a per-entry FST network with the `.huc` artefact rewritten to a binary FST + AST + index format.

This is an **architecture proposal only**. No code changes are made here. The direction is committed; what follows is the design that implementation work will be cut from.

Author: 2026-05-16. Companion to `docs/proposals/slot-morphology.md` (the prior design we just shipped).

---

## 1. Motivation

The slot-morphology rework (Phases 1–8 + reshape) succeeded against its primary metric: the a priori proto compiles in **292 KB / sub-second** instead of **606 MB / minutes** — `auto_fill_lazy_slots` resolves every lazy slot at render time from a few hundred candidate scans (`render.rs:680`, `render.rs:824`). Turkish renders end-to-end at the surface level (`yazıyorum. gelmedim. yazdık.`). The architecture works.

But the cracks are now visible and they are structural, not cosmetic.

**Crack 1 — `.huc` is not self-contained.** The render path for any-lazy `Compose` entries pulls the *original AST* back out of the loaded `.hu` files via `HutPhonContext::find_entry_ast` / `find_inflection_ast` (`render.rs:~1502`, see also `iter_morpheme_entries` at `render.rs:~2597`). The SQLite `.huc` carries `entries` / `entry_tags` / `forms` rows but **does not carry the compose chain, slot defs, or the morpheme inventory the way render needs them**. We re-run phase1+phase2 on the `.hu` sources to materialise it. The `.huc` is, today, a partial cache that lies about being a build artefact.

**Crack 2 — Search has fractured into 4+ disjoint mechanisms.** All of these do "given some piece of information, find a form/entry," but none of them share an implementation:

1. SQL `forms` table + `find_form_by_spec` (`render.rs:32`) — works for all-eager `Compose` / `Rules`, fails (0 rows) for any-lazy.
2. Render-side `auto_fill_lazy_slots` (`render.rs:680`) + `candidate_fits_cell` (`render.rs:824`) — per-slot linear scan over every inflectionless entry. Three AND-predicates: filter fit, cell agreement, domain containment.
3. Reverse lookup (`surface → entry`) — **broken** for lazy bodies. No `forms` row to query and no algorithm to derive one. End-user feature "what entry produces *gewerkt*?" cannot be answered.
4. FTS5 (`emit_sqlite.rs:495`) — searches `name` / `headword` / `meaning` text. Does not index inflected surface forms. For lazy bodies, it has nothing to index even in principle, because the forms aren't enumerated.

**Crack 3 — The most painful gap is concrete: reverse lookup of lazy surfaces.** This is a *user-visible* feature the architecture cannot deliver without a redesign. The renderer maps `entry → surface`. There is no `surface → entry` analyser that handles the lazy path; building one as a separate component duplicates every piece of slot-grammar logic.

**Crack 4 — The current implementation is, structurally, a hand-rolled eager FST + a SQL cache.** Every piece of the slot-morphology design maps cleanly to a primitive from computational morphology:

| current piece | FST equivalent |
|---|---|
| `compose root + sfx1 + sfx2` chain | sub-FST concatenation |
| `slot X matching [filter]` + matching morpheme entries | lexicon FST with tag-labeled paths |
| `?`, `*`, `+`, `{n,m}` quantifiers | standard FST loop / optional constructions |
| phonrule (context-sensitive rewrites) | rewrite-rules → FST (Kaplan & Kay 1994) |
| `compose harmony(elision(chain))` | FST composition (∘) |
| forward render | input-side traversal |
| `auto_fill_lazy_slots` (Phase 7) | one-step path lookup over tag labels |
| reverse lookup (broken) | output-side traversal of the **same** FST |
| `forms` table (eager) | enumerable paths (typically not materialised) |
| FTS over forms | path query with labeled constraints |

This is the textbook XFST / Foma / HFST architecture. We are not inventing anything; we are switching from a hand-rolled approximation to the principled construction the field has used since the early 1990s.

---

## 2. Conceptual model

A morphological grammar is a **finite-state transducer** mapping a sequence on the **analysis side** (tags, morpheme IDs, axis values) to a sequence on the **surface side** (phonemes / graphemes).

- **Lexicon FST.** Each inflectionless morpheme entry contributes a labeled path. Input symbols on that path are *tag values* (e.g. `tense=past`, `person=1`, plus a `morph=ed1` identity to break homophone ties); output symbols are the morpheme's surface phonemes.
- **Compose chain → concatenation.** `compose root + neg_sfx? + tense_sfx + pn_sfx?` becomes `Lroot ∘ (Lneg)? ∘ Ltense ∘ (Lpn)?`. Each sub-FST is a lexicon FST restricted to morphemes that fit the slot.
- **Slot filter as restriction.** `slot tense_sfx matching [tense]` restricts the sub-FST to paths whose input labels include some `tense=...` and *no* other axis labels (matching the existing `candidate_fits_cell` domain-containment predicate, `render.rs:824`). The restriction is itself an FST and applies by composition.
- **Phonrule wrap as composition.** `compose harmony(elision(chain))` is `Lchain ∘ Telision ∘ Tharmony`, where `Telision` and `Tharmony` are FSTs compiled from the corresponding `phonrule` declarations (§3).
- **Whole inflection → one FST per entry.** Each entry of `verb_conj` produces its own per-entry FST (the chain composed with that entry's stems). The compiled artefact is a *network* of per-entry FSTs plus shared sub-FSTs for phonrules and shared lexica.

### Forward traversal vs reverse traversal

The FST is bidirectional. Render is forward traversal: project the *input* alphabet (tags) → walk paths → emit the *output* alphabet (phonemes). Analysis is reverse traversal: project the *output* alphabet → walk paths → emit the *input* alphabet. **Both directions use the same FST.** There is no second analyser to write.

### What does NOT fit cleanly

Three honest caveats up front:

1. **True recursion (§3.4 clitic-with-paradigm, `render.rs::DepthGuard` at ~120-165).** FSTs are flat: a single FST has a finite state set and no notion of "call another FST and come back." We handle this either by **flattening** at compile time (inlining the clitic's paradigm into every host position where it can appear) or with a **network of FSTs** with cross-references at compile time. Neither is as mathematically clean as a flat FST. See §6.
2. **Truly unbounded variadic (`matching *` with `*` quantifier).** Loops in FSTs are fine in isolation. But composing a Kleene loop with a phonrule FST can blow up state count, because the phonrule's context window now has to be tracked across arbitrarily many loop iterations.
3. **Tag-bearing material in CatchAll slots.** The current "catch-all warning" (`slot_parse::validate_slot_fill`) is a render-time soft check. In an FST, this either becomes a path-property warning at compile time (more strict) or is dropped (acceptable — it was always a heuristic).

---

## 3. Rewrite rules → FST (the phonrule part)

The phonrule evaluator (`phonrule_eval.rs`, 1792 LOC) implements context-sensitive rewrite rules of the form `A → B / L _ R`: replace input `A` with output `B` when surrounded by left context `L` and right context `R`. It handles syllable-aware contexts, alternation classes, range LHS, quantified atoms — a substantial DSL.

The construction of Kaplan & Kay (1994) "Regular models of phonological rule systems" compiles every such rule into an FST:

- A rule `A → B / L _ R` compiles to a transducer that, on input matching `A` in the context `L _ R`, outputs `B`, and otherwise outputs the input unchanged.
- A sequence of rules in a phonrule body composes left-to-right: `T_body = T_rule1 ∘ T_rule2 ∘ ...`. This matches our current "iterate body items in order" semantics (`phonrule_eval.rs:158`).
- A phonrule that `apply`s another phonrule (composition) is FST composition.
- Iteration to convergence (the cascading-harmony loop at `phonrule_eval.rs:164`) corresponds to a Kleene-closure construction; in practice we apply the transducer once on bounded input and rely on the fact that the rule cannot fire infinitely often on a finite input.

An alternative formulation worth flagging is **Koskenniemi (1983) two-level morphology**: rules are constraints relating the lexical (analysis) string and surface string position-by-position, compiled to FSTs and intersected. Two-level is theoretically attractive (no rule ordering, no intermediate strings) but practically less commonly used than rewrite-rule compilation; XFST and Foma both lean on Kaplan & Kay. We follow that lineage.

### Implementation cost — honest

This is well-understood algorithmically but **non-trivial to implement correctly**. A clean implementation of rewrite-rule compilation handling the constructs we care about (alternation classes, ranges, quantifiers, syllable-aware contexts) is several hundred lines of careful FST construction code, with a long tail of edge cases (epsilon transitions, marker symbols for context delimitation, determinisation that doesn't blow up).

Options:

- **Roll our own.** Use `rustfst` (which gives us FST primitives but not Kaplan-Kay) as a substrate, write the rule-compilation layer ourselves. Probably 1500–2500 LOC, 3–5 weeks for a competent engineer.
- **FFI to existing implementation.** Foma has a C library (`libfoma`); HFST has a Python binding. Both are GPL-licensed, which is a problem for a Rust crate that intends to remain permissively licensed. Calling them from Rust is doable but adds a build dependency on the host.
- **Use `rustfst` plus a port of an open-source rewrite compiler.** OpenFST has `fstcompile` but no rewrite-rule layer either. No existing Rust crate I know of compiles Kaplan-Kay rules end-to-end.

Recommendation: **roll our own on top of `rustfst`**. The phonrule DSL is ours; we will want to extend it; an FFI binding to a fixed external implementation would slow that down.

---

## 4. Compile pipeline

What `hubullu compile` does post-migration:

1. **Parse + phase1 + phase2 validation.** Unchanged. The AST and the symbol table still drive validation, error reporting, LSP support.
2. **Build per-inflection FSTs by composition.**
   - For each phonrule, compile `phonrule → T_rule` (Kaplan-Kay).
   - For each inflection's compose chain, compile `chain → T_chain` by:
     - Compiling each slot to a sub-FST (eager slots: literal paths from the rule list; lazy slots: lexicon-restriction FST over the morpheme inventory matching the filter).
     - Concatenating and quantifier-wrapping per `SlotQuantifier`.
     - Wrapping with `PhonApply` via composition with the phonrule FSTs.
   - For each entry, specialise `T_chain` by binding the entry's stems. This produces `T_entry`.
3. **Minimise** each FST (standard determinisation + minimisation; `rustfst` provides this). Per-entry FSTs are typically small (the chain's state count times the entry's stem count) and minimise well.
4. **Build cross-cutting indexes**:
   - Per-language **union FST** for analysis (reverse lookup): `T_lang = ⋃_entry T_entry`. This may be large for big lexica; in the first cut, store per-entry FSTs and walk them all for analysis (slow but correct), and add the union as an optimisation later.
   - A **morpheme-tag-axis index** mapping `(axis, value) → [morpheme entries]` for fast slot-restriction during incremental compile.
   - A small **headword/meaning index** for FTS (see §5).
5. **Serialise** the FST network + entry metadata + indexes to a new `.huc` format.

### The new `.huc` format

The new `.huc` is **not** SQLite. It is a single binary container with:

- A **header** with format version, language identity, hash of source AST for cache invalidation.
- A **serialised AST** (the post-phase2 resolved AST, not the parsed AST) for the LSP and any downstream tool that still wants tree access.
- A **per-entry FST table**: each entry's `T_entry`, plus shared phonrule FSTs by ID.
- A **lexical metadata table**: entries, tags, stems, meanings, headword scripts, etymology, links — what `emit_sqlite.rs::insert_data` currently writes, minus `forms` (which is replaced by FST traversal).
- A **headword/meaning index** for FTS.

**Format choice**: I recommend **postcard** as the serialisation format. Rationale:

- `bincode` is widely used and fast but its format is unspecified across versions; we have been bitten by silent breakage when serde derives change.
- `rkyv` gives zero-copy access (very attractive for FST node walking) but requires the entire AST to be `Archive`-clean; our AST derives a mixture of `Hash`/`Serialize`/`Deserialize` and the `Archive` migration is non-trivial.
- **`postcard` is no-std-friendly, has a stable on-wire format, is fast enough, and integrates with serde without intrusive trait derives.** It is the lowest-friction choice that still respects format stability.
- For the per-entry FST blobs themselves, `rustfst`'s native binary format (`.fst`) is the obvious choice; we wrap each in a postcard-framed envelope.

A `compile_meta` block records: hubullu version, format version, source AST hash, FST construction options. Old `.huc` files become unreadable at the boundary we choose (see §8 question 5).

SQLite was load-bearing for `find_form_by_spec` queries and FTS5. Both are replaced:

- `find_form_by_spec` → FST traversal of `T_entry` with the axis values as input labels.
- FTS5 → a separate index built over headwords and meaning text. Either keep a small tantivy index alongside the FST file, or use the read-only `fst` crate to build a sorted-keys index over headwords (FTS over meaning text needs tantivy).

---

## 5. Search story — unified under FST

After migration, **every "search" the engine does is FST traversal** with different inputs.

| operation | today | post-FST |
|---|---|---|
| `find_form_by_spec(entry, axes)` | SQL query on `forms` | forward traversal of `T_entry` with axes as input labels → surface |
| Reverse lookup `surface → entry` | broken | reverse traversal of language union FST → analysis (entry id + cell) |
| `auto_fill_lazy_slots(entry, cell)` | linear scan over morphemes + 3-predicate filter (`render.rs:824`) | one-step lookup on tag labels in the entry's chain FST |
| "find all forms with tense=past" | SQL `LIKE` over `tags` column | path-query restricting tense label on entry FSTs |
| FTS over headword/meaning | SQLite FTS5 | tantivy or `fst` crate index (kept separate; FST gives surface paths, not meaning text) |

The reverse lookup, in particular, **comes for free** as the inverse traversal of the FST that already implements forward render. No separate analyser to write, no separate validation surface. This is the single biggest payoff and the reason the migration is worth doing.

Ambiguity is handled honestly: a reverse traversal may yield multiple analyses (the proto's `-əd` is ABS.NUM = ERG.NUM = noun NUM; `cat[]` could be a noun or verb in some lect). The FST returns all paths; the caller decides whether to surface them, rank them, or pick one. This matches the current §6 stance of slot-morphology.md (ambiguity is downstream's problem).

`auto_fill_lazy_slots` becomes trivial post-FST: traversing `T_entry` with just `[tense=past, person=1, number=sg]` as input labels and a wildcard for the morpheme-ID labels yields the path(s) consistent with the cell; if exactly one, render it; if more, error (ambiguous fill, matching current behaviour); if none, error (no morpheme matching slot). The three-predicate AND in `candidate_fits_cell` is implicit in the FST structure (the slot filter is baked into the sub-FST, the cell agreement is the input-side label match, the domain containment is a label-restriction during slot-FST construction).

---

## 6. Hard parts (called out openly)

### 6.1 Recursion (§3.4 clitic-with-paradigm)

A clitic that carries its own paradigm and attaches to a host's lazy enclitic slot creates a structural recursion: the clitic's own chain may contain other clitics, etc. The current implementation handles this with a runtime depth-counter (`render.rs::LAZY_COMPOSE_DEPTH = 32`, see `render.rs:~120`).

Two approaches in FST land:

**Option A — Flatten at compile time.** Inline the clitic's paradigm into every host position where it can appear. Bounded blowup: clitic paradigms are small (single-digit chains). Host's `enclitics*` becomes `(clitic_chain)*`. Implementation: a compile-time expansion pass that resolves recursive entry refs into their own FSTs and concatenates them in-place. Limitation: depth must be statically bounded (we already cap at 32; in practice natural-language clitic stacking is 1–3 deep).

**Option B — Network of FSTs with runtime cross-references.** Keep per-entry FSTs separate; at analysis/render time, when the chain FST hits a slot filled by a recursive entry ref, jump into that entry's FST, evaluate it, return. This is no longer a single FST — it is a pushdown machine. Mathematically less clean, but matches our current runtime model exactly.

**Recommendation: Option A (flatten with a bound).** The flat-FST property is what gives us reverse traversal, minimisation, and composition with phonrules — losing it would erode the entire motivation. A static depth bound (say 8, generous for any natural language) is acceptable, and the user can raise it per-grammar if needed. Implementation effort is the lower of the two and the cost (a moderate state-count blowup) is borne at compile time, not render time.

### 6.2 Variadic + phonrule interaction

Composing a Kleene-loop FST (`enclitics*`) with a phonrule FST can multiply state counts because the phonrule's left/right context window now spans loop iterations. Mitigation: **aggressive minimisation after each composition step**; **bound the loop** in practice (`{0,8}` instead of `*` for clitic stacking — naturally bounded by language) when we know language-specific maxima. Document the cost; profile after Phase F4.

### 6.3 Circumfix (`SlotKind::Circumfix`, current `^` splice in `lib.rs::CIRCUMFIX_SPLICE`)

Circumfix splits a single morpheme into prefix and suffix halves at the splice marker `^`, emitting them at two positions in the chain. In FST terms this is **two coupled paths** that must select the same morpheme: their input labels share a `morph=geᐧt` identity tag, but their output labels emit different halves.

Construction sketch: compile each circumfix morpheme into a *pair* of sub-FSTs (`T_pre[m]`, `T_suf[m]`) that share an input symbol `circ=m`; the chain FST references the circumfix slot twice, with the second reference constrained to bind to the same `circ=m` symbol consumed at the first. This is expressible as a small synchronisation symbol on the input side. XFST's `compile-replace` mechanism is one prior art; our case is simpler because the splice marker is fixed.

### 6.4 Infix (`SlotKind::Infix`, current `infix_positions` + `apply_infix_fills` at `render.rs:~500`)

Infix inserts a filler into a marked position inside the stem template (e.g. `{root.after_C1}`). This is **insertion at a pre-marked position**. Construction: include a non-emitting marker symbol in the stem FST at the infix position; compose with an insertion FST that consumes the marker and emits the infix's output. XFST has `compile-replace`-style constructions for exactly this pattern.

### 6.5 Bounded quantifier `{n,m}`

Trivially unrolled to alternation (`X{2,4}` = `XX | XXX | XXXX`). State count grows linearly with `m`. OK in practice for our usage (no chain has `m > 8`).

### 6.6 Forward vs reverse ambiguity

Multiple analyses for a single surface are the norm, not the exception (proto `-əd` has three readings; many languages have systematic syncretism). The FST naturally returns *all* analyses. The reverse-lookup API returns `Vec<Analysis>`; the caller decides. Forward ambiguity (one analysis → multiple surfaces) is currently disallowed by phase2's "ambiguous form spec" error — preserve that behaviour by rejecting at compile time any chain whose forward projection is not a function.

### 6.7 Tag labels vs surface labels — alphabet precision

FSTs are transducers: input alphabet ≠ output alphabet. We define:

- **Input alphabet** (analysis side): tag-value symbols (`tense=past`, `person=1`), morpheme-identity symbols (`morph=ed1`), structural markers (`BOUNDARY`, `CIRC=m`, `INFIX_POS=after_C1`).
- **Output alphabet** (surface side): Unicode codepoints / phoneme symbols, plus internal `BOUNDARY` markers preserved through phonrule application and stripped at the very end (matches current `strip_boundaries` at `phonrule_eval.rs:1126`).

This separation must be precise in the FST implementation. `rustfst` distinguishes input and output labels per arc; we use that directly.

---

## 7. Migration strategy

The current AST walker (`render.rs`, `slot_parse.rs`, `phonrule_eval.rs`) is shipped, tested (546 passing tests), and produces byte-identical output for Turkish and the proto. **Do not break it.** Migration is staged behind a feature flag and the FST path co-exists with the AST walker until F7.

### Phase F1 — FST infrastructure (effort: ~2 weeks)

Add `rustfst` as a dependency. Build a thin internal API (`hubullu::fst`) with: FST construction (states, arcs, input/output labels), basic operations (concat, union, Kleene-star, optional, bounded repeat, composition, determinisation, minimisation), traversal (forward and reverse), serialisation/deserialisation in `rustfst`'s native binary format wrapped in postcard envelopes.

Recommendation: **wrap `rustfst` rather than rolling our own.** `rustfst` provides the OpenFST-equivalent primitives (composition, minimisation, weight semirings) we need, and our value-add is the morphology-layer compilation on top, not the FST kernel.

Deliverable: API + unit tests for each primitive. No language code touched.

### Phase F2 — Compile rewrite rules → FST (effort: ~4 weeks)

Implement Kaplan-Kay rewrite-rule compilation in `hubullu::fst::rewrite`. Cover the constructs `phonrule_eval` handles: literal LHS/RHS, alternation classes, ranges, quantified atoms, syllable-aware contexts. Output an FST per phonrule body item, compose for the full body.

Unit-test against the **current** `phonrule_eval` outputs: feed the same input strings to both engines, assert identical surface results across the entire phonrule test suite. This is the safety net for correctness — if we match `phonrule_eval` byte-for-byte across the test corpus, we know the compilation is faithful.

Risk: syllable-aware contexts (F2c) interact with the iteration-to-convergence loop in non-obvious ways. Allocate buffer time.

### Phase F3 — Compile lexicon → FST (effort: ~1 week)

Build the morpheme-entry lexicon FST. Each inflectionless entry becomes a path: input labels are the entry's declared tags plus a morpheme-identity label; output labels are the headword phonemes. Build axis-bucket indexes as a side-effect.

### Phase F4 — Compile compose chain + slot grammar → FST (effort: ~3 weeks)

For each `Compose` body: compile each slot to a sub-FST (eager: literal-rule paths; lazy: lexicon FST restricted by `LazyMatching` filter). Concatenate per the chain expression, wrap quantifiers, compose with phonrule FSTs per `PhonApply`. Validate against `render_lazy_compose` for Turkish + proto across the full integration test suite — surface strings must match byte-for-byte.

Special handling for `SlotKind::Circumfix` and `SlotKind::Infix` deferred to F8 — for now error out on those slots in the FST path (the AST walker still handles them).

### Phase F5 — Per-entry FST network; drop-in replace `find_form_by_spec` + `auto_fill_lazy_slots` (effort: ~2 weeks)

Build per-entry FSTs by binding stems. Replace `find_form_by_spec` and `auto_fill_lazy_slots` with FST traversal under the same function names (drop-in). The AST walker remains; the dispatch picks the FST path when an `--engine fst` flag is set or unconditionally for entries whose FST exists in the cache.

### Phase F6 — Reverse lookup → new CLI `hubullu analyze <surface>` (effort: ~2 weeks)

Wire the FST's reverse traversal into a new CLI command. End-user feature: given a surface string and a language `.huc`, return the analyses (entry id + cell). This is the first feature the FST migration delivers that the old engine cannot.

### Phase F7 — Drop SQLite; new `.huc` format (effort: ~3 weeks)

Cut over `emit_sqlite.rs` → `emit_fst_huc.rs`. New format reader. Migrate render, LSP, and any tool that touches `.huc`. **Backward-compat boundary**: old `.huc` files cease to load at F7 — users recompile. (Open question 5.)

### Phase F8 — Hard parts in the FST formulation (effort: ~3 weeks)

Recursion (Option A flattening), circumfix, infix, variadic+phonrule blowup mitigation. AST walker continues to handle any grammar the FST cannot, until each falls. Deletion of AST walker is the *end* of F8.

### Phase F9 (optional) — Companion artefacts (effort: ~2 weeks)

Parquet export for analytics, tantivy index for meaning-text FTS, alternative read-only `fst` crate index for super-fast headword prefix-search.

**Total estimate: ~22 engineer-weeks of focused work, ~6 months elapsed at a sustainable pace.** Phases F1, F2, F3 can partially overlap; F4 must wait on F3. F5–F6 are short and high-value. F7 is the irreversible cutover.

Each phase lands green: every existing test passes plus new FST-engine tests; the AST walker is the safety net throughout F1–F7. After F7, the FST is the engine; after F8, the AST walker is removed.

---

## 8. Open design decisions (for user resolution)

1. **Roll our own FST vs use `rustfst`.** **RESOLVED 2026-05-16**: wrap `rustfst` (≥1.2.6) behind a **swap-ready in-tree trait/facade** (`hubullu::fst::FstBackend` or similar). No `rustfst` types leak into the morphology layer above; backend swap = re-implement the trait + recompile. Rationale: kernel-design proposal shows C (full in-tree kernel) is 8–11 weeks vs B (rustfst + custom mmap) at 1.5–2 weeks for the same `~1GB mmap` scale ceiling — 7–9 week delta is better spent on Kaplan-Kay. The interface seam (Appendix B of `fst-kernel-design.md`) keeps B↔C reversible: switch is one module if assumptions change. See `docs/proposals/rustfst-survey.md` and `docs/proposals/fst-kernel-design.md`.

2. **Recursion handling — flatten vs network-of-FSTs.** **RESOLVED 2026-05-16**: flatten with a depth bound, **CLI-configurable** (e.g. `--max-recursion=N`), **default 8**. Going past the bound at compile time is a clean error ("recursion bound exceeded"). Preserves flat-FST mathematical properties (reverse traversal, minimisation, composition) and matches the spirit of the current `MAX_LAZY_COMPOSE_DEPTH = 32` runtime cap.

3. **`.huc` format.** Recommendation: **postcard** for the envelope + AST/metadata; **`rustfst` native binary** for FST blobs. Rationale in §4. Alternatives: bincode (format-unstable across versions), rkyv (zero-copy but invasive trait derives), a custom format (maximal control, maximal maintenance burden).

4. **FTS strategy after SQLite removal.** Recommendation: **tantivy** for meaning-text FTS, **`fst` crate** for headword prefix-search. Both are mature Rust crates with permissive licences. Alternative: keep SQLite only for FTS — but that re-introduces the dual-format complexity we are trying to escape.

5. **Backwards-compat boundary.** **RESOLVED 2026-05-16**: no backwards compat. hubullu is pre-1.0.0; old `.huc` files stop loading at F7 (or whenever the format changes). Users recompile from `.hu` sources. No dual-write / no transitional SQLite reader.

6. **Keep AST walker as a permanent fallback?** Recommendation: **hard-delete after F8.** A permanent fallback path is a permanent maintenance tax and the FST engine should be expressive enough for every supported grammar by F8. Alternative: keep the walker behind `--engine ast` for users with grammars the FST cannot handle — only attractive if F8 leaves real gaps; revisit at F8 if so.

7. **(Soft)** Whether to ship the analyser CLI (F6) at F6 or hold for F7. Recommendation: ship at F6 even if it requires the old SQLite `.huc` alongside the new FST cache — the reverse-lookup feature is the user-visible win that justifies the migration to outside observers.

---

## 9. Non-goals / scope

- **Not redoing the source language design.** `compose + slot + matching` syntax and `.hu`/`.hut` semantics are untouched.
- **Not changing what hubullu produces semantically.** Surface strings remain byte-identical, validated by the same test suite. This is an engine swap, not a language change.
- **Not optimising for very large lexica in the first cut.** Proto (hundreds of entries) and Turkish (a few hundred) are the targets through F8. A union-FST analyser that scales to wordlists of hundreds of thousands is a later optimisation (Phase F9-bis).
- **Not implementing weighted FSTs.** No probabilistic morphology, no rule ranking. Unweighted transducers are sufficient for hubullu's deterministic-by-construction grammars.
- **Not replacing the parser, phase1, or phase2.** They produce the AST that the FST compiler consumes; their interfaces are stable.

---

## 10. References

- Kaplan, R. M. & Kay, M. (1994). "Regular models of phonological rule systems." *Computational Linguistics* 20(3).
- Beesley, K. R. & Karttunen, L. (2003). *Finite State Morphology*. CSLI Publications. (The XFST book.)
- Koskenniemi, K. (1983). *Two-level morphology: A general computational model for word-form recognition and production*. University of Helsinki.
- Hulden, M. (2009). *Foma: a finite-state compiler and library*. EACL 2009 demos.
- HFST — Helsinki Finite-State Toolkit. <https://hfst.github.io/>
- `rustfst` — Rust port of OpenFST. <https://crates.io/crates/rustfst>
- `fst` — read-only FST for Rust by BurntSushi. <https://crates.io/crates/fst>
- `tantivy` — full-text search engine in Rust. <https://crates.io/crates/tantivy>
- `postcard` — serde-compatible serialisation format. <https://crates.io/crates/postcard>

---

## Appendix A — Concrete mapping table

For reference, here is the current code → FST mapping with file:line citations:

| current | location | FST equivalent |
|---|---|---|
| `ComposeExpr::Concat` | `ast.rs:686` | FST concatenation |
| `ComposeExpr::PhonApply` | `ast.rs:688` | FST composition |
| `ComposeExpr::Slot { quantifier }` | `ast.rs:684`, `SlotQuantifier` at `ast.rs:698` | sub-FST + Kleene/optional/bounded |
| `SlotBody::Eager` | `ast.rs:790` | literal-rule sub-FST |
| `SlotBody::Lazy(LazyMatching)` | `ast.rs:793`, `AxisFilter` at `ast.rs:811` | lexicon FST + label restriction |
| `SlotKind::Circumfix` | `ast.rs:781` | coupled sub-FST pair with sync symbol |
| `SlotKind::Infix` | `ast.rs:782` | stem FST with marker symbol + insertion FST |
| `PhonRule` body | `phonrule_eval.rs:158` | Kaplan-Kay rule-FST sequence |
| `apply_phonrule_with_resolver` | `phonrule_eval.rs:128` | T_input ∘ T_phonrule |
| `find_form_by_spec` | `render.rs:32` | forward traversal of `T_entry` |
| `auto_fill_lazy_slots` | `render.rs:680` | label-restricted traversal |
| `candidate_fits_cell` | `render.rs:824` | implicit in slot sub-FST construction |
| `render_lazy_compose` | `render.rs:210` | forward traversal end-to-end |
| `LAZY_COMPOSE_DEPTH` guard | `render.rs:~120` | static flatten depth bound (§6.1) |
| `strip_boundaries` | `phonrule_eval.rs:1126` | output-tape post-processing |
| `forms` table | `emit_sqlite.rs:82` | enumerable FST paths (not materialised) |
| FTS5 `entries_fts` | `emit_sqlite.rs:495` | tantivy index (separate from FST) |

This table is the spec for the F4–F7 implementation: each row is a concrete migration unit.
