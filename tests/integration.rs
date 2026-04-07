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
// Merkle-tree incremental cache tests
// =========================================================================

/// Helper: create a multi-file project in a temp dir.
/// `files` is a list of (filename, content) pairs; the first file is the entry.
fn setup_merkle_project(
    name: &str,
    files: &[(&str, &str)],
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (fname, content) in files {
        std::fs::write(dir.path().join(fname), content).unwrap();
    }
    let entry = dir.path().join(files[0].0);
    let output = dir.path().join(format!("{}.huc", name));
    (dir, entry, output)
}

/// Query a single form string for a given entry + tag filter.
fn query_form(conn: &Connection, entry_name: &str, tag_filter: &str) -> String {
    conn.query_row(
        &format!(
            "SELECT form_str FROM forms WHERE entry_id = \
             (SELECT id FROM entries WHERE name = '{}') AND tags LIKE '%{}%'",
            entry_name, tag_filter,
        ),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// Query the form count for a given entry.
fn query_form_count(conn: &Connection, entry_name: &str) -> i64 {
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM forms WHERE entry_id = \
             (SELECT id FROM entries WHERE name = '{}')",
            entry_name,
        ),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn test_merkle_entry_change_only_affects_changed_entry() {
    // Two entries sharing the same inflection. Changing one entry's stem should
    // produce correct forms for both entries (the unchanged one from cache,
    // the changed one freshly expanded).
    let profile = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}
inflection verb for {tense} {
  requires stems: root
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
}
"#;
    let main_v1 = r#"
@use * from "profile.hu"
entry go {
  headword: "go"
  stems { root: "go" }
  inflection_class: verb
  meaning: "to go"
}
entry run {
  headword: "run"
  stems { root: "run" }
  inflection_class: verb
  meaning: "to run"
}
"#;

    let (dir, entry, output) = setup_merkle_project("merkle_entry", &[
        ("main.hu", main_v1),
        ("profile.hu", profile),
    ]);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form(&conn, "go", "tense=present"), "gos");
    assert_eq!(query_form(&conn, "run", "tense=present"), "runs");
    drop(conn);

    // Change "go" entry stem → "walk"
    let main_v2 = main_v1.replace(
        "stems { root: \"go\" }",
        "stems { root: \"walk\" }",
    ).replace(
        "headword: \"go\"",
        "headword: \"walk\"",
    );
    std::fs::write(dir.path().join("main.hu"), main_v2).unwrap();

    // Recompile — "run" should come from cache, "go" re-expanded
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form(&conn, "go", "tense=present"), "walks");
    assert_eq!(query_form(&conn, "go", "tense=past"), "walked");
    // "run" should be unaffected
    assert_eq!(query_form(&conn, "run", "tense=present"), "runs");
    assert_eq!(query_form(&conn, "run", "tense=past"), "runed");
}

#[test]
fn test_merkle_inflection_change_propagates_to_entries() {
    // Changing an inflection class rule should cause entries using it to be
    // re-expanded, while entries using a different class remain cached.
    let profile_v1 = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}
inflection verb_a for {tense} {
  requires stems: root
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
}
inflection verb_b for {tense} {
  requires stems: root
  [tense=present] -> `{root}ing`
  [tense=past] -> `{root}t`
}
"#;
    let main_hu = r#"
@use * from "profile.hu"
entry foo {
  headword: "foo"
  stems { root: "foo" }
  inflection_class: verb_a
  meaning: "foo"
}
entry bar {
  headword: "bar"
  stems { root: "bar" }
  inflection_class: verb_b
  meaning: "bar"
}
"#;

    let (dir, entry, output) = setup_merkle_project("merkle_infl", &[
        ("main.hu", main_hu),
        ("profile.hu", profile_v1),
    ]);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form(&conn, "foo", "tense=present"), "foos");
    assert_eq!(query_form(&conn, "bar", "tense=present"), "baring");
    drop(conn);

    // Change verb_a rule: present now uses "z" suffix instead of "s"
    let profile_v2 = profile_v1.replace(
        "[tense=present] -> `{root}s`",
        "[tense=present] -> `{root}z`",
    );
    std::fs::write(dir.path().join("profile.hu"), profile_v2).unwrap();

    // Recompile — "foo" (verb_a) should change, "bar" (verb_b) cached
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form(&conn, "foo", "tense=present"), "fooz");
    assert_eq!(query_form(&conn, "foo", "tense=past"), "fooed");
    // "bar" must be unchanged
    assert_eq!(query_form(&conn, "bar", "tense=present"), "baring");
    assert_eq!(query_form(&conn, "bar", "tense=past"), "bart");
}

