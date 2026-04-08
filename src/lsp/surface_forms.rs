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
        if let Item::Entry(entry) = &item_spanned.node {
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
            // Glue chain with tags: split into segments at tag boundaries.
            // Each contiguous run of Ref/Lit/Glue tokens becomes one SurfaceFormItem;
            // Tag children are processed recursively as separate groups.
            emit_surface_form_segments(
                &tokens[chain_start..chain_end],
                file_id, phase2, source_map, items,
            );
        } else {
            // Single token — produce a SurfaceFormItem if it resolves,
            // or recurse into Tag children.
            match tokens.get(chain_start) {
                Some(ast::Token::Ref(entry_ref)) => {
                    collect_ref_surface_form(entry_ref, file_id, phase2, source_map, items);
                }
                Some(ast::Token::Tag { children, .. }) => {
                    collect_example_surface_forms(children, file_id, phase2, source_map, items);
                }
                _ => {}
            }
        }
    }
}

/// Walk a glue chain, splitting at Tag/SelfClosingTag boundaries.
/// Contiguous Ref/Lit/Glue runs become one SurfaceFormItem each;
/// Tag children are recursed into as separate groups.
fn emit_surface_form_segments(
    tokens: &[ast::Token],
    file_id: FileId,
    phase2: &Phase2Result,
    source_map: &SourceMap,
    items: &mut Vec<SurfaceFormItem>,
) {
    let mut parts: Vec<String> = Vec::new();
    let mut tooltip_parts: Vec<String> = Vec::new();
    let mut seg_start: Option<usize> = None;
    let mut seg_end: Option<usize> = None;
    let mut all_resolved = true;

    for tok in tokens {
        match tok {
            ast::Token::Ref(entry_ref) if all_resolved => {
                if let Some(form) = resolve_ref_form(entry_ref, phase2) {
                    tooltip_parts.push(source_text_for_ref(entry_ref, source_map));
                    parts.push(form);
                    if entry_ref.span.file_id == file_id {
                        seg_start.get_or_insert(entry_ref.span.start);
                        seg_end = Some(entry_ref.span.end);
                    }
                } else {
                    all_resolved = false;
                }
            }
            ast::Token::Lit(lit) if all_resolved && lit.span.file_id == file_id => {
                parts.push(lit.node.clone());
                tooltip_parts.push(lit.node.clone());
                seg_start.get_or_insert(lit.span.start);
                seg_end = Some(lit.span.end);
            }
            ast::Token::Tag { children, .. } => {
                // Flush the current segment before the tag.
                flush_segment(
                    &mut parts, &mut tooltip_parts,
                    &mut seg_start, &mut seg_end, &mut all_resolved,
                    file_id, source_map, items,
                );
                // Recurse into tag children as a separate group.
                collect_example_surface_forms(children, file_id, phase2, source_map, items);
            }
            ast::Token::SelfClosingTag { .. } => {
                // Flush the current segment; self-closing tags have no children.
                flush_segment(
                    &mut parts, &mut tooltip_parts,
                    &mut seg_start, &mut seg_end, &mut all_resolved,
                    file_id, source_map, items,
                );
            }
            ast::Token::Glue | ast::Token::Newline => {}
            _ => {}
        }
    }
    // Flush trailing segment.
    flush_segment(
        &mut parts, &mut tooltip_parts,
        &mut seg_start, &mut seg_end, &mut all_resolved,
        file_id, source_map, items,
    );
}

