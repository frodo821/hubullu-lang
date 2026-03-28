//! Rename handler — rename a symbol across all files.

use std::collections::HashMap;

use lsp_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};

use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::token::{Token, TokenKind};

use super::convert;

/// Check if rename is valid at the given offset, and return the current name + range.
pub fn prepare_rename(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    source_map: &crate::span::SourceMap,
) -> Option<PrepareRenameResponse> {
    let tok = find_ident_at(tokens, file_id, offset)?;
    let range = convert::span_to_range(&tok.span, source_map);
    if let TokenKind::Ident(name) = &tok.node {
        Some(PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: name.clone(),
        })
    } else {
        None
    }
}

/// Perform a rename of the symbol at the given offset.
pub fn rename(
    file_id: FileId,
    offset: usize,
    new_name: &str,
    tokens: &[Token],
    phase1: &Phase1Result,
    token_cache: &HashMap<FileId, Vec<Token>>,
) -> Option<WorkspaceEdit> {
    let tok = find_ident_at(tokens, file_id, offset)?;
    let old_name = match &tok.node {
        TokenKind::Ident(name) => name.clone(),
        _ => return None,
    };

    let scope = phase1.symbol_table.scope(file_id)?;
    let resolved = scope.resolve(&old_name);
    let def = resolved.first()?;

    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();

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

        for ft in file_tokens {
            if let TokenKind::Ident(name) = &ft.node {
                if name == &old_name || name == &def.name {
                    let is_match = if let Some(scope) = file_scope {
                        scope.resolve(name).iter().any(|r| {
                            r.file_id == def.file_id && r.item_index == def.item_index
                        })
                    } else {
                        name == &def.name
                    };

                    if is_match {
                        let range = convert::span_to_range(&ft.span, &phase1.source_map);
                        changes
                            .entry(file_uri.clone())
                            .or_default()
                            .push(TextEdit {
                                range,
                                new_text: new_name.to_string(),
                            });
                    }
                }
            }
        }
    }

    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

fn find_ident_at(tokens: &[Token], file_id: FileId, offset: usize) -> Option<&Token> {
    tokens.iter().find(|t| {
        t.span.file_id == file_id
            && t.span.start <= offset
            && offset < t.span.end
            && matches!(&t.node, TokenKind::Ident(_))
    })
}
