//! Semantic token generation from the hubullu token stream and AST.
//!
//! Uses AST analysis to provide context-aware semantic classifications,
//! falling back to token-kind-based classification for tokens not covered
//! by the AST (keywords, operators, punctuation).

use std::collections::HashMap;

use lsp_types::{
    SemanticToken, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
    SemanticTokensResult,
};

use crate::ast::{self, Span};
use crate::span::{FileId, SourceMap};
use crate::token::{Token, TokenKind};

// Token type indices (must match LEGEND order).
const KEYWORD: u32 = 0;
const STRING: u32 = 1;
const VARIABLE: u32 = 2;
const TYPE: u32 = 3;
const OPERATOR: u32 = 4;
const COMMENT: u32 = 5;
const NAMESPACE: u32 = 6;
const PROPERTY: u32 = 7;
const ENUM_MEMBER: u32 = 8;

/// Build the semantic tokens legend advertised during initialization.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,      // 0
            SemanticTokenType::STRING,       // 1
            SemanticTokenType::VARIABLE,     // 2
            SemanticTokenType::TYPE,         // 3
            SemanticTokenType::OPERATOR,     // 4
            SemanticTokenType::COMMENT,      // 5
            SemanticTokenType::NAMESPACE,    // 6
            SemanticTokenType::PROPERTY,     // 7
            SemanticTokenType::new("enumMember"), // 8
        ],
        token_modifiers: vec![],
    }
}

/// Known bare keywords that appear as `Ident` tokens.
/// Used as fallback when an ident has no AST-derived classification.
const KEYWORDS: &[&str] = &[
    "entry",
    "tagaxis",
    "inflection",
    "phonrule",
    "headword",
    "stems",
    "meaning",
    "meanings",
    "inflection_class",
    "inflect",
    "for",
    "requires",
    "compose",
    "slot",
    "override",
    "on",
    "as",
    "null",
    "role",
    "inflectional",
    "classificatory",
    "structural",
    "display",
    "index",
    "exact",
    "fulltext",
    "tags",
    "forms",
    "etymology",
    "proto",
    "cognates",
    "derived_from",
    "note",
    "examples",
    "class",
    "map",
    "match",
    "else",
    "separator",
    "no_separator_before",
    "with",
    "harmony",
];

/// Generate semantic tokens for a document.
pub fn generate(
    tokens: &[Token],
    comment_spans: &[Span],
    file_id: FileId,
    source_map: &SourceMap,
    ast_file: &ast::File,
) -> SemanticTokensResult {
    let ast_classes = build_ast_classifications(ast_file, file_id);
    let mut result: Vec<(u32, u32, u32, u32)> = Vec::new(); // (line, col, len, type)

    // Classify tokens using AST-derived types, falling back to token-kind.
    for tok in tokens {
        if tok.span.file_id != file_id {
            continue;
        }
        let key = (tok.span.start, tok.span.end);
        let token_type = if let Some(&ty) = ast_classes.get(&key) {
            ty
        } else {
            match &tok.node {
                TokenKind::AtUse | TokenKind::AtReference | TokenKind::AtExtend | TokenKind::AtRender => KEYWORD,
                TokenKind::Ident(name) if KEYWORDS.contains(&name.as_str()) => KEYWORD,
                TokenKind::Ident(_) => VARIABLE,
                TokenKind::StringLit(_) | TokenKind::TemplateLit(_) => STRING,
                TokenKind::Arrow | TokenKind::Plus | TokenKind::Star | TokenKind::Pipe
                | TokenKind::Tilde | TokenKind::Eq | TokenKind::Bang | TokenKind::Slash | TokenKind::DoubleSlash => OPERATOR,
                TokenKind::Eof => continue,
                _ => continue,
            }
        };
        let (line, col, len) = span_to_line_col_len(&tok.span, source_map);
        result.push((line, col, len, token_type));
    }

    // Collect comment spans.
    for span in comment_spans {
        if span.file_id != file_id {
            continue;
        }
        let (line, col, len) = span_to_line_col_len(span, source_map);
        result.push((line, col, len, COMMENT));
    }

    // Sort by position (line, then column).
    result.sort_by_key(|&(line, col, _, _)| (line, col));

    // Delta-encode.
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    let data: Vec<SemanticToken> = result
        .iter()
        .map(|&(line, col, len, token_type)| {
            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 {
                col - prev_start
            } else {
                col
            };
            prev_line = line;
            prev_start = col;
            SemanticToken {
                delta_line,
                delta_start,
                length: len,
                token_type,
                token_modifiers_bitset: 0,
            }
        })
        .collect();

    SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    })
}