#[test]
fn test_merkle_phonrule_change_propagates_through_inflection() {
    // A phonrule change should propagate through the inflection that uses it
    // to the entries using that inflection.
    let profile_v1 = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}

phonrule doubling {
  class V = ["a", "e", "i", "o", "u"]
  V -> null / _ + _
}

inflection verb_with_phon for {tense} {
  requires stems: root
  apply doubling(cell)
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
}

inflection verb_plain for {tense} {
  requires stems: root
  [tense=present] -> `{root}ing`
  [tense=past] -> `{root}t`
}
"#;
    let main_hu = r#"
@use * from "profile.hu"
entry alpha {
  headword: "alpha"
  stems { root: "alpha" }
  inflection_class: verb_with_phon
  meaning: "alpha"
}
entry beta {
  headword: "beta"
  stems { root: "beta" }
  inflection_class: verb_plain
  meaning: "beta"
}
"#;

    let (dir, entry, output) = setup_merkle_project("merkle_phon", &[
        ("main.hu", main_hu),
        ("profile.hu", profile_v1),
    ]);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let alpha_present_v1 = query_form(&conn, "alpha", "tense=present");
    let beta_present_v1 = query_form(&conn, "beta", "tense=present");
    drop(conn);

    // Change the phonrule: delete vowels instead of null context
    let profile_v2 = profile_v1.replace(
        "V -> null / _ + _",
        "V -> null / _ #",
    );
    std::fs::write(dir.path().join("profile.hu"), profile_v2).unwrap();

    // Recompile — "alpha" (uses doubling via verb_with_phon) should change,
    // "beta" (verb_plain, no phonrule) should be cached
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let alpha_present_v2 = query_form(&conn, "alpha", "tense=present");
    let beta_present_v2 = query_form(&conn, "beta", "tense=present");

    // alpha must have changed (different phonrule result)
    assert_ne!(
        alpha_present_v1, alpha_present_v2,
        "phonrule change must propagate to entries using it"
    );
    // beta must be identical
    assert_eq!(
        beta_present_v1, beta_present_v2,
        "entries not using the changed phonrule must be unaffected"
    );
}

#[test]
fn test_merkle_extend_change_propagates_to_entries() {
    // Adding a value to an @extend should cause entries using that axis to be
    // re-expanded (new forms appear).
    let profile_v1 = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}
inflection verb for {tense} {
  requires stems: root
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
  [_] -> `{root}`
}
"#;
    let main_hu = r#"
@use * from "profile.hu"
entry go {
  headword: "go"
  stems { root: "go" }
  inflection_class: verb
  meaning: "to go"
}
"#;

    let (dir, entry, output) = setup_merkle_project("merkle_extend", &[
        ("main.hu", main_hu),
        ("profile.hu", profile_v1),
    ]);

    // First compile — 2 forms (present, past)
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form_count(&conn, "go"), 2);
    drop(conn);

    // Add "future" to the extend
    let profile_v2 = profile_v1.replace(
        "past { display: { en: \"Past\" } }\n}",
        "past { display: { en: \"Past\" } }\n  future { display: { en: \"Future\" } }\n}",
    );
    std::fs::write(dir.path().join("profile.hu"), profile_v2).unwrap();

    // Recompile — should now have 3 forms
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    assert_eq!(query_form_count(&conn, "go"), 3);
}

#[test]
fn test_merkle_no_change_uses_cache() {
    // Compiling twice with no changes should produce identical results.
    let profile = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}
