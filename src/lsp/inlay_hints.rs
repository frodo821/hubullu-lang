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
