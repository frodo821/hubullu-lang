//! Completion handler.

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionResponse, Documentation,
    MarkupContent, MarkupKind,
};

use crate::ast::{self, Item};
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::symbol_table::SymbolKind;
use crate::token::{Token, TokenKind};

/// Produce completion items for the given cursor position.
pub fn complete(
    file_id: FileId,
    offset: usize,
    tokens: &[Token],
    phase1: Option<&Phase1Result>,
    is_hut: bool,
) -> CompletionResponse {
    let ctx = if is_hut {
        determine_hut_context(tokens, file_id, offset)
    } else {
        determine_hu_context(tokens, file_id, offset)
    };
    let mut items = Vec::new();

    match ctx {
        Context::TopLevel => {
            for &(kw, detail) in TOP_LEVEL_KEYWORDS {
                items.push(keyword_item(kw, detail));
            }
        }
        Context::EntryBody => {
            for &(kw, detail) in ENTRY_FIELD_KEYWORDS {
                items.push(field_item(kw, detail));
            }
        }
        Context::TagAxisBody => {
            for &(kw, detail) in TAGAXIS_FIELD_KEYWORDS {
                items.push(field_item(kw, detail));
            }
        }
        Context::InflectionBody => {
            for &(kw, detail) in &[
                ("requires", "Declare required stems"),
                ("compose", "Agglutinative compose chain"),
            ] {
                items.push(keyword_item(kw, detail));
            }
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::TagAxis, p1);
            }
        }
        Context::PhonRuleBody => {
            for &(kw, detail) in &[
                ("class", "Define a character class"),
                ("map", "Define a character mapping"),
            ] {
                items.push(keyword_item(kw, detail));
            }
        }
        Context::RenderBody => {
            for &(kw, detail) in &[
                ("separator", "Token separator string"),
                ("no_separator_before", "Chars that suppress preceding separator"),
            ] {
                items.push(field_item(kw, detail));
            }
        }
        Context::ExtendBody => {
            for &(kw, detail) in &[
                ("display", "Display name map"),
                ("slots", "Structural slots"),
            ] {
                items.push(field_item(kw, detail));
            }
        }
        Context::EtymologyBody => {
            for &(kw, detail) in &[
                ("proto", "Proto-form"),
                ("cognates", "Cognate entries"),
                ("derived_from", "Source entry reference"),
                ("note", "Etymology note"),
            ] {
                items.push(field_item(kw, detail));
            }
        }
        Context::ExamplesBody | Context::HutDefault => {
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::Entry, p1);
            }
        }
        Context::FormsOverrideBody => {
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::TagAxis, p1);
            }
        }
        Context::InflectionClass => {
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::Inflection, p1);
            }
        }
        Context::TagAxis => {
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::TagAxis, p1);
            }
        }
        Context::TagValue(axis_name) => {
            if let Some(p1) = phase1 {
                add_extend_values(&mut items, &axis_name, p1);
            }
        }
        Context::General => {
            if let Some(p1) = phase1 {
                add_all_symbols(&mut items, file_id, p1);
            }
        }
    }

    CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    })
}

// ---------------------------------------------------------------------------
// Keyword / field tables with descriptions
// ---------------------------------------------------------------------------

const TOP_LEVEL_KEYWORDS: &[(&str, &str)] = &[
    ("@use", "Import symbols from another file"),
    ("@reference", "Reference symbols from another file (read-only)"),
    ("@extend", "Add values to a tag axis"),
    ("@render", "Configure .hut token rendering"),
    ("entry", "Define a dictionary entry"),
    ("tagaxis", "Define a grammatical dimension"),
    ("inflection", "Define an inflection paradigm"),
    ("phonrule", "Define phonological rewrite rules"),
];

const ENTRY_FIELD_KEYWORDS: &[(&str, &str)] = &[
    ("headword", "Entry headword form"),
    ("tags", "Tag classifications (e.g. part of speech)"),
    ("stems", "Stem definitions for inflection"),
    ("inflection_class", "Named inflection paradigm"),
    ("inflect", "Inline inflection rules"),
    ("meaning", "Single meaning definition"),
    ("meanings", "Multiple meaning definitions"),
    ("forms_override", "Override specific inflected forms"),
    ("etymology", "Etymological information"),
    ("examples", "Example sentences"),
];

