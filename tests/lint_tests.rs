use hubullu::lint;

/// Helper: write source to a temp file and run lint on it.
fn lint_source(source: &str) -> lint::LintResult {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.hu");
    std::fs::write(&path, source).unwrap();
    lint::run_lint(&path)
}

/// Helper: write multiple files and run lint on the entry file.
fn lint_project(files: &[(&str, &str)]) -> lint::LintResult {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    let entry = dir.path().join(files[0].0);
    lint::run_lint(&entry)
}

fn has_rule(result: &lint::LintResult, rule: &str) -> bool {
    result.lints.iter().any(|l| l.rule == rule)
}

fn count_rule(result: &lint::LintResult, rule: &str) -> usize {
    result.lints.iter().filter(|l| l.rule == rule).count()
}

fn assert_no_compile_errors(result: &lint::LintResult) {
    assert!(
        !result.compile_errors.has_errors(),
        "unexpected compile errors: {:?}",
        result.compile_errors
    );
}

/// Standard preamble: tense axis with present/past values.
const TENSE_PREAMBLE: &str = r#"
    tagaxis tense {
        role: inflectional
        display: { en: "Tense" }
    }
    @extend tv for tagaxis tense {
        present { display: { en: "Present" } }
        past    { display: { en: "Past" } }
    }
"#;

/// Preamble with tense + number axes.
const TENSE_NUMBER_PREAMBLE: &str = r#"
    tagaxis tense {
        role: inflectional
        display: { en: "Tense" }
    }
    tagaxis number {
        role: inflectional
        display: { en: "Number" }
    }
    @extend tv for tagaxis tense {
        present { display: { en: "Present" } }
        past    { display: { en: "Past" } }
    }
    @extend nv for tagaxis number {
        sg { display: { en: "Singular" } }
        pl { display: { en: "Plural" } }
    }
"#;

// =======================================================================
// single-meaning-multiple
// =======================================================================

