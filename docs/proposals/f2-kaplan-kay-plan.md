# F2 — Compile phonrule rewrite rules to FSTs

Detailed implementation plan for **Phase F2** of the FST migration:
translate every `phonrule` declaration into one or more finite-state
transducers whose application produces **byte-identical** surface strings
to the current `phonrule_eval` evaluator on every input.

Companion to:

- `docs/proposals/fst-morphology.md` — the architecture (esp. §3, §6, §7)
- `docs/proposals/rustfst-survey.md` — the kernel we build on (esp. §4)
- `docs/proposals/fst-kernel-design.md` — algorithm sketches (esp. §3)
- `src/fst/backend.rs` — the `FstBackend` trait F2 consumes (landed in F1)
- `src/fst/tests.rs` — the F1 smoke tests F2 mirrors
- `src/phonrule_eval.rs` — **the source of truth for hubullu's phonrule
  semantics**, against which F2 is validated

This document is **plan only**. No code lands here. The intent is that an
engineer who has *not* read the Kaplan-Kay / Karttunen papers can pick this
up plus the cited references and implement F2 from it.

Date: 2026-05-16. Author: F2 planning.

---

## Section 1 — Algorithm choice and rationale

Three published algorithms compile `A → B / L _ R` rewrite rules to FSTs.

### 1.1 Kaplan & Kay (1994)

*Comp. Ling.* 20(3). The **foundational treatment**. Five-step
compilation: introduce auxiliary brackets, restrict by context, replace
the bracketed material, restrict again to forbid spurious brackets,
erase brackets. Each step is a transducer composed left-to-right.
Mathematically clean. Practically: bracket management is fussy and the
"obligatory / longest-match / leftmost" variants are implicit in how
steps (2)–(3) are written, not first-class.

### 1.2 Mohri & Sproat (1996) — "An Efficient Compiler for Weighted Rewrite Rules"

*ACL 1996*, https://aclanthology.org/P96-1031.pdf. The paper
fst-morphology §3 means when it cites "Mohri 1996". A direct
construction in fewer stages; generalises to weighted rewrites
(irrelevant to us per fst-morphology §9). Explicit about
left-to-right longest-match as default.

### 1.3 Karttunen (1995) — "The Replace Operator"

*ACL 1995*, https://aclanthology.org/P95-1003.pdf. Introduces `->` as a
**first-class operator** with clean semantics for: left vs right context
(`A -> B || L _ R`), obligatory vs optional (`->` vs `(->)`),
longest-match (`@->`), leftmost-vs-rightmost. XFST and Foma's `->`
operator *is* this paper. Under the hood it still uses K-K-style bracket
introduction; the value-add is variant flags as named knobs rather than
implicit constructions.

### 1.4 Recommendation — Karttunen 1995

**Pick Karttunen 1995.** Reasoning in order:

1. **Surface-syntax fit.** Hubullu's `A -> B / L _ R` is literally
   Karttunen's syntax. Translating `PhonRewriteRule` to a Karttunen
   call is a per-field mapping, not a per-construct reformulation.
2. **First-class variant flags.** Karttunen names obligatory /
   longest-match / leftmost as orthogonal options. `phonrule_eval`
   happens to implement all three (§3.5 verifies); we instantiate one
   Karttunen variant. K-K and Mohri make us assemble the variant.
3. **Open-source reference impl.** Foma's `replace.c` (Apache-2.0
   compatible) implements Karttunen `->` directly. K-K has no
   comparable open impl (original XFST source is closed).
4. **Same primitives as K-K underneath.** We are picking a cleaner
   *packaging*, not a different algorithm. If we ever need to drop down
   to raw K-K for a corner case, the bracket-management code is reusable.

Per rustfst-survey §4 finding (1), **`rustfst` ships no rewrite-rule
compilation layer**. F2 builds Karttunen on top of `FstBackend`:
`rustfst` gives composition / determinise / minimise / ε-removal; we
give the bracket protocol and per-step transducers.

Alternatives rejected: K-K (1994) directly (~1.5× the LOC, no open
reference impl); Mohri (1996) (mostly cited for its weighted extension,
which we don't use); Koskenniemi 1983 two-level (out of scope per
fst-morphology §10).

