//! Integration tests for M: phonrule `syllable:` field and the syllable-aware
//! macro context elements (`%syl<head>%` / `%syl<tail>%` / `%syl[...]%` /
//! `%syl<#N>%`).
//!
//! M replaces the F2c non-ASCII `σ[` / `]σ` / `σ#N` digraphs with the ASCII
//! macro syntax. `%syl<head>%` / `%syl<tail>%` and the `%syl[ ... ]%` content
//! block keep the old semantics; `%syl<#N>%` / `%syl<#{a..b}>%` parse into a
//! `SylIndex` AST node but their evaluation is deferred to F7.
//!
//! Covers:
//!   1. Compile-time errors when a syllable macro is used without a `syllable:`
//!      field.
//!   2. Compile-time errors when `syllable:` references an unknown name.
//!   3. Parser handling of the `%syl[ ... ]%` content block.
//!   4. Runtime behaviour: `%syl<tail>%` match (word-final coda devoicing),
//!      `%syl[ _ ]%` internal match (vowel inside a syllable), and lazy
//!      syllabification (the boundary bitset is rebuilt after each rewrite).
//!   5. `%syl<#N>%` / `%syl<#{a..b}>%` parse cleanly but are rejected at
//!      phase2 (F7 not implemented yet).
//!   6. Non-regression: phonrules without a syllable macro are unaffected.

use hubullu::ast::{Item, PhonContextElem, SylSpec};
use hubullu::inflection_eval::PhonRuleResolver;
use hubullu::phoneme::PhonemeInventory;
use hubullu::phonrule_eval::apply_phonrule_with_resolver;
use hubullu::{parse_source, phase1, phase2};

// ---------------------------------------------------------------------------
// Compile-time validation
// ---------------------------------------------------------------------------

#[test]
fn syl_tail_without_syllable_field_errors() {
    // Using `%syl<tail>%` without `syllable: NAME` must be a phase2 error.
    let src = r#"
        phoneme C { "p", "t", "k", "b", "d", "g" }
        phoneme V { "a", "i", "u" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule devoice {
          "b" -> "p" / _ %syl<tail>%
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(
        p2.diagnostics.has_errors(),
        "expected syl-macro-without-syllable error"
    );
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("%syl<tail>%") && rendered.contains("syllable:"),
        "expected diagnostic mentioning '%syl<tail>%' and 'syllable:', got: {}",
        rendered
    );
}

#[test]
fn syl_head_without_syllable_field_errors() {
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          "a" -> "e" / %syl<head>% _
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors());
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(rendered.contains("%syl<head>%"), "got: {}", rendered);
}

#[test]
fn syllable_field_unknown_name_errors() {
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        phonrule r {
          syllable: nope
          "a" -> "e"
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors());
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("undefined syllable") || rendered.contains("nope"),
        "got: {}",
        rendered
    );
}