/// Generate semantic tokens for a `.hut` file using token-context heuristics.
///
/// Since `.hut` files have no AST, we infer token roles from surrounding context:
/// - Ident followed by `[` or at line start without `=` context → entry ref (TYPE)
/// - Ident before `=` inside `[…]` → tag axis (TYPE)
/// - Ident after `=` inside `[…]` → tag value (ENUM_MEMBER)
/// - Comma-separated idents inside `[…]` without `=` → tag axis (TYPE)
///
/// Comments are extracted directly from the source text since the lexer skips them.
pub fn generate_hut(
    tokens: &[Token],
    file_id: FileId,
    source_map: &SourceMap,
) -> SemanticTokensResult {
    let mut result: Vec<(u32, u32, u32, u32)> = Vec::new();

    // Classify tokens using context heuristics.
    let file_tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.span.file_id == file_id)
        .collect();

    let mut bracket_depth: usize = 0;

    // Track whether we are still in the @reference header region.
    let mut in_header = true;

    for (i, tok) in file_tokens.iter().enumerate() {
        let token_type = match &tok.node {
            // @reference directive tokens
            TokenKind::AtReference if in_header => KEYWORD,
            TokenKind::Star if in_header => OPERATOR,
            TokenKind::Ident(name) if in_header && (name == "from" || name == "as") => KEYWORD,
            TokenKind::Ident(_) if in_header && bracket_depth == 0 => {
                // Check if this is a namespace alias or named import entry.
                // If followed by @reference or StringLit → still in header.
                NAMESPACE
            }
            TokenKind::StringLit(_) if in_header => STRING,

            TokenKind::LBracket => {
                in_header = false;
                bracket_depth += 1;
                continue;
            }
            TokenKind::RBracket => {
                bracket_depth = bracket_depth.saturating_sub(1);
                continue;
            }
            TokenKind::Ident(name) if bracket_depth > 0 => {
                in_header = false;
                // Inside brackets: check if this is axis (before =) or value (after =).
                let next = file_tokens.get(i + 1).map(|t| &t.node);
                let prev = if i > 0 {
                    file_tokens.get(i - 1).map(|t| &t.node)
                } else {
                    None
                };
                if matches!(next, Some(TokenKind::Eq)) {
                    TYPE // axis name
                } else if matches!(prev, Some(TokenKind::Eq)) {
                    ENUM_MEMBER // tag value
                } else if KEYWORDS.contains(&name.as_str()) {
                    KEYWORD
                } else {
                    VARIABLE
                }
            }
            TokenKind::Ident(name) if KEYWORDS.contains(&name.as_str()) => {
                in_header = false;
                KEYWORD
            }
            TokenKind::Ident(_) => {
                in_header = false;
                // Outside brackets: entry reference.
                TYPE
            }
            TokenKind::StringLit(_) | TokenKind::TemplateLit(_) => {
                in_header = false;
                STRING
            }
            TokenKind::Arrow | TokenKind::Plus | TokenKind::Star | TokenKind::Pipe
            | TokenKind::Tilde | TokenKind::Eq | TokenKind::Bang | TokenKind::Slash | TokenKind::DoubleSlash => OPERATOR,
            TokenKind::Eof => continue,
            _ => continue,
        };
        let (line, col, len) = span_to_line_col_len(&tok.span, source_map);
        result.push((line, col, len, token_type));
    }

    // Extract comment spans from source text (lexer skips them).
    let source = source_map.source(file_id);
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let leading = line.len() - trimmed.len();
            let utf16_col = line[..leading].encode_utf16().count();
            let utf16_len = trimmed.encode_utf16().count();
            result.push((line_idx as u32, utf16_col as u32, utf16_len as u32, COMMENT));
        }
    }

    // Sort by position.
    result.sort_by_key(|&(line, col, _, _)| (line, col));

    // Delta-encode.
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    let data: Vec<SemanticToken> = result
        .iter()
        .map(|&(line, col, len, token_type)| {
            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 {
                col - prev_start
            } else {
                col
            };
            prev_line = line;
            prev_start = col;
            SemanticToken {
                delta_line,
                delta_start,
                length: len,
                token_type,
                token_modifiers_bitset: 0,
            }
        })
        .collect();

    SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    })
}

