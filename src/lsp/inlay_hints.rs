//! Inlay hints — show resolved inflected forms for EntryRef tokens in examples.
//!
//! When a `.hu` file has an example like `faren[tense=present, person=3]`,
//! the inlay hint shows the actual inflected form (e.g. ` → fährt`) after
//! the closing bracket.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel};

use crate::ast::{self, EntryRef, Item, TagConditionList};
use crate::phase2::{Phase2Result, ResolvedEntry};
use crate::span::{FileId, SourceMap};

use super::convert;

/// Generate inlay hints for a file.
pub fn inlay_hints(
    file_id: FileId,
    file_ast: &ast::File,
    phase2: &Phase2Result,
    source_map: &SourceMap,
) -> Vec<InlayHint> {
    let mut hints = Vec::new();

    for item_spanned in &file_ast.items {
        match &item_spanned.node {
            Item::Entry(entry) => {
                // Hints in examples.
                for example in &entry.examples {
                    collect_example_hints(
                        &example.tokens,
                        file_id,
                        phase2,
                        source_map,
                        &mut hints,
                    );
                }
                // Hints in etymology cognates / derived_from.
                if let Some(ety) = &entry.etymology {
                    if let Some(ref derived) = ety.derived_from {
                        collect_ref_hint(derived, file_id, phase2, source_map, &mut hints);
                    }
                    for cognate in &ety.cognates {
                        collect_ref_hint(&cognate.entry, file_id, phase2, source_map, &mut hints);
                    }
                }
            }
            _ => {}
        }
    }

    hints
}

fn collect_example_hints(
    tokens: &[ast::Token],
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
    // Walk tokens, grouping glue-connected chains.
    // For each chain, collect resolved labels and place them at the chain's last token end.
    let mut i = 0;
    while i < tokens.len() {
        // Collect a chain of tokens connected by Glue.
        let chain_start = i;
        i += 1;
        while i + 1 < tokens.len() {
            if let ast::Token::Glue = &tokens[i] {
                i += 2; // skip Glue and the next token
            } else {
                break;
            }
        }
        let chain_end = i; // exclusive

        // Find the span end of the last non-Glue token in this chain.
        let last_span_end = (chain_start..chain_end)
            .rev()
            .find_map(|j| token_span_end(&tokens[j], file_id));

        // If the chain has only one token and no glue, use normal behavior.
        let is_glued = chain_end - chain_start > 1;

        for j in chain_start..chain_end {
            if let ast::Token::Ref(entry_ref) = &tokens[j] {
                if is_glued {
                    if let Some(end) = last_span_end {
                        collect_ref_hint_at(entry_ref, file_id, end, phase2, source_map, hints);
                    }
                } else {
                    collect_ref_hint(entry_ref, file_id, phase2, source_map, hints);
                }
            }
        }
    }
}

/// Get the span end offset of a token if it belongs to the given file.
fn token_span_end(tok: &ast::Token, file_id: FileId) -> Option<usize> {
    match tok {
        ast::Token::Ref(r) if r.span.file_id == file_id => Some(r.span.end),
        ast::Token::Lit(lit) if lit.span.file_id == file_id => Some(lit.span.end),
        _ => None,
    }
}

fn collect_ref_hint(
    entry_ref: &EntryRef,
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
    if entry_ref.span.file_id != file_id {
        return;
    }
    collect_ref_hint_at(entry_ref, file_id, entry_ref.span.end, phase2, source_map, hints);
}

fn collect_ref_hint_at(
    entry_ref: &EntryRef,
    file_id: FileId,
    hint_offset: usize,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
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
            // No form spec — show headword if it differs from the entry name.
            if resolved.headword == *entry_id {
                return;
            }
            resolved.headword.clone()
        }
    };

    let position = convert::offset_to_position(file_id, hint_offset, source_map);

    hints.push(InlayHint {
        position,
        label: InlayHintLabel::String(format!(" → {}", form_str)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    });
}

pub fn find_resolved_entry<'a>(
    entry_id: &str,
    phase2: &'a Phase2Result,
) -> Option<&'a ResolvedEntry> {
    phase2.entries.iter().find(|e| e.name == entry_id)
}

