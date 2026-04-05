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

        if is_glued {
            // Glue chain: concatenate all resolved parts into one hint.
            // Tags are transparent — recurse into children.
            if let Some(end) = last_span_end {
                let mut parts = Vec::new();
                let mut all_resolved = true;
                collect_chain_parts(
                    &tokens[chain_start..chain_end],
                    file_id, phase2, &mut parts, &mut all_resolved,
                );
                if all_resolved && !parts.is_empty() {
                    let form_str = parts.join("");
                    let position = convert::offset_to_position(file_id, end, source_map);
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
            }
        } else {
            // Single token — produce a hint if it resolves.
            if let Some(ast::Token::Ref(entry_ref)) = tokens.get(chain_start) {
                collect_ref_hint(entry_ref, file_id, phase2, source_map, hints);
            }
        }
    }
}

/// Collect resolved form parts from a token slice, recursing into Tag children.
fn collect_chain_parts(
    tokens: &[ast::Token],
    file_id: FileId,
    phase2: &Phase2Result,
    parts: &mut Vec<String>,
    all_resolved: &mut bool,
) {
    for tok in tokens {
        if !*all_resolved {
            break;
        }
        match tok {
            ast::Token::Ref(entry_ref) => {
                if let Some(form) = resolve_ref_form(entry_ref, phase2) {
                    parts.push(form);
                } else {
                    *all_resolved = false;
                }
            }
            ast::Token::Lit(lit) if lit.span.file_id == file_id => {
                parts.push(lit.node.clone());
            }
            ast::Token::Tag { children, .. } => {
                collect_chain_parts(children, file_id, phase2, parts, all_resolved);
            }
            ast::Token::Glue | ast::Token::Newline | ast::Token::SelfClosingTag { .. } => {}
            _ => {}
        }
    }
}

/// Resolve the form string for a single entry ref.
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

