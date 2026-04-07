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
const SCHEMA_VERSION: &str = "4";

/// Handle to the cache database.
pub struct Cache {
    conn: Connection,
}

/// Snapshot of cache state loaded from disk.
#[derive(Default)]
pub struct CacheState {
    /// `(source_path, entry_name) → (merkle_hash, ResolvedEntry)`
    pub entries: HashMap<(PathBuf, String), ([u8; 32], ResolvedEntry)>,
    /// `file_path → (content_hash, cached_file_id, ast::File)`
    pub asts: HashMap<PathBuf, ([u8; 32], crate::span::FileId, crate::ast::File)>,
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
                 DROP TABLE IF EXISTS entry_cache;
                 DROP TABLE IF EXISTS ast_cache;",
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
                     entry_blob BLOB NOT NULL,
                     PRIMARY KEY (source_path, entry_name)
                 );
                 CREATE TABLE ast_cache (
                     file_path TEXT PRIMARY KEY,
                     content_hash BLOB NOT NULL,
                     file_id INTEGER NOT NULL,
                     ast_blob BLOB NOT NULL
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
            .prepare("SELECT source_path, entry_name, merkle_hash, entry_blob FROM entry_cache")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (path_str, name, hash_bytes, blob) = row;
                    if hash_bytes.len() != 32 {
                        continue;
                    }
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&hash_bytes);
                    if let Ok(entry) = bincode::deserialize::<ResolvedEntry>(&blob) {
                        state
                            .entries
                            .insert((PathBuf::from(path_str), name), (hash, entry));
                    }
                }
            }
        }

        // Load cached ASTs
        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT file_path, content_hash, file_id, ast_blob FROM ast_cache")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (path_str, hash_bytes, fid, blob) = row;
                    if hash_bytes.len() != 32 {
                        continue;
                    }
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&hash_bytes);
                    if let Ok(file) = bincode::deserialize::<crate::ast::File>(&blob) {
                        state.asts.insert(
                            PathBuf::from(path_str),
                            (hash, crate::span::FileId(fid), file),
                        );
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
        asts: &[(PathBuf, [u8; 32], crate::span::FileId, crate::ast::File)],
    ) -> Result<(), String> {
        self.conn
            .execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;

        self.conn
            .execute("DELETE FROM entry_cache", [])
            .map_err(|e| e.to_string())?;
        self.conn
            .execute("DELETE FROM ast_cache", [])
            .map_err(|e| e.to_string())?;

        {
            let mut stmt = self
                .conn
                .prepare(
                    "INSERT INTO entry_cache (source_path, entry_name, merkle_hash, entry_blob) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;

            for (path, name, hash, entry) in entries {
                let blob = bincode::serialize(entry).map_err(|e| e.to_string())?;
                stmt.execute(rusqlite::params![
                    path.to_string_lossy(),
                    name,
                    hash.as_slice(),
                    blob,
                ])
                .map_err(|e| e.to_string())?;
            }
        }

        {
            let mut stmt = self
                .conn
                .prepare(
                    "INSERT INTO ast_cache (file_path, content_hash, file_id, ast_blob) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;

            for (path, hash, fid, file) in asts {
                let blob = bincode::serialize(file).map_err(|e| e.to_string())?;
                stmt.execute(rusqlite::params![
                    path.to_string_lossy(),
                    hash.as_slice(),
                    fid.0,
                    blob,
                ])
                .map_err(|e| e.to_string())?;
            }
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Load a binary value from the meta table (stored as hex string).
    pub fn load_meta(&self, key: &str) -> Option<Vec<u8>> {
        let hex_str: String = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = ?1")
            .ok()?
            .query_row(rusqlite::params![key], |row| row.get(0))
            .ok()?;
        // Decode hex string to bytes
        let mut bytes = Vec::with_capacity(hex_str.len() / 2);
        let chars: Vec<u8> = hex_str.bytes().collect();
        for chunk in chars.chunks_exact(2) {
            let high = Self::hex_val(chunk[0])?;
            let low = Self::hex_val(chunk[1])?;
            bytes.push((high << 4) | low);
        }
        Some(bytes)
    }

    /// Store a binary value in the meta table (as hex string).
    pub fn save_meta(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let hex: String = value.iter().map(|b| format!("{:02x}", b)).collect();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, hex],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn hex_val(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
}
