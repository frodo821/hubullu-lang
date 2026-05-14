//! Phonological rule evaluation engine.
//!
//! Applies phonological rewrite rules (defined in `phonrule` blocks) to
//! morpheme-boundary–annotated strings. Morpheme boundaries are marked with
//! `\0` characters; the engine scans characters near boundaries and applies
//! context-sensitive rewrite rules.
//!
//! Phonrules can also compose other phonrules via `apply IDENT` statements in
//! their body. Composition is recursive: when evaluating a phonrule whose body
//! contains `apply Q`, the engine recurses into Q at that point in body order.
//! Cycle detection happens upstream in phase2; the runtime guards against
//! missing resolvers and (defensively) recursion depth.

use crate::ast::*;
use crate::error::Diagnostic;
use crate::inflection_eval::PhonRuleResolver;
use crate::phoneme::PhonemeInventory;
use crate::syllable::{syllabify, SyllableSpan};

/// Boundary marker character used internally between morphemes.
pub const BOUNDARY: char = '\0';

/// Cached syllable boundaries for the current input (F2c).
///
/// Built lazily by [`compute_syllable_boundaries`] when a phonrule has a
/// `syllable: NAME` field and syllable-aware context elements need to consult the
/// current syllabification. Positions are character indices into the input
/// *with* `\0` boundary markers preserved (the same coordinate system used
/// by [`check_context`]) — we restore positions from the stripped-input
/// coordinates returned by [`syllabify`] by replaying the original char
/// stream.
#[derive(Debug, Clone, Default)]
struct SyllableBoundaries {
    /// Inclusive character indices in the input where a syllable begins.
    starts: Vec<usize>,
    /// Exclusive character indices in the input where a syllable ends
    /// (i.e. one past the last character of the syllable).
    ends: Vec<usize>,
}

impl SyllableBoundaries {
    fn is_start(&self, pos: usize) -> bool {
        self.starts.binary_search(&pos).is_ok()
    }
    fn is_end(&self, pos: usize) -> bool {
        self.ends.binary_search(&pos).is_ok()
    }

    /// Number of syllables in the current syllabification.
    fn syllable_count(&self) -> usize {
        self.starts.len()
    }

    /// Reverse-lookup: which syllable (1-indexed number) does character
    /// position `pos` fall inside? Returns `None` when `pos` is not inside any
    /// syllable (e.g. a syllable-less token, or a position between syllables).
    ///
    /// `starts`/`ends` are sorted-and-deduped *pairwise* (every `starts[i]`
    /// pairs with `ends[i]`), so a linear scan is correct and `O(syllables)`.
    fn syllable_at(&self, pos: usize) -> Option<usize> {
        for (i, (&start, &end)) in self.starts.iter().zip(self.ends.iter()).enumerate() {
            if start <= pos && pos < end {
                return Some(i + 1);
            }
        }
        None
    }

    /// Evaluate a `%syl<#...>%` index spec as a zero-width anchor at `pos`.
    ///
    /// Negative bounds count from the word end (`-1` = last syllable) and are
    /// normalised to a 1-indexed number via `count + n + 1`. Ranges are
    /// inclusive on both ends; an omitted bound is open on that side. A range
    /// whose normalised bounds satisfy `lo > hi` (or any spec that resolves to
    /// a non-positive / out-of-range number) is simply never matched.
    fn matches_syl_index(&self, pos: usize, spec: &SylSpec) -> bool {
        let count = self.syllable_count();
        let Some(here) = self.syllable_at(pos) else {
            return false;
        };
        // Normalise a bound to a 1-indexed syllable number. Positive values
        // pass through; negative values count from the end.
        let normalise = |n: i64| -> i64 {
            if n < 0 {
                count as i64 + n + 1
            } else {
                n
            }
        };
        match spec {
            SylSpec::Index(n) => normalise(*n) == here as i64,
            SylSpec::Range { lo, hi } => {
                let lo = lo.map(normalise).unwrap_or(1);
                let hi = hi.map(normalise).unwrap_or(count as i64);
                let here = here as i64;
                lo <= here && here <= hi
            }
        }
    }
}

/// Per-evaluation context: the phonrule being applied plus the optional
/// global phoneme inventory used to resolve class names that don't match a
/// local `class` definition.
///
/// `syllable_boundaries` is `None` when the rule has no `syllable:` field or
/// when no inventory/resolver is available; syllable-aware context elements then never
/// match (phase2 already rejected such combinations at compile time).
#[derive(Copy, Clone)]
struct EvalCtx<'a> {
    phonrule: &'a PhonRule,
    inventory: Option<&'a PhonemeInventory>,
    syllable_boundaries: Option<&'a SyllableBoundaries>,
}

/// Apply a phonrule to an input string containing `\0` boundary markers.
///
/// This entry point does NOT resolve `apply IDENT` composition (no resolver
/// available). Body `apply` items are silently skipped — callers wanting
/// composition support must use [`apply_phonrule_with_resolver`].
pub fn apply_phonrule(input: &str, phonrule: &PhonRule) -> String {
    apply_phonrule_inner(input, phonrule, None::<&NullResolver>).unwrap_or_else(|_| input.to_string())
}

/// Apply a phonrule, resolving any `apply IDENT` body items through `resolver`.
///
/// Returns an error diagnostic if a referenced phonrule cannot be resolved.
pub fn apply_phonrule_with_resolver(
    input: &str,
    phonrule: &PhonRule,
    resolver: &dyn PhonRuleResolver,
) -> Result<String, Diagnostic> {
    apply_phonrule_inner(input, phonrule, Some(resolver))
}

/// No-op resolver type used as a zero-cost placeholder when no resolver is
/// available. Marker only — never actually called.
struct NullResolver;
impl PhonRuleResolver for NullResolver {
    fn resolve(&self, _name: &str) -> Option<&PhonRule> { None }
}

fn apply_phonrule_inner<R: PhonRuleResolver + ?Sized>(
    input: &str,
    phonrule: &PhonRule,
    resolver: Option<&R>,
) -> Result<String, Diagnostic> {
    let inventory = resolver.and_then(|r| r.inventory());
    // F2c: resolve the optional `syllable:` reference once per phonrule entry.
    // The syllable declaration itself is reused across all rewrite rules, but
    // boundary positions are recomputed (lazy syllabification) after each
    // rewrite that changes the string.
    let syllable = phonrule
        .syllable
        .as_ref()
        .and_then(|name| resolver.and_then(|r| r.resolve_syllable(&name.node)));
    let mut result = input.to_string();
    for item in &phonrule.body {
        match item {
            PhonBodyItem::Rewrite(rule) => {
                // Apply iteratively until convergence (for cascading harmony).
                // Each iteration rebuilds the syllable boundary bitset from
                // the current string (lazy syllabification, F2c).
                loop {
                    let boundaries = compute_syllable_boundaries(
                        &result, syllable, inventory,
                    );
                    let ctx = EvalCtx {
                        phonrule,
                        inventory,
                        syllable_boundaries: boundaries.as_ref(),
                    };
                    let next = apply_rewrite_rule(&result, rule, ctx);
                    if next == result {
                        break;
                    }
                    result = next;
                }
            }
            PhonBodyItem::Apply(apply) => {
                let Some(resolver) = resolver else {
                    // No resolver: skip composition silently (legacy callers).
                    continue;
                };
                let target = resolver.resolve(&apply.rule.node).ok_or_else(|| {
                    Diagnostic::error(format!(
                        "phonrule '{}' not found (referenced from 'apply' in '{}')",
                        apply.rule.node, phonrule.name.node
                    ))
                    .with_label(apply.rule.span, "not found")
                })?;
                result = apply_phonrule_inner(&result, target, Some(resolver))?;
            }
        }
    }
    Ok(result)
}

