//! # hubullu — Hubullu compiler
//!
//! Compiles `.hu` artificial natural language dictionary files into `.huc` files.
//!
//! ## Compilation pipeline
//!
//! 1. **Lex / Parse** — [`lexer`] and [`parser`] produce an [`ast::File`]
//! 2. **Phase 1** ([`phase1`]) — multi-file loading, `@use`/`@reference` resolution,
//!    symbol registration
//! 3. **Phase 2** ([`phase2`]) — `@extend` resolution, inflection validation,
//!    paradigm expansion, DAG check
//! 4. **Emit** ([`emit_sqlite`]) — write entries, forms, links, name resolution,
//!    and FTS5 to a `.huc` file (SQLite format)
//!
//! ## Quick start
//!
//! ```no_run
//! # use std::path::Path;
//! hubullu::compile(Path::new("lang.hu"), Path::new("dict.huc"))
//!     .expect("compilation failed");
//! ```
//!
//! For tooling (LSP, linters), use [`parse_source`] or [`parse_file`] to obtain
//! the AST without running the full pipeline.

/// Abstract syntax tree types for the Hubullu language.
pub mod ast;
/// Generic DAG cycle detector (Kahn's algorithm).
pub mod dag;
/// Incremental compilation cache (file hashing, schema fingerprinting).
#[cfg(feature = "sqlite")]
pub(crate) mod cache;
/// Merkle-tree AST hashing for fine-grained incremental compilation.
#[cfg(feature = "sqlite")]
pub(crate) mod merkle;
/// `.huc` emitter — writes compiled dictionary data to a `.huc` file (SQLite format).
#[cfg(feature = "sqlite")]
pub mod emit_sqlite;
/// `.hut` file rendering — resolves token lists against a compiled `.huc` file.
#[cfg(feature = "sqlite")]
pub mod render;
/// Static HTML site generation from `.hut` files.
#[cfg(feature = "sqlite")]
pub mod render_html;
/// Diagnostic types for error/warning reporting with source locations.
pub mod error;
/// Inflection paradigm evaluator — cartesian expansion, rule matching, compose, delegation.
pub mod inflection_eval;
/// Linter for `.hu` files — warnings and style checks with optional auto-fix.
pub mod lint;
/// Hand-written lexer (scanner) for the Hubullu language.
pub mod lexer;
/// Recursive descent parser — tokens to [`ast::File`].
pub mod parser;
/// Phonological rule evaluation engine.
pub mod phonrule_eval;
/// Phase 1: file loading, `@use`/`@reference` resolution, symbol registration.
pub mod phase1;
/// Phase 2: `@extend` resolution, inflection validation, entry expansion.
pub mod phase2;
/// Source map for multi-file span resolution (FileId → source text, line/col).
pub mod span;
/// Built-in standard library modules for `std:` imports.
pub mod stdlib;
/// Symbol table with per-file scopes and name resolution.
pub mod symbol_table;
/// Token types produced by the [`lexer`].
pub mod token;
/// AST visitor trait with default walk functions.
pub mod visit;
/// Language Server Protocol implementation.
#[cfg(feature = "lsp")]
pub mod lsp;
/// Claude Code skill installer.
#[cfg(feature = "cli")]
pub mod skill;

use std::path::Path;

use crate::error::Diagnostic;
use crate::span::{FileId, SourceMap};
use crate::token::Token;

// ---------------------------------------------------------------------------
// Convenience parse API
// ---------------------------------------------------------------------------

/// Result of parsing a single source string.
pub struct ParseResult {
    pub file: ast::File,
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    pub source_map: SourceMap,
    pub file_id: FileId,
}

impl ParseResult {
    /// Returns `true` if any diagnostic has error severity.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == error::Severity::Error)
    }
}

/// Parse a source string. This is the primary library entry point for
/// tools like LSP servers and refactoring engines.
///
/// ```
/// let result = hubullu::parse_source("entry foo { headword: \"foo\" ... }", "test.hu");
/// if !result.has_errors() {
///     // walk result.file ...
/// }
/// ```
pub fn parse_source(source: &str, filename: &str) -> ParseResult {
    let mut source_map = SourceMap::new();
    let file_id = source_map.add_file(filename.into(), source.to_string());

    let lexer = lexer::Lexer::new(source_map.source(file_id), file_id);
    let (tokens, lex_errors) = lexer.tokenize();

    let parser = parser::Parser::new(tokens.clone(), file_id);
    let (file, parse_errors) = parser.parse();

    let mut diagnostics = lex_errors;
    diagnostics.extend(parse_errors);

    ParseResult {
        file,
        tokens,
        diagnostics,
        source_map,
        file_id,
    }
}

/// Parse a file from disk.
pub fn parse_file(path: &Path) -> Result<ParseResult, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
    let filename = path.to_string_lossy().to_string();
    Ok(parse_source(&source, &filename))
}

/// Lex a source string into tokens (useful for syntax highlighting).
pub fn lex_source(source: &str, filename: &str) -> (Vec<Token>, Vec<Diagnostic>, SourceMap, FileId) {
    let mut source_map = SourceMap::new();
    let file_id = source_map.add_file(filename.into(), source.to_string());

    let lexer = lexer::Lexer::new(source_map.source(file_id), file_id);
    let (tokens, errors) = lexer.tokenize();

    (tokens, errors, source_map, file_id)
}

// ---------------------------------------------------------------------------
// Full compilation (requires "sqlite" feature)
// ---------------------------------------------------------------------------

