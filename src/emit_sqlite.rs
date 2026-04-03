//! SQLite emitter — writes compiled dictionary data to a `.huc` file.
//!
//! Creates tables for entries, forms, links, tag-axis metadata, inflection
//! metadata, name resolution, and an FTS5 virtual table for full-text search.
//! All entity tables use INTEGER PRIMARY KEY; source-level names are stored
//! in `name` columns but are not used as foreign keys.
//!
//! The `name_resolution` table preserves the per-file symbol scope so that
//! `.hut` renderers can resolve entry names without re-compiling sources.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::Diagnostic;
use crate::phase1::Phase1Result;
use crate::phase2::Phase2Result;
use crate::symbol_table::SymbolKind;

/// Write all compiled data to a new `.huc` file (SQLite format) at `output_path`.
pub fn emit(output_path: &Path, p1: &Phase1Result, p2: &Phase2Result) -> Result<(), Diagnostic> {
    let conn = Connection::open(output_path).map_err(|e| {
        Diagnostic::error(format!("cannot open output file: {}", e))
    })?;

    create_schema(&conn)?;
    insert_data(&conn, p2)?;
    insert_render_config(&conn, p2)?;
    insert_name_resolution(&conn, p1, p2)?;
    create_indexes(&conn, p2)?;
    create_fts(&conn)?;

    Ok(())
}

