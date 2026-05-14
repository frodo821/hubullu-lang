//! Integration tests for F6: context-element quantifiers (`* + ? {n} {n,m}
//! {n,}`) and the wildcard `.`.
//!
//! F6 lets phoneme classes, literals and the wildcard `.` carry a quantifier
//! and reworks `check_context` into a greedy backtracking match engine.
//! Anchors (`^` `$` `+` `%syl<head>%` `%syl<tail>%`) remain un-quantifiable.
//!
//! Covers:
//!   1. Basic quantifier matching: `C*`, `V+`, `.?`, `.{2,3}`.
//!   2. Open / closed syllable detection: `%syl[ C* V ]%` / `%syl[ C* V C+ ]%`.
//!   3. Wildcard `.` chains (`.*`).
//!   4. The proposal §4 example `voiced -> voiceless / _ %syl[ .* _ ]% $`.
//!   5. Non-regression: un-quantified v1 rules behave identically.
//!   6. Parse / phase2 errors: quantified anchors, `{n,m}` with `n > m`,
//!      `.` used as an identifier.

use hubullu::ast::{PhonAtom, PhonContextElem, Quantifier};
use hubullu::inflection_eval::PhonRuleResolver;
use hubullu::phoneme::PhonemeInventory;
use hubullu::phonrule_eval::apply_phonrule_with_resolver;
use hubullu::{parse_source, phase1, phase2};

// ---------------------------------------------------------------------------
// shared harness (mirrors integration_syl_macro.rs)
// ---------------------------------------------------------------------------

struct OneShotResolver<'a> {
    rule: &'a hubullu::ast::PhonRule,
    inv: &'a PhonemeInventory,
    syl: Option<&'a hubullu::ast::Syllable>,
}

impl<'a> PhonRuleResolver for OneShotResolver<'a> {
    fn resolve(&self, name: &str) -> Option<&hubullu::ast::PhonRule> {
        (name == self.rule.name.node).then_some(self.rule)
    }
    fn inventory(&self) -> Option<&PhonemeInventory> {
        Some(self.inv)
    }
    fn resolve_syllable(&self, name: &str) -> Option<&hubullu::ast::Syllable> {
        self.syl.filter(|s| s.name.node == name)
    }
}

fn compile(
    src: &str,
) -> (
    hubullu::phase1::Phase1Result,
    hubullu::phase2::Phase2Result,
) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();
    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    (p1, p2)
}

fn pick_phonrule<'a>(
    p1: &'a hubullu::phase1::Phase1Result,
    name: &str,
) -> &'a hubullu::ast::PhonRule {
    p1.files
        .values()
        .flat_map(|f| f.items.iter())
        .find_map(|it| match &it.node {
            hubullu::ast::Item::PhonRule(pr) if pr.name.node == name => Some(pr),
            _ => None,
        })
        .expect("phonrule present")
}

fn pick_syllable<'a>(
    p1: &'a hubullu::phase1::Phase1Result,
    name: &str,
) -> &'a hubullu::ast::Syllable {
    p1.files
        .values()
        .flat_map(|f| f.items.iter())
        .find_map(|it| match &it.node {
            hubullu::ast::Item::Syllable(s) if s.name.node == name => Some(s),
            _ => None,
        })
        .expect("syllable present")
}

/// Apply a non-syllable phonrule by name to `input`.
fn run(src: &str, rule_name: &str, input: &str) -> String {
    let (p1, p2) = compile(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, rule_name);
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: None,
    };
    apply_phonrule_with_resolver(input, rule, &resolver).unwrap()
}

/// Apply a syllable-aware phonrule by name to `input`.
fn run_syl(src: &str, rule_name: &str, syl_name: &str, input: &str) -> String {
    let (p1, p2) = compile(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, rule_name);
    let syl = pick_syllable(&p1, syl_name);
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };
    apply_phonrule_with_resolver(input, rule, &resolver).unwrap()
}

// ---------------------------------------------------------------------------
// 1. basic quantifier matching
// ---------------------------------------------------------------------------