inflection verb for {tense} {
  requires stems: root
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
}
"#;
    let main_hu = r#"
@use * from "profile.hu"
entry go {
  headword: "go"
  stems { root: "go" }
  inflection_class: verb
  meaning: "to go"
}
"#;

    let (_dir, entry, output) = setup_merkle_project("merkle_noop", &[
        ("main.hu", main_hu),
        ("profile.hu", profile),
    ]);

    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let forms1: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT form_str FROM forms ORDER BY form_str")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    drop(conn);

    // Second compile — no changes
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let forms2: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT form_str FROM forms ORDER BY form_str")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    assert_eq!(forms1, forms2, "recompile with no changes must produce identical forms");
}

#[test]
fn test_merkle_cross_file_phonrule_change() {
    // Test that phonrule changes in a separate file propagate correctly
    // to entries that use an inflection referencing that phonrule.
    // The phonrule and inflection live in the same file (profile.hu),
    // entries in main.hu import from profile.hu.
    let profile_v1 = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
@extend tv for tagaxis tense {
  present { display: { en: "Present" } }
  past { display: { en: "Past" } }
}

phonrule doubling {
  class V = ["a", "e", "i", "o", "u"]
  V -> null / _ + _
}

inflection verb for {tense} {
  requires stems: root
  apply doubling(cell)
  [tense=present] -> `{root}s`
  [tense=past] -> `{root}ed`
}

inflection verb_plain for {tense} {
  requires stems: root
  [tense=present] -> `{root}ing`
  [tense=past] -> `{root}t`
}
"#;
    let main_hu = r#"
@use * from "profile.hu"
entry alpha {
  headword: "alpha"
  stems { root: "alpha" }
  inflection_class: verb
  meaning: "alpha"
}
entry beta {
  headword: "beta"
  stems { root: "beta" }
  inflection_class: verb_plain
  meaning: "beta"
}
"#;

    let (dir, entry, output) = setup_merkle_project("merkle_crossfile", &[
        ("main.hu", main_hu),
        ("profile.hu", profile_v1),
    ]);

    // First compile
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let alpha_v1 = query_form(&conn, "alpha", "tense=present");
    let beta_v1 = query_form(&conn, "beta", "tense=present");
    drop(conn);

    // Change the phonrule in profile.hu (only file that changes)
    let profile_v2 = profile_v1.replace(
        "V -> null / _ + _",
        "V -> null / _ #",
    );
    std::fs::write(dir.path().join("profile.hu"), profile_v2).unwrap();

    // Recompile — "alpha" (verb, uses doubling) must change,
    // "beta" (verb_plain, no phonrule) must be cached
    hubullu::compile(&entry, &output).unwrap();
    let conn = Connection::open(&output).unwrap();
    let alpha_v2 = query_form(&conn, "alpha", "tense=present");
    let beta_v2 = query_form(&conn, "beta", "tense=present");

    assert_ne!(
        alpha_v1, alpha_v2,
        "phonrule change must propagate through inflection to entry"
    );
    assert_eq!(
        beta_v1, beta_v2,
        "entry not using changed phonrule must be unaffected"
    );
}

// =========================================================================
// Phase 2 direct tests
// =========================================================================

/// Helper: run phase1+phase2 from temp files and return Phase2Result.
fn run_p1_p2(files: &[(&str, &str)]) -> (hubullu::phase1::Phase1Result, hubullu::phase2::Phase2Result) {
    let dir = tempfile::tempdir().unwrap();
    for (fname, content) in files {
        std::fs::write(dir.path().join(fname), content).unwrap();
    }
    let entry_path = dir.path().join(files[0].0);
    let p1 = hubullu::phase1::run_phase1(&entry_path, std::collections::HashMap::new());
    assert!(!p1.diagnostics.has_errors(), "phase1 errors: {}", p1.diagnostics.render_all(&p1.source_map));
    let p2 = hubullu::phase2::run_phase2(&p1);
    (p1, p2)
}