/// Emit a SurfaceFormItem from accumulated parts and reset state.
#[allow(clippy::too_many_arguments)]
fn flush_segment(
    parts: &mut Vec<String>,
    tooltip_parts: &mut Vec<String>,
    seg_start: &mut Option<usize>,
    seg_end: &mut Option<usize>,
    all_resolved: &mut bool,
    file_id: FileId,
    source_map: &SourceMap,
    items: &mut Vec<SurfaceFormItem>,
) {
    if *all_resolved && !parts.is_empty() {
        if let (Some(start), Some(end)) = (*seg_start, *seg_end) {
            items.push(SurfaceFormItem {
                range: convert::offsets_to_range(file_id, start, end, source_map),
                surface_form: parts.join(""),
                tooltip: Some(tooltip_parts.join("")),
            });
        }
    }
    parts.clear();
    tooltip_parts.clear();
    *seg_start = None;
    *seg_end = None;
    *all_resolved = true;
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
        _ => resolved.headword.clone(),
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
    if let Some(stem_name) = &entry_ref.stem_spec {
        return resolved.stems.get(&stem_name.node).cloned();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{self, Span, Spanned, StringLit};
    use crate::error::Diagnostics;
    use crate::phase2::{Phase2Result, ResolvedEntry};
    use std::collections::HashMap;

    fn fid() -> FileId { FileId(0) }

    fn span(start: usize, end: usize) -> Span {
        Span { file_id: fid(), start, end }
    }

    fn mk_ref(name: &str, s: usize, e: usize) -> ast::Token {
        ast::Token::Ref(EntryRef {
            namespace: vec![],
            entry_id: Spanned { node: name.to_string(), span: span(s, e) },
            meaning: None,
            form_spec: None,
            stem_spec: None,
            span: span(s, e),
        })
    }

    fn _mk_lit(text: &str, s: usize, e: usize) -> ast::Token {
        ast::Token::Lit(StringLit { node: text.to_string(), span: span(s, e) })
    }

    fn mk_tag(name: &str, children: Vec<ast::Token>, s: usize, e: usize) -> ast::Token {
        ast::Token::Tag { name: name.to_string(), attrs: vec![], children, span: span(s, e) }
    }

    fn mk_phase2(entries: Vec<(&str, &str)>) -> Phase2Result {
        Phase2Result {
            axes: HashMap::new(),
            inflections: vec![],
            entries: entries.into_iter().map(|(name, hw)| ResolvedEntry {
                name: name.to_string(),
                source_file: std::path::PathBuf::new(),
                headword: hw.to_string(),
                headword_scripts: HashMap::new(),
                tags: vec![],
                inflection_class: None,
                meaning: String::new(),
                meanings: vec![],
                stems: HashMap::new(),
                forms: vec![],
                links: vec![],
                etymology_proto: None,
                etymology_note: None,
            }).collect(),
            render_config: Default::default(),
            diagnostics: Diagnostics::new(),
        }
    }

    fn mk_source_map() -> SourceMap {
        let mut sm = SourceMap::new();
        // 40 bytes of padding so offsets in test spans are valid.
        sm.add_file("test.hu".into(), " ".repeat(40));
        sm
    }

    fn surface_forms_for(tokens: &[ast::Token], p2: &Phase2Result) -> Vec<String> {
        let sm = mk_source_map();
        let mut items = Vec::new();
        collect_example_surface_forms(tokens, fid(), p2, &sm, &mut items);
        items.into_iter().map(|i| i.surface_form).collect()
    }

    #[test]
    fn surface_form_simple_glue() {
        let p2 = mk_phase2(vec![("a", "alpha"), ("b", "beta")]);
        let tokens = vec![mk_ref("a", 0, 1), ast::Token::Glue, mk_ref("b", 2, 3)];
        assert_eq!(surface_forms_for(&tokens, &p2), vec!["alphabeta"]);
    }

    #[test]
    fn surface_form_tag_splits_chain() {
        // ref1~<em>ref2</em>~ref3 → ["ref1"], ["ref2"], ["ref3"]
        let p2 = mk_phase2(vec![("r1", "one"), ("r2", "two"), ("r3", "three")]);
        let tokens = vec![
            mk_ref("r1", 0, 2),
            ast::Token::Glue,
            mk_tag("em", vec![mk_ref("r2", 7, 9)], 3, 15),
            ast::Token::Glue,
            mk_ref("r3", 16, 18),
        ];
        assert_eq!(surface_forms_for(&tokens, &p2), vec!["one", "two", "three"]);
    }

    #[test]
    fn surface_form_tag_with_glue_inside() {
        // ref1~ref2<em>~ref3~ref4</em> → ["onetwo"], ["threefour"]
        let p2 = mk_phase2(vec![("r1", "one"), ("r2", "two"), ("r3", "three"), ("r4", "four")]);
        let tokens = vec![
            mk_ref("r1", 0, 2),
            ast::Token::Glue,
            mk_ref("r2", 3, 5),
            mk_tag("em", vec![
                ast::Token::Glue,
                mk_ref("r3", 10, 12),
                ast::Token::Glue,
                mk_ref("r4", 13, 15),
            ], 5, 20),
        ];
        assert_eq!(surface_forms_for(&tokens, &p2), vec!["onetwo", "threefour"]);
    }

    #[test]
    fn surface_form_standalone_tag() {
        // <em>ref1~ref2</em> → ["onetwo"]
        let p2 = mk_phase2(vec![("r1", "one"), ("r2", "two")]);
        let tokens = vec![
            mk_tag("em", vec![
                mk_ref("r1", 4, 6),
                ast::Token::Glue,
                mk_ref("r2", 7, 9),
            ], 0, 15),
        ];
        assert_eq!(surface_forms_for(&tokens, &p2), vec!["onetwo"]);
    }
}