/// Get the span end offset of a token if it belongs to the given file.
fn token_span_end(tok: &ast::Token, file_id: FileId) -> Option<usize> {
    match tok {
        ast::Token::Ref(r) if r.span.file_id == file_id => Some(r.span.end),
        ast::Token::Lit(lit) if lit.span.file_id == file_id => Some(lit.span.end),
        ast::Token::Tag { span, .. } if span.file_id == file_id => Some(span.end),
        ast::Token::SelfClosingTag { span, .. } if span.file_id == file_id => Some(span.end),
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

    let form_str = if let Some(stem_name) = &entry_ref.stem_spec {
        match resolved.stems.get(&stem_name.node) {
            Some(v) => v.clone(),
            None => return,
        }
    } else {
        match &entry_ref.form_spec {
            Some(spec) if !spec.conditions.is_empty() => {
                match find_matching_form(resolved, spec) {
                    Some(f) => f,
                    None => return,
                }
            }
            _ => resolved.headword.clone(),
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
        if let TokenKind::Ident(_) = &tokens[i].node {
            if tokens[i].span.file_id == file_id {
                // Collect all parts of this element (possibly tilde-chained).
                let (parts, hint_offset, skip_to) =
                    collect_tilde_chain_parts(tokens, i, file_id, phase2);

                if !parts.is_empty() {
                    let form_str = parts.join("");
                    if let Some(hint) = make_hint(file_id, hint_offset, &form_str, source_map) {
                        hints.push(hint);
                    }
                }
                i = skip_to;
                continue;
            }
        }
        i += 1;
    }
    hints
}

/// Parse a single element (ident with optional brackets) and resolve its form.
/// Returns (resolved_form, next_index_after_element, span_end_offset).
fn parse_single_element(
    tokens: &[Token],
    pos: usize,
    file_id: FileId,
    phase2: &Phase2Result,
) -> Option<(String, usize, usize)> {
    let entry_id = match &tokens[pos].node {
        TokenKind::Ident(id) if tokens[pos].span.file_id == file_id => id,
        _ => return None,
    };

    // Try [$=stem_name] first.
    if let Some((stem_name, end_pos, rbracket_end)) =
        parse_stem_spec(tokens, pos + 1, file_id)
    {
        let resolved = phase2.entries.iter().find(|e| e.name == *entry_id)?;
        let stem_value = resolved.stems.get(&stem_name)?.clone();
        return Some((stem_value, end_pos, rbracket_end));
    }

    if let Some((conditions, end_pos, rbracket_end)) =
        parse_bracket_conditions(tokens, pos + 1, file_id)
    {
        if !conditions.is_empty() {
            let resolved = phase2.entries.iter().find(|e| e.name == *entry_id)?;
            let form_str = resolved.forms.iter().find_map(|form| {
                let all_match = conditions.iter().all(|(axis, val)| {
                    form.tags.iter().any(|(a, v)| a == axis && v == val)
                });
                if all_match { Some(form.form_str.clone()) } else { None }
            })?;
            return Some((form_str, end_pos, rbracket_end));
        }
        // Empty conditions — skip brackets, treat as bare ident.
        let resolved = phase2.entries.iter().find(|e| e.name == *entry_id)?;
        return Some((resolved.headword.clone(), end_pos, rbracket_end));
    }

    // No bracket — headword hint.
    let resolved = phase2.entries.iter().find(|e| e.name == *entry_id)?;
    Some((resolved.headword.clone(), pos + 1, tokens[pos].span.end))
}

/// Collect all parts of a tilde chain starting at `start`, resolving each
/// element's form. Returns (parts, hint_offset, skip_to).
pub fn collect_tilde_chain_parts(
    tokens: &[Token],
    start: usize,
    file_id: FileId,
    phase2: &Phase2Result,
) -> (Vec<String>, usize, usize) {
    let mut parts = Vec::new();
    let mut end_offset;

    // Parse the first element.
    let (form, mut pos, first_end) = match parse_single_element(tokens, start, file_id, phase2) {
        Some(r) => r,
        None => {
            // Couldn't resolve — skip past brackets if present.
            if let Some((_, end_pos, _)) = parse_bracket_conditions(tokens, start + 1, file_id) {
                return (vec![], 0, end_pos);
            }
            return (vec![], 0, start + 1);
        }
    };
    parts.push(form);
    end_offset = first_end;

    // Continue through tilde-connected elements.
    while pos + 1 < tokens.len() {
        if !matches!(tokens[pos].node, TokenKind::Tilde) {
            break;
        }
        let next = pos + 1;
        match parse_single_element(tokens, next, file_id, phase2) {
            Some((form, after, span_end)) => {
                parts.push(form);
                end_offset = span_end;
                pos = after;
            }
            None => {
                // Unresolvable element in chain — include literal text if it's an ident.
                if let TokenKind::Ident(text) = &tokens[next].node {
                    parts.push(text.clone());
                    end_offset = tokens[next].span.end;
                    pos = next + 1;
                } else {
                    break;
                }
            }
        }
    }

    (parts, end_offset, pos)
}

/// Parse `[$=stem_name]` from the token stream starting at `start`.
/// Returns (stem_name, next_index_after_rbracket, rbracket_end_offset).
fn parse_stem_spec(
    tokens: &[Token],
    start: usize,
    file_id: FileId,
) -> Option<(String, usize, usize)> {
    // Expect: [ $ = ident ]
    if start + 4 >= tokens.len() {
        return None;
    }
    if !matches!(tokens[start].node, TokenKind::LBracket) {
        return None;
    }
    if !matches!(tokens[start + 1].node, TokenKind::Dollar) {
        return None;
    }
    if !matches!(tokens[start + 2].node, TokenKind::Eq) {
        return None;
    }
    let stem_name = match &tokens[start + 3].node {
        TokenKind::Ident(name) if tokens[start + 3].span.file_id == file_id => name.clone(),
        _ => return None,
    };
    if !matches!(tokens[start + 4].node, TokenKind::RBracket) {
        return None;
    }
    let rbracket_end = tokens[start + 4].span.end;
    Some((stem_name, start + 5, rbracket_end))
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

    fn mk_lit(text: &str, s: usize, e: usize) -> ast::Token {
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

    #[test]
    fn collect_chain_parts_simple_glue() {
        let p2 = mk_phase2(vec![("a", "alpha"), ("b", "beta")]);
        let tokens = vec![mk_ref("a", 0, 1), ast::Token::Glue, mk_ref("b", 2, 3)];
        let mut parts = Vec::new();
        let mut ok = true;
        collect_chain_parts(&tokens, fid(), &p2, &mut parts, &mut ok);
        assert!(ok);
        assert_eq!(parts, vec!["alpha", "beta"]);
    }

    #[test]
    fn collect_chain_parts_tag_transparent() {
        // ref1~<em>ref2</em>~ref3 — tag is transparent for inlay hints
        let p2 = mk_phase2(vec![("r1", "one"), ("r2", "two"), ("r3", "three")]);
        let tokens = vec![
            mk_ref("r1", 0, 2),
            ast::Token::Glue,
            mk_tag("em", vec![mk_ref("r2", 7, 9)], 3, 15),
            ast::Token::Glue,
            mk_ref("r3", 16, 18),
        ];
        let mut parts = Vec::new();
        let mut ok = true;
        collect_chain_parts(&tokens, fid(), &p2, &mut parts, &mut ok);
        assert!(ok);
        assert_eq!(parts, vec!["one", "two", "three"]);
    }

    #[test]
    fn collect_chain_parts_tag_with_glue_inside() {
        // ref1~ref2<em>~ref3~ref4</em>
        // Top-level: [Ref(r1), Glue, Ref(r2), Tag{children: [Glue, Ref(r3), Glue, Ref(r4)]}]
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
        let mut parts = Vec::new();
        let mut ok = true;
        collect_chain_parts(&tokens, fid(), &p2, &mut parts, &mut ok);
        assert!(ok);
        assert_eq!(parts, vec!["one", "two", "three", "four"]);
    }
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