#[test]
fn test_phase2_extend_resolution() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
tagaxis pos {
  role: classificatory
}
@extend pos_vals for tagaxis pos {
  noun { display: { en: "Noun" } }
  verb { display: { en: "Verb" } }
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors());
    let pos_axis = p2.axes.get("pos").expect("pos axis should exist");
    assert_eq!(pos_axis.values, vec!["noun", "verb"]);
    let noun_display = pos_axis.display.get("noun").unwrap();
    assert_eq!(noun_display, &vec![("en".to_string(), "Noun".to_string())]);
}

#[test]
fn test_phase2_extend_unknown_axis_error() {
    let (p1, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
@extend vals for tagaxis nonexistent {
  a {}
}
"#,
    )]);

    assert!(p2.diagnostics.has_errors());
    let msg = p2.diagnostics.render_all(&p1.source_map);
    assert!(msg.contains("unknown tagaxis"), "should report unknown axis, got: {}", msg);
}

#[test]
fn test_phase2_entry_expansion_basic() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
tagaxis t {
  role: inflectional
}
@extend tv for tagaxis t {
  a {}
  b {}
}
inflection cls for {t} {
  [t=a] -> `xA`
  [t=b] -> `xB`
}
entry foo {
  headword: "foo"
  inflection_class: cls
  meaning: "test"
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors(), "unexpected errors");
    assert_eq!(p2.entries.len(), 1);
    let entry = &p2.entries[0];
    assert_eq!(entry.name, "foo");
    assert_eq!(entry.headword, "foo");
    assert_eq!(entry.meaning, "test");
    assert_eq!(entry.forms.len(), 2);

    let form_strs: Vec<&str> = entry.forms.iter().map(|f| f.form_str.as_str()).collect();
    assert!(form_strs.contains(&"xA"));
    assert!(form_strs.contains(&"xB"));
}

#[test]
fn test_phase2_entry_no_inflection() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
entry bar {
  headword: "bar"
  meaning: "a bar"
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors());
    assert_eq!(p2.entries.len(), 1);
    assert_eq!(p2.entries[0].forms.len(), 0);
}

#[test]
fn test_phase2_derived_from_dag_check() {
    let (p1, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
entry a {
  headword: "a"
  meaning: "a"
  etymology { derived_from: b }
}
entry b {
  headword: "b"
  meaning: "b"
  etymology { derived_from: a }
}
"#,
    )]);

    assert!(p2.diagnostics.has_errors());
    let msg = p2.diagnostics.render_all(&p1.source_map);
    assert!(msg.contains("cyclic derived_from"), "should detect cycle, got: {}", msg);
}

#[test]
fn test_phase2_inflection_unknown_class_error() {
    let (p1, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
entry foo {
  headword: "foo"
  inflection_class: nonexistent
  meaning: "test"
}
"#,
    )]);

    assert!(p2.diagnostics.has_errors());
    let msg = p2.diagnostics.render_all(&p1.source_map);
    assert!(msg.contains("not found"), "should report missing inflection class, got: {}", msg);
}

#[test]
fn test_phase2_multiple_meanings() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
entry poly {
  headword: "poly"
  meanings {
    m1 { "first meaning" }
    m2 { "second meaning" }
  }
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors());
    let entry = &p2.entries[0];
    assert_eq!(entry.meaning, "first meaning"); // first meaning used as primary
    assert_eq!(entry.meanings.len(), 2);
    assert_eq!(entry.meanings[0], ("m1".to_string(), "first meaning".to_string()));
    assert_eq!(entry.meanings[1], ("m2".to_string(), "second meaning".to_string()));
}

#[test]
fn test_phase2_inline_inflection() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
tagaxis t {
  role: inflectional
}
@extend tv for tagaxis t {
  x {}
  y {}
}
entry baz {
  headword: "baz"
  meaning: "baz"
  inflect for {t} {
    [t=x] -> `bazX`
    [t=y] -> `bazY`
  }
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors());
    assert_eq!(p2.entries[0].forms.len(), 2);
    assert!(p2.entries[0].inflection_class.is_none());
}

