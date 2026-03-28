//! Hover information handler.

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::ast::{self, Item};
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::symbol_table::SymbolKind;
use crate::token::{Token, TokenKind};

use super::convert;

/// Produce hover information for the symbol at the given byte offset.
pub fn hover(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    phase1: &Phase1Result,
) -> Option<Hover> {
    // Find the token at cursor.
    let (tok_idx, tok) = tokens.iter().enumerate().find(|(_, t)| {
        t.span.file_id == file_id && t.span.start <= offset && offset < t.span.end
    })?;

    let name = match &tok.node {
        TokenKind::Ident(n) => n,
        _ => return None,
    };

    // Check if cursor is on a tag axis value (after `=` in `[axis=value]`).
    if let Some(axis_name) = find_tag_value_axis(tokens, tok_idx) {
        if let Some((_, ext, val)) = find_extend_value(Some(&axis_name), name, phase1) {
            let markdown = format_tag_value(val, ext);
            let range = convert::span_to_range(&tok.span, &phase1.source_map);
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(range),
            });
        }
    }

    // Resolve symbol.
    let scope = phase1.symbol_table.scope(file_id)?;
    let results = scope.resolve(name);
    let sym = results.first()?;

    // Get the AST item.
    let file_ast = phase1.files.get(&sym.file_id)?;
    let item_spanned = file_ast.items.get(sym.item_index)?;

    let markdown = format_item_info(&item_spanned.node, sym.kind, name);

    let range = convert::span_to_range(&tok.span, &phase1.source_map);

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(range),
    })
}

fn format_item_info(item: &Item, kind: SymbolKind, name: &str) -> String {
    match (kind, item) {
        (SymbolKind::Entry, Item::Entry(e)) => format_entry(e),
        (SymbolKind::Inflection, Item::Inflection(i)) => format_inflection(i),
        (SymbolKind::TagAxis, Item::TagAxis(t)) => format_tagaxis(t),
        (SymbolKind::PhonRule, Item::PhonRule(p)) => format_phonrule(p),
        (SymbolKind::Extend, Item::Extend(ext)) => format_extend(ext),
        _ => format!("```\n{:?} {}\n```", kind, name),
    }
}

fn format_entry(e: &ast::Entry) -> String {
    let headword = match &e.headword {
        ast::Headword::Simple(s) => s.node.clone(),
        ast::Headword::MultiScript(scripts) => {
            scripts
                .iter()
                .map(|(k, v)| format!("{}: \"{}\"", k.node, v.node))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };

    let meaning = match &e.meaning {
        ast::MeaningDef::Single(s) => s.node.clone(),
        ast::MeaningDef::Multiple(entries) => entries
            .iter()
            .map(|m| format!("{}: \"{}\"", m.ident.node, m.text.node))
            .collect::<Vec<_>>()
            .join("; "),
    };

    let infl = match &e.inflection {
        Some(ast::EntryInflection::Class(c)) => format!("inflection_class: {}", c.node),
        Some(ast::EntryInflection::Inline(_)) => "inline inflection".into(),
        None => "no inflection".into(),
    };

    let stems_count = e.stems.len();

    format!(
        "```hubullu\nentry {}\n```\n---\n**headword**: \"{}\"\\\n**meaning**: \"{}\"\\\n**stems**: {} stem(s)\\\n**{}**",
        e.name.node, headword, meaning, stems_count, infl
    )
}

fn format_inflection(i: &ast::Inflection) -> String {
    let axes: Vec<_> = i.axes.iter().map(|a| a.node.as_str()).collect();
    let stems: Vec<_> = i.required_stems.iter().map(|s| s.name.node.as_str()).collect();
    let rule_count = match &i.body {
        ast::InflectionBody::Rules(rules) => rules.len(),
        ast::InflectionBody::Compose(c) => {
            c.slots.iter().map(|s| s.rules.len()).sum::<usize>() + c.overrides.len()
        }
    };
    let body_kind = match &i.body {
        ast::InflectionBody::Rules(_) => "rules",
        ast::InflectionBody::Compose(_) => "compose",
    };

    format!(
        "```hubullu\ninflection {} for {{{}}}\n```\n---\n**requires stems**: {}\\\n**{} rules** ({})",
        i.name.node,
        axes.join(", "),
        if stems.is_empty() { "none".to_string() } else { stems.join(", ") },
        rule_count,
        body_kind
    )
}

fn format_tagaxis(t: &ast::TagAxis) -> String {
    let role = match t.role.node {
        ast::Role::Inflectional => "inflectional",
        ast::Role::Classificatory => "classificatory",
        ast::Role::Structural => "structural",
    };

    format!(
        "```hubullu\ntagaxis {}\n```\n---\n**role**: {}",
        t.name.node, role
    )
}

fn format_phonrule(p: &ast::PhonRule) -> String {
    let class_count = p.classes.len();
    let map_count = p.maps.len();
    let rule_count = p.rules.len();

    format!(
        "```hubullu\nphonrule {}\n```\n---\n**classes**: {}, **maps**: {}, **rules**: {}",
        p.name.node, class_count, map_count, rule_count
    )
}

fn format_extend(ext: &ast::Extend) -> String {
    let values: Vec<_> = ext.values.iter().map(|v| v.name.node.as_str()).collect();

    format!(
        "```hubullu\n@extend {} on {}\n```\n---\n**values**: {}",
        ext.name.node,
        ext.target_axis.node,
        values.join(", ")
    )
}

fn format_tag_value(val: &ast::ExtendValue, ext: &ast::Extend) -> String {
    let display: Vec<_> = val
        .display
        .iter()
        .map(|(k, v)| format!("{}: \"{}\"", k.node, v.node))
        .collect();

    let mut lines = format!(
        "```hubullu\n{} (value of {})\n```\n---\n**axis**: {}",
        val.name.node, ext.target_axis.node, ext.target_axis.node
    );
    if !display.is_empty() {
        lines.push_str(&format!("\\\n**display**: {}", display.join(", ")));
    }
    if !val.slots.is_empty() {
        let slots: Vec<_> = val.slots.iter().map(|s| s.node.as_str()).collect();
        lines.push_str(&format!("\\\n**slots**: {}", slots.join(", ")));
    }
    lines
}

/// Detect if the token at `tok_idx` is in the value position of `[axis=value]`.
/// Returns the axis name if so.
pub(crate) fn find_tag_value_axis(tokens: &[Token], tok_idx: usize) -> Option<String> {
    if tok_idx >= 2 {
        if let TokenKind::Eq = &tokens[tok_idx - 1].node {
            if let TokenKind::Ident(axis) = &tokens[tok_idx - 2].node {
                return Some(axis.clone());
            }
        }
    }
    None
}

/// Search all @extend blocks for a value definition.
/// If `axis_name` is Some, only match extends targeting that axis.
pub(crate) fn find_extend_value<'a>(
    axis_name: Option<&str>,
    value_name: &str,
    phase1: &'a Phase1Result,
) -> Option<(FileId, &'a ast::Extend, &'a ast::ExtendValue)> {
    for (&fid, file) in &phase1.files {
        for item in &file.items {
            if let Item::Extend(ext) = &item.node {
                if axis_name.map_or(true, |a| ext.target_axis.node == a) {
                    for val in &ext.values {
                        if val.name.node == value_name {
                            return Some((fid, ext, val));
                        }
                    }
                }
            }
        }
    }
    None
}