fn create_schema(conn: &Connection) -> Result<(), Diagnostic> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            headword TEXT NOT NULL,
            meaning TEXT NOT NULL,
            inflection_class_id INTEGER,
            etymology_proto TEXT,
            etymology_note TEXT
        );

        CREATE TABLE IF NOT EXISTS entry_tags (
            entry_id INTEGER NOT NULL,
            axis TEXT NOT NULL,
            value TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS entry_meanings (
            entry_id INTEGER NOT NULL,
            meaning_id TEXT NOT NULL,
            meaning_text TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS headword_scripts (
            entry_id INTEGER NOT NULL,
            script_name TEXT NOT NULL,
            script_value TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS stems (
            entry_id INTEGER NOT NULL,
            stem_name TEXT NOT NULL,
            stem_value TEXT NOT NULL,
            FOREIGN KEY (entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS forms (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_str TEXT NOT NULL,
            entry_id INTEGER NOT NULL,
            tags TEXT NOT NULL,
            part TEXT,
            FOREIGN KEY (entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS links (
            src_entry_id INTEGER NOT NULL,
            dst_entry_id INTEGER NOT NULL,
            link_type TEXT NOT NULL,
            FOREIGN KEY (src_entry_id) REFERENCES entries(id),
            FOREIGN KEY (dst_entry_id) REFERENCES entries(id)
        );

        CREATE TABLE IF NOT EXISTS tagaxis_meta (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            axis_name TEXT NOT NULL,
            value_name TEXT NOT NULL,
            display_lang TEXT NOT NULL,
            display_text TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS inflection_meta (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS inflection_display (
            inflection_id INTEGER NOT NULL,
            display_lang TEXT NOT NULL,
            display_text TEXT NOT NULL,
            FOREIGN KEY (inflection_id) REFERENCES inflection_meta(id)
        );

        CREATE TABLE IF NOT EXISTS inflection_axes (
            inflection_id INTEGER NOT NULL,
            axis_name TEXT NOT NULL,
            FOREIGN KEY (inflection_id) REFERENCES inflection_meta(id)
        );

        CREATE TABLE IF NOT EXISTS render_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS compile_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS name_resolution (
            file_hash TEXT NOT NULL,
            name TEXT NOT NULL,
            entry_id INTEGER NOT NULL,
            PRIMARY KEY (file_hash, name),
            FOREIGN KEY (entry_id) REFERENCES entries(id)
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

    // Insert inflection metadata and build name→id map
    let mut inflection_ids: HashMap<String, i64> = HashMap::new();
    for infl in &p2.inflections {
        conn.execute(
            "INSERT INTO inflection_meta (name) VALUES (?1)",
            params![infl.name],
        )
        .map_err(|e| Diagnostic::error(format!("insert inflection_meta failed: {}", e)))?;
        let infl_id = conn.last_insert_rowid();
        inflection_ids.insert(infl.name.clone(), infl_id);

        for (lang, text) in &infl.display {
            conn.execute(
                "INSERT INTO inflection_display (inflection_id, display_lang, display_text) VALUES (?1, ?2, ?3)",
                params![infl_id, lang, text],
            )
            .map_err(|e| Diagnostic::error(format!("insert inflection_display failed: {}", e)))?;
        }

        for axis_name in &infl.axes {
            conn.execute(
                "INSERT INTO inflection_axes (inflection_id, axis_name) VALUES (?1, ?2)",
                params![infl_id, axis_name],
            )
            .map_err(|e| Diagnostic::error(format!("insert inflection_axes failed: {}", e)))?;
        }
    }

    // Insert entries and build name→id map for link resolution
    let mut entry_ids: HashMap<String, i64> = HashMap::new();
    for entry in &p2.entries {
        let infl_class_id = entry
            .inflection_class
            .as_ref()
            .and_then(|name| inflection_ids.get(name).copied());

        conn.execute(
            "INSERT INTO entries (name, headword, meaning, inflection_class_id, etymology_proto, etymology_note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entry.name, entry.headword, entry.meaning, infl_class_id, entry.etymology_proto, entry.etymology_note],
        )
        .map_err(|e| {
            Diagnostic::error(format!(
                "insert entry '{}' failed: {}",
                entry.name, e
            ))
        })?;
        let entry_id = conn.last_insert_rowid();
        entry_ids.insert(entry.name.clone(), entry_id);

        // Tags
        for (axis, value) in &entry.tags {
            conn.execute(
                "INSERT INTO entry_tags (entry_id, axis, value) VALUES (?1, ?2, ?3)",
                params![entry_id, axis, value],
            )
            .map_err(|e| Diagnostic::error(format!("insert entry_tags failed: {}", e)))?;
        }

        // Meanings
        for (mid, mtext) in &entry.meanings {
            conn.execute(
                "INSERT INTO entry_meanings (entry_id, meaning_id, meaning_text) VALUES (?1, ?2, ?3)",
                params![entry_id, mid, mtext],
            )
            .map_err(|e| Diagnostic::error(format!("insert entry_meanings failed: {}", e)))?;
        }

        // Headword scripts
        for (script, value) in &entry.headword_scripts {
            conn.execute(
                "INSERT INTO headword_scripts (entry_id, script_name, script_value) VALUES (?1, ?2, ?3)",
                params![entry_id, script, value],
            )
            .map_err(|e| Diagnostic::error(format!("insert headword_scripts failed: {}", e)))?;
        }

        // Stems
        for (stem_name, stem_value) in &entry.stems {
            conn.execute(
                "INSERT INTO stems (entry_id, stem_name, stem_value) VALUES (?1, ?2, ?3)",
                params![entry_id, stem_name, stem_value],
            )
            .map_err(|e| Diagnostic::error(format!("insert stems failed: {}", e)))?;
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
                params![form.form_str, entry_id, tags_str, Option::<String>::None],
            )
            .map_err(|e| Diagnostic::error(format!("insert forms failed: {}", e)))?;
        }
    }

    // Links (second pass — all entry IDs are now known)
    for entry in &p2.entries {
        let src_id = entry_ids[&entry.name];
        for link in &entry.links {
            if let Some(&dst_id) = entry_ids.get(&link.dst_entry_id) {
                conn.execute(
                    "INSERT INTO links (src_entry_id, dst_entry_id, link_type) VALUES (?1, ?2, ?3)",
                    params![src_id, dst_id, link.link_type],
                )
                .map_err(|e| Diagnostic::error(format!("insert links failed: {}", e)))?;
            }
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

fn insert_name_resolution(
    conn: &Connection,
    p1: &Phase1Result,
    _p2: &Phase2Result,
) -> Result<(), Diagnostic> {
    use sha2::{Digest, Sha256};

    // Build entry name → DB id map (same names used during insert_data)
    let entry_ids: HashMap<String, i64> = {
        let mut stmt = conn
            .prepare("SELECT id, name FROM entries")
            .map_err(|e| Diagnostic::error(format!("query entries failed: {}", e)))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?)))
            .map_err(|e| Diagnostic::error(format!("query entries failed: {}", e)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (name, id) =
                row.map_err(|e| Diagnostic::error(format!("read entry row failed: {}", e)))?;
            map.insert(name, id);
        }
        map
    };

    // Determine entry point directory for relative path computation.
    // The first file added to the source map is the entry point.
    let entry_point_dir = {
        let entry_file_id = crate::span::FileId(0);
        let entry_path = p1.source_map.path(entry_file_id);
        entry_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    };

    conn.execute(
        "INSERT INTO compile_meta (key, value) VALUES (?1, ?2)",
        params!["entry_point_dir", entry_point_dir.to_string_lossy()],
    )
    .map_err(|e| Diagnostic::error(format!("insert compile_meta failed: {}", e)))?;

    // For each file in the symbol table, record all Entry-kind names in scope.
    let mut stmt = conn
        .prepare("INSERT OR IGNORE INTO name_resolution (file_hash, name, entry_id) VALUES (?1, ?2, ?3)")
        .map_err(|e| Diagnostic::error(format!("prepare name_resolution insert failed: {}", e)))?;

    for (&file_id, scope) in &p1.symbol_table.scopes {
        let file_path = p1.source_map.path(file_id);
        let rel_path = file_path
            .strip_prefix(&entry_point_dir)
            .unwrap_or(file_path);
        let file_hash = {
            let mut hasher = Sha256::new();
            hasher.update(rel_path.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };

        // Collect Entry symbols from locals
        for sym in scope.locals.values() {
            if sym.kind == SymbolKind::Entry {
                if let Some(&eid) = entry_ids.get(&sym.name) {
                    stmt.execute(params![file_hash, sym.name, eid]).map_err(|e| {
                        Diagnostic::error(format!("insert name_resolution failed: {}", e))
                    })?;
                }
            }
        }

        // Collect Entry symbols from imports (excluding namespaced — they are ephemeral)
        for imp in &scope.imports {
            if imp.kind == SymbolKind::Entry && imp.namespace.is_none() {
                if let Some(&eid) = entry_ids.get(&imp.original_name) {
                    stmt.execute(params![file_hash, imp.local_name, eid])
                        .map_err(|e| {
                            Diagnostic::error(format!("insert name_resolution failed: {}", e))
                        })?;
                }
            }
        }
    }

    Ok(())
}

fn create_indexes(conn: &Connection, _p2: &Phase2Result) -> Result<(), Diagnostic> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_entries_name ON entries(name);
        CREATE INDEX IF NOT EXISTS idx_forms_entry ON forms(entry_id);
        CREATE INDEX IF NOT EXISTS idx_forms_form ON forms(form_str);
        CREATE INDEX IF NOT EXISTS idx_links_src ON links(src_entry_id);
        CREATE INDEX IF NOT EXISTS idx_links_dst ON links(dst_entry_id);
        CREATE INDEX IF NOT EXISTS idx_stems_entry ON stems(entry_id);
        CREATE INDEX IF NOT EXISTS idx_entry_tags ON entry_tags(entry_id);
        CREATE INDEX IF NOT EXISTS idx_entry_tags_axis ON entry_tags(axis, value);
        CREATE INDEX IF NOT EXISTS idx_inflection_display ON inflection_display(inflection_id);
        CREATE INDEX IF NOT EXISTS idx_inflection_axes ON inflection_axes(inflection_id);
        CREATE INDEX IF NOT EXISTS idx_name_resolution_hash ON name_resolution(file_hash);
        ",
    )
    .map_err(|e| Diagnostic::error(format!("index creation failed: {}", e)))?;

    Ok(())
}

fn create_fts(conn: &Connection) -> Result<(), Diagnostic> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
            name,
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