#[test]
fn test_phase2_render_config_defaults() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
entry x {
  headword: "x"
  meaning: "x"
}
"#,
    )]);

    assert_eq!(p2.render_config.separator, " ");
    assert_eq!(p2.render_config.no_separator_before, ".,;:!?");
}

#[test]
fn test_phase2_render_config_custom() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
@render {
  separator: "-"
  no_separator_before: "."
}
entry x {
  headword: "x"
  meaning: "x"
}
"#,
    )]);

    assert_eq!(p2.render_config.separator, "-");
    assert_eq!(p2.render_config.no_separator_before, ".");
}

#[test]
fn test_phase2_inflection_metadata_collected() {
    let (_, p2) = run_p1_p2(&[(
        "main.hu",
        r#"
tagaxis t {
  role: inflectional
}
@extend tv for tagaxis t { x {} }
inflection my_cls display { en: "My Class", ja: "クラス" } for {t} {
  [t=x] -> `form`
}
"#,
    )]);

    assert!(!p2.diagnostics.has_errors());
    assert_eq!(p2.inflections.len(), 1);
    let infl = &p2.inflections[0];
    assert_eq!(infl.name, "my_cls");
    assert_eq!(infl.axes, vec!["t"]);
    assert!(infl.display.contains(&("en".to_string(), "My Class".to_string())));
    assert!(infl.display.contains(&("ja".to_string(), "クラス".to_string())));
}

// =========================================================================
// Emit SQLite tests
// =========================================================================

