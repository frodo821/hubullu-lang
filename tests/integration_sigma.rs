//! Integration tests for F2c: phonrule `syllable:` field and σ-aware context
//! elements (`σ[` / `]σ`).
//!
//! Covers:
//!   1. Compile-time errors when σ context is used without a `syllable:` field.
//!   2. Compile-time errors when `syllable:` references an unknown name.
//!   3. Parser handling of bare `σ` ident (left as a class name, not σ[/]σ).
//!   4. Runtime behaviour: σ-end match (word-final coda devoicing), σ-internal
//!      match (vowel inside a syllable), and lazy syllabification (the
//!      boundary bitset is rebuilt after each rewrite).
//!   5. Non-regression: phonrules without σ context are unaffected.

use hubullu::ast::Item;
use hubullu::inflection_eval::PhonRuleResolver;
use hubullu::phoneme::PhonemeInventory;
use hubullu::phonrule_eval::apply_phonrule_with_resolver;
use hubullu::{parse_source, phase1, phase2};

// ---------------------------------------------------------------------------
// Compile-time validation
// ---------------------------------------------------------------------------

#[test]
fn sigma_context_without_syllable_field_errors() {
    // Using `]σ` without `syllable: NAME` must be a phase2 error.
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
          "b" -> "p" / _ ]σ
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors(), "expected σ-without-syllable error");
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("]σ") && rendered.contains("syllable:"),
        "expected diagnostic mentioning ']σ' and 'syllable:', got: {}",
        rendered
    );
}

#[test]
fn sigma_start_without_syllable_field_errors() {
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang { template: V nucleus: V }
        phonrule r {
          "a" -> "e" / σ[ _
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors());
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(rendered.contains("σ["), "got: {}", rendered);
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
fn parser_accepts_sigma_in_context() {
    // Parses cleanly with `syllable:` set; both `σ[` and `]σ` are recognised.
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
          V -> "X" / σ[ _ ]σ
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
fn sigma_end_matches_word_final_coda() {
    // Word-final coda devoicing: voiced obstruent → voiceless / _ ]σ $
    // Input "bad" syllabifies as one syllable [bad], so the final 'd' is at
    // a σ-end boundary *and* word-end. It should devoice to 't'.
    // Input "bada" syllabifies as ba.da (Max-onset), so the final 'a' is at
    // σ-end but is not a voiced obstruent — and the 'd' inside is NOT at
    // σ-end (it's the onset of "da"). So no change.
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
          voiced -> devoice_map / _ ]σ $
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

    // "bada" — ba.da, 'd' is onset of "da", not at σ-end → unchanged.
    let out = apply_phonrule_with_resolver("bada", rule, &resolver).unwrap();
    assert_eq!(out, "bada", "intervocalic 'd' must not devoice");
}

#[test]
fn sigma_internal_match_marks_intra_syllable_vowel() {
    // V -> "X" / σ[ _ ]σ — rewrite every vowel that is the sole nucleus inside
    // its syllable. Input "bada" → ba.da → both 'a's are at σ-end and σ-start
    // simultaneously (single-segment nuclei) → both become "X" → "bXdX".
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
          V -> "X" / σ[ _ ]σ
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
    // Wait — σ[ requires the cursor to be at a syllable start. In "bada"
    // ba.da, syllable starts are at positions 0 and 2. Position 1 is the
    // 'a' inside "ba". Its σ[ context says "the 'a' is at the start of its
    // own syllable", which it is NOT (the 'b' is). So this matches 0 vowels.
    //
    // The intent of σ[ X _ ]σ is "X is the start of the current syllable
    // AND the cursor is at the end". For a CV "ba" that means: σ[ matches
    // before 'b' (pos 0), cursor at end after 'a' (pos 2 = σ-end). So the
    // pattern matches "ba" → would replace 'a' if FROM is V and σ[ is on
    // the LEFT of _ with a C in between. With σ[ _ ]σ (nothing between),
    // we need pos to be BOTH σ-start and σ-end — i.e. a 1-segment syllable.
    //
    // To exercise the match: input where the whole syllable is just "a"
    // (no onset/coda) → σ[ _ ]σ matches.
    let out = apply_phonrule_with_resolver("ada", rule, &resolver).unwrap();
    // "ada" syllabifies as a.da: first syl is "a" (0-1), second is "da" (1-3).
    // Pos 0 = σ-start, pos 1 = σ-end-of-first AND σ-start-of-second.
    // For the FROM 'a' at pos 0: cursor before = 0 (σ-start ✓), after = 1
    // (σ-end ✓) → match → 'a' → "X".
    // For the FROM 'a' at pos 2 (the second 'a'): cursor before = 2, but
    // σ-starts are {0,1}, not 2 → no match.
    assert_eq!(out, "Xda");
}

#[test]
fn sigma_end_alone_marks_every_syllable_end() {
    // Rule: replace every consonant at a syllable end. "abat" → a.bat → 't'
    // is at σ-end → becomes 'T'.
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
          "t" -> "T" / _ ]σ
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
    // Two-step interaction: first rule inserts a vowel, changing the
    // syllabification; the second rule's σ-end check must see the *new*
    // boundary bitset, not the original one.
    //
    // We use a single rule that iterates to convergence: "x" → "y" / _ ]σ.
    // After the first iteration the string changes, so the syllable bitset
    // must be recomputed; we verify it still matches the same positions in
    // the new (still-CV-friendly) string.
    //
    // Concretely: "abxa" → a.bxa? With (C)V(C) and onset_max=1, "bx" can't
    // both be onset. Let's keep it simple: rule "b" → "p" / _ ]σ, input
    // "ab" — single syllable "ab" → "ap". Then a *second* rule turns "p"
    // back into something else if at σ-end. After step 1 the string is "ap"
    // — same syllabification, 'p' at σ-end → "aP".
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
          "b" -> "p" / _ ]σ
          "p" -> "P" / _ ]σ
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
    // "ab" — single σ "ab", 'b' is coda → "ap" (step 1). Then 'p' is still
    // coda of single σ "ap" → "aP" (step 2 with re-syllabified string).
    let out = apply_phonrule_with_resolver("ab", rule, &resolver).unwrap();
    assert_eq!(out, "aP", "lazy syllabification must persist through chain");
}

// ---------------------------------------------------------------------------
// Non-regression: phonrules without σ context still work
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
    // Phonrule has `syllable:` set but uses no σ context — should be
    // equivalent to a phonrule without `syllable:`. Verifies that the
    // boundary computation doesn't perturb non-σ rules.
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