---

## Section 2 — Construct-by-construct mapping

For each phonrule AST piece (per `src/ast.rs` and `src/phonrule_eval.rs`),
the corresponding FST construction.

### 2.1 AST → FST table

| AST piece                          | FST construction                                                                                  | Output type             | LOC est |
|------------------------------------|---------------------------------------------------------------------------------------------------|-------------------------|---------|
| `CharClassDef { List(members) }`   | acceptor: single-state self-loop, one arc per member symbol (identity input=output)               | acceptor                | 40      |
| `CharClassDef { Union(refs) }`     | union of compiled member-class acceptors (`FstBackend::union` chained)                            | acceptor                | 30      |
| `PhonMapDef { arms, else_arm }`    | transducer: one arc per `arm` (in_label → out_label); `else_arm` = identity over class complement | transducer              | 100     |
| `PhonRewriteRule { from, to, ctx }`| Karttunen replace: brackets-intro ∘ context-restrict ∘ A→B ∘ brackets-erase                       | transducer              | **400** |
| `PhonPattern::Class(id)`           | reference to compiled class acceptor                                                              | reused                  | —       |
| `PhonPattern::Literal(s)`          | path acceptor; one arc per char, identity I/O                                                     | acceptor                | 40      |
| `PhonPattern::Range(elems)`        | sequence of compiled `PhonContextElem`s — see §2.3                                                | acceptor                | 80      |
| `PhonReplacement::Literal(s)`      | path emitter; ε-input, char-output per char                                                       | acceptor (output side)  | 30      |
| `PhonReplacement::Null`            | ε-emitter (no output arcs)                                                                        | trivial                 | 10      |
| `PhonReplacement::Map(name)`       | composition with compiled `PhonMapDef`                                                            | reused                  | —       |
| `PhonContextElem::Atom(Class,Q)`   | quantifier-wrapped class acceptor                                                                 | acceptor                | 30      |
| `PhonContextElem::Atom(NegClass,Q)`| Σ\class complement, then quantifier-wrapped                                                       | acceptor                | 60      |
| `PhonContextElem::Atom(Lit,Q)`     | quantifier-wrapped literal-path acceptor                                                          | acceptor                | 30      |
| `PhonContextElem::Atom(Wildcard,Q)`| quantifier-wrapped Σ-acceptor                                                                     | acceptor                | 20      |
| `PhonContextElem::Atom(Alt,Q)`     | union of compiled alternatives, then quantifier-wrapped                                           | acceptor                | 40      |
| `PhonContextElem::Atom(SylBlock,_)`| **F2 NON-GOAL** — see §10                                                                         | —                       | 0       |
| `PhonContextElem::Boundary` (`+`)  | acceptor for BOUNDARY symbol *or* edge anchor — see §2.4                                          | acceptor                | 40      |
| `PhonContextElem::WordStart` (`^`) | edge anchor; uses `^WORD_START` marker symbol — see §2.4                                          | acceptor                | 30      |
| `PhonContextElem::WordEnd` (`$`)   | edge anchor; uses `WORD_END$` marker symbol — see §2.4                                            | acceptor                | 30      |
| `PhonContextElem::SylHead/Tail/Index` | **F2 NON-GOAL** — syllable-aware; see §10                                                       | —                       | 0       |
| `Quantifier` wrapping              | dispatch on `Star/Plus/Question/Exact/AtLeast/Range` → `closure_*` / `bounded` from trait        | combinator              | 40      |
| `PhonBodyItem::Rewrite` sequence   | left-to-right composition of compiled rule transducers (matches `phonrule_eval` body loop)        | transducer              | 50      |
| `PhonBodyItem::Apply(name)`        | resolve name → compose the resolved phonrule's FST at that position in the body order             | transducer              | 80      |
| **Iterative-to-convergence**       | not part of the per-rule FST; lives in the runtime apply loop — see §4                            | —                       | varies  |

**Total LOC for the rule compiler module proper: ~1100–1300 LOC.** Plus
~300 LOC of tests and harness inside the module, plus the ~300 LOC
validation harness (§6) outside it. **Grand total F2: ~1700–1900 LOC.**

### 2.2 Notes per construct

