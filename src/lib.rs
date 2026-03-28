//! # hubullu — LexDSL compiler
//!
//! Compiles `.hu` artificial natural language dictionary files into SQLite databases.
//!
//! ## Compilation pipeline
//!
//! 1. **Lex / Parse** — [`lexer`] and [`parser`] produce an [`ast::File`]
//! 2. **Phase 1** ([`phase1`]) — multi-file loading, `@use`/`@reference` resolution,
//!    symbol registration
//! 3. **Phase 2** ([`phase2`]) — `@extend` resolution, inflection validation,
//!    paradigm expansion, DAG check
//! 4. **Emit** ([`emit_sqlite`]) — write entries, forms, links, and FTS5 to SQLite
//!
//! ## Quick start
//!
//! ```no_run
//! # use std::path::Path;
//! hubullu::compile(Path::new("lang.hu"), Path::new("dict.sqlite"))
//!     .expect("compilation failed");
//! ```
//!
//! For tooling (LSP, linters), use [`parse_source`] or [`parse_file`] to obtain
//! the AST without running the full pipeline.

/// Abstract syntax tree types for the LexDSL language.
pub mod ast;
/// Generic DAG cycle detector (Kahn's algorithm).
pub mod dag;
/// SQLite emitter — writes compiled dictionary data to a SQLite database.
#[cfg(feature = "sqlite")]
pub mod emit_sqlite;
/// Diagnostic types for error/warning reporting with source locations.
pub mod error;
/// Inflection paradigm evaluator — cartesian expansion, rule matching, compose, delegation.
pub mod inflection_eval;
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

/// Compile a LexDSL project from the given entry file to SQLite output.
#[cfg(feature = "sqlite")]
pub fn compile(entry_path: &Path, output_path: &Path) -> Result<(), String> {
    // Phase 1: load, parse, register symbols
    let p1 = phase1::run_phase1(entry_path);
    if p1.diagnostics.has_errors() {
        return Err(p1.diagnostics.render_all(&p1.source_map));
    }

    // Phase 2: resolve references, validate, expand
    let p2 = phase2::run_phase2(&p1);
    if p2.diagnostics.has_errors() {
        return Err(p2.diagnostics.render_all(&p1.source_map));
    }

    // Emit SQLite
    if let Err(diag) = emit_sqlite::emit(output_path, &p2) {
        return Err(diag.render(&p1.source_map));
    }

    Ok(())
}