#[test]
fn star_matches_zero_or_more() {
    // `a -> e / ^ C* _` — 'a' becomes 'e' only when preceded by zero or more
    // consonants from the word start. With `C*` (zero allowed), every
    // word-initial-onset 'a' matches: "a", "ta", "tta" all rewrite their 'a'.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / ^ C* _
        }
    "#;
    assert_eq!(run(src, "r", "a"), "e");
    assert_eq!(run(src, "r", "ta"), "te");
    assert_eq!(run(src, "r", "tta"), "tte");
    // A vowel between word-start and the 'a' breaks the `^ C*` chain.
    assert_eq!(run(src, "r", "aa"), "ea");
}

#[test]
fn plus_requires_at_least_one() {
    // `a -> e / C+ _` — needs one or more preceding consonants.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / C+ _
        }
    "#;
    assert_eq!(run(src, "r", "ta"), "te");
    assert_eq!(run(src, "r", "tta"), "tte");
    // No preceding consonant → no match.
    assert_eq!(run(src, "r", "a"), "a");
}

#[test]
fn question_matches_zero_or_one() {
    // `a -> e / ^ C? _` — at most one word-initial consonant.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / ^ C? _
        }
    "#;
    assert_eq!(run(src, "r", "a"), "e");
    assert_eq!(run(src, "r", "ta"), "te");
    // Two consonants exceed `C?`.
    assert_eq!(run(src, "r", "tta"), "tta");
}

#[test]
fn exact_count_quantifier() {
    // `a -> e / ^ C{2} _` — exactly two word-initial consonants.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / ^ C{2} _
        }
    "#;
    assert_eq!(run(src, "r", "tta"), "tte");
    assert_eq!(run(src, "r", "ta"), "ta");
    assert_eq!(run(src, "r", "ttta"), "ttta");
}

#[test]
fn range_quantifier() {
    // `a -> e / ^ C{2,3} _` — two or three word-initial consonants.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / ^ C{2,3} _
        }
    "#;
    assert_eq!(run(src, "r", "ta"), "ta");
    assert_eq!(run(src, "r", "tta"), "tte");
    assert_eq!(run(src, "r", "ttta"), "ttte");
    assert_eq!(run(src, "r", "tttta"), "tttta");
}

#[test]
fn at_least_quantifier() {
    // `a -> e / ^ C{2,} _` — two or more word-initial consonants.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / ^ C{2,} _
        }
    "#;
    assert_eq!(run(src, "r", "ta"), "ta");
    assert_eq!(run(src, "r", "tta"), "tte");
    assert_eq!(run(src, "r", "ttttta"), "ttttte");
}

#[test]
fn wildcard_optional_and_range() {
    // `.?` — any single phoneme, optional. `b -> p / ^ .? _`: 'b' devoices at
    // word start or after exactly one segment.
    let src = r#"
        phoneme V { "a" }
        phonrule opt {
          "b" -> "p" / ^ .? _
        }
    "#;
    assert_eq!(run(src, "opt", "b"), "p");
    assert_eq!(run(src, "opt", "ab"), "ap");
    assert_eq!(run(src, "opt", "aab"), "aab");

    // `.{2,3}` — between two and three arbitrary phonemes.
    let src = r#"
        phoneme V { "a" }
        phonrule rng {
          "b" -> "p" / ^ .{2,3} _
        }
    "#;
    assert_eq!(run(src, "rng", "ab"), "ab");
    assert_eq!(run(src, "rng", "aab"), "aap");
    assert_eq!(run(src, "rng", "aaab"), "aaap");
    assert_eq!(run(src, "rng", "aaaab"), "aaaab");
}

#[test]
fn wildcard_star_chain() {
    // `.*` — any run of phonemes. `b -> p / ^ .* _`: every 'b' devoices
    // regardless of what precedes it (greedy `.*` then backtracks).
    let src = r#"
        phoneme V { "a" }
        phoneme C { "t" }
        phonrule r {
          "b" -> "p" / ^ .* _
        }
    "#;
    assert_eq!(run(src, "r", "b"), "p");
    assert_eq!(run(src, "r", "atb"), "atp");
    assert_eq!(run(src, "r", "tatab"), "tatap");
}

// ---------------------------------------------------------------------------
// 2. open / closed syllable detection
// ---------------------------------------------------------------------------