- **Classes** are acceptors (identity I/O); compose on either side of
  a transducer. Built once per phonrule, cached by name.
- **Map** is the only inherently transducing primitive below rewrites.
  The `else` arm covers the complement of the explicit arms over the
  input alphabet — requires alphabet closure (§5).
- **Range LHS** (`PhonPattern::Range`) reuses `PhonContextElem`
  compilation: one `compile_elem_sequence` function serves both LHS-range
  and L/R-context, since both are `Vec<PhonContextElem>`. The structural
  difference is only in how the resulting acceptor plugs into the
  Karttunen construction. This mirrors `phonrule_eval`'s F8
  LHS-range behaviour (`phonrule_eval.rs:423-491`).
- **Quantifiers** map to `FstBackend::closure_{star,plus,optional,bounded}`.
  `Quantifier::Exact(n)` and `AtLeast(n)` unroll to `concat(n) ∘ closure_*`.

### 2.4 Anchors and boundaries — alphabet conventions

`phonrule_eval` carries three specials in the input stream: `BOUNDARY`
(`\0`, `phonrule_eval.rs:21`), the start of string (matched by `^`),
and the end (matched by `$`). The `+` context elem matches a literal
`BOUNDARY` char *or* the word edge (`phonrule_eval.rs:638-653`).

FSTs need explicit reserved labels. F2 defines three input-alphabet
markers (§5): `BOUNDARY_LABEL`, `WORD_START_LABEL`, `WORD_END_LABEL`.

**Runtime convention** (matches eval semantics):

1. The caller prepends `WORD_START_LABEL` and appends `WORD_END_LABEL`
   to the input *if any rule references `^` or `$`* (phase2 knows
   this already).
2. The caller strips both markers + all `BOUNDARY_LABEL`s after the
   final apply step (matches `strip_boundaries` at `phonrule_eval.rs:1126`).
3. `Boundary` (`+`) compiles to the union
   `boundary_acceptor ∪ word_start_acceptor ∪ word_end_acceptor`,
   reproducing the eval's "boundary OR edge" semantics
   (`phonrule_eval.rs:638-653`).

Standard FST trick: positional anchors become symbol-stream markers.

---

## Section 3 — Concrete Karttunen replace construction

The genuinely hard subsection. We build `compile_rewrite_rule(rule,
alphabet) -> Fst`.

Karttunen 1995 §3 defines `T_repl(A, B, L, R)` as the transducer that
replaces every leftmost, longest occurrence of `A` with `B` provided
the occurrence is preceded by a string in `L` and followed by a string
in `R`. Elsewhere identity. Construction (Karttunen §3.2; Beesley &
Karttunen 2003 Chapter 3 is the readable expansion):

```
T_repl = Mark ∘ Constraint ∘ Replace ∘ Unmark
```

### 3.1 Step 1 — `Mark`: introduce bracket markers around every `A`

A transducer that scans the input and, at every position where `A`
matches, inserts a pair of bracket symbols `[` and `]` around the
matching span. Mathematically: an identity over Σ extended by ε-arcs
that emit `[` before and `]` after each `A`-match.

Notation: reserve four bracket symbols on the output alphabet of `Mark`
(equivalently, the input alphabet of `Constraint`):

- `[+]` — opening bracket for an obligatory rewrite site.
- `[-]` — opening bracket for an optional/disallowed site (we don't use
  this; obligatory only).
- `]+` — closing bracket for an obligatory rewrite site.
- `]-` — closing for optional.

(Four symbols vs two so that Step 2's restriction can talk about
"obligatory-but-disallowed-by-context" vs "always-disallowed" — Karttunen
1995 §3.3.)

Construction sketch: `Mark` is `(Σ* ∘ [+] A ]+ ∘ Σ*)*` interpreted as an
FST that consumes Σ and either passes it through or wraps an `A` match in
brackets. Roughly:

```
Mark = (Σ:Σ)* concat
       ((ε:[+]) concat A_identity concat (ε:]+))*
       concat (Σ:Σ)*
```

Where `A_identity` is the LHS-acceptor `A` projected as an identity
transducer (input=output) — built from §2's `PhonContextElem`/`PhonPattern`
compilation. The construction is non-deterministic on purpose: Step 2
will throw out the wrong markings.

