//! Go-to-definition handler.

use lsp_types::{GotoDefinitionResponse, Location};

use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::token::{Token, TokenKind};

use super::convert;
use super::hover::{find_extend_value, find_tag_value_axis};

/// Find the definition of the symbol at the given byte offset.
pub fn goto_definition(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    phase1: &Phase1Result,
) -> Option<GotoDefinitionResponse> {
    let (tok_idx, tok) = find_token_at(tokens, file_id, offset)?;

    match &tok.node {
        TokenKind::Ident(name) => {
            // Check tag value context first.
            if let Some(axis_name) = find_tag_value_axis(tokens, tok_idx) {
                if let Some((val_fid, _ext, val)) =
                    find_extend_value(Some(&axis_name), name, phase1)
                {
                    let uri = convert::path_to_uri(phase1.source_map.path(val_fid))?;
                    let range = convert::span_to_range(&val.name.span, &phase1.source_map);
                    return Some(GotoDefinitionResponse::Scalar(Location { uri, range }));
                }
            }
            if let Some(ns_name) = find_namespace(tokens, tok_idx) {
                resolve_qualified(&ns_name, name, file_id, phase1)
            } else {
                resolve_simple(name, file_id, phase1)
            }
        }
        TokenKind::StringLit(_) => resolve_import_path(tok, phase1),
        _ => None,
    }
}

fn find_token_at(tokens: &[Token], file_id: FileId, offset: usize) -> Option<(usize, &Token)> {
    tokens.iter().enumerate().find(|(_, t)| {
        t.span.file_id == file_id && t.span.start <= offset && offset < t.span.end
    })
}

fn find_namespace(tokens: &[Token], idx: usize) -> Option<String> {
    if idx >= 2 {
        if let TokenKind::Dot = &tokens[idx - 1].node {
            if let TokenKind::Ident(ns) = &tokens[idx - 2].node {
                return Some(ns.clone());
            }
        }
    }
    None
}

fn resolve_simple(
    name: &str,
    file_id: FileId,
    phase1: &Phase1Result,
) -> Option<GotoDefinitionResponse> {
    let scope = phase1.symbol_table.scope(file_id)?;
    let results = scope.resolve(name);
    let result = results.first()?;
    let uri = convert::path_to_uri(phase1.source_map.path(result.file_id))?;
    let range = convert::span_to_range(&result.span, &phase1.source_map);
    Some(GotoDefinitionResponse::Scalar(Location { uri, range }))
}

fn resolve_qualified(
    namespace: &str,
    name: &str,
    file_id: FileId,
    phase1: &Phase1Result,
) -> Option<GotoDefinitionResponse> {
    let scope = phase1.symbol_table.scope(file_id)?;
    let results = scope.resolve_qualified(namespace, name);
    let result = results.first()?;
    let uri = convert::path_to_uri(phase1.source_map.path(result.file_id))?;
    let range = convert::span_to_range(&result.span, &phase1.source_map);
    Some(GotoDefinitionResponse::Scalar(Location { uri, range }))
}

fn resolve_import_path(
    tok: &Token,
    phase1: &Phase1Result,
) -> Option<GotoDefinitionResponse> {
    if let TokenKind::StringLit(path_str) = &tok.node {
        // std: imports have no filesystem location to jump to.
        if path_str.contains("://") || path_str.starts_with("std:") {
            return None;
        }
        let suffix = convert::normalize_import_suffix(path_str);
        for fid in phase1.source_map.file_ids() {
            let file_path = phase1.source_map.path(fid);
            if file_path.to_string_lossy().ends_with(&suffix) {
                let uri = convert::path_to_uri(file_path)?;
                let range = lsp_types::Range::default();
                return Some(GotoDefinitionResponse::Scalar(Location { uri, range }));
            }
        }
    }
    None
}
