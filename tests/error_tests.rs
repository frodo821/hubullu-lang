#![cfg(feature = "sqlite")]

use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Helper: parse source and return rendered error string, or panic if no errors.
fn parse_errors(source: &str) -> String {
    let result = hubullu::parse_source(source, "test.hu");
    assert!(result.has_errors(), "expected parse errors but got none");
    result
        .diagnostics
        .iter()
        .map(|d| d.render(&result.source_map))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Helper: compile a fixture file and return the error string, or panic if no errors.
fn compile_error(fixture: &str) -> String {
    let input = fixture_path(fixture);
    let output = std::env::temp_dir().join(format!(
        "hubullu_err_test_{}.sqlite",
        fixture.replace('/', "_")
    ));
    let _ = std::fs::remove_file(&output);
    let result = hubullu::compile(&input, &output);
    let _ = std::fs::remove_file(&output);
    result.expect_err(&format!(
        "expected compile error for fixture '{}' but got Ok",
        fixture
    ))
}

fn assert_error_contains(errors: &str, substring: &str) {
    assert!(
        errors.contains(substring),
        "expected error to contain {:?}, got:\n{}",
        substring,
        errors
    );
}

// =========================================================================
// Lexer errors (via parse_source)
// =========================================================================

#[test]
fn test_unterminated_string() {
    let errors = parse_errors(r#"entry foo { headword: "hello }"#);
    assert_error_contains(&errors, "unterminated string literal");
}

#[test]
fn test_unterminated_template() {
    let errors = parse_errors("entry foo { headword: `{foo ");
    assert_error_contains(&errors, "unterminated template literal");
}

#[test]
fn test_unknown_escape_string() {
    let errors = parse_errors(r#"entry foo { headword: "\q" }"#);
    assert_error_contains(&errors, "unknown escape sequence");
}

#[test]
fn test_unknown_escape_template() {
    let errors = parse_errors(r"entry foo { headword: `\q` }");
    assert_error_contains(&errors, "unknown escape sequence");
}

#[test]
fn test_missing_template_close_brace() {
    let errors = parse_errors("entry foo { headword: `{foo` }");
    assert_error_contains(&errors, "expected '}'");
}

#[test]
fn test_unknown_directive() {
    let errors = parse_errors("@foobar");
    assert_error_contains(&errors, "unknown directive");
}

#[test]
fn test_unexpected_character() {
    // bare `-` is now lexed as Minus, but rejected by the parser at top level
    let errors = parse_errors("- foo");
    assert_error_contains(&errors, "expected top-level item");
}

// =========================================================================
// Parser errors (via parse_source)
// =========================================================================

#[test]
fn test_missing_entry_headword() {
    let errors = parse_errors(
        r#"entry foo {
  tags: []
  stems {}
  meaning: "test"
}"#,
    );
    assert_error_contains(&errors, "entry missing 'headword' field");
}

#[test]
fn test_missing_entry_meaning() {
    let errors = parse_errors(
        r#"entry foo {
  headword: "foo"
  tags: []
  stems {}
}"#,
    );
    assert_error_contains(&errors, "entry missing 'meaning'");
}

#[test]
fn test_unexpected_entry_field() {
    let errors = parse_errors(
        r#"entry foo {
  headword: "foo"
  badfield: "x"
  meaning: "test"
}"#,
    );
    assert_error_contains(&errors, "unexpected field in entry");
}

#[test]
fn test_expected_identifier() {
    // `for` without a proper identifier where one is expected
    let errors = parse_errors("tagaxis { role: inflectional }");
    // The parser expects an identifier after `tagaxis`
    assert_error_contains(&errors, "expected identifier");
}

#[test]
fn test_expected_top_level_item() {
    let errors = parse_errors("foobar baz");
    assert_error_contains(&errors, "expected top-level item");
}

#[test]
fn test_unknown_tagaxis_role() {
    let errors = parse_errors(
        r#"tagaxis foo {
  role: foobar
  display: { en: "Foo" }
}"#,
    );
    assert_error_contains(&errors, "unknown role");
}

#[test]
fn test_missing_tagaxis_role() {
    let errors = parse_errors(
        r#"tagaxis foo {
  display: { en: "Foo" }
}"#,
    );
    assert_error_contains(&errors, "tagaxis missing 'role' field");
}

// =========================================================================
// Phase 1 errors (via compile with fixture files)
// =========================================================================

#[test]
fn test_circular_use() {
    let errors = compile_error("errors/circular_a.hu");
    assert_error_contains(&errors, "circular @use detected");
}

#[test]
fn test_file_not_found() {
    let errors = compile_error("errors/missing_file.hu");
    // The compile function itself will fail trying to read the entry file
    assert!(
        errors.contains("cannot read") || errors.contains("No such file"),
        "expected file-not-found error, got:\n{}",
        errors
    );
}

#[test]
fn test_import_entry_via_use() {
    let errors = compile_error("errors/bad_import_kind.hu");
    assert_error_contains(&errors, "cannot import entry");
    assert_error_contains(&errors, "via @use");
}

#[test]
fn test_import_declaration_via_reference() {
    let errors = compile_error("errors/bad_import_ref.hu");
    assert_error_contains(&errors, "cannot import declaration");
    assert_error_contains(&errors, "via @reference");
}

#[test]
fn test_symbol_not_found_in_import() {
    let errors = compile_error("errors/missing_import.hu");
    assert_error_contains(&errors, "not found in imported file");
}

#[test]
fn test_duplicate_definition() {
    let errors = compile_error("errors/duplicate_symbol.hu");
    assert_error_contains(&errors, "duplicate definition");
}

// =========================================================================
// Phase 2 errors (via compile with fixture files)
// =========================================================================

#[test]
fn test_extend_unknown_axis() {
    let errors = compile_error("errors/extend_unknown_axis.hu");
    assert_error_contains(&errors, "@extend targets unknown tagaxis");
}

#[test]
fn test_undeclared_axis_in_inflection() {
    let errors = compile_error("errors/undeclared_axis_inflection.hu");
    assert_error_contains(&errors, "references undeclared axis");
}

#[test]
fn test_inflection_class_not_found() {
    let errors = compile_error("errors/missing_inflection_class.hu");
    assert_error_contains(&errors, "inflection class");
    assert_error_contains(&errors, "not found");
}

#[test]
fn test_no_rule_matches_cell() {
    let errors = compile_error("errors/no_rule_matches.hu");
    assert_error_contains(&errors, "no rule matches cell");
}

#[test]
fn test_ambiguous_rule_match() {
    let errors = compile_error("errors/ambiguous_rules.hu");
    assert_error_contains(&errors, "ambiguous rule match");
}

#[test]
fn test_undefined_stem() {
    let errors = compile_error("errors/undefined_stem.hu");
    assert_error_contains(&errors, "undefined stem");
}

#[test]
fn test_cyclic_derived_from() {
    let errors = compile_error("errors/cyclic_derived_from.hu");
    assert_error_contains(&errors, "cyclic derived_from");
}

// =========================================================================
// Import scheme errors
// =========================================================================

#[test]
fn test_unknown_std_module() {
    let errors = compile_error("errors/unknown_std_module.hu");
    assert_error_contains(&errors, "unknown standard library module");
    assert_error_contains(&errors, "nonexistent");
}

#[test]
fn test_unsupported_import_scheme() {
    let errors = compile_error("errors/unsupported_scheme.hu");
    assert_error_contains(&errors, "unsupported import scheme");
}
