//! Integration tests for F8: LHS range rewrite.
//!
//! F8 lets the left-hand side of a `phonrule` rewrite rule carry quantifiers
//! (`* + ? {n} {n,m} {n,}`), the wildcard `.`, and `%syl[ ... ]%` blocks (which
//! may themselves be quantified). The whole matched range is replaced by the
//! rhs as a single unit, reusing the F6 match engine (`match_seq` /
//! `consume_atom` / `match_atom_quant`) on the LHS.
//!
//! Covers:
//!   1. `C+ -> ∅` — collapse a run of consonants in one shot.
//!   2. `C{2,} -> C` — cluster reduction.
//!   3. `(%syl[ C* V C* ]%)+ -> "" / %syl<#3>% _` — whole-syllable loss from
//!      the 3rd syllable onward (the proposal's flagship use case).
//!   4. `(%syl[...]%){2,}` — quantified syl block.
//!   5. Non-regression: single-segment v1 LHS rules behave identically.
//!   6. `{n,m}` with `n > m` is a parse error.
//!   7. The convergence loop terminates on range deletion.

use hubullu::inflection_eval::PhonRuleResolver;
use hubullu::phoneme::PhonemeInventory;
use hubullu::phonrule_eval::apply_phonrule_with_resolver;
use hubullu::{phase1, phase2};

// ---------------------------------------------------------------------------
// shared harness (mirrors integration_quantifier.rs)
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
// 1. C+ -> ∅ : delete a whole run of consonants at once
// ---------------------------------------------------------------------------

#[test]
fn consonant_run_deletion() {
    let src = r#"
        phoneme Cs { "p", "t", "k", "r", "s", "n", "m" }
        phoneme Vs { "a", "e", "i", "o", "u", "ə" }
        phonrule drop_consonants {
          class C = ["p", "t", "k", "r", "s", "n", "m"]
          C+ -> null
        }
    "#;
    // Every maximal consonant run vanishes, vowels survive.
    assert_eq!(run(src, "drop_consonants", "marəpəsən"), "aəəə");
    assert_eq!(run(src, "drop_consonants", "ptka"), "a");
    assert_eq!(run(src, "drop_consonants", "aeiou"), "aeiou");
}

// ---------------------------------------------------------------------------
// 2. C{2,} -> C : cluster reduction (range delete + single insert)
// ---------------------------------------------------------------------------

#[test]
fn cluster_reduction() {
    let src = r#"
        phoneme Cs { "p", "t", "k", "s", "r" }
        phoneme Vs { "a", "e", "i" }
        phonrule reduce_cluster {
          class C = ["p", "t", "k", "s", "r"]
          C{2,} -> "t"
        }
    "#;
    // Each maximal cluster of 2+ consonants collapses to a single "t".
    // `akstra` = a + kstr + a → the one 4-consonant cluster becomes "t".
    assert_eq!(run(src, "reduce_cluster", "akstra"), "ata");
    // Two separate clusters each collapse independently.
    assert_eq!(run(src, "reduce_cluster", "ksaprta"), "tata");
    // A lone consonant is left untouched (no infinite loop turning C into C).
    assert_eq!(run(src, "reduce_cluster", "apa"), "apa");
    assert_eq!(run(src, "reduce_cluster", "pp"), "t");
}

// ---------------------------------------------------------------------------
// 3. flagship use case: whole-syllable loss from the 3rd syllable onward
// ---------------------------------------------------------------------------