LOC: ~120.

### 3.2 Step 2 — `Constraint`: keep only context-licensed brackets

A transducer that accepts only those marker placements where every `[+]
... ]+` pair is preceded by something matching `L` and followed by
something matching `R`. Otherwise rejects the path (no accepting output).

Construction sketch:

```
Constraint = ¬(Σ_with_brackets* concat
              ([+] concat Σ_no_brackets* concat ]+)
              concat (¬R | ¬anything_after)
              concat Σ_with_brackets*)
∩ ¬(Σ_with_brackets* concat
    (¬L | ¬anything_before)
    concat ([+] concat Σ_no_brackets* concat ]+)
    concat Σ_with_brackets*)
```

In words: reject any marking where an obligatory-bracket pair is followed
by *not-R* or preceded by *not-L*. The complement and intersection are
standard FSA operations; on a transducer we project to the output side
(brackets visible) and back. `rustfst` gives us `union` directly;
complement and intersection are buildable as `union`-of-complements or
`compose`-with-restrictions; the wrapper helper `restrict_by_label_set`
mentioned in rustfst-survey §4 (2) is the workhorse.

This is the **fussiest** step. Karttunen 1995 §3.4 gives the precise
construction; Beesley & Karttunen 2003 §3.5–3.6 is a more readable
walkthrough.

LOC: ~180.

### 3.3 Step 3 — `Replace`: substitute `B` for the bracketed `A`

A transducer that, at every `[+] A ]+` sequence in its input, deletes
the brackets and `A` and emits `B` in their place. Elsewhere identity.

Construction sketch:

```
Replace = (Σ:Σ)* concat
          (([+]:ε) concat (A:B) concat (]+:ε))*
          concat (Σ:Σ)*
```

Where `(A:B)` is a transducer that consumes the LHS `A` and emits the RHS
`B`. For `PhonReplacement::Literal`, `B` is a path emitter; for `Null`,
`B` is empty (ε); for `Map`, `B` is composition with the compiled
`PhonMapDef`.

LOC: ~80.

### 3.4 Step 4 — `Unmark`: erase residual brackets

Identity on Σ, ε on bracket symbols. Trivial.

LOC: ~20.

### 3.5 Variant choice — obligatory + longest-match + leftmost-first

Karttunen §4 enumerates `->` variant flags. For each, hubullu's
required behaviour, **verified** against `phonrule_eval`:

- **Obligatory.** `phonrule_eval` rewrites every match it finds (no
  "(->)" syntax exists). `apply_replacement_rule`
  (`phonrule_eval.rs:314-406`) and `apply_range_rewrite_rule`
  (`phonrule_eval.rs:423-491`) push every context-licensed match and
  apply them all.
- **Longest-match.** `apply_range_rewrite_rule`
  (`phonrule_eval.rs:438-456`) enumerates LHS-match ends in
  greedy-first order via `match_lhs_range_ends`
  (`phonrule_eval.rs:967-1054`, see the comment at line 977: "Greedy:
  descend from the largest repetition count down to `min`"), then
  `.find()`s the first context-passing one — the longest consistent
  with context.
- **Leftmost-first.** Range scanner walks forward, accepts the first
  longest match, resumes past it (`phonrule_eval.rs:466-467`
  "Non-overlapping: resume scanning past this match."). For
  single-symbol LHS, matches cannot overlap so the order is academic.

**Verdict**: F2 instantiates Karttunen `@->` (obligatory + longest-match
+ leftmost-first). This is also XFST's default `->` semantics (Beesley
& Karttunen 2003 Chapter 3 "Replace by Default") — when we crib from
Foma's `replace.c`, we're reading the right code path.

### 3.6 Where the construction is delicate

1. **Bracket-symbol reservation.** Labels `[+]/[-]/]+/]-` must never
   collide with phoneme labels. Mitigation: reserved labels live in
   1..16; alphabet interning refuses that range.
2. **Complement in `Constraint`.** `rustfst` ships no FSA complement;
   build as `Σ* difference L_R_violating_set` (intersect-with-complement).
   The wrapper helper `restrict_by_label_set` (rustfst-survey §4 item 2)
   covers this.
