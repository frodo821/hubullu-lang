//! Surface forms — return entry-ref ranges and their resolved forms for overlay display.

use serde::Serialize;

use crate::ast::{self, EntryRef, Item};
use crate::phase2::Phase2Result;
use crate::span::{FileId, SourceMap};
use crate::token::{Token, TokenKind};

use super::convert;
use super::inlay_hints::{find_matching_form, find_resolved_entry, collect_tilde_chain_parts};

#[derive(Serialize)]
pub struct SurfaceFormsResult {
    pub items: Vec<SurfaceFormItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceFormItem {
    pub range: lsp_types::Range,
    pub surface_form: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

/// Generate surface form items for a `.hu` file from its AST.
pub fn surface_forms(
    file_id: FileId,
    file_ast: &ast::File,
    phase2: &Phase2Result,
    source_map: &SourceMap,
) -> Vec<SurfaceFormItem> {
    let mut items = Vec::new();

    for item_spanned in &file_ast.items {
        match &item_spanned.node {
            Item::Entry(entry) => {
                for example in &entry.examples {
                    collect_example_surface_forms(
                        &example.tokens, file_id, phase2, source_map, &mut items,
                    );
                }
                if let Some(ety) = &entry.etymology {
                    if let Some(ref derived) = ety.derived_from {
                        collect_ref_surface_form(derived, file_id, phase2, source_map, &mut items);
                    }
                    for cognate in &ety.cognates {
                        collect_ref_surface_form(&cognate.entry, file_id, phase2, source_map, &mut items);
                    }
                }
            }
            _ => {}
        }
    }

    items
}

fn collect_example_surface_forms(
    tokens: &[ast::Token],
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    items: &mut Vec<SurfaceFormItem>,
) {
    let mut i = 0;
    while i < tokens.len() {
        // Collect a chain of tokens connected by Glue.
        let chain_start = i;
        i += 1;
        while i + 1 < tokens.len() {
            if let ast::Token::Glue = &tokens[i] {
                i += 2;
            } else {
                break;
            }
        }
        let chain_end = i;

        let is_glued = chain_end - chain_start > 1;

        if is_glued {
            // Glue chain: produce one SurfaceFormItem spanning the entire chain.
            let chain_start_offset = (chain_start..chain_end)
                .find_map(|j| token_span_start(&tokens[j], file_id));
            let chain_end_offset = (chain_start..chain_end)
                .rev()
                .find_map(|j| token_span_end(&tokens[j], file_id));

            if let (Some(start), Some(end)) = (chain_start_offset, chain_end_offset) {
                // Collect all resolved forms in the chain, concatenated (glue = no separator).
                let mut parts = Vec::new();
                let mut tooltip_parts = Vec::new();
                let mut all_resolved = true;
                for j in chain_start..chain_end {
                    match &tokens[j] {
                        ast::Token::Ref(entry_ref) => {
                            if let Some(form) = resolve_ref_form(entry_ref, phase2) {
                                tooltip_parts.push(source_text_for_ref(entry_ref, source_map));
                                parts.push(form);
                            } else {
                                all_resolved = false;
                                break;
                            }
                        }
                        ast::Token::Lit(lit) if lit.span.file_id == file_id => {
                            parts.push(lit.node.clone());
                            tooltip_parts.push(lit.node.clone());
                        }
                        ast::Token::Glue => {}
                        _ => {}
                    }
                }
                if all_resolved && !parts.is_empty() {
                    let surface_form = parts.join("");
                    let tooltip = tooltip_parts.join("");
                    items.push(SurfaceFormItem {
                        range: convert::offsets_to_range(file_id, start, end, source_map),
                        surface_form,
                        tooltip: Some(tooltip),
                    });
                }
            }
        } else {
            // Single token — produce a SurfaceFormItem if it resolves.
            if let Some(ast::Token::Ref(entry_ref)) = tokens.get(chain_start) {
                collect_ref_surface_form(entry_ref, file_id, phase2, source_map, items);
            }
        }
    }
}

fn collect_ref_surface_form(
    entry_ref: &EntryRef,
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    items: &mut Vec<SurfaceFormItem>,
) {
    if entry_ref.span.file_id != file_id {
        return;
    }
    let entry_id = &entry_ref.entry_id.node;
    let resolved = match find_resolved_entry(entry_id, phase2) {
        Some(e) => e,
        None => return,
    };

    let form_str = match &entry_ref.form_spec {
        Some(spec) if !spec.conditions.is_empty() => {
            match find_matching_form(resolved, spec) {
                Some(f) => f,
                None => return,
            }
        }
        _ => {
            if resolved.headword == *entry_id {
                return;
            }
            resolved.headword.clone()
        }
    };

    let tooltip = source_text_for_ref(entry_ref, source_map);
    items.push(SurfaceFormItem {
        range: convert::offsets_to_range(file_id, entry_ref.span.start, entry_ref.span.end, source_map),
        surface_form: form_str,
        tooltip: Some(tooltip),
    });
}

/// Generate surface form items for a `.hut` file from its token stream.
pub fn surface_forms_from_tokens(
    file_id: FileId,
    tokens: &[Token],
    phase2: &Phase2Result,
    source_map: &SourceMap,
) -> Vec<SurfaceFormItem> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let TokenKind::Ident(_) = &tokens[i].node {
            if tokens[i].span.file_id == file_id {
                let range_start = tokens[i].span.start;

                let (parts, range_end, skip_to) =
                    collect_tilde_chain_parts(tokens, i, file_id, phase2);

                if !parts.is_empty() {
                    let surface_form = parts.join("");
                    let tooltip = source_map
                        .source_slice(file_id, range_start, range_end)
                        .map(|s| s.to_string());

                    items.push(SurfaceFormItem {
                        range: convert::offsets_to_range(file_id, range_start, range_end, source_map),
                        surface_form,
                        tooltip,
                    });
                }
                i = skip_to;
                continue;
            }
        }
        i += 1;
    }
    items
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_ref_form(entry_ref: &EntryRef, phase2: &Phase2Result) -> Option<String> {
    let entry_id = &entry_ref.entry_id.node;
    let resolved = find_resolved_entry(entry_id, phase2)?;
    match &entry_ref.form_spec {
        Some(spec) if !spec.conditions.is_empty() => find_matching_form(resolved, spec),
        _ => Some(resolved.headword.clone()),
    }
}

fn source_text_for_ref(entry_ref: &EntryRef, source_map: &SourceMap) -> String {
    source_map.source_slice(entry_ref.span.file_id, entry_ref.span.start, entry_ref.span.end)
        .unwrap_or("")
        .to_string()
}

fn token_span_start(tok: &ast::Token, file_id: FileId) -> Option<usize> {
    match tok {
        ast::Token::Ref(r) if r.span.file_id == file_id => Some(r.span.start),
        ast::Token::Lit(lit) if lit.span.file_id == file_id => Some(lit.span.start),
        _ => None,
    }
}

fn token_span_end(tok: &ast::Token, file_id: FileId) -> Option<usize> {
    match tok {
        ast::Token::Ref(r) if r.span.file_id == file_id => Some(r.span.end),
        ast::Token::Lit(lit) if lit.span.file_id == file_id => Some(lit.span.end),
        _ => None,
    }
}
