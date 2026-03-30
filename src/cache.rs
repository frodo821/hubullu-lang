//! Incremental compilation cache.
//!
//! Stores per-file content hashes, a schema fingerprint, and serialized
//! [`ResolvedEntry`](crate::phase2::ResolvedEntry) data in a sidecar SQLite
//! file (`<output>.cache`).  When the schema (axes, inflections, phonrules,
//! render config) has not changed, only entries from modified source files
//! need to be re-expanded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::ast::Item;
use crate::phase1::Phase1Result;
use crate::phase2::ResolvedEntry;
use crate::span::FileId;

/// Handle to the cache database.
pub struct Cache {
    conn: Connection,
}

/// Snapshot of cache state loaded from disk.
#[derive(Default)]
pub struct CacheState {
    pub file_hashes: HashMap<PathBuf, String>,
    pub schema_fingerprint: Option<String>,
    pub cached_entries: HashMap<PathBuf, Vec<ResolvedEntry>>,
}

impl Cache {
    /// Open (or create) the cache database.  Returns `None` on any I/O or
    /// schema error so that the caller can fall back to a full compile.
    pub fn open(path: &Path) -> Option<Self> {
        let conn = Connection::open(path).ok()?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS file_manifest (
                path TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cached_entries (
                source_path TEXT NOT NULL,
                entries_json TEXT NOT NULL
            );
            ",
        )
        .ok()?;
        Some(Self { conn })
    }

    /// Load the full cache state.  Any individual read failure is silently
    /// ignored — the affected portion is simply empty, causing a cache miss.
    pub fn load(&self) -> CacheState {
        let mut state = CacheState::default();

        // file_manifest
        if let Ok(mut stmt) = self.conn.prepare("SELECT path, content_hash FROM file_manifest") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    state.file_hashes.insert(PathBuf::from(row.0), row.1);
                }
            }
        }

        // schema fingerprint
        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = 'schema_fingerprint'")
        {
            if let Ok(val) = stmt.query_row([], |row| row.get::<_, String>(0)) {
                state.schema_fingerprint = Some(val);
            }
        }

        // cached entries
        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT source_path, entries_json FROM cached_entries")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    if let Ok(entries) = serde_json::from_str::<Vec<ResolvedEntry>>(&row.1) {
                        state
                            .cached_entries
                            .insert(PathBuf::from(row.0), entries);
                    }
                }
            }
        }

        state
    }

    /// Persist the current compile state into the cache.
    pub fn save(
        &self,
        file_hashes: &HashMap<PathBuf, String>,
        schema_fingerprint: &str,
        entries_by_file: &HashMap<PathBuf, Vec<ResolvedEntry>>,
    ) -> Result<(), String> {
        self.conn
            .execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;

        // Clear old data
        self.conn
            .execute_batch(
                "DELETE FROM file_manifest; DELETE FROM meta; DELETE FROM cached_entries;",
            )
            .map_err(|e| e.to_string())?;

        // file_manifest
        {
            let mut stmt = self
                .conn
                .prepare("INSERT INTO file_manifest (path, content_hash) VALUES (?1, ?2)")
                .map_err(|e| e.to_string())?;
            for (path, hash) in file_hashes {
                stmt.execute(rusqlite::params![path.to_string_lossy(), hash])
                    .map_err(|e| e.to_string())?;
            }
        }

        // schema fingerprint
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES ('schema_fingerprint', ?1)",
                rusqlite::params![schema_fingerprint],
            )
            .map_err(|e| e.to_string())?;

        // cached entries
        {
            let mut stmt = self
                .conn
                .prepare("INSERT INTO cached_entries (source_path, entries_json) VALUES (?1, ?2)")
                .map_err(|e| e.to_string())?;
            for (path, entries) in entries_by_file {
                let json =
                    serde_json::to_string(entries).map_err(|e| e.to_string())?;
                stmt.execute(rusqlite::params![path.to_string_lossy(), json])
                    .map_err(|e| e.to_string())?;
            }
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fingerprint / hashing helpers
// ---------------------------------------------------------------------------

/// Compute SHA-256 content hash for every source file in the Phase1Result.
pub fn compute_file_hashes(p1: &Phase1Result) -> HashMap<PathBuf, String> {
    let mut result = HashMap::new();
    for &file_id in p1.files.keys() {
        let path = p1.source_map.path(file_id).to_path_buf();
        let source = p1.source_map.source(file_id);
        let mut hasher = Sha256::new();
        hasher.update(source.as_bytes());
        result.insert(path, format!("{:x}", hasher.finalize()));
    }
    result
}

/// Compute a deterministic fingerprint over all "schema" items (everything
/// except entries and import directives).  If this fingerprint changes between
/// compiles, all entries must be re-expanded because their expansion depends
/// on axis values, inflection rules, phonological rules, or render config.
pub fn compute_schema_fingerprint(p1: &Phase1Result) -> String {
    let mut hasher = Sha256::new();

    // Sort by FileId for deterministic ordering.
    let mut file_ids: Vec<FileId> = p1.files.keys().copied().collect();
    file_ids.sort_by_key(|id| id.0);

    for file_id in file_ids {
        let file = &p1.files[&file_id];
        let path = p1.source_map.path(file_id);
        for item in &file.items {
            match &item.node {
                Item::TagAxis(_)
                | Item::Extend(_)
                | Item::Inflection(_)
                | Item::PhonRule(_)
                | Item::Render(_) => {
                    // Include the file path so that moving a definition between
                    // files invalidates the fingerprint.
                    hasher.update(path.to_string_lossy().as_bytes());
                    if let Some(src) =
                        p1.source_map
                            .source_slice(item.span.file_id, item.span.start, item.span.end)
                    {
                        hasher.update(src.as_bytes());
                    }
                }
                Item::Entry(_) | Item::Use(_) | Item::Reference(_) | Item::Export(_) => {}
            }
        }
    }

    format!("{:x}", hasher.finalize())
}
