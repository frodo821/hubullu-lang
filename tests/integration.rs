#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use rusqlite::Connection;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Create a temporary directory with .hu files for incremental tests.
/// Returns (dir, entry_path, output_path).
fn setup_incremental_fixture(
    name: &str,
    profile_hu: &str,
    main_hu: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("profile.hu"), profile_hu).unwrap();
    std::fs::write(dir.path().join("main.hu"), main_hu).unwrap();
    let entry = dir.path().join("main.hu");
    let output = dir.path().join(format!("{}.huc", name));
    (dir, entry, output)
}

#[test]
fn test_simple_compile() {
    let input = fixture_path("simple/main.hu");
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("simple.huc");

    let result = hubullu::compile(&input, &output);
    assert!(result.is_ok(), "compile failed: {:?}", result.err());

    // Verify SQLite contents
    let conn = Connection::open(&output).unwrap();

    // Check entries
    let entry_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(entry_count, 2, "expected 2 entries");

    // Check faren entry
    let headword: String = conn
        .query_row(
            "SELECT headword FROM entries WHERE name = 'faren'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(headword, "faren");

    // Check forms for faren
    let form_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'faren')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form_count, 4, "expected 4 forms for faren (2 tenses x 2 numbers)");

    // Check specific form
    let form: String = conn
        .query_row(
            "SELECT form_str FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'faren') AND tags LIKE '%tense=present%' AND tags LIKE '%number=sg%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form, "fars");

    // Check no forms for hus (no inflection)
    let hus_forms: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'hus')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hus_forms, 0);

    // Check tags
    let tag_value: String = conn
        .query_row(
            "SELECT value FROM entry_tags WHERE entry_id = (SELECT id FROM entries WHERE name = 'faren') AND axis = 'parts_of_speech'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tag_value, "verb");

    // Check FTS
    let fts_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries_fts WHERE entries_fts MATCH 'go'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_count, 1, "FTS should find 'faren' by meaning 'to go'");

    // Check that entries have integer IDs
    let entry_id: i64 = conn
        .query_row(
            "SELECT id FROM entries WHERE name = 'faren'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(entry_id > 0, "entry should have a positive integer ID");
}

#[test]
fn test_inline_inflection() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("inline.huc");

    std::fs::write(
        &input,
        r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}

entry sein {
  headword: "sein"
  tags: []
  stems {}
  meaning: "to be"
  inflect for {tense} {
    [tense=present] -> `bin`
    [tense=past] -> `war`
  }
}
"#,
    )
    .unwrap();

    let result = hubullu::compile(&input, &output);
    assert!(result.is_ok(), "compile failed: {:?}", result.err());

    let conn = Connection::open(&output).unwrap();
    let form_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'sein')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form_count, 2);
}

// ---------------------------------------------------------------------------
// Incremental compilation tests
// ---------------------------------------------------------------------------

const PROFILE_HU: &str = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}

@extend number_vals for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}

inflection strong_I for {tense, number} {
  requires stems: pres, past

  [tense=present, number=sg] -> `{pres}s`
  [tense=present, number=pl] -> `{pres}en`
  [tense=past, number=sg] -> `{past}`
  [tense=past, number=pl] -> `{past}en`
}
"#;

const MAIN_HU: &str = r#"
@use * from "profile.hu"

entry faren {
  headword: "faren"
  tags: [tense=present]
  stems { pres: "far", past: "for" }
  inflection_class: strong_I
  meaning: "to go"
}
"#;

/// Derive the cache path matching the production logic in lib.rs.
fn cache_path_for(output_path: &std::path::Path) -> PathBuf {
    let dir = output_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".hubullu-cache");
    let mut name = output_path
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    name.push(".cache");
    dir.join(name)
}

#[test]
fn test_incremental_cache_created() {
    let (_dir, entry, output) = setup_incremental_fixture("cache_created", PROFILE_HU, MAIN_HU);
    let cache_path = cache_path_for(&output);

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&cache_path);

    hubullu::compile(&entry, &output).unwrap();
    assert!(cache_path.exists(), "cache file should be created after first compile");
}

#[test]
fn test_incremental_no_change() {
    let (_dir, entry, output) = setup_incremental_fixture("no_change", PROFILE_HU, MAIN_HU);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let count1: i64 = conn.query_row("SELECT COUNT(*) FROM forms", [], |r| r.get(0)).unwrap();
    drop(conn);

    // Second compile (no changes)
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let count2: i64 = conn.query_row("SELECT COUNT(*) FROM forms", [], |r| r.get(0)).unwrap();

    assert_eq!(count1, count2, "form count should be identical after incremental recompile");
    assert_eq!(count1, 4, "expected 4 forms");
}

#[test]
fn test_incremental_entry_change() {
    let (dir, entry, output) = setup_incremental_fixture("entry_change", PROFILE_HU, MAIN_HU);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let count1: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
    drop(conn);
    assert_eq!(count1, 1);

    // Add a new entry to main.hu
    let new_main = format!(
        "{}\n{}",
        MAIN_HU,
        r#"
entry hus {
  headword: "hus"
  tags: []
  stems {}
  meaning: "house"
}
"#
    );
    std::fs::write(dir.path().join("main.hu"), new_main).unwrap();

    // Recompile (incremental — schema unchanged, only main.hu changed)
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let count2: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
    assert_eq!(count2, 2, "new entry should appear after incremental recompile");
}

#[test]
fn test_incremental_schema_change_forces_full_rebuild() {
    let (dir, entry, output) =
        setup_incremental_fixture("schema_change", PROFILE_HU, MAIN_HU);

    // First compile
    hubullu::compile(&entry, &output).unwrap();

    // Modify schema (change display text of an extend value)
    let new_profile = PROFILE_HU.replace(
        "sg { display: { en: \"Singular\" } }",
        "sg { display: { en: \"Sing.\" } }",
    );
    std::fs::write(dir.path().join("profile.hu"), new_profile).unwrap();

    // Recompile (should trigger full rebuild due to schema change)
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    // Still 2 tenses × 2 numbers = 4 forms, but verify rebuild happened
    let form_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'faren')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form_count, 4, "schema change should cause full rebuild");

    // Verify the display text changed in metadata
    let display: String = conn
        .query_row(
            "SELECT display_text FROM tagaxis_meta WHERE axis_name = 'number' AND value_name = 'sg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(display, "Sing.", "metadata should reflect schema change");
}

#[test]
fn test_incremental_cache_deleted() {
    let (_dir, entry, output) = setup_incremental_fixture("cache_deleted", PROFILE_HU, MAIN_HU);
    let cache_path = cache_path_for(&output);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    assert!(cache_path.exists());

    // Delete cache
    std::fs::remove_file(&cache_path).unwrap();

    // Recompile should succeed (falls back to full compile)
    hubullu::compile(&entry, &output).unwrap();
    assert!(cache_path.exists(), "cache should be recreated");

    let conn = Connection::open(&output).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM forms", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 4);
}

// =========================================================================
// Standard library imports (std: scheme)
// =========================================================================

#[test]
fn test_std_import() {
    let input = fixture_path("std_import/main.hu");
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("std_import.huc");

    let result = hubullu::compile(&input, &output);
    assert!(result.is_ok(), "compile with std: import failed: {:?}", result.err());

    let conn = Connection::open(&output).unwrap();
    let entry_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(entry_count, 1, "expected 1 entry using std:_test axis");
}
