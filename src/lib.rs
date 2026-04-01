//! # hubullu — LexDSL compiler
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

/// Abstract syntax tree types for the LexDSL language.
pub mod ast;
/// Generic DAG cycle detector (Kahn's algorithm).
pub mod dag;
/// Incremental compilation cache (file hashing, schema fingerprinting).
#[cfg(feature = "sqlite")]
pub(crate) mod cache;
/// `.huc` emitter — writes compiled dictionary data to a `.huc` file (SQLite format).
#[cfg(feature = "sqlite")]
pub mod emit_sqlite;
/// `.hut` file rendering — resolves token lists against a compiled `.huc` file.
#[cfg(feature = "sqlite")]
pub mod render;
/// Diagnostic types for error/warning reporting with source locations.
pub mod error;
/// Inflection paradigm evaluator — cartesian expansion, rule matching, compose, delegation.
pub mod inflection_eval;
/// Linter for `.hu` files — warnings and style checks with optional auto-fix.
pub mod lint;
/// Hand-written lexer (scanner) for the LexDSL language.
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

/// Compile a LexDSL project from the given entry file to a `.huc` file.
///
/// Automatically uses incremental compilation when a valid cache exists
/// alongside the output file.  To force a full rebuild, delete the
/// `<output>.cache` file.
#[cfg(feature = "sqlite")]
pub fn compile(entry_path: &Path, output_path: &Path) -> Result<(), String> {
    use std::collections::{HashMap, HashSet};

    // Phase 1: always run fully (parsing is fast)
    let p1 = phase1::run_phase1(entry_path);
    if p1.diagnostics.has_errors() {
        return Err(p1.diagnostics.render_all(&p1.source_map));
    }

    // Load cache
    let cache_path = cache_path_for(output_path);
    let cache = cache::Cache::open(&cache_path);
    let cache_state = cache.as_ref().map(|c| c.load()).unwrap_or_default();

    // Compute current state
    let current_hashes = cache::compute_file_hashes(&p1);
    let current_fingerprint = cache::compute_schema_fingerprint(&p1);

    let schema_changed =
        cache_state.schema_fingerprint.as_deref() != Some(&current_fingerprint);

    // Phase 2: full or incremental
    let p2 = if schema_changed {
        phase2::run_phase2(&p1)
    } else {
        // Identify changed files
        let changed_paths: HashSet<&std::path::Path> = current_hashes
            .iter()
            .filter(|(path, hash)| {
                cache_state
                    .file_hashes
                    .get(*path)
                    .map(|h| h.as_str())
                    != Some(hash.as_str())
            })
            .map(|(path, _)| path.as_path())
            .collect();

        let changed_file_ids: HashSet<span::FileId> = changed_paths
            .iter()
            .filter_map(|path| p1.path_to_id.get(*path).copied())
            .collect();

        // Collect cached entries from unchanged, still-existing files
        let cached_entries: Vec<phase2::ResolvedEntry> = cache_state
            .cached_entries
            .iter()
            .filter(|(path, _)| {
                !changed_paths.contains(path.as_path()) && current_hashes.contains_key(path.as_path())
            })
            .flat_map(|(_, entries)| entries.clone())
            .collect();

        phase2::run_phase2_incremental(&p1, &changed_file_ids, cached_entries)
    };

    if p2.diagnostics.has_errors() {
        return Err(p2.diagnostics.render_all(&p1.source_map));
    }

    // Emit .huc file (always full rebuild — remove old file first)
    let _ = std::fs::remove_file(output_path);
    if let Err(diag) = emit_sqlite::emit(output_path, &p1, &p2) {
        return Err(diag.render(&p1.source_map));
    }

    // Update cache (failure is non-fatal)
    let entries_by_file: HashMap<std::path::PathBuf, Vec<phase2::ResolvedEntry>> = {
        let mut map: HashMap<std::path::PathBuf, Vec<phase2::ResolvedEntry>> = HashMap::new();
        for entry in &p2.entries {
            map.entry(entry.source_file.clone())
                .or_default()
                .push(entry.clone());
        }
        map
    };

    if let Some(c) = cache.or_else(|| cache::Cache::open(&cache_path)) {
        let _ = c.save(&current_hashes, &current_fingerprint, &entries_by_file);
    }

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