const TAGAXIS_FIELD_KEYWORDS: &[(&str, &str)] = &[
    ("role", "inflectional | classificatory | structural"),
    ("display", "Display name map (e.g. { ja: \"...\", en: \"...\" })"),
    ("index", "Search index kind: exact | fulltext"),
];

// ---------------------------------------------------------------------------
// Completion item builders
// ---------------------------------------------------------------------------

fn keyword_item(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}

fn field_item(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Context enum
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Context {
    TopLevel,
    EntryBody,
    TagAxisBody,
    ExtendBody,
    InflectionBody,
    PhonRuleBody,
    RenderBody,
    EtymologyBody,
    ExamplesBody,
    FormsOverrideBody,
    InflectionClass,
    TagAxis,
    TagValue(String),
    HutDefault,
    General,
}

/// Collect preceding tokens for the given file and offset.
fn preceding_tokens<'a>(tokens: &'a [Token], file_id: FileId, offset: usize) -> Vec<&'a Token> {
    tokens
        .iter()
        .filter(|t| t.span.file_id == file_id && t.span.end <= offset)
        .collect()
}

// ---------------------------------------------------------------------------
// .hut context detection
// ---------------------------------------------------------------------------

fn determine_hut_context(tokens: &[Token], file_id: FileId, offset: usize) -> Context {
    let preceding = preceding_tokens(tokens, file_id, offset);
    if preceding.is_empty() {
        return Context::HutDefault;
    }
    let len = preceding.len();

    if len >= 2 {
        if let TokenKind::Eq = &preceding[len - 1].node {
            if let TokenKind::Ident(axis) = &preceding[len - 2].node {
                return Context::TagValue(axis.clone());
            }
        }
    }

    if let TokenKind::LBracket = &preceding[len - 1].node {
        return Context::TagAxis;
    }

    if is_inside_brackets(&preceding) {
        if let TokenKind::Comma = &preceding[len - 1].node {
            return Context::TagAxis;
        }
    }

    Context::HutDefault
}

// ---------------------------------------------------------------------------
// .hu context detection
// ---------------------------------------------------------------------------

fn determine_hu_context(tokens: &[Token], file_id: FileId, offset: usize) -> Context {
    let preceding = preceding_tokens(tokens, file_id, offset);

    if preceding.is_empty() {
        return Context::TopLevel;
    }

    let len = preceding.len();

    // --- High-priority token-specific contexts ---

    if len >= 2 {
        if let TokenKind::Colon = &preceding[len - 1].node {
            if let TokenKind::Ident(name) = &preceding[len - 2].node {
                if name == "inflection_class" {
                    return Context::InflectionClass;
                }
            }
        }
    }

    if len >= 2 {
        if let TokenKind::Eq = &preceding[len - 1].node {
            if let TokenKind::Ident(axis) = &preceding[len - 2].node {
                return Context::TagValue(axis.clone());
            }
        }
    }

    if let TokenKind::LBracket = &preceding[len - 1].node {
        return Context::TagAxis;
    }

    if let TokenKind::Ident(name) = &preceding[len - 1].node {
        if name == "on" {
            return Context::TagAxis;
        }
    }

    if is_inside_brackets(&preceding) {
        if let TokenKind::Comma = &preceding[len - 1].node {
            return Context::TagAxis;
        }
    }

    // --- Brace-nesting-based context detection ---

    let (depth, item_depth) = compute_brace_depths(&preceding);

    if depth == 0 {
        return Context::TopLevel;
    }

    if item_depth >= 2 {
        if let Some(sub) = find_sub_block_context(&preceding) {
            return sub;
        }
    }

    find_enclosing_item_context(&preceding).unwrap_or(Context::General)
}