#[test]
fn parser_accepts_syl_macro_in_context() {
    // Parses cleanly with `syllable:` set; the `%syl[ _ ]%` content block is
    // recognised.
    let src = r#"
        phoneme C { "p", "t", "k", "b", "d", "g" }
        phoneme V { "a", "i", "u" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule r {
          syllable: lang
          V -> "X" / %syl[ _ ]%
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);
    let phonrule = result
        .file
        .items
        .iter()
        .find_map(|it| match &it.node {
            Item::PhonRule(pr) => Some(pr),
            _ => None,
        })
        .expect("phonrule present");
    assert_eq!(
        phonrule.syllable.as_ref().map(|i| i.node.as_str()),
        Some("lang")
    );
    // `%syl[ _ ]%` desugars to a SylHead anchor in the left context and a
    // SylTail anchor in the right context (the `_` sits between them).
    let rule = phonrule.body.iter().find_map(|item| match item {
        hubullu::ast::PhonBodyItem::Rewrite(rw) => Some(rw),
        _ => None,
    });
    let rule = rule.expect("rewrite rule present");
    let ctx = rule.context.as_ref().expect("context present");
    assert_eq!(ctx.left, vec![PhonContextElem::SylHead]);
    assert_eq!(ctx.right, vec![PhonContextElem::SylTail]);
}

#[test]
fn parser_accepts_syl_index_specs() {
    // `%syl<#N>%`, negative indices, and `#{a..b}` ranges all parse cleanly
    // into `SylIndex(SylSpec)` nodes (evaluation is F7's job).
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          syllable: lang
          "a" -> "e" / %syl<#1>% _
          "a" -> "i" / %syl<#-1>% _
          "a" -> "o" / %syl<#{2..4}>% _
          "a" -> "u" / %syl<#{3..}>% _
          "a" -> "y" / %syl<#{..-2}>% _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);
    let phonrule = result
        .file
        .items
        .iter()
        .find_map(|it| match &it.node {
            Item::PhonRule(pr) => Some(pr),
            _ => None,
        })
        .expect("phonrule present");
    let specs: Vec<SylSpec> = phonrule
        .body
        .iter()
        .filter_map(|item| match item {
            hubullu::ast::PhonBodyItem::Rewrite(rw) => rw.context.as_ref(),
            _ => None,
        })
        .flat_map(|ctx| ctx.left.iter())
        .filter_map(|elem| match elem {
            PhonContextElem::SylIndex(spec) => Some(spec.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        specs,
        vec![
            SylSpec::Index(1),
            SylSpec::Index(-1),
            SylSpec::Range { lo: Some(2), hi: Some(4) },
            SylSpec::Range { lo: Some(3), hi: None },
            SylSpec::Range { lo: None, hi: Some(-2) },
        ]
    );
}

#[test]
fn syl_index_macro_is_rejected_at_phase2_pending_f7() {
    // `%syl<#N>%` parses, but its evaluation is deferred to F7; phase2 must
    // reject its use rather than let the rule silently no-op.
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          syllable: lang
          "a" -> "e" / %syl<#-1>% _
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors(), "expected F7-pending error");
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("%syl<#...>%") && rendered.contains("F7"),
        "expected diagnostic mentioning '%syl<#...>%' and 'F7', got: {}",
        rendered
    );
}

#[test]
fn empty_macro_is_parse_error() {
    // `%syl%` with neither a `<spec>` nor a `[...]` block is invalid.
    let src = r#"
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          syllable: lang
          "a" -> "e" / %syl% _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(result.has_errors(), "expected parse error for empty '%syl%'");
}

#[test]
fn bare_negative_spec_without_hash_is_parse_error() {
    // `<-2>` without a leading `#` is ambiguous and must be a parse error;
    // `<#-2>` is the correct form.
    let src = r#"
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          syllable: lang
          "a" -> "e" / %syl<-2>% _
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(
        result.has_errors(),
        "expected parse error for '#'-less numeric spec"
    );
}

// ---------------------------------------------------------------------------
// Runtime evaluation
// ---------------------------------------------------------------------------

/// Minimal resolver: hands out a single phonrule + phoneme inventory + a
/// single syllable declaration. Mirrors the shape used by render's HutPhonResolver.
struct OneShotResolver<'a> {
    rule: &'a hubullu::ast::PhonRule,
    inv: &'a PhonemeInventory,
    syl: Option<&'a hubullu::ast::Syllable>,
}

impl<'a> PhonRuleResolver for OneShotResolver<'a> {
    fn resolve(&self, name: &str) -> Option<&hubullu::ast::PhonRule> {
        if name == self.rule.name.node {
            Some(self.rule)
        } else {
            None
        }
    }
    fn inventory(&self) -> Option<&PhonemeInventory> {
        Some(self.inv)
    }
    fn resolve_syllable(&self, name: &str) -> Option<&hubullu::ast::Syllable> {
        self.syl
            .filter(|s| s.name.node == name)
    }
}

fn compile_one_phonrule(
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

fn pick_phonrule<'a>(p1: &'a hubullu::phase1::Phase1Result, name: &str) -> &'a hubullu::ast::PhonRule {
    p1.files
        .values()
        .flat_map(|f| f.items.iter())
        .find_map(|it| match &it.node {
            Item::PhonRule(pr) if pr.name.node == name => Some(pr),
            _ => None,
        })
        .expect("phonrule present")
}

fn pick_syllable<'a>(p1: &'a hubullu::phase1::Phase1Result, name: &str) -> &'a hubullu::ast::Syllable {
    p1.files
        .values()
        .flat_map(|f| f.items.iter())
        .find_map(|it| match &it.node {
            Item::Syllable(s) if s.name.node == name => Some(s),
            _ => None,
        })
        .expect("syllable present")
}

#[test]
fn syl_tail_matches_word_final_coda() {
    // Word-final coda devoicing: voiced obstruent → voiceless / _ %syl<tail>% $
    // Input "bad" syllabifies as one syllable [bad], so the final 'd' is at
    // a syllable-end boundary *and* word-end. It should devoice to 't'.
    // Input "bada" syllabifies as ba.da (Max-onset), so the final 'a' is at
    // syllable-end but is not a voiced obstruent — and the 'd' inside is NOT
    // at syllable-end (it's the onset of "da"). So no change.
    let src = r#"
        phoneme C { "p", "t", "k", "b", "d", "g" }
        phoneme V { "a", "i", "u" }
        phoneme voiced { "b", "d", "g" }
        phoneme voiceless { "p", "t", "k" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
          onset_priority: max
        }
        phonrule devoice {
          syllable: lang
          map devoice_map = c -> match {
            "b" -> "p",
            "d" -> "t",
            "g" -> "k",
            else -> c
          }
          voiced -> devoice_map / _ %syl<tail>% $
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "devoice");
    let syl = pick_syllable(&p1, "lang");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };

    // "bad" — single syllable, 'd' is final coda → "bat".
    let out = apply_phonrule_with_resolver("bad", rule, &resolver).unwrap();
    assert_eq!(out, "bat", "word-final coda 'd' should devoice");

    // "bada" — ba.da, 'd' is onset of "da", not at syllable-end → unchanged.
    let out = apply_phonrule_with_resolver("bada", rule, &resolver).unwrap();
    assert_eq!(out, "bada", "intervocalic 'd' must not devoice");
}