#[test]
fn test_single_meaning_multiple() {
    let result = lint_source(r#"
        entry foo {
            headword: "foo"
            stems {}
            meanings {
                only_one { "the foo" }
            }
        }
    "#);
    assert_no_compile_errors(&result);
    assert!(has_rule(&result, "single-meaning-multiple"));
}

#[test]
fn test_multiple_meanings_no_warning() {
    let result = lint_source(r#"
        entry foo {
            headword: "foo"
            stems {}
            meanings {
                sense1 { "a foo" }
                sense2 { "the foo" }
            }
        }
    "#);
    assert!(!has_rule(&result, "single-meaning-multiple"));
}

// =======================================================================
// import-order
// =======================================================================

#[test]
fn test_import_order_violation() {
    let result = lint_project(&[
        ("main.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
            @use pos from "other.hu"
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(has_rule(&result, "import-order"));
}

#[test]
fn test_import_order_ok() {
    let result = lint_project(&[
        ("main.hu", r#"
            @use pos from "other.hu"
            tagaxis tense {
                role: inflectional
                display: { en: "Tense" }
            }
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(!has_rule(&result, "import-order"));
}

// =======================================================================
// duplicate-condition
// =======================================================================

#[test]
fn test_duplicate_condition() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [tense=present] -> `{{root}}es`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(has_rule(&result, "duplicate-condition"));
}

#[test]
fn test_no_duplicate_condition() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [tense=past] -> `{{root}}ed`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "duplicate-condition"));
}

// =======================================================================
// glob-import
// =======================================================================

#[test]
fn test_glob_import_warning() {
    let result = lint_project(&[
        ("main.hu", r#"
            @use * from "other.hu"
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(has_rule(&result, "glob-import"));
}

#[test]
fn test_glob_import_with_alias_no_warning() {
    let result = lint_project(&[
        ("main.hu", r#"
            @use * as lib from "other.hu"
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(!has_rule(&result, "glob-import"));
}

// =======================================================================
// unused-import
// =======================================================================

#[test]
fn test_unused_import() {
    let result = lint_project(&[
        ("main.hu", r#"
            @use pos from "other.hu"
            entry foo {
                headword: "foo"
                stems {}
                meaning: "a foo"
            }
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(has_rule(&result, "unused-import"));
}

#[test]
fn test_used_import_no_warning() {
    let result = lint_project(&[
        ("main.hu", r#"
            @use pos from "other.hu"
            @extend pos_vals for tagaxis pos {
                verb { display: { en: "Verb" } }
            }
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(!has_rule(&result, "unused-import"));
}

// =======================================================================
// unused-tagaxis
// =======================================================================

#[test]
fn test_unused_tagaxis() {
    let result = lint_source(r#"
        tagaxis tense {
            role: inflectional
            display: { en: "Tense" }
        }
        tagaxis mood {
            role: inflectional
            display: { en: "Mood" }
        }
        @extend tv for tagaxis tense {
            present { display: { en: "Present" } }
        }
    "#);
    assert!(has_rule(&result, "unused-tagaxis"));
    let lint = result.lints.iter().find(|l| l.rule == "unused-tagaxis").unwrap();
    assert!(lint.diagnostic.message.contains("mood"));
}

// =======================================================================
// unused-inflection
// =======================================================================

#[test]
fn test_unused_inflection() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(has_rule(&result, "unused-inflection"));
}

#[test]
fn test_used_inflection_no_warning() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "unused-inflection"));
}

// =======================================================================
// unused-extend-value
// =======================================================================

#[test]
fn test_unused_extend_value() {
    let result = lint_source(r#"
        tagaxis tense {
            role: inflectional
            display: { en: "Tense" }
        }
        @extend tv for tagaxis tense {
            present { display: { en: "Present" } }
            past    { display: { en: "Past" } }
            future  { display: { en: "Future" } }
        }
        inflection verb for {tense} {
            requires stems: root
            [tense=present] -> `{root}s`
            [tense=past] -> `{root}ed`
            [_] -> `{root}`
        }
        entry go {
            headword: "go"
            stems { root: "go" }
            inflection_class: verb
            meaning: "to go"
        }
    "#);
    assert_no_compile_errors(&result);
    // "future" is defined but never appears in any rule condition or entry tag
    assert!(has_rule(&result, "unused-extend-value"));
    let lint = result.lints.iter().find(|l| l.rule == "unused-extend-value").unwrap();
    assert!(lint.diagnostic.message.contains("future"));
}

#[test]
fn test_all_extend_values_used() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [tense=past] -> `{{root}}ed`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "unused-extend-value"));
}

// =======================================================================
// unused-stem
// =======================================================================

#[test]
fn test_unused_stem() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go", past: "went" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(has_rule(&result, "unused-stem"));
    let lint = result.lints.iter().find(|l| l.rule == "unused-stem").unwrap();
    assert!(lint.diagnostic.message.contains("past"));
}

#[test]
fn test_all_stems_used() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root, past
            [tense=present] -> `{{root}}s`
            [tense=past] -> `{{past}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go", past: "went" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "unused-stem"));
}

// =======================================================================
// shadowed-rule
// =======================================================================

#[test]
fn test_shadowed_rule() {
    let src = format!(r#"
        {}
        inflection verb for {{tense, number}} {{
            requires stems: root
            [tense=present, number=sg] -> `{{root}}s`
            [tense=present, number=pl] -> `{{root}}`
            [tense=past, number=sg] -> `{{root}}ed`
            [tense=past, number=pl] -> `{{root}}ed`
            [tense=present] -> `{{root}}!`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_NUMBER_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    // [tense=present] is shadowed: both present+sg and present+pl have higher-specificity matches
    assert!(has_rule(&result, "shadowed-rule"));
}

#[test]
fn test_no_shadowed_rule() {
    let src = format!(r#"
        {}
        inflection verb for {{tense, number}} {{
            requires stems: root
            [tense=present, number=sg] -> `{{root}}s`
            [tense=present] -> `{{root}}`
            [_] -> `{{root}}ed`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_NUMBER_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    // [tense=present] is NOT shadowed: present+pl still falls through to it
    assert!(!has_rule(&result, "shadowed-rule"));
}

// =======================================================================
// incomplete-coverage
// =======================================================================

#[test]
fn test_incomplete_coverage() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    // tense=past is not covered
    assert!(has_rule(&result, "incomplete-coverage"));
    let lint = result.lints.iter().find(|l| l.rule == "incomplete-coverage").unwrap();
    assert!(lint.diagnostic.message.contains("past"));
}

#[test]
fn test_complete_coverage_with_wildcard() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "incomplete-coverage"));
}

#[test]
fn test_complete_coverage_explicit() {
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [tense=past] -> `{{root}}ed`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meaning: "to go"
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert!(!has_rule(&result, "incomplete-coverage"));
}

// =======================================================================
// clean file
// =======================================================================

#[test]
fn test_clean_file_no_lints() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a foo\"\n}\n");
    assert!(!result.has_lints(), "expected no lints, got: {:?}",
        result.lints.iter().map(|l| l.rule).collect::<Vec<_>>());
}

// =======================================================================
// --fix tests
// =======================================================================

fn has_fix(result: &lint::LintResult, rule: &str) -> bool {
    result.lints.iter().any(|l| l.rule == rule && l.fix.is_some())
}

fn fix_count(result: &lint::LintResult) -> usize {
    result.lints.iter().filter(|l| l.fix.is_some()).count()
}

/// Run lint with --fix on a temp file, return the fixed source.
fn lint_and_fix(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.hu");
    std::fs::write(&path, source).unwrap();
    let result = lint::run_lint(&path);
    assert!(!result.compile_errors.has_errors(), "unexpected compile errors");
    let n = lint::apply_fixes(&result.lints, &result.source_map).unwrap();
    assert!(n > 0, "expected at least one fix to be applied");
    std::fs::read_to_string(&path).unwrap()
}

/// Run lint with --fix on a multi-file project, return the fixed source of the entry file.
fn lint_and_fix_project(files: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    let entry = dir.path().join(files[0].0);
    let result = lint::run_lint(&entry);
    assert!(!result.compile_errors.has_errors(), "unexpected compile errors");
    let n = lint::apply_fixes(&result.lints, &result.source_map).unwrap();
    assert!(n > 0, "expected at least one fix to be applied");
    std::fs::read_to_string(&entry).unwrap()
}

#[test]
fn test_fix_unused_import() {
    let fixed = lint_and_fix_project(&[
        ("main.hu", r#"
            @use pos from "other.hu"
            entry foo {
                headword: "foo"
                stems {}
                meaning: "a foo"
            }
        "#),
        ("other.hu", r#"
            tagaxis pos {
                role: classificatory
                display: { en: "POS" }
            }
        "#),
    ]);
    assert!(!fixed.contains("@use"), "unused @use should have been removed");
    assert!(fixed.contains("entry foo"), "entry should remain");
}

// =======================================================================
// trailing-whitespace
// =======================================================================

#[test]
fn test_trailing_whitespace() {
    let result = lint_source("entry foo {  \n    stems {}\n}\n");
    assert!(has_rule(&result, "trailing-whitespace"));
    assert!(has_fix(&result, "trailing-whitespace"));
}

#[test]
fn test_fix_trailing_whitespace() {
    let fixed = lint_and_fix("entry foo {  \n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}\n");
    assert_eq!(fixed, "entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}\n");
}

#[test]
fn test_no_trailing_whitespace() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}\n");
    assert!(!has_rule(&result, "trailing-whitespace"));
}

// =======================================================================
// consecutive-blank-lines
// =======================================================================

#[test]
fn test_consecutive_blank_lines() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n\n\n\n    stems {}\n    meaning: \"a\"\n}\n");
    assert!(has_rule(&result, "consecutive-blank-lines"));
    assert!(has_fix(&result, "consecutive-blank-lines"));
}

#[test]
fn test_fix_consecutive_blank_lines() {
    let fixed = lint_and_fix("entry foo {\n    headword: \"foo\"\n\n\n\n    stems {}\n    meaning: \"a\"\n}\n");
    assert_eq!(fixed, "entry foo {\n    headword: \"foo\"\n\n    stems {}\n    meaning: \"a\"\n}\n");
}

#[test]
fn test_single_blank_line_ok() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n\n    stems {}\n    meaning: \"a\"\n}\n");
    assert!(!has_rule(&result, "consecutive-blank-lines"));
}

// =======================================================================
// trailing-newline
// =======================================================================

#[test]
fn test_missing_trailing_newline() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}");
    assert!(has_rule(&result, "trailing-newline"));
    assert!(has_fix(&result, "trailing-newline"));
}

#[test]
fn test_fix_missing_trailing_newline() {
    let fixed = lint_and_fix("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}");
    assert!(fixed.ends_with("}\n"));
}

#[test]
fn test_multiple_trailing_newlines() {
    let result = lint_source("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}\n\n\n");
    assert!(has_rule(&result, "trailing-newline"));
}

#[test]
fn test_fix_multiple_trailing_newlines() {
    let fixed = lint_and_fix("entry foo {\n    headword: \"foo\"\n    stems {}\n    meaning: \"a\"\n}\n\n\n");
    assert_eq!(&fixed[fixed.len()-2..], "}\n");
}

// =======================================================================
// import-order --fix
// =======================================================================

#[test]
fn test_fix_import_order() {
    let fixed = lint_and_fix_project(&[
        ("main.hu", "tagaxis pos {\n    role: classificatory\n    display: { en: \"POS\" }\n}\n@use tense from \"other.hu\"\n@extend tv for tagaxis tense {\n    present { display: { en: \"Present\" } }\n}\n"),
        ("other.hu", "tagaxis tense {\n    role: inflectional\n    display: { en: \"Tense\" }\n}\n"),
    ]);
    // @use should now appear before tagaxis pos
    let use_pos = fixed.find("@use").unwrap();
    let tagaxis_pos = fixed.find("tagaxis pos").unwrap();
    assert!(use_pos < tagaxis_pos, "import should be moved before tagaxis: {}", fixed);
}

// =======================================================================
// fix priority: unused-import wins over import-order
// =======================================================================

#[test]
fn test_fix_priority_unused_import_over_import_order() {
    // An import that is both unused AND out of order.
    // unused-import (priority 0) should win: the line is deleted, not moved.
    let fixed = lint_and_fix_project(&[
        ("main.hu", "tagaxis pos {\n    role: classificatory\n    display: { en: \"POS\" }\n}\n@use tense from \"other.hu\"\n"),
        ("other.hu", "tagaxis tense {\n    role: inflectional\n    display: { en: \"Tense\" }\n}\n"),
    ]);
    assert!(!fixed.contains("@use"), "unused import should be deleted, not moved");
    assert!(fixed.contains("tagaxis pos"), "other items should remain");
}

// =======================================================================
// @suppress next-line
// =======================================================================

#[test]
fn test_suppress_single_rule() {
    let src = format!(r#"
        {}
        # @suppress next-line: unused-inflection
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(!has_rule(&result, "unused-inflection"));
}

#[test]
fn test_suppress_does_not_affect_other_rules() {
    // Suppress unused-inflection on the entry line — should not affect the inflection lint
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        # @suppress next-line: unused-inflection
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meanings {{
                only_one {{ "to go" }}
            }}
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    // unused-inflection is NOT suppressed because the comment is before
    // the entry, not the inflection definition.
    assert!(!has_rule(&result, "unused-inflection"));
    // single-meaning-multiple should still fire (not suppressed)
    assert!(has_rule(&result, "single-meaning-multiple"));
}

#[test]
fn test_suppress_multiple_rules() {
    let src = format!(r#"
        {}
        # @suppress next-line: unused-inflection
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meanings {{
                # @suppress next-line: single-meaning-multiple
                only_one {{ "to go" }}
            }}
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(!has_rule(&result, "unused-inflection"));
    assert!(!has_rule(&result, "single-meaning-multiple"));
}

#[test]
fn test_suppress_with_double_hash() {
    let src = format!(r#"
        {}
        ## @suppress next-line: unused-inflection
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(!has_rule(&result, "unused-inflection"));
}

#[test]
fn test_suppress_wrong_line_no_effect() {
    // Suppress comment two lines above (blank line between) — should not take effect
    let src = format!(r#"
        {}
        # @suppress next-line: unused-inflection

        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(has_rule(&result, "unused-inflection"));
}

#[test]
fn test_suppress_entire_file() {
    let src = format!(r#"
        # @suppress entire-file: unused-inflection
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(!has_rule(&result, "unused-inflection"));
}

#[test]
fn test_suppress_entire_file_multiple_rules() {
    let src = format!(r#"
        # @suppress entire-file: unused-inflection, single-meaning-multiple
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        entry go {{
            headword: "go"
            stems {{ root: "go" }}
            inflection_class: verb
            meanings {{
                only_one {{ "to go" }}
            }}
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(!has_rule(&result, "unused-inflection"));
    assert!(!has_rule(&result, "single-meaning-multiple"));
}

#[test]
fn test_suppress_entire_file_does_not_affect_other_rules() {
    let src = format!(r#"
        # @suppress entire-file: single-meaning-multiple
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    // unused-inflection should still fire
    assert!(has_rule(&result, "unused-inflection"));
}

#[test]
fn test_suppress_entire_file_after_content_no_effect() {
    // entire-file after non-comment content should be ignored
    let src = format!(r#"
        {}
        inflection verb for {{tense}} {{
            requires stems: root
            [tense=present] -> `{{root}}s`
            [_] -> `{{root}}`
        }}
        # @suppress entire-file: unused-inflection
    "#, TENSE_PREAMBLE);
    let result = lint_source(&src);
    assert_no_compile_errors(&result);
    assert!(has_rule(&result, "unused-inflection"));
}
