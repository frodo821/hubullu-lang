#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use rusqlite::Connection;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn test_simple_compile() {
    let input = fixture_path("simple/main.hu");
    let output = std::env::temp_dir().join("hubullu_test_simple.sqlite");

    // Clean up any previous output
    let _ = std::fs::remove_file(&output);

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
            "SELECT headword FROM entries WHERE entry_id = 'faren'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(headword, "faren");

    // Check forms for faren
    let form_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = 'faren'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form_count, 4, "expected 4 forms for faren (2 tenses x 2 numbers)");

    // Check specific form
    let form: String = conn
        .query_row(
            "SELECT form_str FROM forms WHERE entry_id = 'faren' AND tags LIKE '%tense=present%' AND tags LIKE '%number=sg%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form, "fars");

    // Check no forms for hus (no inflection)
    let hus_forms: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM forms WHERE entry_id = 'hus'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hus_forms, 0);

    // Check tags
    let tag_value: String = conn
        .query_row(
            "SELECT value FROM entry_tags WHERE entry_id = 'faren' AND axis = 'parts_of_speech'",
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

    // Clean up
    let _ = std::fs::remove_file(&output);
}

#[test]
fn test_inline_inflection() {
    let input = fixture_path("inline/main.hu");
    let output = std::env::temp_dir().join("hubullu_test_inline.sqlite");
    let _ = std::fs::remove_file(&output);

    // Create fixture
    let fixture_dir = fixture_path("inline");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::write(
        fixture_dir.join("main.hu"),
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
            "SELECT COUNT(*) FROM forms WHERE entry_id = 'sein'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(form_count, 2);

    let _ = std::fs::remove_file(&output);
}