pub fn find_matching_form(
    entry: &ResolvedEntry,
    form_spec: &TagConditionList,
) -> Option<String> {
    // A form matches if all conditions in the spec are satisfied by the form's tags.
    for form in &entry.forms {
        let all_match = form_spec.conditions.iter().all(|cond| {
            form.tags
                .iter()
                .any(|(axis, val)| axis == &cond.axis.node && val == &cond.value.node)
        });
        if all_match {
            return Some(form.form_str.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// .hut token-stream inlay hints
// ---------------------------------------------------------------------------

use crate::token::{Token, TokenKind};

/// Generate inlay hints for a `.hut` file by scanning the token stream for
/// entry-ref patterns like `entry_id[axis=value, ...]`.
pub fn inlay_hints_from_tokens(
    file_id: FileId,
    tokens: &[Token],
    phase2: &Phase2Result,
    source_map: &SourceMap,
) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let TokenKind::Ident(entry_id) = &tokens[i].node {
            if tokens[i].span.file_id == file_id {
                if let Some((conditions, end_pos, rbracket_end)) =
                    parse_bracket_conditions(tokens, i + 1, file_id)
                {
                    if !conditions.is_empty() {
                        // If followed by Tilde chain, defer the hint to the end of the chain.
                        let (hint_offset, skip_to) =
                            find_tilde_chain_end(tokens, end_pos, file_id, rbracket_end);
                        if let Some(hint) =
                            resolve_token_hint(entry_id, &conditions, hint_offset, phase2, source_map, file_id)
                        {
                            hints.push(hint);
                        }
                        i = skip_to;
                        continue;
                    }
                    i = end_pos;
                    continue;
                }
                // No bracket — show headword hint if it differs from the entry name.
                let ident_end = tokens[i].span.end;
                let (hint_offset, skip_to) =
                    find_tilde_chain_end(tokens, i + 1, file_id, ident_end);
                if let Some(hint) =
                    resolve_headword_hint(entry_id, hint_offset, phase2, source_map, file_id)
                {
                    hints.push(hint);
                }
                i = skip_to;
                continue;
            }
        }
        i += 1;
    }
    hints
}

/// Walk past a `~ token (~ token)*` chain and return the span-end of the last
/// token in the chain plus the next index to resume scanning from.
pub fn find_tilde_chain_end(
    tokens: &[Token],
    start: usize,
    file_id: FileId,
    default_end: usize,
) -> (usize, usize) {
    let mut pos = start;
    let mut end_offset = default_end;
    while pos + 1 < tokens.len() {
        if matches!(tokens[pos].node, TokenKind::Tilde) {
            let next = pos + 1;
            // Advance past `~ token`, possibly including `Ident [ ... ]` patterns.
            let after_next = if let TokenKind::Ident(_) = &tokens[next].node {
                if let Some((_, ep, rb)) = parse_bracket_conditions(tokens, next + 1, file_id) {
                    // The next token is an entry_id with brackets — skip them too.
                    // (We don't generate a separate hint for this ref; it's part of the glue chain.)
                    end_offset = rb;
                    ep
                } else {
                    end_offset = tokens[next].span.end;
                    next + 1
                }
            } else {
                end_offset = tokens[next].span.end;
                next + 1
            };
            pos = after_next;
        } else {
            break;
        }
    }
    (end_offset, pos)
}

/// Parse `[axis=value, axis=value, ...]` from the token stream starting at `start`.
/// Returns (conditions, next_index_after_rbracket, rbracket_end_offset).
pub fn parse_bracket_conditions(
    tokens: &[Token],
    start: usize,
    file_id: FileId,
) -> Option<(Vec<(String, String)>, usize, usize)> {
    if start >= tokens.len() {
        return None;
    }
    if !matches!(tokens[start].node, TokenKind::LBracket) {
        return None;
    }
    let mut i = start + 1;
    let mut conditions = Vec::new();
    loop {
        if i >= tokens.len() {
            return None;
        }
        // RBracket ends the list.
        if matches!(tokens[i].node, TokenKind::RBracket) {
            let rbracket_end = tokens[i].span.end;
            return Some((conditions, i + 1, rbracket_end));
        }
        // Expect: Ident Eq Ident
        let axis = match &tokens[i].node {
            TokenKind::Ident(a) if tokens[i].span.file_id == file_id => a.clone(),
            _ => return None,
        };
        i += 1;
        if i >= tokens.len() || !matches!(tokens[i].node, TokenKind::Eq) {
            return None;
        }
        i += 1;
        let value = match tokens.get(i).map(|t| &t.node) {
            Some(TokenKind::Ident(v)) => v.clone(),
            _ => return None,
        };
        i += 1;
        conditions.push((axis, value));
        // Optional comma.
        if i < tokens.len() && matches!(tokens[i].node, TokenKind::Comma) {
            i += 1;
        }
    }
}

fn resolve_token_hint(
    entry_id: &str,
    conditions: &[(String, String)],
    hint_offset: usize,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    file_id: FileId,
) -> Option<InlayHint> {
    let resolved = phase2.entries.iter().find(|e| e.name == entry_id)?;
    let form_str = resolved.forms.iter().find_map(|form| {
        let all_match = conditions.iter().all(|(axis, val)| {
            form.tags.iter().any(|(a, v)| a == axis && v == val)
        });
        if all_match { Some(form.form_str.clone()) } else { None }
    })?;

    make_hint(file_id, hint_offset, &form_str, source_map)
}

fn resolve_headword_hint(
    entry_id: &str,
    hint_offset: usize,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    file_id: FileId,
) -> Option<InlayHint> {
    let resolved = phase2.entries.iter().find(|e| e.name == entry_id)?;
    if resolved.headword == entry_id {
        return None;
    }
    make_hint(file_id, hint_offset, &resolved.headword, source_map)
}

fn make_hint(
    file_id: FileId,
    hint_offset: usize,
    form_str: &str,
    source_map: &SourceMap,
) -> Option<InlayHint> {
    let position = convert::offset_to_position(file_id, hint_offset, source_map);
    Some(InlayHint {
        position,
        label: InlayHintLabel::String(format!(" → {}", form_str)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    })
}
