#![cfg(feature = "sqlite")]

use std::path::PathBuf;
use std::rc::Rc;

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

// =========================================================================
// Phase 2 errors (via temp files)
// =========================================================================

/// Helper: write temp files, compile, and return the error string.
fn compile_error_from_sources(files: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().unwrap();
    for (fname, content) in files {
        std::fs::write(dir.path().join(fname), content).unwrap();
    }
    let entry_path = dir.path().join(files[0].0);
    let output = dir.path().join("out.huc");
    let result = hubullu::compile(&entry_path, &output);
    result.expect_err("expected compile error but got Ok")
}

#[test]
fn test_duplicate_extend_values() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
@extend a for tagaxis t { x {} }
@extend b for tagaxis t { x {} }
"#,
    )]);
    assert_error_contains(&errors, "added by multiple @extends");
}

#[test]
fn test_inflection_undeclared_axis_in_rule() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection cls for {t} {
  [bad_axis=a] -> `form`
}
entry e { headword: "e" inflection_class: cls meaning: "e" }
"#,
    )]);
    assert_error_contains(&errors, "not in for {} declaration");
}

#[test]
fn test_phonrule_class_union_undefined() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
phonrule pr {
  class V = ["a", "e"]
  class ALL = V | MISSING
  V -> null / _ #
}
"#,
    )]);
    assert_error_contains(&errors, "class union references undefined class");
}

#[test]
fn test_phonrule_from_undefined_class() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
phonrule pr {
  MISSING -> null / _ #
}
"#,
    )]);
    assert_error_contains(&errors, "rewrite rule references undefined class");
}

#[test]
fn test_phonrule_to_undefined_map() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
phonrule pr {
  class V = ["a", "e"]
  V -> MISSING / _ #
}
"#,
    )]);
    assert_error_contains(&errors, "rewrite rule references undefined map");
}

#[test]
fn test_phonrule_context_undefined_class() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
phonrule pr {
  class V = ["a", "e"]
  V -> null / MISSING _
}
"#,
    )]);
    assert_error_contains(&errors, "context references undefined class");
}

#[test]
fn test_stem_slot_mismatch() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: structural }
@extend tv for tagaxis t {
  a { slots: [s1, s2, s3] }
}
inflection cls for {t} {
  requires stems: root [t=a]
  [t=a] -> `{root.s1}`
}
entry e {
  headword: "e"
  stems { root: "ab" }
  inflection_class: cls
  meaning: "e"
}
"#,
    )]);
    assert_error_contains(&errors, "stem length mismatch");
}

#[test]
fn test_axis_no_values() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
inflection cls for {t} {
  [t=x] -> `form`
}
entry e {
  headword: "e"
  inflection_class: cls
  meaning: "e"
}
"#,
    )]);
    assert_error_contains(&errors, "has no values");
}

#[test]
fn test_delegate_target_not_found() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection cls for {t} {
  [t=a] -> nonexistent[t=a]
}
entry e {
  headword: "e"
  inflection_class: cls
  meaning: "e"
}
"#,
    )]);
    assert_error_contains(&errors, "delegate target");
    assert_error_contains(&errors, "not found");
}

#[test]
fn test_template_undefined_stem() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection cls for {t} {
  [t=a] -> `{nonexistent}`
}
entry e {
  headword: "e"
  stems {}
  inflection_class: cls
  meaning: "e"
}
"#,
    )]);
    assert_error_contains(&errors, "undefined stem");
}

#[test]
fn test_inflection_references_undeclared_axis() {
    let errors = compile_error_from_sources(&[(
        "main.hu",
        r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection cls for {t} {
  [nonexistent=a] -> `form`
}
entry e { headword: "e" inflection_class: cls meaning: "e" }
"#,
    )]);
    assert_error_contains(&errors, "not in for {} declaration");
}

// =========================================================================
// Inflection evaluation errors (phonrule / compose / delegate)
// =========================================================================

#[test]
fn test_phonrule_not_found_in_apply() {
    let errors = compile_error("errors/phonrule_not_found_apply.hu");
    assert_error_contains(&errors, "phonrule 'harmony' not found");
}

#[test]
fn test_no_rule_matches_slot() {
    let errors = compile_error("errors/slot_no_rule_matches.hu");
    assert_error_contains(&errors, "no rule matches slot");
}

// =========================================================================
// Parser errors (via parse_source)
// =========================================================================

#[test]
fn test_apply_expr_expected_phonrule_or_cell() {
    let errors = parse_errors(
        r#"tagaxis t { role: inflectional }
inflection cls for {t} {
  apply (cell)
  [t=a] -> `form`
}"#,
    );
    assert_error_contains(&errors, "expected phonrule name or 'cell' in apply expression");
}

#[test]
fn test_unknown_tagaxis_index_kind() {
    let errors = parse_errors(
        r#"tagaxis foo {
  role: inflectional
  index: foobar
}"#,
    );
    assert_error_contains(&errors, "unknown index kind");
}

#[test]
fn test_unknown_tagaxis_field() {
    let errors = parse_errors(
        r#"tagaxis foo {
  role: inflectional
  badfield: "x"
}"#,
    );
    assert_error_contains(&errors, "unknown tagaxis field");
}

#[test]
fn test_headword_bad_syntax() {
    let errors = parse_errors(
        r#"entry foo {
  headword 123
  meaning: "test"
}"#,
    );
    assert_error_contains(&errors, "expected");
}

#[test]
fn test_unexpected_extend_value_field() {
    let errors = parse_errors(
        r#"tagaxis t { role: inflectional }
@extend ev for tagaxis t {
  x { badfield: "x" }
}"#,
    );
    assert_error_contains(&errors, "unexpected field in @extend value");
}