fn span_to_line_col_len(span: &Span, source_map: &SourceMap) -> (u32, u32, u32) {
    let (line_1, col_1) = source_map.line_col(span.file_id, span.start);
    let line_text = source_map.line_text(span.file_id, line_1);
    let byte_col = col_1 - 1;
    let utf16_col = line_text[..byte_col.min(line_text.len())]
        .encode_utf16()
        .count();
    let byte_len = span.end.saturating_sub(span.start);
    // Approximate UTF-16 length from source bytes in the span.
    let source = source_map.source(span.file_id);
    let span_text = &source[span.start..span.end.min(source.len())];
    let utf16_len = span_text.encode_utf16().count();
    (
        (line_1 - 1) as u32,
        utf16_col as u32,
        utf16_len.max(byte_len.min(1)) as u32,
    )
}

// ---------------------------------------------------------------------------
// AST-based classification
// ---------------------------------------------------------------------------

fn build_ast_classifications(file: &ast::File, file_id: FileId) -> HashMap<(usize, usize), u32> {
    let mut map = HashMap::new();
    for item in &file.items {
        classify_item(&item.node, file_id, &mut map);
    }
    map
}

fn put(map: &mut HashMap<(usize, usize), u32>, span: &Span, file_id: FileId, ty: u32) {
    if span.file_id == file_id {
        map.insert((span.start, span.end), ty);
    }
}

fn classify_item(item: &ast::Item, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match item {
        ast::Item::TagAxis(ta) => {
            put(map, &ta.name.span, fid, TYPE);
            put(map, &ta.role.span, fid, ENUM_MEMBER);
            classify_display(&ta.display, fid, map);
            if let Some(ref idx) = ta.index {
                put(map, &idx.span, fid, ENUM_MEMBER);
            }
        }
        ast::Item::Extend(ext) => {
            put(map, &ext.name.span, fid, TYPE);
            put(map, &ext.target_axis.span, fid, TYPE);
            for val in &ext.values {
                put(map, &val.name.span, fid, ENUM_MEMBER);
                classify_display(&val.display, fid, map);
            }
        }
        ast::Item::Inflection(infl) => classify_inflection(infl, fid, map),
        ast::Item::Entry(entry) => classify_entry(entry, fid, map),
        ast::Item::PhonRule(pr) => classify_phonrule(pr, fid, map),
        ast::Item::Use(imp) | ast::Item::Reference(imp) => classify_import(imp, fid, map),
        ast::Item::Export(exp) => {
            // Classify the import target names/aliases within the @export
            match &exp.target {
                ast::ImportTarget::Glob { alias } => {
                    if let Some(alias) = alias {
                        put(map, &alias.span, fid, NAMESPACE);
                    }
                }
                ast::ImportTarget::Named(entries) => {
                    for entry in entries {
                        put(map, &entry.name.span, fid, TYPE);
                        if let Some(ref alias) = entry.alias {
                            put(map, &alias.span, fid, TYPE);
                        }
                    }
                }
            }
        }
        ast::Item::Render(_) => {}
    }
}

fn classify_display(display: &ast::DisplayMap, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    for (key, _) in display {
        put(map, &key.span, fid, PROPERTY);
    }
}