#[test]
fn test_emit_schema_tables_exist() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("schema.huc");
    std::fs::write(&input, r#"
entry x {
  headword: "x"
  meaning: "x"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    // Verify all expected tables exist
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    let expected_core = [
        "compile_meta", "entries", "entries_fts",
        "entry_meanings", "entry_tags",
        "forms", "headword_scripts", "inflection_axes",
        "inflection_display", "inflection_meta",
        "links", "name_resolution", "render_config", "stems",
        "tagaxis_meta",
    ];
    for t in &expected_core {
        assert!(tables.contains(&t.to_string()), "missing table: {}", t);
    }
}

#[test]
fn test_emit_indexes_exist() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("idx.huc");
    std::fs::write(&input, r#"
entry x { headword: "x" meaning: "x" }
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let indexes: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    let expected_idx = [
        "idx_entries_name", "idx_forms_entry", "idx_forms_form",
        "idx_links_src", "idx_links_dst", "idx_stems_entry",
        "idx_entry_tags", "idx_entry_tags_axis",
        "idx_inflection_display", "idx_inflection_axes",
        "idx_name_resolution_hash",
    ];
    for idx in &expected_idx {
        assert!(indexes.contains(&idx.to_string()), "missing index: {}", idx);
    }
}

#[test]
fn test_emit_tags_stored_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("tags.huc");
    std::fs::write(&input, r#"
tagaxis pos { role: classificatory }
@extend pv for tagaxis pos {
  noun {}
  verb {}
}
entry w {
  headword: "w"
  tags: [pos=noun]
  meaning: "word"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let (axis, value): (String, String) = conn
        .query_row(
            "SELECT axis, value FROM entry_tags WHERE entry_id = (SELECT id FROM entries WHERE name = 'w')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(axis, "pos");
    assert_eq!(value, "noun");
}

#[test]
fn test_emit_forms_tag_format() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("formtag.huc");
    std::fs::write(&input, r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection c for {t} {
  [t=a] -> `form_a`
}
entry e {
  headword: "e"
  inflection_class: c
  meaning: "e"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let tags: String = conn
        .query_row(
            "SELECT tags FROM forms WHERE entry_id = (SELECT id FROM entries WHERE name = 'e')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tags, "t=a");
}

#[test]
fn test_emit_fts_search() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("fts.huc");
    std::fs::write(&input, r#"
entry alpha {
  headword: "alpha"
  meaning: "first letter"
}
entry beta {
  headword: "beta"
  meaning: "second letter"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    // Search by headword
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entries_fts WHERE entries_fts MATCH 'alpha'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    // Search by meaning
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entries_fts WHERE entries_fts MATCH 'letter'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_emit_etymology_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("ety.huc");
    std::fs::write(&input, r#"
entry parent {
  headword: "parent"
  meaning: "parent word"
  etymology {
    proto: "*proto"
    note: "from old language"
  }
}
entry child {
  headword: "child"
  meaning: "child word"
  etymology { derived_from: parent }
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    // Check etymology fields
    let (proto, note): (String, String) = conn
        .query_row(
            "SELECT etymology_proto, etymology_note FROM entries WHERE name = 'parent'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(proto, "*proto");
    assert_eq!(note, "from old language");

    // Check derived_from link
    let link_type: String = conn
        .query_row(
            "SELECT link_type FROM links WHERE src_entry_id = (SELECT id FROM entries WHERE name = 'child')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(link_type, "derived_from");
}

#[test]
fn test_emit_inflection_meta() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("inflmeta.huc");
    std::fs::write(&input, r#"
tagaxis t { role: inflectional }
tagaxis n { role: inflectional }
@extend tv for tagaxis t { a {} }
@extend nv for tagaxis n { x {} }
inflection cls display { en: "My Class" } for {t, n} {
  [t=a, n=x] -> `form`
}
entry e {
  headword: "e"
  inflection_class: cls
  meaning: "e"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    // Check inflection_meta
    let infl_name: String = conn
        .query_row("SELECT name FROM inflection_meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(infl_name, "cls");

    // Check inflection_display
    let display: String = conn
        .query_row("SELECT display_text FROM inflection_display WHERE display_lang = 'en'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(display, "My Class");

    // Check inflection_axes
    let axes: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT axis_name FROM inflection_axes ORDER BY axis_name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(axes, vec!["n", "t"]);
}

#[test]
fn test_emit_stems_stored() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("stems.huc");
    std::fs::write(&input, r#"
tagaxis t { role: inflectional }
@extend tv for tagaxis t { a {} }
inflection cls for {t} {
  requires stems: root, past
  [t=a] -> `{root}`
}
entry w {
  headword: "w"
  stems { root: "wr", past: "wt" }
  inflection_class: cls
  meaning: "w"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let stems: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT stem_name, stem_value FROM stems WHERE entry_id = (SELECT id FROM entries WHERE name = 'w') ORDER BY stem_name")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(stems, vec![
        ("past".to_string(), "wt".to_string()),
        ("root".to_string(), "wr".to_string()),
    ]);
}

#[test]
fn test_emit_render_config() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("rc.huc");
    std::fs::write(&input, r#"
@render {
  separator: "·"
  no_separator_before: "!"
}
entry x { headword: "x" meaning: "x" }
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let sep: String = conn
        .query_row("SELECT value FROM render_config WHERE key = 'separator'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sep, "·");

    let nsb: String = conn
        .query_row("SELECT value FROM render_config WHERE key = 'no_separator_before'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nsb, "!");
}

#[test]
fn test_emit_name_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("nr.huc");
    std::fs::write(&input, r#"
entry hello { headword: "hello" meaning: "greeting" }
entry world { headword: "world" meaning: "planet" }
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let nr_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM name_resolution", [], |r| r.get(0))
        .unwrap();
    assert!(nr_count >= 2, "name_resolution should have entries for 'hello' and 'world'");
}

#[test]
fn test_emit_headword_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.hu");
    let output = dir.path().join("scripts.huc");
    std::fs::write(&input, r#"
entry multi {
  headword { default: "word", alt: "wörd" }
  meaning: "test"
}
"#).unwrap();

    hubullu::compile(&input, &output).unwrap();
    let conn = Connection::open(&output).unwrap();

    let headword: String = conn
        .query_row("SELECT headword FROM entries WHERE name = 'multi'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(headword, "word");

    let scripts: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT script_name, script_value FROM headword_scripts WHERE entry_id = (SELECT id FROM entries WHERE name = 'multi') ORDER BY script_name")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(scripts.len(), 2);
    assert!(scripts.contains(&("default".to_string(), "word".to_string())));
    assert!(scripts.contains(&("alt".to_string(), "wörd".to_string())));
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