3. **`rustfst` issue #288 (determinise divergence).** Regression-test
   `Mark ∘ Constraint` on a small hand-built input where the determinised
   form is known.
4. **Functional input for determinisation.** `T_repl` is functional by
   construction; per fst-morphology §6.6 we reject ambiguous chains at
   compile time. If determinisation diverges anyway, fall back to a
   non-deterministic FST (correct, slower).
5. **Empty-LHS (insertion) rules.** `phonrule_eval` special-cases
   `"" -> X` (`is_insertion_rule`, `phonrule_eval.rs:255-310`).
   Karttunen 1995 §5 handles this without changes: LHS acceptor becomes
   ε; Mark inserts brackets at every context-licensed position. Verify
   with explicit empty-LHS test cases.

---

## Section 4 — Iterative-to-convergence

`phonrule_eval` runs each rewrite rule in a loop until the string stops
changing (`phonrule_eval.rs:164-178`). Cascading harmony: one
application of vowel harmony may propagate through a long suffix chain
only after several iterations. In FST terms this is the **transitive
closure** of the rule transducer.

### 4.1 Options

- **(a) Compile-time transitive closure.** Compose `T_rule` with itself
  until the composition stabilises. Single FST that converges in one
  pass at apply time. Risk: may not converge; state count grows per
  iteration. Bound `N` at compile time; error on overflow.
- **(b) Runtime iteration loop.** Compile each rule to its single-pass
  FST; at runtime, reapply until the surface stabilises. Mirrors
  current eval behaviour.
- **(c) Built-in closure in the Karttunen operator.** Mathematically
  cleaner; harder to get right because closure interacts with the
  bracket protocol.

### 4.2 Recommendation — Option (b)

Pick **runtime iteration loop**. Reasoning:

1. **Matches `phonrule_eval` exactly.** Validation gate (§6) is
   byte-identity; same-shape iteration is the smallest behavioural delta.
2. **Lowest risk.** No FST construction beyond single-pass.
3. **Easiest to bound.** Counter `MAX_PHONRULE_ITER = 64` (configurable)
   catches runaway rules cleanly.
4. **Profile-driven optimisation.** Turkish converges in 1–3 iterations;
   if profiling later proves runtime closure hot, switch to (a) — the
   single-pass FST and validation harness remain valid.

### 4.3 Apply-driver pseudocode

```
apply_phonrule_fst(input, phonrule):
    let mut s = input
    for item in phonrule.body_items:
        match item:
            CompiledRewrite(fst):
                loop:
                    let next = apply_fst(&fst, &s)
                    if next == s: break
                    if iter_count > MAX_ITER: error
                    s = next
            CompiledApply(other_fst):
                s = apply_phonrule_fst(s, other_fst)
    strip_boundaries(s)
```

Mirrors `phonrule_eval.rs:158-196` line-for-line with `apply_fst`
replacing `apply_rewrite_rule`. ~80 LOC.

---

## Section 5 — Alphabet management

The Karttunen construction needs a **closed alphabet** at compile time:
the `Σ` in `(Σ:Σ)*` from §3 must enumerate every symbol that can appear
in an FST input. `phonrule_eval` ducks this by working over `&str` and
inspecting characters at scan time; FSTs cannot.

### 5.1 Where the alphabet comes from

For each phonrule, the alphabet is the union of:

1. **Phoneme declarations.** `phoneme NAME { ... }` produces a set of
   terminal strings via `PhonemeInventory::all_terminals` (see
   `src/phoneme.rs:28` and `resolve_inventory` at `phoneme.rs:66`). This
   is the **primary source**.
2. **Class member literals.** Every literal appearing in a
   `CharClassDef::List` (`ast.rs:401`) — these may or may not already be
   phonemes; phase2 doesn't require they are. Treat as alphabet members.
3. **Literals in rewrite rules and contexts.** Any `PhonReplacement::Literal`,
   `PhonPattern::Literal`, or `PhonAtom::Literal` may introduce characters
   not declared as phonemes. Treat as alphabet members.
4. **Map outputs.** `PhonMapArm::to` literals and `else_arm` literals.
5. **Reserved markers.** `BOUNDARY_LABEL`, `WORD_START_LABEL`,
   `WORD_END_LABEL`, and the four bracket markers `[+]/[-]/]+/]-`. These
   live in a fixed low-label range and are never available to phoneme
   interning.