/// Compute syllable-boundary positions for `input` under `syllable`. Returns
/// `None` if either ingredient is missing — that signals "syllable-aware context elements
/// cannot match" to the caller. Phase2 prevents syllable-aware macro usage without a `syllable:`
/// field, so missing-here means a legacy non-resolver call: syllable-aware elements then
/// fail open (never match) instead of erroring at runtime.
///
/// We translate stripped-input character offsets back to *raw* offsets (the
/// coordinate system used by `check_context`, which walks the input char-by-
/// char including any `\0` boundary markers) by replaying the original char
/// stream and skipping over internal markers (`\0`, `+`) — these are exactly
/// the chars [`syllabify`] strips before tokenisation.
fn compute_syllable_boundaries(
    input: &str,
    syllable: Option<&Syllable>,
    inventory: Option<&PhonemeInventory>,
) -> Option<SyllableBoundaries> {
    let syl = syllable?;
    let inv = inventory?;
    let result = syllabify(input, syl, inv);
    if result.syllables.is_empty() {
        return Some(SyllableBoundaries::default());
    }

    // Map each stripped-input offset to a raw-input offset.
    let mut stripped_to_raw: Vec<usize> = Vec::new();
    for (raw_idx, ch) in input.chars().enumerate() {
        if ch == BOUNDARY || ch == '+' {
            continue;
        }
        stripped_to_raw.push(raw_idx);
    }
    // Sentinel for `end == stripped_len`: raw end is char count of input.
    let raw_end_sentinel = input.chars().count();
    stripped_to_raw.push(raw_end_sentinel);

    let lookup = |stripped: usize| -> usize {
        stripped_to_raw
            .get(stripped)
            .copied()
            .unwrap_or(raw_end_sentinel)
    };

    let mut starts = Vec::with_capacity(result.syllables.len());
    let mut ends = Vec::with_capacity(result.syllables.len());
    for SyllableSpan { start, end, .. } in &result.syllables {
        starts.push(lookup(*start));
        ends.push(lookup(*end));
    }
    starts.sort_unstable();
    starts.dedup();
    ends.sort_unstable();
    ends.dedup();
    Some(SyllableBoundaries { starts, ends })
}

/// Check if the FROM pattern is an empty literal (insertion rule).
fn is_insertion_rule(rule: &PhonRewriteRule) -> bool {
    matches!(&rule.from, PhonPattern::Literal(lit) if lit.node.is_empty())
}

/// Apply a single rewrite rule to the input.
/// All matches are found first, then applied simultaneously.
fn apply_rewrite_rule(input: &str, rule: &PhonRewriteRule, ctx: EvalCtx<'_>) -> String {
    if is_insertion_rule(rule) {
        apply_insertion_rule(input, rule, ctx)
    } else {
        apply_replacement_rule(input, rule, ctx)
    }
}

/// Apply an insertion rule (empty FROM pattern) to the input.
/// Scans all inter-character positions (0..=len) and checks context.
fn apply_insertion_rule(input: &str, rule: &PhonRewriteRule, ctx: EvalCtx<'_>) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut insertions: Vec<(usize, String)> = Vec::new();

    let replacement = match &rule.to {
        PhonReplacement::Literal(lit) => lit.node.clone(),
        PhonReplacement::Null => return input.to_string(),
        PhonReplacement::Map(_) => return input.to_string(),
    };

    // Try every inter-character position, including before first and after last
    for i in 0..=chars.len() {
        if let Some(rctx) = &rule.context {
            if !check_context(&chars, i, 0, rctx, ctx) {
                continue;
            }
        }
        insertions.push((i, replacement.clone()));
    }

    if insertions.is_empty() {
        return input.to_string();
    }

    // Build result with insertions
    let mut result = String::new();
    for (ci, ch) in chars.iter().enumerate() {
        // Insert before this position if needed
        if let Some((_, ins)) = insertions.iter().find(|(pos, _)| *pos == ci) {
            result.push_str(ins);
        }
        result.push(*ch);
    }
    // Insert at the very end if needed
    if let Some((_, ins)) = insertions.iter().find(|(pos, _)| *pos == chars.len()) {
        result.push_str(ins);
    }

    result
}

/// Apply a non-insertion rewrite rule (non-empty FROM pattern).
/// All matches are found first, then applied simultaneously.
fn apply_replacement_rule(input: &str, rule: &PhonRewriteRule, ctx: EvalCtx<'_>) -> String {
    // F8: an LHS range (`PhonPattern::Range`) needs the range match engine.
    if matches!(&rule.from, PhonPattern::Range(_)) {
        return apply_range_rewrite_rule(input, rule, ctx);
    }
    let chars: Vec<char> = input.chars().collect();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();

    // Find all character positions that match the FROM pattern
    for i in 0..chars.len() {
        if chars[i] == BOUNDARY {
            continue;
        }

        // Check if the character matches the FROM pattern
        let ch_str = chars[i].to_string();
        let matched = match &rule.from {
            PhonPattern::Class(class_name) => {
                char_in_class(&ch_str, &class_name.node, ctx)
            }
            PhonPattern::Literal(lit) => {
                // Multi-char literal match
                let lit_chars: Vec<char> = lit.node.chars().collect();
                if i + lit_chars.len() <= chars.len() {
                    let mut ok = true;
                    for (j, lc) in lit_chars.iter().enumerate() {
                        if chars[i + j] != *lc {
                            ok = false;
                            break;
                        }
                    }
                    ok
                } else {
                    false
                }
            }
            // F8 ranges are dispatched to `apply_range_rewrite_rule` above.
            PhonPattern::Range(_) => unreachable!("Range LHS handled separately"),
        };

        if !matched {
            continue;
        }

        // Determine match length
        let match_len = match &rule.from {
            PhonPattern::Literal(lit) => lit.node.chars().count(),
            PhonPattern::Class(_) => 1,
            PhonPattern::Range(_) => unreachable!("Range LHS handled separately"),
        };

        // Check context
        if let Some(rctx) = &rule.context {
            if !check_context(&chars, i, match_len, rctx, ctx) {
                continue;
            }
        }

        // Compute replacement
        let replacement = match &rule.to {
            PhonReplacement::Map(map_name) => {
                apply_map(&ch_str, &map_name.node, ctx.phonrule)
            }
            PhonReplacement::Literal(lit) => lit.node.clone(),
            PhonReplacement::Null => String::new(),
        };

        replacements.push((i, match_len, replacement));
    }

    // Apply replacements in reverse order to preserve indices
    if replacements.is_empty() {
        return input.to_string();
    }

    // Build result by applying all replacements (non-overlapping, simultaneous)
    // Since we want simultaneous application, use a marker approach
    let mut result = String::new();
    let mut skip_until = 0;
    for (ci, ch) in chars.iter().enumerate() {
        if ci < skip_until {
            continue;
        }
        if let Some((_, match_len, replacement)) = replacements.iter().find(|(pos, _, _)| *pos == ci) {
            result.push_str(replacement);
            skip_until = ci + match_len;
        } else {
            result.push(*ch);
        }
    }

    result
}