#[test]
fn open_syllable_detection() {
    // `%syl[ C* _ ]%` — the cursor (the vowel being rewritten) is the
    // nucleus, preceded by an optional onset `C*` from the syllable head and
    // followed immediately by the syllable tail: an *open* syllable.
    //
    // "ta"  → single open syllable "ta"  → 'a' lengthens.
    // "tat" → single closed syllable "tat" → 'a' must NOT lengthen (a coda 't'
    //         sits between the nucleus and the syllable tail).
    // "tata"→ ta.ta → both 'a's are open nuclei → both lengthen.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "A" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule lengthen {
          syllable: lang
          "a" -> "A" / %syl[ C* _ ]%
        }
    "#;
    assert_eq!(run_syl(src, "lengthen", "lang", "ta"), "tA");
    assert_eq!(run_syl(src, "lengthen", "lang", "tat"), "tat");
    assert_eq!(run_syl(src, "lengthen", "lang", "tata"), "tAtA");
    // Onset-less open syllable: `C*` matches zero consonants.
    assert_eq!(run_syl(src, "lengthen", "lang", "a"), "A");
}

#[test]
fn closed_syllable_detection() {
    // `%syl[ C* _ C+ ]%` — the cursor (the coda consonant being rewritten) is
    // preceded by `C* V`-ish material... actually here we mark the *coda*: it
    // is preceded by the optional onset+nucleus and followed by one or more
    // consonants up to the syllable tail. Simpler framing: rewrite the vowel
    // of a *closed* syllable via `%syl[ C* _ C+ ]%` — the nucleus is followed
    // by at least one coda consonant.
    //
    // "tat" → closed → nucleus 'a' marked.
    // "ta"  → open   → no coda, no match.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "A" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule mark_closed {
          syllable: lang
          "a" -> "A" / %syl[ C* _ C+ ]%
        }
    "#;
    assert_eq!(run_syl(src, "mark_closed", "lang", "tat"), "tAt");
    assert_eq!(run_syl(src, "mark_closed", "lang", "ta"), "ta");
}

// ---------------------------------------------------------------------------
// 3. proposal §4 F6 example: any syllable-final via wildcard
// ---------------------------------------------------------------------------

#[test]
fn final_devoicing_via_wildcard_block() {
    // proposal §4 F6 (wildcard-based form): devoice a voiced obstruent at the
    // end of the final syllable, where "syllable end" is expressed with the
    // wildcard form `%syl[ .* _ ]%` rather than the `%syl<tail>%` anchor.
    //
    // `%syl[ .* _ ]%` desugars (M) to a `%syl<head>%`, a `.*` run, then the
    // rewrite position; the cursor is therefore at a syllable end reachable
    // from a syllable head over any run of phonemes. Combined with `$` this
    // pins the rewrite to the very end of the word.
    //
    // "bad"  → single syllable [bad]; the 'd' is the coda, the syllable head
    //          is reachable over ".* = ba", and `$` holds → devoices to 't'.
    // "bada" → ba.da; the final segment is 'a' (not a voiced obstruent), and
    //          the internal 'd' is the onset of "da" not at a syllable end →
    //          unchanged.
    let src = r#"
        phoneme C { "p", "t", "k", "b", "d", "g" }
        phoneme V { "a", "i", "u" }
        phoneme voiced { "b", "d", "g" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
          onset_priority: max
        }
        phonrule final_devoice {
          syllable: lang
          map dv = c -> match {
            "b" -> "p",
            "d" -> "t",
            "g" -> "k",
            else -> c
          }
          voiced -> dv / %syl[ .* _ ]% $
        }
    "#;
    assert_eq!(run_syl(src, "final_devoice", "lang", "bad"), "bat");
    assert_eq!(run_syl(src, "final_devoice", "lang", "bada"), "bada");
}

// ---------------------------------------------------------------------------
// 4. non-regression: un-quantified v1 rules unchanged
// ---------------------------------------------------------------------------

#[test]
fn unquantified_single_segment_unchanged() {
    // Plain `V _ V` context — no quantifiers. Behaviour must be identical to
    // v1: intervocalic 't' voices to 'd'.
    let src = r#"
        phoneme C { "t", "d" }
        phoneme V { "a" }
        phonrule voice {
          "t" -> "d" / V _ V
        }
    "#;
    assert_eq!(run(src, "voice", "ata"), "ada");
    assert_eq!(run(src, "voice", "ta"), "ta");
    assert_eq!(run(src, "voice", "at"), "at");
}

