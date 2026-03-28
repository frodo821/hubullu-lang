//! Disk cache for ProjectState — persist analysis results across LSP restarts.
//!
//! Cache layout:
//!   `<project_root>/.hubullu-cache/project.json`
//!
//! The cache stores the serialized Phase1Result, Phase2Result, and token cache.
//! On load, file modification times are checked; if any source file changed
//! since the cache was written, the cache is invalidated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::phase1::Phase1Result;
use crate::phase2::Phase2Result;
use crate::span::FileId;
use crate::token::Token;

const CACHE_VERSION: u32 = 1;
const CACHE_DIR: &str = ".hubullu-cache";
const CACHE_FILE: &str = "project.json";

/// Serializable cache envelope.
#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    version: u32,
    entry_path: String,
    /// mtime (seconds since epoch) per source file at cache write time.
    file_mtimes: HashMap<String, u64>,
    phase1: Phase1Result,
    phase2: Option<Phase2Result>,
    token_cache: HashMap<FileId, Vec<Token>>,
}

/// Data recovered from disk cache.
pub struct CachedProject {
    pub phase1: Phase1Result,
    pub phase2: Option<Phase2Result>,
    pub token_cache: HashMap<FileId, Vec<Token>>,
}

/// Try to load a cached project. Returns `None` if cache is missing, stale, or corrupt.
pub fn load(entry_path: &Path) -> Option<CachedProject> {
    let cache_path = cache_file_path(entry_path)?;
    let data = std::fs::read_to_string(&cache_path).ok()?;
    let envelope: CacheEnvelope = serde_json::from_str(&data).ok()?;

    if envelope.version != CACHE_VERSION {
        return None;
    }

    let canonical = entry_path.canonicalize().unwrap_or_else(|_| entry_path.to_path_buf());
    if envelope.entry_path != canonical.to_string_lossy() {
        return None;
    }

    // Validate file mtimes.
    for (path_str, &cached_mtime) in &envelope.file_mtimes {
        let path = Path::new(path_str);
        let current_mtime = file_mtime(path)?;
        if current_mtime != cached_mtime {
            return None; // Stale.
        }
    }

    Some(CachedProject {
        phase1: envelope.phase1,
        phase2: envelope.phase2,
        token_cache: envelope.token_cache,
    })
}

/// Write project data to disk cache.
pub fn save(
    entry_path: &Path,
    phase1: &Phase1Result,
    phase2: Option<&Phase2Result>,
    token_cache: &HashMap<FileId, Vec<Token>>,
) {
    let cache_path = match cache_file_path(entry_path) {
        Some(p) => p,
        None => return,
    };

    // Collect mtimes for all source files in the project.
    let mut file_mtimes = HashMap::new();
    for fid in phase1.source_map.file_ids() {
        let path = phase1.source_map.path(fid);
        if let Some(mtime) = file_mtime(path) {
            file_mtimes.insert(path.to_string_lossy().to_string(), mtime);
        }
    }

    let canonical = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.to_path_buf());

    let envelope = CacheEnvelope {
        version: CACHE_VERSION,
        entry_path: canonical.to_string_lossy().to_string(),
        file_mtimes,
        phase1: phase1.clone(),
        phase2: phase2.cloned(),
        token_cache: token_cache.clone(),
    };

    // Ensure cache directory exists.
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Write atomically: write to temp, then rename.
    let tmp_path = cache_path.with_extension("tmp");
    if let Ok(data) = serde_json::to_string(&envelope) {
        if std::fs::write(&tmp_path, data).is_ok() {
            let _ = std::fs::rename(&tmp_path, &cache_path);
        }
    }
}

/// Determine the cache file path for a given entry file.
fn cache_file_path(entry_path: &Path) -> Option<PathBuf> {
    let dir = entry_path.parent()?;
    Some(dir.join(CACHE_DIR).join(CACHE_FILE))
}

/// Get file modification time as seconds since epoch.
fn file_mtime(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs())
}