fn compute_brace_depths(preceding: &[&Token]) -> (usize, usize) {
    let mut depth: usize = 0;
    for tok in preceding {
        match &tok.node {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (depth, depth)
}

fn find_sub_block_context(preceding: &[&Token]) -> Option<Context> {
    let mut depth: usize = 0;
    let mut last_subblock_idx = None;

    for (i, tok) in preceding.iter().enumerate() {
        match &tok.node {
            TokenKind::LBrace => {
                depth += 1;
                if depth >= 2 {
                    last_subblock_idx = Some(i);
                }
            }
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    let idx = last_subblock_idx?;
    if idx == 0 {
        return None;
    }
    if let TokenKind::Ident(name) = &preceding[idx - 1].node {
        return match name.as_str() {
            "etymology" => Some(Context::EtymologyBody),
            "examples" => Some(Context::ExamplesBody),
            "forms_override" => Some(Context::FormsOverrideBody),
            "meanings" => Some(Context::General),
            _ => None,
        };
    }
    None
}

fn find_enclosing_item_context(preceding: &[&Token]) -> Option<Context> {
    let mut depth: usize = 0;
    for i in (0..preceding.len()).rev() {
        match &preceding[i].node {
            TokenKind::RBrace => depth += 1,
            TokenKind::LBrace => {
                if depth == 0 {
                    return item_keyword_before(preceding, i);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn item_keyword_before(preceding: &[&Token], brace_idx: usize) -> Option<Context> {
    for i in (0..brace_idx).rev() {
        match &preceding[i].node {
            TokenKind::Ident(name) => {
                return match name.as_str() {
                    "entry" => Some(Context::EntryBody),
                    "tagaxis" => Some(Context::TagAxisBody),
                    "inflection" => Some(Context::InflectionBody),
                    "phonrule" => Some(Context::PhonRuleBody),
                    _ => continue,
                };
            }
            TokenKind::AtExtend => return Some(Context::ExtendBody),
            TokenKind::AtRender => return Some(Context::RenderBody),
            TokenKind::LBrace | TokenKind::RBrace => {
                return None;
            }
            _ => continue,
        }
    }
    None
}

fn is_inside_brackets(preceding: &[&Token]) -> bool {
    let mut bracket_depth: usize = 0;
    for tok in preceding.iter() {
        match &tok.node {
            TokenKind::LBracket => bracket_depth += 1,
            TokenKind::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
    }
    bracket_depth > 0
}

// ---------------------------------------------------------------------------
// Symbol helpers
// ---------------------------------------------------------------------------

/// Build a detail string for a symbol by looking up its AST item.
fn symbol_detail(sym_file_id: FileId, item_index: usize, phase1: &Phase1Result) -> Option<String> {
    let file_ast = phase1.files.get(&sym_file_id)?;
    let item_spanned = file_ast.items.get(item_index)?;
    Some(match &item_spanned.node {
        Item::Entry(e) => format_entry_detail(e),
        Item::Inflection(i) => {
            let axes: Vec<_> = i.axes.iter().map(|a| a.node.as_str()).collect();
            format!("for {{{}}}", axes.join(", "))
        }
        Item::TagAxis(t) => {
            let role = match t.role.node {
                ast::Role::Inflectional => "inflectional",
                ast::Role::Classificatory => "classificatory",
                ast::Role::Structural => "structural",
            };
            role.to_string()
        }
        Item::Extend(ext) => {
            let values: Vec<_> = ext.values.iter().map(|v| v.name.node.as_str()).collect();
            format!("on {} — {}", ext.target_axis.node, values.join(", "))
        }
        Item::PhonRule(p) => {
            format!("{} classes, {} rules", p.classes.len(), p.rules.len())
        }
        _ => return None,
    })
}

/// Format entry detail in dictionary style (A-style):
///
/// ```text
/// faren
/// strong_I
///
/// "to go"
/// ```
fn format_entry_detail(e: &ast::Entry) -> String {
    let headword = match &e.headword {
        ast::Headword::Simple(s) => s.node.clone(),
        ast::Headword::MultiScript(scripts) => scripts
            .iter()
            .map(|(k, v)| format!("{}: {}", k.node, v.node))
            .collect::<Vec<_>>()
            .join(", "),
    };

    let mut lines = headword;

    // Inflection class / inline inflection
    match &e.inflection {
        Some(ast::EntryInflection::Class(c)) => {
            lines.push_str(&format!("\n{}", c.node));
        }
        Some(ast::EntryInflection::Inline(_)) => {
            lines.push_str("\ninline inflection");
        }
        None => {}
    }

    // Meaning(s)
    lines.push('\n');
    match &e.meaning {
        ast::MeaningDef::Single(s) => {
            lines.push_str(&format!("\n\"{}\"", s.node));
        }
        ast::MeaningDef::Multiple(entries) => {
            for (i, m) in entries.iter().enumerate() {
                lines.push_str(&format!("\n{}. \"{}\"", i + 1, m.text.node));
            }
        }
    }

    lines
}

/// Build a documentation string for a symbol.
fn symbol_documentation(
    sym_file_id: FileId,
    item_index: usize,
    phase1: &Phase1Result,
) -> Option<Documentation> {
    let file_ast = phase1.files.get(&sym_file_id)?;
    let item_spanned = file_ast.items.get(item_index)?;
    let md = match &item_spanned.node {
        Item::Entry(e) => format_entry_doc(e),
        Item::Inflection(i) => format_inflection_doc(i),
        Item::TagAxis(t) => format_tagaxis_doc(t),
        Item::PhonRule(p) => format_phonrule_doc(p),
        Item::Extend(ext) => format_extend_doc(ext),
        _ => return None,
    };
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: md,
    }))
}

/// Format entry documentation in compact dictionary style (B-style):
///
/// ```markdown
/// **faren** (*strong_I*) — "to go"
/// stems: pres, past
/// ```
fn format_entry_doc(e: &ast::Entry) -> String {
    let headword = match &e.headword {
        ast::Headword::Simple(s) => s.node.clone(),
        ast::Headword::MultiScript(scripts) => scripts
            .iter()
            .map(|(k, v)| format!("{}: {}", k.node, v.node))
            .collect::<Vec<_>>()
            .join(", "),
    };

    let infl_part = match &e.inflection {
        Some(ast::EntryInflection::Class(c)) => format!(" (*{}*)", c.node),
        Some(ast::EntryInflection::Inline(_)) => " (*inline*)" .into(),
        None => String::new(),
    };

    let meaning = match &e.meaning {
        ast::MeaningDef::Single(s) => format!("\"{}\"", s.node),
        ast::MeaningDef::Multiple(entries) => entries
            .iter()
            .map(|m| format!("\"{}\"", m.text.node))
            .collect::<Vec<_>>()
            .join("; "),
    };

    let mut lines = format!("**{}**{} — {}", headword, infl_part, meaning);

    if !e.stems.is_empty() {
        let stems: Vec<_> = e.stems.iter().map(|s| s.name.node.as_str()).collect();
        lines.push_str(&format!("  \nstems: {}", stems.join(", ")));
    }

    if !e.tags.is_empty() {
        let tags: Vec<_> = e.tags
            .iter()
            .map(|t| format!("{}={}", t.axis.node, t.value.node))
            .collect();
        lines.push_str(&format!("  \ntags: {}", tags.join(", ")));
    }

    lines
}

fn format_inflection_doc(i: &ast::Inflection) -> String {
    let axes: Vec<_> = i.axes.iter().map(|a| a.node.as_str()).collect();
    let stems: Vec<_> = i.required_stems.iter().map(|s| s.name.node.as_str()).collect();
    let (rule_count, body_kind) = match &i.body {
        ast::InflectionBody::Rules(rules) => (rules.len(), "rules"),
        ast::InflectionBody::Compose(c) => (
            c.slots.iter().map(|s| s.rules.len()).sum::<usize>() + c.overrides.len(),
            "compose",
        ),
    };
    format!(
        "```hubullu\ninflection {} for {{{}}}\n```\n---\n\
         **requires stems**: {}\\\n\
         **{} rules** ({})",
        i.name.node,
        axes.join(", "),
        if stems.is_empty() { "none".to_string() } else { stems.join(", ") },
        rule_count,
        body_kind
    )
}

fn format_tagaxis_doc(t: &ast::TagAxis) -> String {
    let role = match t.role.node {
        ast::Role::Inflectional => "inflectional",
        ast::Role::Classificatory => "classificatory",
        ast::Role::Structural => "structural",
    };
    let mut lines = format!(
        "```hubullu\ntagaxis {}\n```\n---\n**role**: {}",
        t.name.node, role
    );
    if let Some(ref idx) = t.index {
        let kind = match idx.node {
            ast::IndexKind::Exact => "exact",
            ast::IndexKind::Fulltext => "fulltext",
        };
        lines.push_str(&format!("\\\n**index**: {}", kind));
    }
    lines
}

fn format_phonrule_doc(p: &ast::PhonRule) -> String {
    format!(
        "```hubullu\nphonrule {}\n```\n---\n\
         **classes**: {}, **maps**: {}, **rules**: {}",
        p.name.node, p.classes.len(), p.maps.len(), p.rules.len()
    )
}

fn format_extend_doc(ext: &ast::Extend) -> String {
    let values: Vec<_> = ext.values.iter().map(|v| v.name.node.as_str()).collect();
    format!(
        "```hubullu\n@extend {} on {}\n```\n---\n**values**: {}",
        ext.name.node, ext.target_axis.node, values.join(", ")
    )
}

/// Build a documentation string for an @extend value.
fn extend_value_documentation(
    val: &ast::ExtendValue,
    ext: &ast::Extend,
) -> Option<Documentation> {
    let display: Vec<_> = val
        .display
        .iter()
        .map(|(k, v)| format!("{}: \"{}\"", k.node, v.node))
        .collect();
    if display.is_empty() && val.slots.is_empty() {
        return None;
    }
    let mut lines = format!("**axis**: {}", ext.target_axis.node);
    if !display.is_empty() {
        lines.push_str(&format!("\\\n**display**: {}", display.join(", ")));
    }
    if !val.slots.is_empty() {
        let slots: Vec<_> = val.slots.iter().map(|s| s.node.as_str()).collect();
        lines.push_str(&format!("\\\n**slots**: {}", slots.join(", ")));
    }
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: lines,
    }))
}

fn add_symbols_of_kind(
    items: &mut Vec<CompletionItem>,
    file_id: FileId,
    kind: SymbolKind,
    phase1: &Phase1Result,
) {
    if let Some(scope) = phase1.symbol_table.scope(file_id) {
        for sym in scope.locals.values() {
            if sym.kind == kind {
                items.push(CompletionItem {
                    label: sym.name.clone(),
                    kind: Some(symbol_kind_to_completion(kind)),
                    detail: symbol_detail(sym.file_id, sym.item_index, phase1),
                    documentation: symbol_documentation(sym.file_id, sym.item_index, phase1),
                    ..Default::default()
                });
            }
        }
        for imp in &scope.imports {
            if imp.kind == kind {
                items.push(CompletionItem {
                    label: imp.local_name.clone(),
                    kind: Some(symbol_kind_to_completion(kind)),
                    detail: symbol_detail(imp.source_file, imp.item_index, phase1),
                    documentation: symbol_documentation(imp.source_file, imp.item_index, phase1),
                    ..Default::default()
                });
            }
        }
    }
}

fn add_extend_values(
    items: &mut Vec<CompletionItem>,
    axis_name: &str,
    phase1: &Phase1Result,
) {
    for file_ast in phase1.files.values() {
        for item_spanned in &file_ast.items {
            if let Item::Extend(ext) = &item_spanned.node {
                if ext.target_axis.node == axis_name {
                    for val in &ext.values {
                        let display: Vec<_> = val
                            .display
                            .iter()
                            .map(|(k, v)| format!("{}: \"{}\"", k.node, v.node))
                            .collect();
                        items.push(CompletionItem {
                            label: val.name.node.clone(),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
                            detail: if display.is_empty() {
                                None
                            } else {
                                Some(display.join(", "))
                            },
                            documentation: extend_value_documentation(val, ext),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }
}

fn add_all_symbols(
    items: &mut Vec<CompletionItem>,
    file_id: FileId,
    phase1: &Phase1Result,
) {
    if let Some(scope) = phase1.symbol_table.scope(file_id) {
        for sym in scope.locals.values() {
            items.push(CompletionItem {
                label: sym.name.clone(),
                kind: Some(symbol_kind_to_completion(sym.kind)),
                detail: symbol_detail(sym.file_id, sym.item_index, phase1),
                documentation: symbol_documentation(sym.file_id, sym.item_index, phase1),
                ..Default::default()
            });
        }
        for imp in &scope.imports {
            items.push(CompletionItem {
                label: imp.local_name.clone(),
                kind: Some(symbol_kind_to_completion(imp.kind)),
                detail: symbol_detail(imp.source_file, imp.item_index, phase1),
                documentation: symbol_documentation(imp.source_file, imp.item_index, phase1),
                ..Default::default()
            });
        }
    }
}

fn symbol_kind_to_completion(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Entry => CompletionItemKind::VALUE,
        SymbolKind::Inflection => CompletionItemKind::CLASS,
        SymbolKind::TagAxis => CompletionItemKind::ENUM,
        SymbolKind::Extend => CompletionItemKind::MODULE,
        SymbolKind::PhonRule => CompletionItemKind::FUNCTION,
    }
}