/// Apply an F8 LHS range rewrite rule (`PhonPattern::Range`).
///
/// At every non-boundary start position the LHS element sequence is matched
/// via [`match_lhs_range_ends`], which yields every reachable end cursor in
/// greedy-first order. The longest match whose surrounding context holds
/// (left context anchored at the match start, right context just past the
/// match end) is accepted, and the *whole matched span* is replaced by the
/// rhs as a single unit. Matches are non-overlapping (scanning resumes past
/// each accepted match) and applied simultaneously.
///
/// Convergence: each accepted match rewrites a span of ≥ 1 character
/// (zero-width matches are rejected — see below), and the outer loop in
/// [`apply_phonrule_inner`] only re-runs while the string keeps changing. A
/// rule whose rhs reproduces its own LHS (e.g. `C+ -> C` on a lone `C`)
/// leaves the string unchanged and the loop stops.
fn apply_range_rewrite_rule(input: &str, rule: &PhonRewriteRule, ctx: EvalCtx<'_>) -> String {
    let PhonPattern::Range(lhs_elems) = &rule.from else {
        return input.to_string();
    };
    let chars: Vec<char> = input.chars().collect();
    let lhs_refs: Vec<&PhonContextElem> = lhs_elems.iter().collect();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        if chars[i] == BOUNDARY {
            i += 1;
            continue;
        }
        // Every reachable match end, longest (greedy) first.
        let mut ends = Vec::new();
        match_lhs_range_ends(&chars, i, &lhs_refs, ctx, &mut ends);
        // Pick the longest match whose context holds. Zero-width matches
        // (`end <= i`) are rejected: they would neither make progress nor
        // terminate the convergence loop — a range rewrite must consume at
        // least one character.
        let accepted = ends.into_iter().find(|&end| {
            if end <= i {
                return false;
            }
            match &rule.context {
                Some(rctx) => check_context(&chars, i, end - i, rctx, ctx),
                None => true,
            }
        });
        let Some(end) = accepted else {
            i += 1;
            continue;
        };
        let match_len = end - i;
        let replacement = match &rule.to {
            PhonReplacement::Literal(lit) => lit.node.clone(),
            PhonReplacement::Null => String::new(),
            // A per-character `map` over a multi-segment range is not
            // meaningful; leave the matched span untouched (no-op).
            PhonReplacement::Map(_) => chars[i..end].iter().collect(),
        };
        replacements.push((i, match_len, replacement));
        // Non-overlapping: resume scanning past this match.
        i = end;
    }

    if replacements.is_empty() {
        return input.to_string();
    }

    let mut result = String::new();
    let mut skip_until = 0;
    for (ci, ch) in chars.iter().enumerate() {
        if ci < skip_until {
            continue;
        }
        if let Some((_, match_len, replacement)) =
            replacements.iter().find(|(pos, _, _)| *pos == ci)
        {
            result.push_str(replacement);
            skip_until = ci + match_len;
        } else {
            result.push(*ch);
        }
    }

    result
}

/// Check if a surface form (as string) belongs to a named character class.
///
/// Resolution order:
///   1. Local `class` definitions on the phonrule.
///   2. Global phoneme inventory (if a resolver provided one).
///
/// This mirrors the F2a proposal: phoneme names are usable directly in place
/// of (or alongside) phonrule-local `class` definitions.
fn char_in_class(ch: &str, class_name: &str, ctx: EvalCtx<'_>) -> bool {
    for cls in &ctx.phonrule.classes {
        if cls.name.node == class_name {
            return match &cls.body {
                CharClassBody::List(members) => {
                    members.iter().any(|m| m.node == ch)
                }
                CharClassBody::Union(refs) => {
                    refs.iter().any(|r| char_in_class(ch, &r.node, ctx))
                }
            };
        }
    }
    if let Some(inv) = ctx.inventory {
        if inv.contains(class_name, ch) {
            return true;
        }
    }
    false
}

/// Apply a named map to a character string.
fn apply_map(ch: &str, map_name: &str, phonrule: &PhonRule) -> String {
    for map_def in &phonrule.maps {
        if map_def.name.node != map_name {
            continue;
        }
        let PhonMapBody::Match { arms, else_arm } = &map_def.body;
        for arm in arms {
            if arm.from.node == ch {
                return match &arm.to {
                    PhonMapResult::Literal(lit) => lit.node.clone(),
                    PhonMapResult::Var(_) => ch.to_string(),
                };
            }
        }
        if let Some(else_arm) = else_arm {
            return match else_arm {
                PhonMapElse::Literal(lit) => lit.node.clone(),
                PhonMapElse::Var(_) => ch.to_string(),
            };
        }
        break;
    }
    ch.to_string()
}

// ===========================================================================
// F6: context match engine — greedy backtracking
// ===========================================================================
//
// `check_context` walks the left and right context element lists against the
// `\0`-boundary-marked input. v1 only had fixed-width / un-backtracking
// elements; F6 introduces quantifiers (`* + ? {n} {n,m} {n,}`) and the
// wildcard `.`, which require a proper greedy backtracking matcher.
//
// The matcher is a straightforward recursive backtracker over the element
// list. For each [`PhonContextElem::Atom(atom, quant)`] it greedily consumes
// as many occurrences of `atom` as possible (up to `quant.max()`), then
// retreats one occurrence at a time on failure of the rest of the list.
// Anchors (`^ $ %syl...%` and word boundaries) are zero-width and match in
// place. Boundary markers (`\0`) are transparent and skipped over when
// consuming an atom — the same rule v1 used.
//
// The left context is matched right-to-left (the element closest to the
// rewrite position is the rightmost one); the right context left-to-right.
// `Direction` abstracts over the two so the backtracker is shared.

/// Implementation-level cap on how far a context match may scan from the
/// rewrite position, in segments. Guards against pathological backtracking
/// and runaway `*` / `{n,}` quantifiers (proposal §4 F6: "context 長は実装上
/// 50 segment 上限など").
const CONTEXT_SCAN_LIMIT: usize = 50;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Direction {
    /// Right context: cursor advances forwards through `chars`.
    Forward,
    /// Left context: cursor moves backwards through `chars`.
    Backward,
}

/// Check if the context condition matches at position `pos` in the character
/// array. `match_len` is the width of the LHS match (always 1 in F6 — LHS
/// ranges are F8).
fn check_context(
    chars: &[char],
    pos: usize,
    match_len: usize,
    rctx: &PhonContext,
    ctx: EvalCtx<'_>,
) -> bool {
    // Left context: match right-to-left starting just left of `pos`. The
    // element list is reversed so the element closest to the rewrite site is
    // consumed first.
    let left: Vec<&PhonContextElem> = rctx.left.iter().rev().collect();
    if !match_seq(chars, pos, &left, ctx, Direction::Backward, pos) {
        return false;
    }
    // Right context: match left-to-right starting just past the LHS match.
    let right: Vec<&PhonContextElem> = rctx.right.iter().collect();
    let rstart = pos + match_len;
    if !match_seq(chars, rstart, &right, ctx, Direction::Forward, rstart) {
        return false;
    }
    true
}