### 5.2 The `Alphabet` builder

A new struct `PhonRuleAlphabet` in `src/fst/rewrite/alphabet.rs` bundles:
the surface `SymbolTable`; a precomputed `(Σ:Σ)*` acceptor reused
everywhere; the reserved label constants (`boundary_label`,
`word_start_label`, `word_end_label`, `bracket_*`). Constructor takes
a `PhonRule` + `&PhonemeInventory`, walks rule + classes + maps to
collect every symbol, interns into a fresh `SymbolTable`, builds
sigma_acceptor once. Downstream compilation passes `&PhonRuleAlphabet`.
~150 LOC.

### 5.3 Symbol-table sharing across phonrules

`compose harmony(elision(chain))` runs two phonrule FSTs back-to-back;
the surface symbol `e` must have the same label in both for output→input
piping to work. Discipline:

1. Build a per-language **shared `SurfaceAlphabet`** at compile time
   from `PhonemeInventory::all_terminals` + reserved markers.
2. Each `PhonRuleAlphabet` *extends* that base — same labels for shared
   symbols, new labels only for phonrule-private brackets / unknown
   literals.
3. At the `compose`-chain level (F4/F5), reuse the shared base; bracket
   labels are stripped by each phonrule's own `Unmark` step (§3.4) so
   they never leak across phonrule boundaries.

~100 LOC.

### 5.4 Alphabet-drift warning

A phonrule literal not in `PhonemeInventory::all_terminals` is almost
always a typo; F2 warns at compile time (escalatable to error via a
config flag). ~50 LOC.

---

## Section 6 — Validation strategy

The F2 acceptance criterion is: **for every (phonrule, input) pair in the
validation corpus, the FST and `phonrule_eval` produce byte-identical
surface strings.** Less than this is a bug.

### 6.1 Corpus composition

1. Every phonrule-touching `fixtures/` and `examples/` directory:
   `fixtures/inline`, `fixtures/comprehensive`, `examples/turkish`
   (harmony + elision), `examples/syllable-rules`.
2. The proto's phonrules from the a priori project's `hu/profile.hu`.
3. `phonrule_eval`'s own unit tests (`phonrule_eval.rs:1130–1792`).
4. **Fuzz corpus.** Random strings over each phonrule's alphabet,
   lengths 1–50, with random BOUNDARY insertions. ~1k inputs during
   development, ~10k at acceptance time. Bias toward inputs near
   BOUNDARY chars and word edges (the cornercase-rich region in
   `phonrule_eval.rs:638-678`).

### 6.2 Differential harness

A new test module `src/fst/rewrite/validation_tests.rs` exposes
`assert_fst_matches_eval(phonrule, input, alphabet)` which compiles +
applies the FST, calls `apply_phonrule`, asserts equality with a
diagnostic showing both outputs. Driver: enumerate corpus, accumulate
failures, report. Also exposed as `hubullu fst validate <hu_file>` for
ad-hoc developer use. ~300 LOC.

### 6.3 Acceptance threshold

**Hard requirement: 100% match on the golden corpus (items 1–3).** Any
mismatch blocks F2.

**Soft requirement: ≥99.9% match on fuzz (item 4).** Sub-1 mismatch per
10k inputs are investigated case-by-case; if divergence is a
`phonrule_eval` bug (1792 LOC of subtle context-matching, plausibly
buggy), **fix `phonrule_eval` to match the FST** — the FST is the
rigorous version, eval is the older heuristic. This is open question §8.5.

### 6.4 Cross-engine dual-write phase

After F2 lands, run both engines in parallel for one release cycle:
renderer calls both, diffs, logs diffs, uses **eval** output. Once a
release passes with zero diffs, switch to FST. F5+ switchover; F2 only
delivers the harness.

---

## Section 7 — Step-by-step implementation breakdown