#[test]
fn syl_block_match_marks_intra_syllable_vowel() {
    // V -> "X" / %syl[ _ ]% — rewrite every vowel that is the sole nucleus
    // inside its syllable. `%syl[ _ ]%` desugars to `%syl<head>% _ %syl<tail>%`,
    // so the cursor must be both a syllable start and a syllable end — i.e. a
    // single-segment syllable.
    let src = r#"
        phoneme C { "b", "d" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule shout {
          syllable: lang
          V -> "X" / %syl[ _ ]%
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "shout");
    let syl = pick_syllable(&p1, "lang");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };
    // "ada" syllabifies as a.da: first syl is "a" (0-1), second is "da" (1-3).
    // Pos 0 = syllable-start, pos 1 = syllable-end-of-first AND
    // syllable-start-of-second. For the FROM 'a' at pos 0: cursor before = 0
    // (head ✓), after = 1 (tail ✓) → match → 'a' → "X". For the FROM 'a' at
    // pos 2 (the second 'a'): cursor before = 2, but syllable-starts are
    // {0,1}, not 2 → no match.
    let out = apply_phonrule_with_resolver("ada", rule, &resolver).unwrap();
    assert_eq!(out, "Xda");
}

#[test]
fn syl_tail_alone_marks_every_syllable_end() {
    // Rule: replace every consonant at a syllable end. "abat" → a.bat → 't'
    // is at syllable-end → becomes 'T'.
    let src = r#"
        phoneme C { "b", "t" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule mark_coda {
          syllable: lang
          "t" -> "T" / _ %syl<tail>%
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "mark_coda");
    let syl = pick_syllable(&p1, "lang");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };
    // "abat" → a.bat → 't' is the coda → "abaT"
    let out = apply_phonrule_with_resolver("abat", rule, &resolver).unwrap();
    assert_eq!(out, "abaT");
    // "ata" → a.ta → 't' is onset of "ta", not coda → unchanged.
    let out = apply_phonrule_with_resolver("ata", rule, &resolver).unwrap();
    assert_eq!(out, "ata");
}

#[test]
fn lazy_syllabification_rebuilds_after_each_rewrite() {
    // Two-step interaction: the rewrite loop iterates to convergence, so the
    // syllable bitset must be recomputed after each rewrite. Rule 1 turns 'b'
    // into 'p' at a syllable end; rule 2 turns 'p' into 'P' at a syllable end.
    //
    // "ab" — single syllable "ab", 'b' is coda → "ap" (step 1). Then 'p' is
    // still coda of single syllable "ap" → "aP" (step 2 with re-syllabified
    // string).
    let src = r#"
        phoneme C { "b", "p", "P" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule chain {
          syllable: lang
          "b" -> "p" / _ %syl<tail>%
          "p" -> "P" / _ %syl<tail>%
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "chain");
    let syl = pick_syllable(&p1, "lang");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };
    let out = apply_phonrule_with_resolver("ab", rule, &resolver).unwrap();
    assert_eq!(out, "aP", "lazy syllabification must persist through chain");
}

// ---------------------------------------------------------------------------
// Non-regression: phonrules without a syllable macro still work
// ---------------------------------------------------------------------------

#[test]
fn legacy_phonrule_unchanged_without_syllable_field() {
    let src = r#"
        phoneme C { "p", "t", "k", "b", "d", "g" }
        phoneme V { "a", "i", "u" }
        phonrule simple {
          "b" -> "p"
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "simple");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: None,
    };
    let out = apply_phonrule_with_resolver("bada", rule, &resolver).unwrap();
    assert_eq!(out, "pada");
}

#[test]
fn syllable_field_alone_does_not_change_legacy_behaviour() {
    // Phonrule has `syllable:` set but uses no syllable macro — should be
    // equivalent to a phonrule without `syllable:`. Verifies that the
    // boundary computation doesn't perturb non-macro rules.
    let src = r#"
        phoneme C { "b", "p" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule simple {
          syllable: lang
          "b" -> "p"
        }
    "#;
    let (p1, p2) = compile_one_phonrule(src);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );
    let rule = pick_phonrule(&p1, "simple");
    let syl = pick_syllable(&p1, "lang");
    let resolver = OneShotResolver {
        rule,
        inv: &p2.phonemes,
        syl: Some(syl),
    };
    let out = apply_phonrule_with_resolver("baba", rule, &resolver).unwrap();
    assert_eq!(out, "papa");
}
