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
    for tok in tokens {
        if let ast::Token::Ref(entry_ref) = tok {
            collect_ref_hint(entry_ref, file_id, phase2, source_map, hints);
        }
    }
}

fn collect_ref_hint(
    entry_ref: &EntryRef,
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
    // Only show hint if there's a form spec.
    let form_spec = match &entry_ref.form_spec {
        Some(spec) if !spec.conditions.is_empty() => spec,
        _ => return,
    };

    // Find the resolved entry.
    let entry_id = &entry_ref.entry_id.node;
    let resolved = match find_resolved_entry(entry_id, phase2) {
        Some(e) => e,
        None => return,
    };

    // Find the matching form.
    let form_str = match find_matching_form(resolved, form_spec) {
        Some(f) => f,
        None => return,
    };

    // Place the hint right after the entry ref span.
    if entry_ref.span.file_id != file_id {
        return;
    }
    let position = convert::offset_to_position(file_id, entry_ref.span.end, source_map);

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

fn find_resolved_entry<'a>(
    entry_id: &str,
    phase2: &'a Phase2Result,
) -> Option<&'a ResolvedEntry> {
    phase2.entries.iter().find(|e| e.entry_id == entry_id)
}

fn find_matching_form(
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
        // Look for pattern: Ident LBracket ... RBracket
        if let TokenKind::Ident(entry_id) = &tokens[i].node {
            if tokens[i].span.file_id == file_id {
                if let Some((conditions, end_pos, rbracket_end)) =
                    parse_bracket_conditions(tokens, i + 1, file_id)
                {
                    if !conditions.is_empty() {
                        if let Some(hint) =
                            resolve_token_hint(entry_id, &conditions, rbracket_end, phase2, source_map, file_id)
                        {
                            hints.push(hint);
                        }
                    }
                    i = end_pos;
                    continue;
                }
            }
        }
        i += 1;
    }
    hints
}

/// Parse `[axis=value, axis=value, ...]` from the token stream starting at `start`.
/// Returns (conditions, next_index_after_rbracket, rbracket_end_offset).
fn parse_bracket_conditions(
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
    rbracket_end: usize,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    file_id: FileId,
) -> Option<InlayHint> {
    let resolved = phase2.entries.iter().find(|e| e.entry_id == entry_id)?;
    // Find the form matching all conditions.
    let form_str = resolved.forms.iter().find_map(|form| {
        let all_match = conditions.iter().all(|(axis, val)| {
            form.tags.iter().any(|(a, v)| a == axis && v == val)
        });
        if all_match { Some(form.form_str.clone()) } else { None }
    })?;

    let position = convert::offset_to_position(file_id, rbracket_end, source_map);
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