| # | Substep                                             | LOC     | Days | Risk   |
|---|-----------------------------------------------------|---------|------|--------|
| 1 | Alphabet derivation + symtab sharing (§5)           | 250     | 3    | low    |
| 2 | `CharClassDef` → acceptor (§2.1 r1–2)              | 80      | 1    | low    |
| 3 | `PhonMapDef` → transducer (§2.1 r3)                | 120     | 2    | low    |
| 4 | `PhonContextElem` compilation (§2.1 r7–13)         | 280     | 4    | medium |
| 5 | **Karttunen rewrite-rule compilation (§3)**         | **450** | **7–10** | **HIGH** |
| 6 | Rule sequence + `apply` chain (§2.1 r14–15)         | 100     | 2    | medium |
| 7 | Runtime iteration loop (§4)                         | 80      | 1    | low    |
| 8 | Validation harness + corpus + fuzz (§6)             | 300     | 4    | low    |
| 9 | Integration / feature-flag dispatch (§6.4)          | 50      | 1    | low    |
| 10 | Docs + memory + handoff                            | —       | 0.5  | low    |

**Sums.** LOC: ~1660–1860 (plus ~400 LOC of inline tests not
double-counted) → total file footprint ~2100 LOC.
Working days: **25.5–30 = 5.1–6 working weeks** for one engineer.

**Honest framing on slippage.** The fst-morphology proposal §7
estimated F2 at **4 weeks**. This plan estimates **5–6 weeks**. The
delta is concentrated in step 5: the proposal costed the algorithm in
the abstract; this plan costs it *including* bracket protocol,
complement-and-intersect in Constraint, and inevitable debugging on the
first non-toy phonrule. For a fresh engineer, 7–10 days is honest.

**Critical path.** Step 5 is the spike. Steps 1–4 land in week 1;
step 5 occupies weeks 2–3; steps 6–10 land in weeks 4–5 with week 6 as
buffer.

**Bail-out checkpoint.** If at end of week 3 step 5 is still red on
basic Turkish harmony: either drop to plain K-K 1994 and rebuild step 5
from the ground primitives we already have, or escalate. Do not bury
time in a Karttunen construction that isn't converging.

---

## Section 8 — Open design decisions

### RESOLVED 2026-05-16
- **Karttunen variant**: `@->` only (obligatory + longest-match + leftmost-first). Source syntax stays as the current `->`. No variant flags exposed; defer multi-variant until there's a real need.
- **Validation gate**: **strict — 100% match against `phonrule_eval` on every input, golden + fuzz both**. No escape hatches. Adds ~+1–2 weeks to the §7 tail (corner-case hunt); revised F2 total: **~6–8 weeks**.

### Remaining open (follow plan defaults unless flagged in implementation)

1. **Karttunen variant.** Recommend `@->` (obligatory + longest-match
   + leftmost-first), verified against `phonrule_eval` in §3.5.
   **Confirm**, or opt into other variants. Sub-decision: expose
   variant flags in source syntax (`A ->? B`, `A @-> B`), or keep
   `@->` always implicit? **RESOLVED — see above.**

2. **Iteration: runtime-loop (Option (b)) vs compile-time-closure
   (Option (a)).** Recommend (b) per §4.2. Switch to (a) later is
   additive; API unchanged.

3. **Alphabet closure source.** §5 takes `phoneme` declarations + class
   literals + rule/context/map literals + reserved markers. Restrict to
   `phoneme` declarations only (strict, errors on undeclared literals)
   or accept ad-hoc literals with a warning (permissive, recommended)?

4. **Per-rule FST cache location.** (a) in-process LRU keyed by
   `(phonrule_ast_hash, alphabet_hash)` — easy, lost on restart; (b)
   on-disk in `.huc` per fst-morphology §4 — needs F7 first; (c) both.
   Recommend (a) for F2, (c) eventually.

5. **Validation gate.** §6.3 proposes "100% on golden corpus + ≥99.9%
   on fuzz". Tighten ("100% on fuzz, no escape hatches") or loosen?

6. **Switchover.** Dual-write phase (run both, diff in production for
   one cycle) vs hard cutover when validation is green. Recommend
   dual-write per §6.4.

**Top 2 unresolved questions** in order of constraint on F2 work:
**(1) Karttunen variant** (small confirmation if `@->` is right; rework
if not — recommend confirming `@->`) and **(5) validation gate** (the
stricter the gate, the longer the F2 tail; the looser, the higher the
post-F2 bug rate).

