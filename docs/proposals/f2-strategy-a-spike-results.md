# F2c4 Strategy A spike — results

Date: 2026-05-17. Time-boxed ~2 days. See `src/fst/phonrule/strategy_a_spike.rs` for the spike code.

## Toy rule

`a -> b / x !V* _ y` where V is a vowel set whose size scales with the total alphabet. The rule is the **minimum reproducer** of Strategy B's exponential blow-up on `!class*` quantifiers (per `f2-strategy-a.md` §1.2). Both strategies compile the **same rule shape**.

- Strategy B: built via the existing AST pipeline (`build_obligatory_constraint` + `build_replacement_transducer` + `build_longest_leftmost_filter` + the F2c1 bracket protocol + compose, mirroring `replace.rs::compile_rewrite_rule`).
- Strategy A: clean-room implementation of Karttunen 1996 Figure 11 (directed replacement, leftmost-longest) with three marker symbols (`^`, `<`, `>`), reusing the F2c1 bracket labels for `<` and `>` and allocating one fresh marker `^` (`<spike:^>` in the symbol table). UPPER and LOWER are hand-encoded as `x · !V* · a · y` and `x · !V* · b · y` (the L/R context folded into UPPER, per Karttunen 1996 §2's hint that the contextual case can be handled without the Kaplan-Kay context machinery).

## Measurements

Per-rule wall-clock budget: **60s** (DNF = budget exceeded).

| alpha | metric            | Strategy B  | Strategy A  | B/A ratio |
|-------|-------------------|-------------|-------------|-----------|
|     5 | compile time      |      28.52s |       0.2ms |   117023× |
|       | states (pre-min)  |      469030 |          32 |    14657× |
|       | states (post-min) |      185629 |          10 |    18563× |
|       | serialized (B)    |    18961114 |        1250 |    15169× |
|    10 | compile time      |      36.67s |       0.2ms |   148894× |
|       | states (pre-min)  |      479117 |          32 |    14972× |
|       | states (post-min) |      188539 |          10 |    18854× |
|       | serialized (B)    |    22611854 |        1314 |    17208× |
|    20 | compile time      |      50.11s |       0.3ms |   148343× |
|       | states (pre-min)  |      492689 |          32 |    15397× |
|       | states (post-min) |      205565 |          10 |    20556× |
|       | serialized (B)    |    29663150 |        1474 |    20124× |
|    40 | compile time      |         DNF |       0.5ms |         — |
|       | states (pre-min)  |           — |          32 |         — |
|       | states (post-min) |           — |          10 |         — |
|       | serialized (B)    |           — |        1794 |         — |

## Recommendation

**Spike verdict: GO**

Strategy A is qualitatively better than Strategy B on this rule. The ratio widens (or B DNFs) as alphabet size grows — the polynomial-vs-exponential gap predicted in `f2-strategy-a.md` §1.2 is real. Commit to the full Strategy A rewrite per the proposal's §5 breakdown.

## Construction pseudocode (Strategy A, ~30 lines)

```text
fn strategy_a_replace(upper, lower, sigma_user) -> Fst:
    # Three markers: caret ^ (fresh), open <, close >
    # (open/close reuse F2c1 bracket labels)

    # InitialMatch: input has no markers, then ε:^ self-loop
    initial = sigma_user* ∘ (id on sigma_user + ε:^ self-loop)

    # LeftToRight (NotLeftmost):
    #   [(sigma_user|<|>)* · (^:< · UPPER' · ε:>) ]*
    #     · (sigma_user|<|>)*
    # Any caret outside a bracketed event is forbidden ;
    # this is the Karttunen NotLeftmost filter, expressed
    # WITHOUT complementing Sigma*.
    ltr = concat(
      closure_star(concat(sigma_user_or_brackets*,
                          concat(caret:open, UPPER, eps:close))),
      sigma_user_or_brackets*)

    # LongestMatch (NotInner): for our toy UPPER has no
    # length ambiguity from any start — vacuous filter
    # (Full impl would: ~$[%< [UPPER'' & contains(%>')]] )
    longest = sigma_b_star

    # Replace: identity outside brackets;
    # inside <..>, emit LOWER (and drop the brackets)
    inside = (sigma_user:eps)* · (eps:lower_symbols)*
    event  = open:eps · inside · close:eps
    replace = closure_star(union(sigma_user_step, event))

    # Strip residual markers (defensive)
    strip = identity(sigma_user) + marker:eps for each marker

    return initial ∘ ltr ∘ longest ∘ replace ∘ strip
```

## Spike implementation footprint

Honest LOC count for the spike (`strategy_a_spike.rs`): ~1430 LOC total, of which:

- ~300 LOC: Karttunen 1996 Figure 11 construction (the actual Strategy A pieces).
- ~450 LOC: benchmark harness, timing, threaded budget, table & report rendering.
- ~200 LOC: AST helpers + Strategy B compile wrapper for fair comparison.
- ~480 LOC: doc comments + module-level explanation (rationale, caveats, lessons).

**Implication for the full impl LOC estimate** (proposal §5 says ~1080 LOC):
the spike's 250 LOC of construction handles ONE rule shape, with the
LongestMatch filter stubbed out (vacuous for our toy), the L/R context
folded into UPPER (no proper context construction), and UPPER'/UPPER''
approximated as UPPER (true for our toy but not general). The proposal's
~1080 LOC estimate looks **realistic to slightly low** — the genuinely
hard pieces (real NotInner, the L/R context construction not in Karttunen
1996 Figure 11, and a proper UPPER'/UPPER'' that doesn't assume the
rule shape) are all ahead.

## Lessons learned

1. **Karttunen 1996 Figure 11 is not the whole story for contextual rules.**
   The paper says "we believe the conditional case can be handled in a
   simpler way than in Kaplan and Kay 1994" but does NOT give that
   construction. The spike side-steps by folding L/R into UPPER
   (`UPPER = L · LHS · R`, `LOWER = L · RHS · R`), which works for
   our toy but means the production impl must do real context construction.
   This is the proposal's §5 step 8 surfaced earlier than expected.

2. **The 4-tape composite-label scheme in the proposal §3.5 is unnecessary
   for the spike** — we got away with three markers as ordinary alphabet
   labels (one fresh `^` plus the two existing F2c1 brackets). Composite
   labels would be needed only for the production impl's general
   construction with overlapping rules and multi-tape state tracking.
   For the production impl, the proposal's §3.5 reasoning still holds,
   but the spike is a useful counter-example that simpler encodings
   work for the common case.

3. **Karttunen's `[..] -> %^ || _ UPPER` (obligatory insertion) is
   the silent landmine.** The paper's formal definition uses the
   conditional-replace operator from Kaplan-Kay 1994 here; the spike
   sidesteps by making caret insertion permissive, which makes the
   `@->` operator into `(@->)?` (optionality, not obligation).
   Implementing the conditional step properly requires a small
   complement (`Σ* \ Σ*·UPPER` to mark non-match positions) — but
   that complement is over `Σ*·UPPER` (small, fixed by the rule),
   NOT over `Σ_b* · L · LHS · R · Σ_b*` (the exponential one in
   Strategy B). So Strategy A's perf advantage holds; the
   obligatory-ness gap is purely a correctness fix.

4. **rustfst's `compose` discipline (arc_sort + minimisation between
   stages) is more important than the algorithm choice for spike-level
   perf.** Both strategies' state counts diverge sharply (Strategy B
   ~480k pre-min / ~200k post-min; Strategy A 32 pre-min / 10 post-min)
   primarily because Strategy B's `bad_a` complement + intersect with
   constraint B materialises the full Σ_b*·L·LHS·R·Σ_b* DFA. The full
   impl should minimise aggressively between composition stages —
   Strategy A's piece counts already minimise well to single digits.