/// Try to match `elems[0..]` starting at `cursor`, going in `dir`. `origin`
/// is the cursor position where this context side started (used to enforce
/// [`CONTEXT_SCAN_LIMIT`]). Returns whether a full match of the remaining
/// elements is possible (the matcher only needs a yes/no answer — context
/// matching never consumes input outside itself).
fn match_seq(
    chars: &[char],
    cursor: usize,
    elems: &[&PhonContextElem],
    ctx: EvalCtx<'_>,
    dir: Direction,
    origin: usize,
) -> bool {
    // Bail out if we have scanned too far from the rewrite site.
    let scanned = cursor.abs_diff(origin);
    if scanned > CONTEXT_SCAN_LIMIT {
        return false;
    }

    let (first, rest) = match elems.split_first() {
        Some(split) => split,
        None => return true, // all elements consumed → success
    };

    match first {
        // ---- zero-width anchors ------------------------------------------
        PhonContextElem::Boundary => {
            // `+` matches a literal boundary marker, OR the word edge.
            match dir {
                Direction::Backward => {
                    if cursor > 0 && chars[cursor - 1] == BOUNDARY {
                        match_seq(chars, cursor - 1, rest, ctx, dir, origin)
                    } else {
                        cursor == 0 && match_seq(chars, cursor, rest, ctx, dir, origin)
                    }
                }
                Direction::Forward => {
                    if cursor < chars.len() && chars[cursor] == BOUNDARY {
                        match_seq(chars, cursor + 1, rest, ctx, dir, origin)
                    } else {
                        cursor >= chars.len()
                            && match_seq(chars, cursor, rest, ctx, dir, origin)
                    }
                }
            }
        }
        PhonContextElem::WordStart => {
            cursor == 0 && match_seq(chars, cursor, rest, ctx, dir, origin)
        }
        PhonContextElem::WordEnd => {
            cursor >= chars.len() && match_seq(chars, cursor, rest, ctx, dir, origin)
        }
        PhonContextElem::SylHead => {
            let ok = matches!(ctx.syllable_boundaries, Some(b) if b.is_start(cursor));
            ok && match_seq(chars, cursor, rest, ctx, dir, origin)
        }
        PhonContextElem::SylTail => {
            let ok = matches!(ctx.syllable_boundaries, Some(b) if b.is_end(cursor));
            ok && match_seq(chars, cursor, rest, ctx, dir, origin)
        }
        // `%syl<#N>%` / `%syl<#{a..b}>%` (F7): a zero-width anchor that is true
        // when the cursor sits inside the syllable(s) named by the spec. Like
        // `^`, it consumes nothing.
        PhonContextElem::SylIndex(spec) => {
            let ok = match ctx.syllable_boundaries {
                Some(b) => b.matches_syl_index(cursor, spec),
                None => false,
            };
            ok && match_seq(chars, cursor, rest, ctx, dir, origin)
        }

        // ---- quantifiable atom -------------------------------------------
        PhonContextElem::Atom(atom, quant) => {
            match_atom_quant(chars, cursor, atom, *quant, rest, ctx, dir, origin)
        }
    }
}

/// Greedily match `atom` repeated within the bounds of `quant`, then the rest
/// of the element list. Tries the largest repetition count first and
/// backtracks down to `quant.min()`.
#[allow(clippy::too_many_arguments)]
fn match_atom_quant(
    chars: &[char],
    cursor: usize,
    atom: &PhonAtom,
    quant: Quantifier,
    rest: &[&PhonContextElem],
    ctx: EvalCtx<'_>,
    dir: Direction,
    origin: usize,
) -> bool {
    let min = quant.min() as usize;
    let max = quant.max().map(|m| m as usize);

    // Collect the cursor positions reachable by consuming 0, 1, 2, … copies
    // of `atom`, greedily, until either `max` is reached or `atom` no longer
    // matches. `stops[k]` is the cursor after consuming `k` copies.
    let mut stops = vec![cursor];
    let mut cur = cursor;
    loop {
        if let Some(m) = max {
            if stops.len() > m {
                break;
            }
        }
        // Hard guard against runaway `*` / `{n,}` on a zero-consuming atom or
        // a very long input.
        if stops.len() > CONTEXT_SCAN_LIMIT + 1 {
            break;
        }
        match consume_atom(chars, cur, atom, ctx, dir) {
            Some(next) => {
                cur = next;
                stops.push(cur);
            }
            None => break,
        }
    }

    // Not even the minimum count is reachable → fail.
    if stops.len() - 1 < min {
        return false;
    }

    // Greedy: try the largest count first, backtrack down to `min`.
    let mut k = stops.len() - 1;
    loop {
        if match_seq(chars, stops[k], rest, ctx, dir, origin) {
            return true;
        }
        if k == min {
            return false;
        }
        k -= 1;
    }
}

/// Try to consume exactly one occurrence of `atom` at `cursor` going `dir`.
/// Returns the new cursor position, or `None` if `atom` does not match.
/// Boundary markers (`\0`) are transparent and skipped over before/at the
/// segment being matched, mirroring v1 behaviour.
fn consume_atom(
    chars: &[char],
    cursor: usize,
    atom: &PhonAtom,
    ctx: EvalCtx<'_>,
    dir: Direction,
) -> Option<usize> {
    match atom {
        PhonAtom::Class(name) => {
            let (idx, next) = segment_at(chars, cursor, dir)?;
            let ch_str = chars[idx].to_string();
            if char_in_class(&ch_str, &name.node, ctx) {
                Some(next)
            } else {
                None
            }
        }
        PhonAtom::NegClass(name) => {
            let (idx, next) = segment_at(chars, cursor, dir)?;
            let ch_str = chars[idx].to_string();
            if !char_in_class(&ch_str, &name.node, ctx) {
                Some(next)
            } else {
                None
            }
        }
        // Wildcard `.` — any single (non-boundary) phoneme.
        PhonAtom::Wildcard => {
            let (_idx, next) = segment_at(chars, cursor, dir)?;
            Some(next)
        }
        PhonAtom::Literal(lit) => consume_literal(chars, cursor, &lit.node, dir),
        // Alternation: first alternative that matches wins. Each alternative
        // is a full element; we run the single-element matcher and report the
        // cursor it would leave. To keep the cursor well-defined we only
        // accept alternatives that are themselves a single consuming step;
        // this matches v1 semantics where `Alt` held simple elements.
        PhonAtom::Alt(alts) => {
            for alt in alts {
                if let Some(next) = consume_one_elem(chars, cursor, alt, ctx, dir) {
                    return Some(next);
                }
            }
            None
        }
        // `%syl[ ... ]%` block (F8): consume one whole syllable whose content
        // matches the inner element sequence. The cursor must start at a
        // syllable head, the inner sequence is matched greedily, and the
        // cursor must land exactly on a syllable tail. Only the forward
        // direction is meaningful (syl blocks appear on the LHS / right
        // context, both walked forward); a backward attempt fails closed.
        PhonAtom::SylBlock(inner) => {
            if dir != Direction::Forward {
                return None;
            }
            let b = ctx.syllable_boundaries?;
            // Skip transparent boundary markers to reach the syllable head.
            let mut start = cursor;
            while start < chars.len() && chars[start] == BOUNDARY {
                start += 1;
            }
            if !b.is_start(start) {
                return None;
            }
            let inner_refs: Vec<&PhonContextElem> = inner.iter().collect();
            // The block content must consume exactly up to a syllable tail.
            consume_seq_to_syl_tail(chars, start, &inner_refs, ctx, b)
        }
    }
}