#[test]
fn late_syllable_loss() {
    // `marəpəsən` syllabifies as `ma.rə.pə.sən` (CV, CV, CV, CVC). Deleting
    // every syllable from the 3rd onward leaves `marə`.
    let src = r#"
        phoneme C { "p", "t", "k", "r", "s", "n", "m" }
        phoneme V { "a", "e", "i", "o", "u", "ə" }
        syllable proto {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule late_syllable_loss {
          syllable: proto
          class Cc = ["p", "t", "k", "r", "s", "n", "m"]
          (%syl[ Cc* V Cc* ]%)+ -> "" / %syl<#3>% _
        }
    "#;
    assert_eq!(run_syl(src, "late_syllable_loss", "proto", "marəpəsən"), "marə");
    // A two-syllable word has no 3rd syllable: untouched.
    assert_eq!(run_syl(src, "late_syllable_loss", "proto", "marə"), "marə");
    // Five syllables: still trimmed back to the first two.
    assert_eq!(
        run_syl(src, "late_syllable_loss", "proto", "marəpəsənta"),
        "marə"
    );
}

// ---------------------------------------------------------------------------
// 4. quantified syl block: (%syl[...]%){2,}
// ---------------------------------------------------------------------------

#[test]
fn quantified_syl_block() {
    // Replace any run of 2+ syllables with a single "X". `bada` = ba.da
    // (two syllables) -> "X"; a single syllable stays.
    let src = r#"
        phoneme C { "b", "d", "p", "t" }
        phoneme V { "a", "i" }
        syllable proto {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
        }
        phonrule collapse_pairs {
          syllable: proto
          class Cc = ["b", "d", "p", "t"]
          (%syl[ Cc* V Cc* ]%){2,} -> "X"
        }
    "#;
    assert_eq!(run_syl(src, "collapse_pairs", "proto", "bada"), "X");
    assert_eq!(run_syl(src, "collapse_pairs", "proto", "badati"), "X");
    // A single syllable does not reach the `{2,}` minimum.
    assert_eq!(run_syl(src, "collapse_pairs", "proto", "ba"), "ba");
}

// ---------------------------------------------------------------------------
// 5. non-regression: single-segment v1 LHS rules behave identically
// ---------------------------------------------------------------------------

#[test]
fn single_segment_lhs_unchanged() {
    // A plain `class -> literal` rule still parses to the v1 `PhonPattern::Class`
    // form and rewrites one segment at a time.
    let src = r#"
        phoneme Cs { "p", "t" }
        phoneme Vs { "a", "e", "i" }
        phonrule raise {
          class V = ["a", "e", "i"]
          V -> "i"
        }
    "#;
    assert_eq!(run(src, "raise", "pata"), "piti");

    // Literal LHS, single segment.
    let src2 = r#"
        phonrule swap {
          "a" -> "o"
        }
    "#;
    assert_eq!(run(src2, "swap", "banana"), "bonono");

    // Insertion rule (empty literal LHS) still parses to the v1 form and runs
    // — the empty literal must not be re-routed through the F8 range engine.
    let src3 = r#"
        phoneme C { "p", "t" }
        phoneme V { "a" }
        phonrule insert {
          class Cl = ["p", "t"]
          "" -> "x" / Cl _ Cl
        }
    "#;
    // `x` is inserted between any two consonants (`tt` → `txt`); the rule
    // converges because the inserted `x` is not itself a consonant.
    assert_eq!(run(src3, "insert", "atta"), "atxta");
}

// ---------------------------------------------------------------------------
// 6. {n,m} with n > m is a parse error
// ---------------------------------------------------------------------------

#[test]
fn lhs_quantifier_n_gt_m_is_error() {
    let src = r#"
        phoneme Cs { "p", "t" }
        phonrule bad {
          class C = ["p", "t"]
          C{3,2} -> null
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();
    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(
        p1.diagnostics.has_errors(),
        "expected a parse error for the '{{3,2}}' LHS quantifier"
    );
    let rendered = p1.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("lower bound") || rendered.contains("3,2"),
        "expected an n>m quantifier diagnostic, got: {}",
        rendered
    );
}

// ---------------------------------------------------------------------------
// 7. the convergence loop terminates on range deletion
// ---------------------------------------------------------------------------

#[test]
fn range_deletion_loop_terminates() {
    // `.+ -> ∅` deletes everything in a single pass; the outer convergence
    // loop must then see an unchanged (empty) string and stop. If the loop
    // did not terminate this test would hang rather than fail.
    let src = r#"
        phoneme Cs { "p", "t", "a" }
        phonrule wipe {
          .+ -> null
        }
    "#;
    assert_eq!(run(src, "wipe", "patapata"), "");

    // `C+ -> C` reproduces a lone consonant; the string stabilises instead of
    // looping forever.
    let src2 = r#"
        phoneme Cs { "p", "t" }
        phoneme Vs { "a" }
        phonrule stabilize {
          class C = ["p", "t"]
          C+ -> "p"
        }
    "#;
    assert_eq!(run(src2, "stabilize", "ttapatt"), "papap");
}