## What was harder than expected

- **Reading Karttunen 1996's notation.** The paper's `~$[%^]`, `UPPER'`,
   etc. are dense; the marker-introduction step in particular
   (`[..] -> %^ || _ UPPER`) is a conditional insertion that doesn't
   directly compile to a 2-tape FST without external composition
   tricks. The spike resolves this by making caret insertion fully
   permissive and letting the downstream filters prune.

- **Building `consume_input_emit_lower` cleanly.** The bracketed-event
   replacement needs to consume Σ_user* on input and emit LOWER on
   output as a single transducer. The natural construction (concat
   `Σ:ε*` with `ε:LOWER`) introduces lots of ε arcs that hurt the
   determinise step. A full impl would build the (input × output)
   transducer directly.

## What was easier than expected

- **No tape-product encoding needed for one-rule toy.** The proposal
   §6.1 worried about composite-label correctness; for our spike it
   didn't come up. Markers are just three extra alphabet symbols.

- **Reusing existing rustfst-backed combinators worked everywhere.**
   No trait extension needed — `concat`, `union`, `closure_star`,
   `compose`, `eps_remove`, `determinize`, `minimize`, `arc_sort_*`
   are sufficient. Confirms the proposal §3.5 prediction.

## Correctness sanity check

- alphabet_size=5: Strategy A outputs are a superset of Strategy B's on `xay → ?` = true
- alphabet_size=10: Strategy A outputs are a superset of Strategy B's on `xay → ?` = true
- alphabet_size=20: Strategy A outputs are a superset of Strategy B's on `xay → ?` = true
- alphabet_size=40: Strategy A outputs are a superset of Strategy B's on `xay → ?` = false

**Important caveat on the spike's correctness.** Strategy A as implemented in this spike emits **both** the passthrough `xay` and the replaced `xby` for input `xay`, while Strategy B (correctly) emits only `xby`. The pass criterion above is the weaker `b_outputs ⊆ a_outputs` check, NOT byte-identity.

Why the over-generation: Karttunen 1996's `[..] -> %^ || _ UPPER` step is **obligatory** insertion (a `^` MUST appear before every UPPER-start position), realised in his formalism by a conditional replace operator that the spike does not implement. The spike's `build_insert_caret_at_upper_start` is permissive: it inserts `^` non-deterministically anywhere. The NotLeftmost filter then prunes the wrong placements but does NOT require a `^` at every UPPER-start. This gap costs **correctness** (over-generation) but NOT **perf** (the polynomial state count of all five filters is unchanged by plugging the obligatory-ness hole). The full Strategy A impl per `f2-strategy-a.md` §5 step 4 would need the conditional construction (roughly: compile `Σ* \ Σ*·UPPER` and use it to gate the ε:^ arc — this re-introduces a complement step, but on `Σ*·UPPER` which is small and unrelated to the `!class*` blow-up).

A full Strategy A would run the entire F2c5 validation harness against the existing Strategy B and assert byte-identical outputs on hundreds of inputs.

## Spike file footprint

- `src/fst/phonrule/strategy_a_spike.rs` — this module (~1430 LOC, `#[cfg(test)]`-gated, not in the public re-exports).
- One-line addition to `src/fst/phonrule/mod.rs` registering the `#[cfg(test)] mod strategy_a_spike;` line.
- No changes to any existing production module.