/// Greedily match `elems` from `cursor` (forward) and return the end cursor
/// only if it lands exactly on a syllable tail — the success condition for a
/// `%syl[ ... ]%` block atom (F8). Backtracks across the element list so that
/// e.g. `C* V C*` lands the trailing `C*` on the coda/tail rather than
/// over-running.
fn consume_seq_to_syl_tail(
    chars: &[char],
    cursor: usize,
    elems: &[&PhonContextElem],
    ctx: EvalCtx<'_>,
    b: &SyllableBoundaries,
) -> Option<usize> {
    let (first, rest) = match elems.split_first() {
        None => {
            // All inner elements consumed: must be exactly at a syllable tail.
            return b.is_end(cursor).then_some(cursor);
        }
        Some(split) => split,
    };
    match first {
        PhonContextElem::Atom(atom, quant) => {
            // Enumerate the cursors reachable by consuming 0..=max copies of
            // `atom`, greedily, then try the rest from the largest first.
            let min = quant.min() as usize;
            let max = quant.max().map(|m| m as usize);
            let mut stops = vec![cursor];
            let mut cur = cursor;
            loop {
                if let Some(m) = max {
                    if stops.len() > m {
                        break;
                    }
                }
                if stops.len() > CONTEXT_SCAN_LIMIT + 1 {
                    break;
                }
                match consume_atom(chars, cur, atom, ctx, Direction::Forward) {
                    Some(next) if next > cur => {
                        cur = next;
                        stops.push(cur);
                    }
                    // A zero-width or non-advancing match would loop forever;
                    // stop enumerating (the count so far is enough).
                    _ => break,
                }
            }
            if stops.len() - 1 < min {
                return None;
            }
            let mut k = stops.len() - 1;
            loop {
                if let Some(end) = consume_seq_to_syl_tail(chars, stops[k], rest, ctx, b) {
                    return Some(end);
                }
                if k == min {
                    return None;
                }
                k -= 1;
            }
        }
        // Anchors inside a syl block are zero-width; honour them in place.
        PhonContextElem::Boundary => {
            if cursor < chars.len() && chars[cursor] == BOUNDARY {
                consume_seq_to_syl_tail(chars, cursor + 1, rest, ctx, b)
            } else {
                None
            }
        }
        PhonContextElem::WordStart => {
            (cursor == 0).then(|| consume_seq_to_syl_tail(chars, cursor, rest, ctx, b))?
        }
        PhonContextElem::WordEnd => {
            (cursor >= chars.len()).then(|| consume_seq_to_syl_tail(chars, cursor, rest, ctx, b))?
        }
        PhonContextElem::SylHead => {
            b.is_start(cursor).then(|| consume_seq_to_syl_tail(chars, cursor, rest, ctx, b))?
        }
        PhonContextElem::SylTail => {
            b.is_end(cursor).then(|| consume_seq_to_syl_tail(chars, cursor, rest, ctx, b))?
        }
        PhonContextElem::SylIndex(spec) => b
            .matches_syl_index(cursor, spec)
            .then(|| consume_seq_to_syl_tail(chars, cursor, rest, ctx, b))?,
    }
}

/// Consume one [`PhonContextElem`] (used inside an alternation). Zero-width
/// anchors leave the cursor unchanged when they hold; quantified atoms are
/// consumed at their minimum count greedily for one step. Returns the new
/// cursor or `None`.
fn consume_one_elem(
    chars: &[char],
    cursor: usize,
    elem: &PhonContextElem,
    ctx: EvalCtx<'_>,
    dir: Direction,
) -> Option<usize> {
    match elem {
        PhonContextElem::Boundary => match dir {
            Direction::Backward => {
                if cursor > 0 && chars[cursor - 1] == BOUNDARY {
                    Some(cursor - 1)
                } else if cursor == 0 {
                    Some(cursor)
                } else {
                    None
                }
            }
            Direction::Forward => {
                if cursor < chars.len() && chars[cursor] == BOUNDARY {
                    Some(cursor + 1)
                } else if cursor >= chars.len() {
                    Some(cursor)
                } else {
                    None
                }
            }
        },
        PhonContextElem::WordStart => (cursor == 0).then_some(cursor),
        PhonContextElem::WordEnd => (cursor >= chars.len()).then_some(cursor),
        PhonContextElem::SylHead => {
            matches!(ctx.syllable_boundaries, Some(b) if b.is_start(cursor)).then_some(cursor)
        }
        PhonContextElem::SylTail => {
            matches!(ctx.syllable_boundaries, Some(b) if b.is_end(cursor)).then_some(cursor)
        }
        PhonContextElem::SylIndex(_) => None,
        // A quantified atom inside an alternation: consume one occurrence
        // (the common case is an un-quantified atom). For `?`/`*` the
        // zero-count branch is left to other alternatives / the empty match.
        PhonContextElem::Atom(atom, _quant) => consume_atom(chars, cursor, atom, ctx, dir),
    }
}

/// Match the LHS element sequence `elems` starting at `start` (forward) and
/// collect *every* end cursor reachable by a full match of the sequence, in
/// **greedy-first order** (largest span first). Used by F8 LHS range rewrite:
/// unlike [`match_seq`] (yes/no for context), the rewrite engine needs the
/// match span, and — because a rule's right context is anchored just past the
/// match end — it must be able to try shorter matches when the greedy one
/// fails the context check. The caller walks the returned list longest-first,
/// keeping greedy semantics while still honouring context.
///
/// The matcher is the same greedy backtracker as [`match_atom_quant`]: each
/// element greedily consumes the largest repetition count first.
fn match_lhs_range_ends(
    chars: &[char],
    start: usize,
    elems: &[&PhonContextElem],
    ctx: EvalCtx<'_>,
    out: &mut Vec<usize>,
) {
    let (first, rest) = match elems.split_first() {
        None => {
            // Whole LHS consumed — `start` is a valid end. Greedy-first order
            // is preserved because callers descend repetition counts.
            out.push(start);
            return;
        }
        Some(split) => split,
    };
    match first {
        PhonContextElem::Atom(atom, quant) => {
            let min = quant.min() as usize;
            let max = quant.max().map(|m| m as usize);
            let mut stops = vec![start];
            let mut cur = start;
            loop {
                if let Some(m) = max {
                    if stops.len() > m {
                        break;
                    }
                }
                if stops.len() > CONTEXT_SCAN_LIMIT + 1 {
                    break;
                }
                match consume_atom(chars, cur, atom, ctx, Direction::Forward) {
                    Some(next) if next > cur => {
                        cur = next;
                        stops.push(cur);
                    }
                    // Non-advancing match (e.g. a zero-width or already-consumed
                    // position): stop enumerating to keep the loop finite.
                    _ => break,
                }
            }
            if stops.len() - 1 < min {
                return;
            }
            // Greedy: descend from the largest repetition count down to `min`.
            let mut k = stops.len() - 1;
            loop {
                match_lhs_range_ends(chars, stops[k], rest, ctx, out);
                if k == min {
                    break;
                }
                k -= 1;
            }
        }
        // Zero-width anchors on the LHS: honoured in place (rare, but the
        // grammar admits them inside `%syl[...]%` blocks / alternations).
        PhonContextElem::Boundary => {
            if start < chars.len() && chars[start] == BOUNDARY {
                match_lhs_range_ends(chars, start + 1, rest, ctx, out);
            }
        }
        PhonContextElem::WordStart => {
            if start == 0 {
                match_lhs_range_ends(chars, start, rest, ctx, out);
            }
        }
        PhonContextElem::WordEnd => {
            if start >= chars.len() {
                match_lhs_range_ends(chars, start, rest, ctx, out);
            }
        }
        PhonContextElem::SylHead => {
            if matches!(ctx.syllable_boundaries, Some(b) if b.is_start(start)) {
                match_lhs_range_ends(chars, start, rest, ctx, out);
            }
        }
        PhonContextElem::SylTail => {
            if matches!(ctx.syllable_boundaries, Some(b) if b.is_end(start)) {
                match_lhs_range_ends(chars, start, rest, ctx, out);
            }
        }
        PhonContextElem::SylIndex(spec) => {
            if matches!(ctx.syllable_boundaries, Some(b) if b.matches_syl_index(start, spec)) {
                match_lhs_range_ends(chars, start, rest, ctx, out);
            }
        }
    }
}

/// Locate the next *segment* (non-boundary char) at or beyond `cursor` going
/// `dir`, skipping transparent boundary markers. Returns `(index_of_char,
/// cursor_after_consuming_it)`.
fn segment_at(chars: &[char], cursor: usize, dir: Direction) -> Option<(usize, usize)> {
    match dir {
        Direction::Forward => {
            let mut c = cursor;
            while c < chars.len() && chars[c] == BOUNDARY {
                c += 1;
            }
            if c >= chars.len() {
                None
            } else {
                Some((c, c + 1))
            }
        }
        Direction::Backward => {
            let mut c = cursor;
            while c > 0 && chars[c - 1] == BOUNDARY {
                c -= 1;
            }
            if c == 0 {
                None
            } else {
                Some((c - 1, c - 1))
            }
        }
    }
}