#[test]
fn unquantified_word_boundary_context_unchanged() {
    // `b -> p / _ $` — word-final devoicing, no quantifiers.
    let src = r#"
        phoneme V { "a" }
        phonrule devoice {
          "b" -> "p" / _ $
        }
    "#;
    assert_eq!(run(src, "devoice", "ab"), "ap");
    assert_eq!(run(src, "devoice", "ba"), "ba");
}

#[test]
fn unquantified_negclass_context_unchanged() {
    // `a -> e / _ !V` — 'a' before a non-vowel. NegClass with no quantifier.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a", "e" }
        phonrule r {
          "a" -> "e" / _ !V
        }
    "#;
    assert_eq!(run(src, "r", "at"), "et");
    assert_eq!(run(src, "r", "aa"), "aa");
}

// ---------------------------------------------------------------------------
// 5. parse / phase2 errors
// ---------------------------------------------------------------------------

#[test]
fn quantifier_on_word_start_is_error() {
    // `^*` — quantifying the word-start anchor is not allowed (§7.1).
    let src = r#"
        phoneme V { "a" }
        phonrule r {
          "a" -> "e" / ^* _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for quantified '^' anchor"
    );
}

#[test]
fn quantifier_on_dollar_is_error() {
    let src = r#"
        phoneme V { "a" }
        phonrule r {
          "a" -> "e" / _ $+
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for quantified '$' anchor"
    );
}

#[test]
fn quantifier_on_syl_tail_anchor_is_error() {
    // `%syl<tail>%*` — quantifying a macro anchor is not allowed in F6
    // (syl-block quantification is F8).
    let src = r#"
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          syllable: lang
          "a" -> "e" / _ %syl<tail>%*
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for quantified '%syl<tail>%' anchor"
    );
}

#[test]
fn range_with_lo_greater_than_hi_is_error() {
    // `C{3,2}` — lower bound exceeds upper bound.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a" }
        phonrule r {
          "a" -> "e" / C{3,2} _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for '{{3,2}}' (n > m)"
    );
}

#[test]
fn dot_as_identifier_is_parse_error() {
    // `.` is a reserved token; using it where an identifier is expected
    // (here as a phoneme class name) is a parse error.
    let src = r#"
        phoneme V { "a" }
        phonrule r {
          "a" -> "e" / . { "x" } _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for '.' used as an identifier"
    );
}

// ---------------------------------------------------------------------------
// 6. AST shape sanity check
// ---------------------------------------------------------------------------

#[test]
fn parser_builds_quantified_atoms() {
    // Verify the parser produces `Atom(_, Quantifier)` nodes with the right
    // quantifier kinds, and that an un-quantified atom uses `Exact(1)`.
    let src = r#"
        phoneme C { "t" }
        phoneme V { "a" }
        phonrule r {
          "a" -> "e" / C* C+ C? C{2} C{2,4} C{2,} . _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);
    let pr = result
        .file
        .items
        .iter()
        .find_map(|it| match &it.node {
            hubullu::ast::Item::PhonRule(pr) => Some(pr),
            _ => None,
        })
        .expect("phonrule present");
    let rule = pr
        .body
        .iter()
        .find_map(|item| match item {
            hubullu::ast::PhonBodyItem::Rewrite(rw) => Some(rw),
            _ => None,
        })
        .expect("rewrite rule present");
    let left = &rule.context.as_ref().expect("context").left;
    let quants: Vec<Quantifier> = left
        .iter()
        .filter_map(|e| match e {
            PhonContextElem::Atom(_, q) => Some(*q),
            _ => None,
        })
        .collect();
    assert_eq!(
        quants,
        vec![
            Quantifier::Star,
            Quantifier::Plus,
            Quantifier::Question,
            Quantifier::Exact(2),
            Quantifier::Range(2, 4),
            Quantifier::AtLeast(2),
            Quantifier::Exact(1), // the wildcard `.` is un-quantified
        ]
    );
    // The last atom is the wildcard.
    assert!(matches!(
        left.last(),
        Some(PhonContextElem::Atom(PhonAtom::Wildcard, _))
    ));
}
