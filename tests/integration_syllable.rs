//! Integration tests for F2b: top-level `syllable` declarations and the
//! greedy `syllabify` pipeline built on top of F2a's `PhonemeInventory`.

use hubullu::ast::{Item, OnsetPriority, UnknownMode};
use hubullu::syllable::syllabify;
use hubullu::{parse_source, phase1, phase2};

#[test]
fn parses_minimal_syllable_declaration() {
    let src = r#"
        phoneme C { "p", "t", "k", "s" }
        phoneme V { "a", "i", "u" }
        syllable lang {
          template: (C) V (C)
          nucleus: V
          onset_max: 1
          coda_max: 1
          onset_priority: max
          unknown: warn
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);

    let syl = result.file.items.iter().find_map(|it| match &it.node {
        Item::Syllable(s) => Some(s),
        _ => None,
    });
    let syl = syl.expect("syllable item present");
    assert_eq!(syl.name.node, "lang");
    assert_eq!(syl.nucleus.node, "V");
    assert_eq!(syl.onset_max, Some(1));
    assert_eq!(syl.coda_max, Some(1));
    assert_eq!(syl.onset_priority, OnsetPriority::Max);
    assert_eq!(syl.unknown, UnknownMode::Warn);
    assert_eq!(syl.template.slots.len(), 3);
    assert!(syl.template.slots[0].optional);
    assert!(!syl.template.slots[1].optional);
    assert!(syl.template.slots[2].optional);
}

#[test]
fn parses_unknown_overrides_block() {
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang {
          template: V
          nucleus: V
          unknown: error
          unknown_overrides: {
            " ": skip,
            "-": ignore
          }
        }
    "#;
    let result = parse_source(src, "t.hu");
    assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);
    let syl = result.file.items.iter().find_map(|it| match &it.node {
        Item::Syllable(s) => Some(s),
        _ => None,
    }).expect("syllable present");
    // Sorted alphabetically by parser for deterministic AST hashing.
    assert_eq!(syl.unknown_overrides.len(), 2);
    assert_eq!(syl.unknown_overrides[0].0, " ");
    assert_eq!(syl.unknown_overrides[0].1, UnknownMode::Skip);
    assert_eq!(syl.unknown_overrides[1].0, "-");
    assert_eq!(syl.unknown_overrides[1].1, UnknownMode::Ignore);
}

#[test]
fn phase2_validates_undefined_nucleus() {
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang {
          template: V
          nucleus: nope
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
        rendered.contains("unknown phoneme") && rendered.contains("nope"),
        "expected undefined-phoneme diagnostic, got: {}",
        rendered
    );
}

#[test]
fn phase2_validates_template_class_references() {
    // (W) is undefined.
    let src = r#"
        phoneme V { "a" }
        syllable lang {
          template: (W) V
          nucleus: V
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
        rendered.contains("W"),
        "expected template-references-W diagnostic, got: {}",
        rendered
    );
}

#[test]
fn phase2_validates_onset_max_does_not_exceed_template() {
    // template `(C) V` has 1 pre-nucleus slot; onset_max: 2 should error.
    let src = r#"
        phoneme C { "p" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V
          nucleus: V
          onset_max: 2
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
        rendered.contains("onset_max"),
        "expected onset_max diagnostic, got: {}",
        rendered
    );
}

#[test]
fn end_to_end_syllabify_cv_language() {
    let src = r#"
        phoneme C { "b", "k", "n" }
        phoneme V { "a", "i" }
        syllable lang {
          template: (C) V
          nucleus: V
          onset_max: 1
          coda_max: 0
          onset_priority: max
          unknown: warn
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

    // Find the syllable item.
    let file = p1.files.values().next().unwrap();
    let syl = file
        .items
        .iter()
        .find_map(|it| match &it.node {
            Item::Syllable(s) => Some(s),
            _ => None,
        })
        .expect("syllable defined");

    let r = syllabify("banana", syl, &p2.phonemes);
    let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
    assert_eq!(surfaces, vec!["ba", "na", "na"]);

    let r2 = syllabify("bani", syl, &p2.phonemes);
    let surfaces2: Vec<&str> = r2.syllables.iter().map(|s| s.surface.as_str()).collect();
    assert_eq!(surfaces2, vec!["ba", "ni"]);
}

#[test]
fn duplicate_syllable_name_errors() {
    let src = r#"
        phoneme V { "a" }
        syllable lang { template: V; nucleus: V }
        syllable lang { template: V; nucleus: V }
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

#[test]
fn syllabify_unknown_skip_breaks_on_space() {
    let src = r#"
        phoneme C { "b", "k" }
        phoneme V { "a" }
        syllable lang {
          template: (C) V
          nucleus: V
          unknown: skip
        }
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.hu");
    std::fs::write(&path, src).unwrap();

    let p1 = phase1::run_phase1(&path, Default::default());
    let p2 = phase2::run_phase2(&p1);
    assert!(
        !p2.diagnostics.has_errors(),
        "phase2 errors: {}",
        p2.diagnostics.render_all(&p1.source_map)
    );

    let file = p1.files.values().next().unwrap();
    let syl = file.items.iter().find_map(|it| match &it.node {
        Item::Syllable(s) => Some(s),
        _ => None,
    }).unwrap();

    let r = syllabify("ba ka", syl, &p2.phonemes);
    let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
    assert_eq!(surfaces, vec!["ba", "ka"]);
    assert!(r.events.is_empty(), "skip should be silent");
}