/// Consume a multi-char literal at `cursor` going `dir`, skipping transparent
/// boundary markers between characters (mirrors v1 `Literal` handling).
fn consume_literal(
    chars: &[char],
    cursor: usize,
    lit: &str,
    dir: Direction,
) -> Option<usize> {
    let lit_chars: Vec<char> = lit.chars().collect();
    match dir {
        Direction::Forward => {
            let mut c = cursor;
            for lch in &lit_chars {
                while c < chars.len() && chars[c] == BOUNDARY {
                    c += 1;
                }
                if c >= chars.len() || chars[c] != *lch {
                    return None;
                }
                c += 1;
            }
            Some(c)
        }
        Direction::Backward => {
            let mut c = cursor;
            for lch in lit_chars.iter().rev() {
                while c > 0 && chars[c - 1] == BOUNDARY {
                    c -= 1;
                }
                if c == 0 || chars[c - 1] != *lch {
                    return None;
                }
                c -= 1;
            }
            Some(c)
        }
    }
}

/// Strip all boundary markers from a string.
pub fn strip_boundaries(s: &str) -> String {
    s.chars().filter(|c| *c != BOUNDARY).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    fn make_span() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn make_ident(s: &str) -> Ident {
        Spanned::new(s.to_string(), make_span())
    }

    fn make_string_lit(s: &str) -> StringLit {
        Spanned::new(s.to_string(), make_span())
    }

    /// Wrap a list of rewrite rules into a `PhonRule.body` (no `apply` items).
    fn rewrite_body(rules: Vec<PhonRewriteRule>) -> Vec<PhonBodyItem> {
        rules.into_iter().map(PhonBodyItem::Rewrite).collect()
    }

    fn make_test_harmony() -> PhonRule {
        // A simplified Turkish vowel harmony phonrule
        PhonRule {
            name: make_ident("harmony"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![
                CharClassDef {
                    name: make_ident("front"),
                    body: CharClassBody::List(vec![
                        make_string_lit("e"), make_string_lit("i"),
                        make_string_lit("ö"), make_string_lit("ü"),
                    ]),
                },
                CharClassDef {
                    name: make_ident("back"),
                    body: CharClassBody::List(vec![
                        make_string_lit("a"), make_string_lit("ı"),
                        make_string_lit("o"), make_string_lit("u"),
                    ]),
                },
                CharClassDef {
                    name: make_ident("V"),
                    body: CharClassBody::Union(vec![
                        make_ident("front"), make_ident("back"),
                    ]),
                },
            ],
            maps: vec![
                PhonMapDef {
                    name: make_ident("to_back"),
                    param: make_ident("c"),
                    body: PhonMapBody::Match {
                        arms: vec![
                            PhonMapArm { from: make_string_lit("e"), to: PhonMapResult::Literal(make_string_lit("a")) },
                            PhonMapArm { from: make_string_lit("i"), to: PhonMapResult::Literal(make_string_lit("ı")) },
                        ],
                        else_arm: Some(PhonMapElse::Var(make_ident("c"))),
                    },
                },
            ],
            body: rewrite_body(vec![
                // V -> to_back / back !back* + !back* _
                PhonRewriteRule {
                    from: PhonPattern::Class(make_ident("V")),
                    to: PhonReplacement::Map(make_ident("to_back")),
                    context: Some(PhonContext {
                        left: vec![
                            PhonContextElem::class(make_ident("back")),
                            PhonContextElem::Atom(PhonAtom::NegClass(make_ident("back")), Quantifier::Star),
                            PhonContextElem::Boundary,
                            PhonContextElem::Atom(PhonAtom::NegClass(make_ident("back")), Quantifier::Star),
                        ],
                        right: vec![],
                    }),
                    span: make_span(),
                },
            ]),
            span: make_span(),
        }
    }

    #[test]
    fn test_harmony_back_vowel() {
        let harmony = make_test_harmony();
        // "yol" + "ler" → boundary-marked: "yol\0ler"
        // The 'e' in "ler" should become 'a' (back harmony from 'o')
        let input = format!("yol{}ler", BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "yollar");
    }

    #[test]
    fn test_harmony_front_vowel_unchanged() {
        let harmony = make_test_harmony();
        // "ev" + "ler" → "ev\0ler"
        // 'e' is front, last vowel in "ev" is 'e' (front), so no change
        let input = format!("ev{}ler", BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "evler");
    }

    #[test]
    fn test_harmony_cascade() {
        let harmony = make_test_harmony();
        // "yol" + "ler" + "in" → "yol\0ler\0in"
        // First 'e' → 'a' (from 'o'), then 'i' → 'ı' (from 'a')
        let input = format!("yol{}ler{}in", BOUNDARY, BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "yolların");
    }

    #[test]
    fn test_strip_boundaries() {
        assert_eq!(strip_boundaries("yol\0lar"), "yollar");
        assert_eq!(strip_boundaries("abc"), "abc");
        assert_eq!(strip_boundaries("\0a\0b\0"), "ab");
    }

    /// Helper: build a phonrule with a single rewrite rule for word-boundary tests.
    fn make_word_boundary_rule(
        from: PhonPattern,
        to: PhonReplacement,
        context: PhonContext,
    ) -> PhonRule {
        PhonRule {
            name: make_ident("wb_test"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![
                CharClassDef {
                    name: make_ident("C"),
                    body: CharClassBody::List(vec![
                        make_string_lit("p"), make_string_lit("t"), make_string_lit("k"),
                        make_string_lit("b"), make_string_lit("d"), make_string_lit("g"),
                    ]),
                },
            ],
            maps: vec![],
            body: rewrite_body(vec![
                PhonRewriteRule {
                    from,
                    to,
                    context: Some(context),
                    span: make_span(),
                },
            ]),
            span: make_span(),
        }
    }

    #[test]
    fn test_word_start_matches_beginning() {
        // "k" -> "g" / ^ _   (voice word-initial k)
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("k")),
            PhonReplacement::Literal(make_string_lit("g")),
            PhonContext {
                left: vec![PhonContextElem::WordStart],
                right: vec![],
            },
        );
        // Word-initial k should become g
        assert_eq!(strip_boundaries(&apply_phonrule("kale", &rule)), "gale");
        // k after morpheme boundary should NOT change (^ ≠ +)
        let input = format!("a{}kale", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "akale");
    }

    #[test]
    fn test_word_end_matches_end() {
        // "b" -> "p" / _ $   (devoice word-final b)
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("b")),
            PhonReplacement::Literal(make_string_lit("p")),
            PhonContext {
                left: vec![],
                right: vec![PhonContextElem::WordEnd],
            },
        );
        // Word-final b should become p
        assert_eq!(strip_boundaries(&apply_phonrule("kitab", &rule)), "kitap");
        // b before morpheme boundary should NOT change ($ ≠ +)
        let input = format!("kitab{}a", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "kitaba");
    }

    #[test]
    fn test_word_end_with_trailing_boundary() {
        // "b" -> "p" / _ $   — input has trailing boundary marker
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("b")),
            PhonReplacement::Literal(make_string_lit("p")),
            PhonContext {
                left: vec![],
                right: vec![PhonContextElem::WordEnd],
            },
        );
        // Even with a trailing \0, the string ends after it, so b\0 → b is NOT word-final
        let input = format!("kitab{}", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "kitab");
    }

    #[test]
    fn test_boundary_still_matches_word_edges() {
        // Verify + still matches word start/end (existing behavior preserved)
        let harmony = make_test_harmony();
        // Single morpheme, no \0 — the + in context should still match at string start
        let result = apply_phonrule("yollar", &harmony);
        assert_eq!(strip_boundaries(&result), "yollar");
    }

    fn make_insertion_rule(
        to: &str,
        context: PhonContext,
        classes: Vec<CharClassDef>,
    ) -> PhonRule {
        PhonRule {
            name: make_ident("insert_test"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes,
            maps: vec![],
            body: rewrite_body(vec![
                PhonRewriteRule {
                    from: PhonPattern::Literal(make_string_lit("")),
                    to: PhonReplacement::Literal(make_string_lit(to)),
                    context: Some(context),
                    span: make_span(),
                },
            ]),
            span: make_span(),
        }
    }

    fn consonant_class() -> CharClassDef {
        CharClassDef {
            name: make_ident("C"),
            body: CharClassBody::List(vec![
                make_string_lit("p"), make_string_lit("t"), make_string_lit("k"),
                make_string_lit("b"), make_string_lit("d"), make_string_lit("g"),
                make_string_lit("l"), make_string_lit("r"), make_string_lit("n"),
            ]),
        }
    }

    #[test]
    fn test_insertion_at_morpheme_boundary() {
        // "" -> "e" / C + _ C  (epenthesis: insert 'e' between consonants across boundary)
        let rule = make_insertion_rule(
            "e",
            PhonContext {
                left: vec![PhonContextElem::class(make_ident("C")), PhonContextElem::Boundary],
                right: vec![PhonContextElem::class(make_ident("C"))],
            },
            vec![consonant_class()],
        );
        // "park\0ta" → "parke\0ta" (sic — inserted before boundary? no...)
        // Actually: position is between chars. At position of \0:
        //   left: C then + → 'k' then boundary → match
        //   right: C → 't' → match
        // But \0 itself... let's check the insertion logic.
        // Positions: p(0) a(1) r(2) k(3) \0(4) t(5) a(6)
        // At insertion point 5 (before 't'):
        //   left context: check from pos 5 backwards
        //     first elem (rightmost): Boundary → chars[4] == \0 ✓, cursor=4
        //     second elem: C → chars[3] == 'k' ✓
        //   right context: check from pos 5 forwards
        //     C → chars[5] == 't' ✓
        // → insert 'e' at position 5
        let input = format!("park{}ta", BOUNDARY);
        let result = apply_phonrule(&input, &rule);
        assert_eq!(strip_boundaries(&result), "parketa");
    }

    #[test]
    fn test_insertion_at_word_start() {
        // "" -> "e" / ^ _ C C  (prothesis: insert 'e' before initial CC cluster)
        let rule = make_insertion_rule(
            "e",
            PhonContext {
                left: vec![PhonContextElem::WordStart],
                right: vec![PhonContextElem::class(make_ident("C")), PhonContextElem::class(make_ident("C"))],
            },
            vec![consonant_class()],
        );
        // "plan" → "eplan" (insert before initial pl cluster)
        assert_eq!(strip_boundaries(&apply_phonrule("plan", &rule)), "eplan");
        // "an" → "an" (no initial CC, no insertion)
        assert_eq!(strip_boundaries(&apply_phonrule("an", &rule)), "an");
    }

    #[test]
    fn test_insertion_at_word_end() {
        // "" -> "e" / C C _ $  (paragoge: insert 'e' after final CC cluster)
        let rule = make_insertion_rule(
            "e",
            PhonContext {
                left: vec![PhonContextElem::class(make_ident("C")), PhonContextElem::class(make_ident("C"))],
                right: vec![PhonContextElem::WordEnd],
            },
            vec![consonant_class()],
        );
        // "park" → "parke" (insert after final rk cluster)
        assert_eq!(strip_boundaries(&apply_phonrule("park", &rule)), "parke");
        // "par" → "par" (only one final C, no insertion)
        assert_eq!(strip_boundaries(&apply_phonrule("par", &rule)), "par");
    }

    #[test]
    fn test_insertion_no_context_match() {
        // "" -> "x" / C _ C  (insert between consonants, no boundary required)
        let rule = make_insertion_rule(
            "x",
            PhonContext {
                left: vec![PhonContextElem::class(make_ident("C"))],
                right: vec![PhonContextElem::class(make_ident("C"))],
            },
            vec![consonant_class()],
        );
        // "apt" → "axpxt" (insert between a-p? no, 'a' is not C. between p-t: yes)
        // positions: a(0) p(1) t(2)
        // pos 0: left=nothing, right=C('a')→'a' not in C → no
        // pos 1: left=C→'a' not in C → no
        // pos 2: left=C→'p' ✓, right=C→'t' ✓ → insert 'x'
        // pos 3: left=C→'t' ✓, right=end → no C → no
        assert_eq!(strip_boundaries(&apply_phonrule("apt", &rule)), "apxt");
    }

    fn vowel_class() -> CharClassDef {
        CharClassDef {
            name: make_ident("V"),
            body: CharClassBody::List(vec![
                make_string_lit("a"), make_string_lit("e"), make_string_lit("i"),
                make_string_lit("o"), make_string_lit("u"),
            ]),
        }
    }

    #[test]
    fn test_alt_right_context() {
        // "b" -> "p" / _ (C | $)  (devoice b before consonant or word end)
        let rule = PhonRule {
            name: make_ident("devoice"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![consonant_class(), vowel_class()],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("b")),
                to: PhonReplacement::Literal(make_string_lit("p")),
                context: Some(PhonContext {
                    left: vec![],
                    right: vec![PhonContextElem::atom(PhonAtom::Alt(vec![
                        PhonContextElem::class(make_ident("C")),
                        PhonContextElem::WordEnd,
                    ]))],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // word-final b → p (matches $)
        assert_eq!(strip_boundaries(&apply_phonrule("kitab", &rule)), "kitap");
        // b before consonant → p (matches C)
        assert_eq!(strip_boundaries(&apply_phonrule("abt", &rule)), "apt");
        // b before vowel → no change
        assert_eq!(strip_boundaries(&apply_phonrule("aba", &rule)), "aba");
    }

    #[test]
    fn test_alt_left_context() {
        // "k" -> "g" / (^ | V) _  (voice k after vowel or at word start)
        let rule = PhonRule {
            name: make_ident("voice"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![consonant_class(), vowel_class()],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("k")),
                to: PhonReplacement::Literal(make_string_lit("g")),
                context: Some(PhonContext {
                    left: vec![PhonContextElem::atom(PhonAtom::Alt(vec![
                        PhonContextElem::WordStart,
                        PhonContextElem::class(make_ident("V")),
                    ]))],
                    right: vec![],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // word-initial k → g (matches ^)
        assert_eq!(strip_boundaries(&apply_phonrule("kal", &rule)), "gal");
        // k after vowel → g (matches V)
        assert_eq!(strip_boundaries(&apply_phonrule("ake", &rule)), "age");
        // k after consonant → no change
        assert_eq!(strip_boundaries(&apply_phonrule("tka", &rule)), "tka");
    }

    #[test]
    fn test_alt_with_repeat() {
        // (C | V)* — match zero or more of consonant or vowel
        // "b" -> "p" / _ (C | V)* $  (devoice b if only C/V follow until end)
        let rule = PhonRule {
            name: make_ident("devoice2"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![consonant_class(), vowel_class()],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("b")),
                to: PhonReplacement::Literal(make_string_lit("p")),
                context: Some(PhonContext {
                    left: vec![],
                    right: vec![
                        PhonContextElem::Atom(
                            PhonAtom::Alt(vec![
                                PhonContextElem::class(make_ident("C")),
                                PhonContextElem::class(make_ident("V")),
                            ]),
                            Quantifier::Star,
                        ),
                        PhonContextElem::WordEnd,
                    ],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // All following chars are C or V → devoice
        assert_eq!(strip_boundaries(&apply_phonrule("bat", &rule)), "pat");
        assert_eq!(strip_boundaries(&apply_phonrule("b", &rule)), "p");
    }

    #[test]
    fn test_right_context_crosses_boundary() {
        // Right context should see through slot boundaries.
        // Rule: "k" -> "g" / _ V  (voice k before a vowel)
        let rule = PhonRule {
            name: make_ident("voicing"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![vowel_class()],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("k")),
                to: PhonReplacement::Literal(make_string_lit("g")),
                context: Some(PhonContext {
                    left: vec![],
                    right: vec![PhonContextElem::class(make_ident("V"))],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // Without boundary: works normally
        assert_eq!(strip_boundaries(&apply_phonrule("ka", &rule)), "ga");
        // k before consonant: no change
        assert_eq!(strip_boundaries(&apply_phonrule("kt", &rule)), "kt");
        // k at boundary followed by vowel: should cross boundary
        assert_eq!(strip_boundaries(&apply_phonrule("ak\0e", &rule)), "age");
        // k at boundary followed by consonant: no change
        assert_eq!(strip_boundaries(&apply_phonrule("ak\0t", &rule)), "akt");
    }

    #[test]
    fn test_right_context_literal_crosses_boundary() {
        // Literal right context should also cross boundaries.
        // Rule: "b" -> "p" / _ "an"
        let rule = PhonRule {
            name: make_ident("lit_cross"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("b")),
                to: PhonReplacement::Literal(make_string_lit("p")),
                context: Some(PhonContext {
                    left: vec![],
                    right: vec![PhonContextElem::literal(make_string_lit("an"))],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // Without boundary
        assert_eq!(strip_boundaries(&apply_phonrule("ban", &rule)), "pan");
        // With boundary in the middle of literal match
        assert_eq!(strip_boundaries(&apply_phonrule("b\0an", &rule)), "pan");
        // No match
        assert_eq!(strip_boundaries(&apply_phonrule("b\0ox", &rule)), "box");
    }

    #[test]
    fn test_right_context_negclass_crosses_boundary() {
        // NegClass right context should cross boundaries.
        // Rule: "s" -> "z" / _ !V  (voice s before non-vowel)
        let rule = PhonRule {
            name: make_ident("neg_cross"),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![vowel_class()],
            maps: vec![],
            body: rewrite_body(vec![PhonRewriteRule {
                from: PhonPattern::Literal(make_string_lit("s")),
                to: PhonReplacement::Literal(make_string_lit("z")),
                context: Some(PhonContext {
                    left: vec![],
                    right: vec![PhonContextElem::neg_class(make_ident("V"))],
                }),
                span: make_span(),
            }]),
            span: make_span(),
        };
        // s before consonant across boundary
        assert_eq!(strip_boundaries(&apply_phonrule("s\0t", &rule)), "zt");
        // s before vowel across boundary: no change
        assert_eq!(strip_boundaries(&apply_phonrule("s\0a", &rule)), "sa");
    }

    // =====================================================================
    // F3: phonrule composition (`apply IDENT` body items) — unit tests.
    // =====================================================================

    /// Test-only resolver: holds a flat list of phonrules and looks up by name.
    struct VecResolver {
        rules: Vec<PhonRule>,
    }
    impl PhonRuleResolver for VecResolver {
        fn resolve(&self, name: &str) -> Option<&PhonRule> {
            self.rules.iter().find(|r| r.name.node == name)
        }
    }

    /// Build a phonrule with the given body items (no classes/maps).
    fn make_phonrule(name: &str, body: Vec<PhonBodyItem>) -> PhonRule {
        PhonRule {
            name: make_ident(name),
            display: vec![],
            derived_from: None,
            syllable: None,
            classes: vec![],
            maps: vec![],
            body,
            span: make_span(),
        }
    }

    /// Build a single rewrite-rule item: `from -> to` (no context).
    fn rw(from: &str, to: &str) -> PhonBodyItem {
        PhonBodyItem::Rewrite(PhonRewriteRule {
            from: PhonPattern::Literal(make_string_lit(from)),
            to: PhonReplacement::Literal(make_string_lit(to)),
            context: None,
            span: make_span(),
        })
    }

    /// Build an apply item referencing the named phonrule.
    fn ap(name: &str) -> PhonBodyItem {
        PhonBodyItem::Apply(PhonApply {
            rule: make_ident(name),
            span: make_span(),
        })
    }

    #[test]
    fn test_f3_compose_two_levels_a_applies_b() {
        // A: apply B
        // B: "x" -> "y"
        // input "fax" → "fay"
        let b = make_phonrule("B", vec![rw("x", "y")]);
        let a = make_phonrule("A", vec![ap("B")]);
        let resolver = VecResolver { rules: vec![b, a.clone()] };
        let result = apply_phonrule_with_resolver("fax", &a, &resolver).unwrap();
        assert_eq!(result, "fay");
    }

    #[test]
    fn test_f3_compose_three_level_chain() {
        // A: apply B
        // B: apply C
        // C: "a" -> "o"
        // input "fab" → A → B → C: "a" → "o" → "fob"
        let c = make_phonrule("C", vec![rw("a", "o")]);
        let b = make_phonrule("B", vec![ap("C")]);
        let a = make_phonrule("A", vec![ap("B")]);
        let resolver = VecResolver { rules: vec![c, b, a.clone()] };
        let result = apply_phonrule_with_resolver("fab", &a, &resolver).unwrap();
        assert_eq!(result, "fob");
    }

    #[test]
    fn test_f3_compose_apply_and_inline_rewrite_mixed_order() {
        // A:
        //   "x" -> "y"      (1st: x → y)
        //   apply B         (2nd: y → z)
        //   "z" -> "ZZ"     (3rd: z → ZZ)
        // B: "y" -> "z"
        // input "fax":
        //   step1 "x"→"y" → "fay"
        //   step2 B: "y"→"z" → "faz"
        //   step3 "z"→"ZZ" → "faZZ"
        let b = make_phonrule("B", vec![rw("y", "z")]);
        let a = make_phonrule(
            "A",
            vec![rw("x", "y"), ap("B"), rw("z", "ZZ")],
        );
        let resolver = VecResolver { rules: vec![b, a.clone()] };
        let result = apply_phonrule_with_resolver("fax", &a, &resolver).unwrap();
        assert_eq!(result, "faZZ");
    }

    #[test]
    fn test_f3_compose_order_matters() {
        // Inverted order: apply B first, then rewrite.
        // A:
        //   apply B          (B turns "y" → "z", but no "y" present yet)
        //   "x" -> "y"       (now "x" → "y" — but B already ran)
        // Result of A on "fax": A.apply B (no-op) → then "x"→"y" → "fay"
        let b = make_phonrule("B", vec![rw("y", "z")]);
        let a = make_phonrule("A", vec![ap("B"), rw("x", "y")]);
        let resolver = VecResolver { rules: vec![b, a.clone()] };
        let result = apply_phonrule_with_resolver("fax", &a, &resolver).unwrap();
        assert_eq!(result, "fay");
    }

    #[test]
    fn test_f3_compose_unresolved_apply_errors() {
        // A applies "Q" which does not exist → error.
        let a = make_phonrule("A", vec![ap("Q")]);
        let resolver = VecResolver { rules: vec![a.clone()] };
        let result = apply_phonrule_with_resolver("fax", &a, &resolver);
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Q") && msg.contains("not found"), "msg = {}", msg);
    }

    #[test]
    fn test_f3_legacy_apply_phonrule_skips_apply_items() {
        // The non-resolver entry point silently skips `apply` items so old
        // callers don't break. Inline rewrites still run.
        let a = make_phonrule("A", vec![ap("B"), rw("x", "y")]);
        // No resolver provided to apply_phonrule — the apply is silently a no-op,
        // and only the inline rewrite runs.
        let result = apply_phonrule("fax", &a);
        assert_eq!(result, "fay");
    }
}