/// Compile a Hubullu project from the given entry file to a `.huc` file.
///
/// Automatically uses incremental compilation when a valid cache exists
/// alongside the output file.  To force a full rebuild, delete the
/// `<output>.cache` file.
#[cfg(feature = "sqlite")]
pub fn compile(entry_path: &Path, output_path: &Path) -> Result<(), String> {
    use std::collections::HashSet;

    log::info!("compiling {} → {}", entry_path.display(), output_path.display());

    // Load cache (before phase1, so AST cache can be used)
    let cache_path = cache_path_for(output_path);
    let cache = cache::Cache::open(&cache_path);
    let cache_state = cache.as_ref().map(|c| c.load()).unwrap_or_default();
    let cache::CacheState { entries: cached_entry_state, asts: cached_asts } = cache_state;

    // Phase 1: load and parse files (using AST cache when possible)
    log::info!("phase1: loading and parsing files");
    let p1 = phase1::run_phase1(entry_path, cached_asts);
    log::info!("phase1: loaded {} file(s)", p1.files.len());
    if p1.diagnostics.has_errors() {
        return Err(p1.diagnostics.render_all(&p1.source_map));
    }

    // Compute Merkle hashes for all items
    log::debug!("computing Merkle hashes");
    let merkle = merkle::compute(&p1);

    // Diff: find entries whose Merkle hash changed (or are new)
    let mut entries_to_resolve: HashSet<(std::path::PathBuf, String)> = HashSet::new();
    let mut cached_entries: Vec<phase2::ResolvedEntry> = Vec::new();

    for (key, new_hash) in &merkle.entries {
        if let Some((old_hash, resolved)) = cached_entry_state.get(key) {
            if old_hash == new_hash {
                log::trace!("cache hit: {}:{}", key.0.display(), key.1);
                cached_entries.push(resolved.clone());
                continue;
            }
        }
        log::trace!("cache miss: {}:{}", key.0.display(), key.1);
        entries_to_resolve.insert(key.clone());
    }
    log::debug!("cache: {} hit(s), {} miss(es)", cached_entries.len(), entries_to_resolve.len());

    // Phase 2: full if no cache hits at all, incremental otherwise
    let p2 = if cached_entries.is_empty() {
        log::info!("phase2: full resolution");
        phase2::run_phase2(&p1)
    } else {
        log::info!("phase2: incremental resolution ({} to resolve)", entries_to_resolve.len());
        phase2::run_phase2_incremental(&p1, &entries_to_resolve, cached_entries)
    };

    log::info!("phase2: resolved {} entry/entries", p2.entries.len());

    if p2.diagnostics.has_errors() {
        return Err(p2.diagnostics.render_all(&p1.source_map));
    }

    // Compute output fingerprint from all Merkle entry hashes to detect no-op builds.
    let output_fingerprint = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let mut sorted_keys: Vec<_> = merkle.entries.keys().collect();
        sorted_keys.sort();
        for key in sorted_keys {
            hasher.update(merkle.entries[key]);
        }
        let result: [u8; 32] = hasher.finalize().into();
        result
    };

    // Check if emit can be skipped (output unchanged and .huc file exists)
    let prev_fingerprint = cache.as_ref().and_then(|c| c.load_meta("output_fingerprint"));
    let emit_needed = prev_fingerprint.as_deref() != Some(&output_fingerprint[..])
        || !output_path.exists();

    if emit_needed {
        log::info!("emit: writing {}", output_path.display());
        let _ = std::fs::remove_file(output_path);
        if let Err(diag) = emit_sqlite::emit(output_path, &p1, &p2) {
            return Err(diag.render(&p1.source_map));
        }
    } else {
        log::info!("emit: skipped (output unchanged)");
    }

    // Update cache (failure is non-fatal)
    let cache_data: Vec<(std::path::PathBuf, String, [u8; 32], phase2::ResolvedEntry)> = p2
        .entries
        .iter()
        .filter_map(|entry| {
            let key = (entry.source_file.clone(), entry.name.clone());
            merkle.entries.get(&key).map(|hash| {
                (entry.source_file.clone(), entry.name.clone(), *hash, entry.clone())
            })
        })
        .collect();

    // Collect AST data for cache persistence
    let ast_data: Vec<(std::path::PathBuf, [u8; 32], span::FileId, ast::File)> = p1
        .path_to_id
        .iter()
        .filter_map(|(path, &file_id)| {
            let hash = p1.content_hashes.get(path)?;
            let file = p1.files.get(&file_id)?;
            Some((path.clone(), *hash, file_id, file.clone()))
        })
        .collect();

    if let Some(c) = cache.or_else(|| cache::Cache::open(&cache_path)) {
        let _ = c.save(&cache_data, &ast_data);
        let _ = c.save_meta("output_fingerprint", &output_fingerprint);
    }

    log::info!("done");

    Ok(())
}

/// Derive the cache file path from the output path.
///
/// Places the cache in `.hubullu-cache/` next to the output file:
/// e.g. `/path/to/dict.huc` → `/path/to/.hubullu-cache/dict.huc.cache`
#[cfg(feature = "sqlite")]
fn cache_path_for(output_path: &Path) -> std::path::PathBuf {
    let dir = output_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(".hubullu-cache");
    let _ = std::fs::create_dir_all(&dir);
    let mut name = output_path
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    name.push(".cache");
    dir.join(name)
}