fn classify_import(imp: &ast::Import, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match &imp.target {
        ast::ImportTarget::Glob { alias } => {
            if let Some(alias) = alias {
                put(map, &alias.span, fid, NAMESPACE);
            }
        }
        ast::ImportTarget::Named(entries) => {
            for entry in entries {
                put(map, &entry.name.span, fid, TYPE);
                if let Some(ref alias) = entry.alias {
                    put(map, &alias.span, fid, TYPE);
                }
            }
        }
    }
}

fn classify_inflection(infl: &ast::Inflection, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    put(map, &infl.name.span, fid, TYPE);
    for axis in &infl.axes {
        put(map, &axis.span, fid, TYPE);
    }
    for stem in &infl.required_stems {
        put(map, &stem.name.span, fid, VARIABLE);
        classify_tag_conditions(&stem.constraint, fid, map);
    }
    classify_inflection_body(&infl.body, fid, map);
}

fn classify_inflection_body(body: &ast::InflectionBody, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match body {
        ast::InflectionBody::Rules(rules) => {
            for rule in rules {
                classify_inflection_rule(rule, fid, map);
            }
        }
        ast::InflectionBody::Compose(comp) => {
            classify_compose_expr(&comp.chain, fid, map);
            for slot in &comp.slots {
                put(map, &slot.name.span, fid, VARIABLE);
                for rule in &slot.rules {
                    classify_inflection_rule(rule, fid, map);
                }
            }
            for rule in &comp.overrides {
                classify_inflection_rule(rule, fid, map);
            }
        }
    }
}

fn classify_inflection_rule(rule: &ast::InflectionRule, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    classify_tag_condition_list(&rule.condition, fid, map);
    classify_rule_rhs(&rule.rhs.node, fid, map);
}

fn classify_tag_condition_list(tcl: &ast::TagConditionList, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    for tc in &tcl.conditions {
        classify_tag_condition(tc, fid, map);
    }
}

fn classify_tag_conditions(conditions: &[ast::TagCondition], fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    for tc in conditions {
        classify_tag_condition(tc, fid, map);
    }
}

fn classify_tag_condition(tc: &ast::TagCondition, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    put(map, &tc.axis.span, fid, TYPE);
    put(map, &tc.value.span, fid, ENUM_MEMBER);
}

fn classify_rule_rhs(rhs: &ast::RuleRhs, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match rhs {
        ast::RuleRhs::Template(_) | ast::RuleRhs::Null => {}
        ast::RuleRhs::Delegate(del) => {
            put(map, &del.target.span, fid, TYPE);
            for tag in &del.tags {
                match tag {
                    ast::DelegateTag::Fixed(tc) => classify_tag_condition(tc, fid, map),
                    ast::DelegateTag::PassThrough(ident) => put(map, &ident.span, fid, TYPE),
                }
            }
            for sm in &del.stem_mapping {
                put(map, &sm.target_stem.span, fid, VARIABLE);
                match &sm.source {
                    ast::StemSource::Stem(ident) => {
                        put(map, &ident.span, fid, VARIABLE);
                    }
                    ast::StemSource::Literal(lit) => {
                        put(map, &lit.span, fid, STRING);
                    }
                }
            }
        }
        ast::RuleRhs::PhonApply { rule, inner } => {
            put(map, &rule.span, fid, TYPE);
            classify_rule_rhs(&inner.node, fid, map);
        }
    }
}

fn classify_compose_expr(expr: &ast::ComposeExpr, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match expr {
        ast::ComposeExpr::Slot(ident) => put(map, &ident.span, fid, VARIABLE),
        ast::ComposeExpr::Concat(exprs) => {
            for e in exprs {
                classify_compose_expr(e, fid, map);
            }
        }
        ast::ComposeExpr::PhonApply { rule, inner } => {
            put(map, &rule.span, fid, TYPE);
            classify_compose_expr(inner, fid, map);
        }
    }
}

