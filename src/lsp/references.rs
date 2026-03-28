//! Find references handler.

use lsp_types::Location;

use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::token::{Token, TokenKind};

use super::convert;

/// Find all references to the symbol at the given byte offset.
pub fn find_references(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    phase1: &Phase1Result,
    include_declaration: bool,
) -> Vec<Location> {
    // First, resolve the symbol name at cursor.
    let target_name = match find_ident_at(tokens, file_id, offset) {
        Some(name) => name,
        None => return vec![],
    };

    // Resolve to get the canonical definition.
    let scope = match phase1.symbol_table.scope(file_id) {
        Some(s) => s,
        None => return vec![],
    };
    let resolved = scope.resolve(&target_name);
    let def = match resolved.first() {
        Some(d) => d,
        None => return vec![],
    };

    let mut locations = Vec::new();

    // Optionally include the declaration itself.
    if include_declaration {
        if let Some(uri) = convert::path_to_uri(phase1.source_map.path(def.file_id)) {
            locations.push(Location {
                uri,
                range: convert::span_to_range(&def.span, &phase1.source_map),
            });
        }
    }

    // Scan all files' tokens for matching identifiers that resolve to the same symbol.
    for (&fid, _file_ast) in &phase1.files {
        let file_uri = match convert::path_to_uri(phase1.source_map.path(fid)) {
            Some(u) => u,
            None => continue,
        };

        // Re-lex this file to get its token stream.
        let source = phase1.source_map.source(fid);
        let lexer = crate::lexer::Lexer::new(source, fid);
        let (file_tokens, _) = lexer.tokenize();

        let file_scope = phase1.symbol_table.scope(fid);

        for tok in &file_tokens {
            if let TokenKind::Ident(name) = &tok.node {
                if name == &target_name || name == &def.name {
                    // Check if this ident resolves to the same definition.
                    let is_match = if let Some(scope) = file_scope {
                        scope.resolve(name).iter().any(|r| {
                            r.file_id == def.file_id && r.item_index == def.item_index
                        })
                    } else {
                        // No scope info — match by name.
                        name == &def.name
                    };

                    if is_match {
                        // Skip the declaration span itself (already added above).
                        if include_declaration
                            || tok.span.file_id != def.file_id
                            || tok.span.start != def.span.start
                        {
                            let range =
                                convert::span_to_range(&tok.span, &phase1.source_map);
                            // Avoid duplicate for declaration.
                            let loc = Location {
                                uri: file_uri.clone(),
                                range,
                            };
                            if !locations.contains(&loc) {
                                locations.push(loc);
                            }
                        }
                    }
                }
            }
        }
    }

    locations
}

fn find_ident_at(tokens: &[Token], file_id: FileId, offset: usize) -> Option<String> {
    for tok in tokens {
        if tok.span.file_id == file_id && tok.span.start <= offset && offset < tok.span.end {
            if let TokenKind::Ident(name) = &tok.node {
                return Some(name.clone());
            }
        }
    }
    None
}
