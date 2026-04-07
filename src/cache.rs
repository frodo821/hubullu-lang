//! Incremental compilation cache.
//!
//! Stores per-entry Merkle hashes and serialized [`ResolvedEntry`] data in a
//! sidecar SQLite file (`<output>.cache`).  When an entry's Merkle hash has
//! not changed since the last compile, its cached [`ResolvedEntry`] is reused
//! without re-expansion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::phase2::ResolvedEntry;

/// Current cache schema version.  Bump this when the table layout changes to
/// force a cache rebuild on upgrade.
const SCHEMA_VERSION: &str = "2";

/// Handle to the cache database.
pub struct Cache {
    conn: Connection,
}

/// Snapshot of cache state loaded from disk.
#[derive(Default)]
pub struct CacheState {
    /// `(source_path, entry_name) → (merkle_hash, ResolvedEntry)`
    pub entries: HashMap<(PathBuf, String), ([u8; 32], ResolvedEntry)>,
}

impl Cache {
    /// Open (or create) the cache database.  Returns `None` on any I/O or
    /// schema error so that the caller can fall back to a full compile.
    pub fn open(path: &Path) -> Option<Self> {
        let conn = Connection::open(path).ok()?;

        // Check schema version; if mismatched, recreate.
        let current_version: Option<String> = conn
            .prepare("SELECT value FROM meta WHERE key = 'schema_version'")
            .ok()
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).ok());

        if current_version.as_deref() != Some(SCHEMA_VERSION) {
            // Drop old tables (ignore errors — they may not exist)
            conn.execute_batch(
                "DROP TABLE IF EXISTS file_manifest;
                 DROP TABLE IF EXISTS meta;
                 DROP TABLE IF EXISTS cached_entries;
                 DROP TABLE IF EXISTS entry_cache;",
            )
            .ok()?;

            conn.execute_batch(
                "CREATE TABLE meta (
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 CREATE TABLE entry_cache (
                     source_path TEXT NOT NULL,
                     entry_name TEXT NOT NULL,
                     merkle_hash BLOB NOT NULL,
                     entry_json TEXT NOT NULL,
                     PRIMARY KEY (source_path, entry_name)
                 );",
            )
            .ok()?;

            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                rusqlite::params![SCHEMA_VERSION],
            )
            .ok()?;
        }

        Some(Self { conn })
    }

    /// Load the full cache state.  Any individual read failure is silently
    /// ignored — the affected portion is simply empty, causing a cache miss.
    pub fn load(&self) -> CacheState {
        let mut state = CacheState::default();

        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT source_path, entry_name, merkle_hash, entry_json FROM entry_cache")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (path_str, name, hash_bytes, json) = row;
                    if hash_bytes.len() != 32 {
                        continue;
                    }
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&hash_bytes);
                    if let Ok(entry) = serde_json::from_str::<ResolvedEntry>(&json) {
                        state
                            .entries
                            .insert((PathBuf::from(path_str), name), (hash, entry));
                    }
                }
            }
        }

        state
    }

    /// Persist the current compile state into the cache.
    pub fn save(
        &self,
        entries: &[(PathBuf, String, [u8; 32], ResolvedEntry)],
    ) -> Result<(), String> {
        self.conn
            .execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;

        self.conn
            .execute("DELETE FROM entry_cache", [])
            .map_err(|e| e.to_string())?;

        {
            let mut stmt = self
                .conn
                .prepare(
                    "INSERT INTO entry_cache (source_path, entry_name, merkle_hash, entry_json) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;

            for (path, name, hash, entry) in entries {
                let json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
                stmt.execute(rusqlite::params![
                    path.to_string_lossy(),
                    name,
                    hash.as_slice(),
                    json,
                ])
                .map_err(|e| e.to_string())?;
            }
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
