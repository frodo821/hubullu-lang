//! SQLite emitter — writes compiled dictionary data to a SQLite database.
//!
//! Creates tables for entries, forms, links, tag-axis metadata, and an FTS5
//! virtual table for full-text search.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::Diagnostic;
use crate::phase2::Phase2Result;

/// Write all compiled data to a new SQLite database at `output_path`.
pub fn emit(output_path: &Path, p2: &Phase2Result) -> Result<(), Diagnostic> {
    let conn = Connection::open(output_path).map_err(|e| {
        Diagnostic::error(format!("cannot open output database: {}", e))
    })?;

    create_schema(&conn)?;
    insert_data(&conn, p2)?;
    insert_render_config(&conn, p2)?;
    create_indexes(&conn, p2)?;
    create_fts(&conn)?;

    Ok(())
}

fn create_schema(conn: &Connection) -> Result<(), Diagnostic> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id TEXT NOT NULL UNIQUE,
            headword TEXT NOT NULL,
            meaning TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS entry_tags (
            entry_id TEXT NOT NULL,
            axis TEXT NOT NULL,
            value TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
        );

        CREATE TABLE IF NOT EXISTS entry_meanings (
            entry_id TEXT NOT NULL,
            meaning_id TEXT NOT NULL,
            meaning_text TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
        );

        CREATE TABLE IF NOT EXISTS headword_scripts (
            entry_id TEXT NOT NULL,
            script_name TEXT NOT NULL,
            script_value TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
        );

        CREATE TABLE IF NOT EXISTS forms (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_str TEXT NOT NULL,
            entry_id TEXT NOT NULL,
            tags TEXT NOT NULL,
            part TEXT,
            FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
        );

        CREATE TABLE IF NOT EXISTS links (
            src_entry_id TEXT NOT NULL,
            dst_entry_id TEXT NOT NULL,
            link_type TEXT NOT NULL,
            FOREIGN KEY (src_entry_id) REFERENCES entries(entry_id)
        );

        CREATE TABLE IF NOT EXISTS tagaxis_meta (
            axis_name TEXT NOT NULL,
            value_name TEXT NOT NULL,
            display_lang TEXT NOT NULL,
            display_text TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS render_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| Diagnostic::error(format!("schema creation failed: {}", e)))?;

    Ok(())
}

fn insert_data(conn: &Connection, p2: &Phase2Result) -> Result<(), Diagnostic> {
    // Insert axis metadata
    for (axis_name, axis) in &p2.axes {
        for (value_name, displays) in &axis.display {
            for (lang, text) in displays {
                conn.execute(
                    "INSERT INTO tagaxis_meta (axis_name, value_name, display_lang, display_text) VALUES (?1, ?2, ?3, ?4)",
                    params![axis_name, value_name, lang, text],
                ).map_err(|e| Diagnostic::error(format!("insert tagaxis_meta failed: {}", e)))?;
            }
        }
    }

    // Insert entries
    for entry in &p2.entries {
        conn.execute(
            "INSERT INTO entries (entry_id, headword, meaning) VALUES (?1, ?2, ?3)",
            params![entry.entry_id, entry.headword, entry.meaning],
        )
        .map_err(|e| {
            Diagnostic::error(format!(
                "insert entry '{}' failed: {}",
                entry.entry_id, e
            ))
        })?;

        // Tags
        for (axis, value) in &entry.tags {
            conn.execute(
                "INSERT INTO entry_tags (entry_id, axis, value) VALUES (?1, ?2, ?3)",
                params![entry.entry_id, axis, value],
            )
            .map_err(|e| Diagnostic::error(format!("insert entry_tags failed: {}", e)))?;
        }

        // Meanings
        for (mid, mtext) in &entry.meanings {
            conn.execute(
                "INSERT INTO entry_meanings (entry_id, meaning_id, meaning_text) VALUES (?1, ?2, ?3)",
                params![entry.entry_id, mid, mtext],
            )
            .map_err(|e| Diagnostic::error(format!("insert entry_meanings failed: {}", e)))?;
        }

        // Headword scripts
        for (script, value) in &entry.headword_scripts {
            conn.execute(
                "INSERT INTO headword_scripts (entry_id, script_name, script_value) VALUES (?1, ?2, ?3)",
                params![entry.entry_id, script, value],
            )
            .map_err(|e| Diagnostic::error(format!("insert headword_scripts failed: {}", e)))?;
        }

        // Forms
        for form in &entry.forms {
            let tags_str = form
                .tags
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");

            conn.execute(
                "INSERT INTO forms (form_str, entry_id, tags, part) VALUES (?1, ?2, ?3, ?4)",
                params![form.form_str, entry.entry_id, tags_str, Option::<String>::None],
            )
            .map_err(|e| Diagnostic::error(format!("insert forms failed: {}", e)))?;
        }

        // Links
        for link in &entry.links {
            conn.execute(
                "INSERT INTO links (src_entry_id, dst_entry_id, link_type) VALUES (?1, ?2, ?3)",
                params![entry.entry_id, link.dst_entry_id, link.link_type],
            )
            .map_err(|e| Diagnostic::error(format!("insert links failed: {}", e)))?;
        }
    }

    Ok(())
}

fn insert_render_config(conn: &Connection, p2: &Phase2Result) -> Result<(), Diagnostic> {
    conn.execute(
        "INSERT INTO render_config (key, value) VALUES (?1, ?2)",
        params!["separator", p2.render_config.separator],
    )
    .map_err(|e| Diagnostic::error(format!("insert render_config failed: {}", e)))?;

    conn.execute(
        "INSERT INTO render_config (key, value) VALUES (?1, ?2)",
        params!["no_separator_before", p2.render_config.no_separator_before],
    )
    .map_err(|e| Diagnostic::error(format!("insert render_config failed: {}", e)))?;

    Ok(())
}

fn create_indexes(conn: &Connection, _p2: &Phase2Result) -> Result<(), Diagnostic> {
    // Standard indexes
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_forms_entry ON forms(entry_id);
        CREATE INDEX IF NOT EXISTS idx_forms_form ON forms(form_str);
        CREATE INDEX IF NOT EXISTS idx_links_src ON links(src_entry_id);
        CREATE INDEX IF NOT EXISTS idx_links_dst ON links(dst_entry_id);
        CREATE INDEX IF NOT EXISTS idx_entry_tags ON entry_tags(entry_id);
        CREATE INDEX IF NOT EXISTS idx_entry_tags_axis ON entry_tags(axis, value);
        ",
    )
    .map_err(|e| Diagnostic::error(format!("index creation failed: {}", e)))?;

    Ok(())
}

fn create_fts(conn: &Connection) -> Result<(), Diagnostic> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
            entry_id,
            headword,
            meaning,
            content='entries',
            content_rowid='id'
        );

        INSERT INTO entries_fts(entries_fts) VALUES('rebuild');
        ",
    )
    .map_err(|e| Diagnostic::error(format!("FTS creation failed: {}", e)))?;

    Ok(())
}