---

## Section 9 — Risks

1. **Karttunen construction correctness (§3).** The single biggest
   risk. Step 2's complement-of-complement is easy to get subtly wrong
   and mis-licenses sites silently. Mitigation: crib from Foma's
   `replace.c`; hand-build reference FSTs for the simplest cases (one
   rule, one symbol, no context) and grow the test set outward.
   Validation harness (§6) catches divergence early.

2. **Iterative-to-convergence non-termination.** A malformed rule
   (e.g. `a -> aa / _`) loops forever in `phonrule_eval` too — its loop
   at `phonrule_eval.rs:164-178` is unbounded. F2's iteration **must**
   bound (`MAX_PHONRULE_ITER = 64`, configurable) and error with a
   clear diagnostic. This makes F2 strictly better than the evaluator
   on this axis; do not regress for "compatibility".

3. **Alphabet drift.** A literal in a class / rule / map not present in
   the `phoneme` inventory. Mitigation: §5.4 warning at compile time;
   option to escalate to error.

4. **`rustfst` issues #288 (determinise divergence) and the
   `optimize` panic.** Per rustfst-survey §1, §6. Mitigations: regression
   test using a hand-built FST with known determinised form; fall back
   to non-deterministic FST if needed (`FstBackend::determinize` already
   returns `Result`); never route through `rustfst::optimize` — call
   `determinize` + `minimize` explicitly (F1 already enforces).

5. **Surface mismatch on a corner case.** Validation suite must be
   thorough. Mitigation: fuzz aggressively (§6.1 item 4), especially
   on inputs near BOUNDARY and word-edge positions
   (`phonrule_eval.rs:638-678` has the most special-cases there). Run
   against Turkish (harmony + elision) and the proto.

6. **Per-rule FST blow-up.** Step-2 complement can grow large before
   minimisation. Mitigation: minimise after each composition step;
   profile on the largest phonrule (Turkish harmony) at F2 mid-point
   and abort if any single rule exceeds ~100k states.

---

## Section 10 — Non-goals

1. **No phonrule source syntax changes.** The AST is the input; no new
   operators, context elements, or map kinds in F2.
2. **No new phonrule operators exposed to users.** Internally we
   instantiate one Karttunen variant (§3.5.4); the source language stays
   `A -> B / L _ R`.
3. **`phonrule_eval` not removed.** F2 ships the FST engine *in
   addition to* the evaluator. Switchover is F5-F7 territory; the
   evaluator stays as reference, fallback, and test oracle.
4. **No two-level (Koskenniemi 1983) morphology.** Out of scope per
   fst-morphology §10.
5. **No syllable-aware contexts** (`%syl<head>%`, `%syl<tail>%`,
   `%syl<#N>%`, `%syl[...]%`). F2 emits a clear "not yet supported in
   FST engine" diagnostic and falls back to `phonrule_eval` for any
   such phonrule. Compiling syllabification to FSTs is its own
   sub-project.
6. **No weighted FSTs.** Per fst-morphology §9. All F2 FSTs use
   `TrivialWeight` / `BooleanWeight`.
7. **No `compose` chain compilation.** That's F4. F2 ends at "per-phonrule
   FST applies correctly to a single input string".
8. **No backward compatibility.** Pre-1.0 per fst-morphology §8 decision 5.

---

## Appendix A — Reading order for the implementer

1. This document.
2. `docs/proposals/fst-morphology.md` §3, §6. The why.
3. `docs/proposals/rustfst-survey.md` §3, §4, §7. What `rustfst` gives.
4. `src/fst/backend.rs` + `src/fst/tests.rs`. The trait and worked
   examples F2 consumes.
5. `src/phonrule_eval.rs`. The semantics F2 reproduces.
6. **Karttunen, L. (1995). "The Replace Operator."**
   https://aclanthology.org/P95-1003.pdf. The algorithm; 8 pages.
7. **Beesley, K. R. & Karttunen, L. (2003). *Finite State Morphology*,
   Chapter 3.** The book-length tutorial; fall back to this when the
   paper compresses.
8. **Foma source `replace.c`.** https://github.com/mhulden/foma.
   Reference implementation to read alongside (8).

End of plan.