#[test]
fn test_entry_missing_both_headword_and_meaning() {
    let errors = parse_errors(
        r#"entry foo {
  tags: []
}"#,
    );
    // Should report missing headword (first required field)
    assert_error_contains(&errors, "missing");
}

// =========================================================================
// Phase 1 errors (via temp files)
// =========================================================================

#[test]
fn test_export_symbol_not_found() {
    let errors = compile_error_from_sources(&[
        ("main.hu", r#"
@export reference nonexistent from "dep.hu"
"#),
        ("dep.hu", r#"
entry hello { headword: "hello" meaning: "hi" }
"#),
    ]);
    assert_error_contains(&errors, "not found in imported file");
}

#[test]
fn test_cannot_export_entry_via_use() {
    let errors = compile_error_from_sources(&[
        ("main.hu", r#"
@use * from "dep.hu"
@export use hello from "dep.hu"
"#),
        ("dep.hu", r#"
entry hello { headword: "hello" meaning: "hi" }
"#),
    ]);
    assert_error_contains(&errors, "cannot");
}

#[test]
fn test_export_undefined_symbol_from_scope() {
    let errors = compile_error_from_sources(&[
        ("main.hu", r#"
tagaxis t { role: inflectional }
@export use nonexistent
"#),
    ]);
    assert_error_contains(&errors, "symbol 'nonexistent' not found in scope for @export");
}

// =========================================================================
// Render errors (form-spec resolution via compile + .hut resolve)
// =========================================================================

/// Helper: compile a .hu source, write a .hut source, and resolve.
/// Returns Ok(resolved text) or Err(error string).
fn render_resolve(hu_source: &str, hut_source: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    let hu_path = dir.path().join("dict.hu");
    let huc_path = dir.path().join("dict.huc");
    std::fs::write(&hu_path, hu_source).unwrap();
    hubullu::compile(&hu_path, &huc_path)?;

    let hut_src = format!("@reference * from \"dict.hu\"\n{}", hut_source);
    let (hut_file, source_map) = hubullu::render::parse_hut(&hut_src, "test.hut")?;

    let conn = Rc::new(
        rusqlite::Connection::open_with_flags(
            &huc_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("cannot open huc: {}", e))?,
    );
    let source = hubullu::render::EntrySource { conn, name_map: None };
    let ctx = hubullu::render::ResolveContext {
        namespaced: std::collections::HashMap::new(),
        default_sources: vec![source],
    };

    let parts = hubullu::render::resolve(&hut_file.tokens, &ctx, &source_map)?;
    Ok(hubullu::render::smart_join(&parts, " ", ".,;:!?"))
}

#[test]
fn test_render_form_spec_no_match() {
    let hu = r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} b {} }
inflection cls for {t} {
  requires stems: root
  [t=a] -> `{root}ed`
  [t=b] -> `{root}ing`
}
entry walk {
  headword: "walk"
  stems { root: "walk" }
  inflection_class: cls
  meaning: "to walk"
}
"#;
    let hut = "walk[t=c]";
    let err = render_resolve(hu, hut).unwrap_err();
    assert_error_contains(&err, "has no form matching");
}

#[test]
fn test_render_form_spec_ambiguous() {
    let hu = r#"
tagaxis t { role: inflectional }
tagaxis n { role: inflectional }
@extend tv for tagaxis t { a {} b {} }
@extend nv for tagaxis n { x {} y {} }
inflection cls for {t, n} {
  requires stems: root
  [t=a, n=x] -> `{root}ax`
  [t=a, n=y] -> `{root}ay`
  [t=b, n=x] -> `{root}bx`
  [t=b, n=y] -> `{root}by`
}
entry walk {
  headword: "walk"
  stems { root: "walk" }
  inflection_class: cls
  meaning: "to walk"
}
"#;
    // t=a matches two cells: (t=a,n=x) and (t=a,n=y) — ambiguous
    let hut = "walk[t=a]";
    let err = render_resolve(hu, hut).unwrap_err();
    assert_error_contains(&err, "ambiguous form spec");
}

#[test]
fn test_render_form_spec_exact_match() {
    let hu = r#"
tagaxis t { role: inflectional }
tagaxis n { role: inflectional }
@extend tv for tagaxis t { a {} b {} }
@extend nv for tagaxis n { x {} y {} }
inflection cls for {t, n} {
  requires stems: root
  [t=a, n=x] -> `{root}ax`
  [t=a, n=y] -> `{root}ay`
  [t=b, n=x] -> `{root}bx`
  [t=b, n=y] -> `{root}by`
}
entry walk {
  headword: "walk"
  stems { root: "walk" }
  inflection_class: cls
  meaning: "to walk"
}
"#;
    // Fully specified — should resolve uniquely
    let hut = "walk[t=a, n=x]";
    let result = render_resolve(hu, hut).unwrap();
    assert_eq!(result, "walkax");
}

#[test]
fn test_render_form_spec_partial_unique() {
    let hu = r#"
tagaxis t { role: inflectional }
tagaxis n { role: inflectional }
@extend tv for tagaxis t { a {} }
@extend nv for tagaxis n { x {} y {} }
inflection cls for {t, n} {
  requires stems: root
  [t=a, n=x] -> `{root}ax`
  [t=a, n=y] -> `{root}ay`
}
entry walk {
  headword: "walk"
  stems { root: "walk" }
  inflection_class: cls
  meaning: "to walk"
}
"#;
    // n=x matches only one cell (t=a,n=x) — unique partial match
    let hut = "walk[n=x]";
    let result = render_resolve(hu, hut).unwrap();
    assert_eq!(result, "walkax");
}
