//! Integration tests for F2a: top-level `phoneme` declarations.
//!
//! Covers parsing, phase1 symbol registration, phase2 inventory resolution
//! (including cycle detection and multigraph tokenization), and phonrule
//! evaluation against phoneme-based classes.

use hubullu::ast::{Item, PhonemeMember};
use hubullu::phoneme::longest_match_tokenize;
use hubullu::{parse_source, phase1, phase2};

#[test]
fn parses_simple_phoneme_declaration() {
    let src = r#"
        phoneme V { "a", "œ" }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);

    let items = &result.file.items;
    assert_eq!(items.len(), 1);
    match &items[0].node {
        Item::Phoneme(ph) => {
            assert_eq!(ph.name.node, "V");
            assert_eq!(ph.members.len(), 2);
            assert!(matches!(&ph.members[0], PhonemeMember::Lit(s) if s.node == "a"));
            assert!(matches!(&ph.members[1], PhonemeMember::Lit(s) if s.node == "œ"));
        }
        other => panic!("expected Phoneme, got {:?}", other),
    }
}

#[test]
fn parses_phoneme_union() {
    let src = r#"
        phoneme vowels_front { "e", "i" }
        phoneme vowels_back { "a", "o" }
        phoneme V {
          vowels_front
          vowels_back
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);
    assert_eq!(result.file.items.len(), 3);
    match &result.file.items[2].node {
        Item::Phoneme(ph) => {
            assert_eq!(ph.name.node, "V");
            assert_eq!(ph.members.len(), 2);
            assert!(matches!(&ph.members[0], PhonemeMember::Ref(i) if i.node == "vowels_front"));
            assert!(matches!(&ph.members[1], PhonemeMember::Ref(i) if i.node == "vowels_back"));
        }
        _ => panic!("expected Phoneme V"),
    }
}

#[test]
fn phase2_resolves_inventory() {
    let src = r#"
        phoneme front { "e", "i" }
        phoneme back { "a", "o" }
        phoneme V {
          front
          back
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1 errors: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2 errors: {:?}",
        p2.diagnostics
    );

    assert!(p2.phonemes.contains("V", "e"));
    assert!(p2.phonemes.contains("V", "a"));
    assert!(p2.phonemes.contains("V", "o"));
    assert!(p2.phonemes.contains("front", "e"));
    assert!(!p2.phonemes.contains("V", "z"));
}

#[test]
fn phase2_detects_phoneme_cycle() {
    let src = r#"
        phoneme A { B }
        phoneme B { A }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1 errors: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors(), "expected a cycle diagnostic");
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(rendered.contains("cycle"), "got: {}", rendered);
}

#[test]
fn phase2_detects_undefined_phoneme_reference() {
    let src = r#"
        phoneme A { nope }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(p2.diagnostics.has_errors());
    let rendered = p2.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("undefined"),
        "expected undefined-reference diagnostic, got: {}",
        rendered
    );
}

#[test]
fn longest_match_multigraph_resolution() {
    let src = r#"
        phoneme C { "n", "ng" }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(!p2.diagnostics.has_errors());

    let toks = longest_match_tokenize("nga", &p2.phonemes);
    assert_eq!(toks.len(), 2);
    assert_eq!(toks[0].surface, "ng");
    assert!(toks[0].known);
    assert_eq!(toks[1].surface, "a");
}

#[test]
fn phonrule_can_reference_phoneme() {
    // A phonrule that rewrites every member of phoneme V to "X". Validation
    // must accept the bare name "V" (no local `class V = ...`), and
    // evaluation must consult the phoneme inventory at run time.
    let src = r#"
        phoneme V { "a", "e" }
        phonrule devowel {
          V -> "X"
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(!p1.diagnostics.has_errors(), "phase1: {:?}", p1.diagnostics);
    let p2 = phase2::run_phase2(&p1);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );

    // Find the phonrule in the AST and apply it with the inventory plugged in.
    use hubullu::inflection_eval::PhonRuleResolver;
    use hubullu::phoneme::PhonemeInventory;
    use hubullu::phonrule_eval::apply_phonrule_with_resolver;

    struct InvResolver<'a> {
        inv: &'a PhonemeInventory,
    }
    impl<'a> PhonRuleResolver for InvResolver<'a> {
        fn resolve(&self, _: &str) -> Option<&hubullu::ast::PhonRule> {
            None
        }
        fn inventory(&self) -> Option<&PhonemeInventory> {
            Some(self.inv)
        }
    }

    let file = p1.files.values().next().unwrap();
    let phonrule = file
        .items
        .iter()
        .find_map(|it| match &it.node {
            Item::PhonRule(pr) => Some(pr),
            _ => None,
        })
        .expect("phonrule present");

    let resolver = InvResolver { inv: &p2.phonemes };
    let out = apply_phonrule_with_resolver("kale", phonrule, &resolver).unwrap();
    assert_eq!(out, "kXlX");
}

#[test]
fn duplicate_phoneme_name_errors() {
    let src = r#"
        phoneme A { "a" }
        phoneme A { "b" }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    assert!(p1.diagnostics.has_errors());
    let rendered = p1.diagnostics.render_all(&p1.source_map);
    assert!(
        rendered.contains("duplicate"),
        "expected duplicate diagnostic, got: {}",
        rendered
    );
}