fn classify_entry(entry: &ast::Entry, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    put(map, &entry.name.span, fid, TYPE);
    if let ast::Headword::MultiScript(scripts) = &entry.headword {
        for (key, _) in scripts {
            put(map, &key.span, fid, PROPERTY);
        }
    }
    classify_tag_conditions(&entry.tags, fid, map);
    for stem in &entry.stems {
        put(map, &stem.name.span, fid, VARIABLE);
    }
    if let Some(ref infl) = entry.inflection {
        match infl {
            ast::EntryInflection::Class(ident) => put(map, &ident.span, fid, TYPE),
            ast::EntryInflection::Inline(inline) => {
                for axis in &inline.axes {
                    put(map, &axis.span, fid, TYPE);
                }
                classify_inflection_body(&inline.body, fid, map);
            }
        }
    }
    for rule in &entry.forms_override {
        classify_inflection_rule(rule, fid, map);
    }
    if let ast::MeaningDef::Multiple(entries) = &entry.meaning {
        for me in entries {
            put(map, &me.ident.span, fid, PROPERTY);
        }
    }
    if let Some(ref ety) = entry.etymology {
        if let Some(ref df) = ety.derived_from {
            classify_entry_ref(df, fid, map);
        }
        for cog in &ety.cognates {
            classify_entry_ref(&cog.entry, fid, map);
        }
    }
    for ex in &entry.examples {
        for tok in &ex.tokens {
            if let ast::Token::Ref(er) = tok {
                classify_entry_ref(er, fid, map);
            }
        }
    }
}

fn classify_entry_ref(er: &ast::EntryRef, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    for ns in &er.namespace {
        put(map, &ns.span, fid, NAMESPACE);
    }
    put(map, &er.entry_id.span, fid, TYPE);
    if let Some(ref meaning) = er.meaning {
        put(map, &meaning.span, fid, PROPERTY);
    }
    if let Some(ref fs) = er.form_spec {
        classify_tag_condition_list(fs, fid, map);
    }
}

fn classify_phonrule(pr: &ast::PhonRule, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    put(map, &pr.name.span, fid, TYPE);
    for cls in &pr.classes {
        put(map, &cls.name.span, fid, TYPE);
        if let ast::CharClassBody::Union(refs) = &cls.body {
            for r in refs {
                put(map, &r.span, fid, TYPE);
            }
        }
    }
    for m in &pr.maps {
        put(map, &m.name.span, fid, TYPE);
        put(map, &m.param.span, fid, VARIABLE);
        let ast::PhonMapBody::Match { arms, else_arm } = &m.body;
        for arm in arms {
            if let ast::PhonMapResult::Var(v) = &arm.to {
                put(map, &v.span, fid, VARIABLE);
            }
        }
        if let Some(ast::PhonMapElse::Var(v)) = else_arm {
            put(map, &v.span, fid, VARIABLE);
        }
    }
    for rule in &pr.rules {
        if let ast::PhonPattern::Class(c) = &rule.from {
            put(map, &c.span, fid, TYPE);
        }
        if let ast::PhonReplacement::Map(m) = &rule.to {
            put(map, &m.span, fid, TYPE);
        }
        if let Some(ref ctx) = rule.context {
            classify_phon_context_elems(&ctx.left, fid, map);
            classify_phon_context_elems(&ctx.right, fid, map);
        }
    }
}

fn classify_phon_context_elems(elems: &[ast::PhonContextElem], fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    for elem in elems {
        classify_phon_context_elem(elem, fid, map);
    }
}

fn classify_phon_context_elem(elem: &ast::PhonContextElem, fid: FileId, map: &mut HashMap<(usize, usize), u32>) {
    match elem {
        ast::PhonContextElem::Class(c) | ast::PhonContextElem::NegClass(c) => {
            put(map, &c.span, fid, TYPE);
        }
        ast::PhonContextElem::Repeat(inner) => {
            classify_phon_context_elem(inner, fid, map);
        }
        ast::PhonContextElem::Alt(alts) => {
            for alt in alts {
                classify_phon_context_elem(alt, fid, map);
            }
        }
        _ => {}
    }
}
