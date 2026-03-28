//! Find references handler.

use std::collections::HashMap;

use lsp_types::Location;

use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::token::{Token, TokenKind};

use super::convert;
use super::hover::{find_extend_value, find_tag_value_axis};

/// Find all references to the symbol at the given byte offset.
pub fn find_references(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    phase1: &Phase1Result,
    token_cache: &HashMap<FileId, Vec<Token>>,
    include_declaration: bool,
) -> Vec<Location> {
    let (tok_idx, target_name) = match find_ident_at(tokens, file_id, offset) {
        Some(r) => r,
        None => return vec![],
    };

    // Check if this is a tag axis value reference.
    if let Some(axis_name) = find_tag_value_axis(tokens, tok_idx) {
        if find_extend_value(Some(&axis_name), &target_name, phase1).is_some() {
            return find_tag_value_references(
                &axis_name, &target_name, phase1, token_cache, include_declaration,
            );
        }
    }

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

    if include_declaration {
        if let Some(uri) = convert::path_to_uri(phase1.source_map.path(def.file_id)) {
            locations.push(Location {
                uri,
                range: convert::span_to_range(&def.span, &phase1.source_map),
            });
        }
    }

    // Scan all files using cached tokens.
    for (&fid, _) in &phase1.files {
        let file_tokens = match token_cache.get(&fid) {
            Some(t) => t,
            None => continue,
        };

        let file_uri = match convert::path_to_uri(phase1.source_map.path(fid)) {
            Some(u) => u,
            None => continue,
        };

        let file_scope = phase1.symbol_table.scope(fid);

        for tok in file_tokens {
            if let TokenKind::Ident(name) = &tok.node {
                if name == &target_name || name == &def.name {
                    let is_match = if let Some(scope) = file_scope {
                        scope.resolve(name).iter().any(|r| {
                            r.file_id == def.file_id && r.item_index == def.item_index
                        })
                    } else {
                        name == &def.name
                    };

                    if is_match {
                        if include_declaration
                            || tok.span.file_id != def.file_id
                            || tok.span.start != def.span.start
                        {
                            let range =
                                convert::span_to_range(&tok.span, &phase1.source_map);
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

fn find_ident_at(tokens: &[Token], file_id: FileId, offset: usize) -> Option<(usize, String)> {
    for (idx, tok) in tokens.iter().enumerate() {
        if tok.span.file_id == file_id && tok.span.start <= offset && offset < tok.span.end {
            if let TokenKind::Ident(name) = &tok.node {
                return Some((idx, name.clone()));
            }
        }
    }
    None
}

/// Find all references to a tag axis value across all files.
fn find_tag_value_references(
    axis_name: &str,
    value_name: &str,
    phase1: &Phase1Result,
    token_cache: &HashMap<FileId, Vec<Token>>,
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();

    // Include the definition site from @extend block.
    if include_declaration {
        if let Some((fid, _ext, val)) = find_extend_value(Some(axis_name), value_name, phase1) {
            if let Some(uri) = convert::path_to_uri(phase1.source_map.path(fid)) {
                locations.push(Location {
                    uri,
                    range: convert::span_to_range(&val.name.span, &phase1.source_map),
                });
            }
        }
    }

    // Scan all files for `axis=value` patterns in token streams.
    for (&fid, _) in &phase1.files {
        let file_tokens = match token_cache.get(&fid) {
            Some(t) => t,
            None => continue,
        };
        let file_uri = match convert::path_to_uri(phase1.source_map.path(fid)) {
            Some(u) => u,
            None => continue,
        };

        for (idx, tok) in file_tokens.iter().enumerate() {
            if let TokenKind::Ident(name) = &tok.node {
                if name == value_name {
                    if let Some(ax) = find_tag_value_axis(file_tokens, idx) {
                        if ax == axis_name {
                            let loc = Location {
                                uri: file_uri.clone(),
                                range: convert::span_to_range(&tok.span, &phase1.source_map),
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

    // Also find uses inside @extend block definitions (the value name itself).
    if !include_declaration {
        // Already handled above via token scan
    }

    locations
}
