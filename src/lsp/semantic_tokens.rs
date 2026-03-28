//! Semantic token generation from the hubullu token stream.

use lsp_types::{
    SemanticToken, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
    SemanticTokensResult,
};

use crate::ast::Span;
use crate::span::{FileId, SourceMap};
use crate::token::{Token, TokenKind};

// Token type indices (must match LEGEND order).
const KEYWORD: u32 = 0;
const STRING: u32 = 1;
const VARIABLE: u32 = 2;
const _TYPE: u32 = 3;
const OPERATOR: u32 = 4;
const COMMENT: u32 = 5;
const _NAMESPACE: u32 = 6;

/// Build the semantic tokens legend advertised during initialization.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,   // 0
            SemanticTokenType::STRING,    // 1
            SemanticTokenType::VARIABLE,  // 2
            SemanticTokenType::TYPE,      // 3
            SemanticTokenType::OPERATOR,  // 4
            SemanticTokenType::COMMENT,   // 5
            SemanticTokenType::NAMESPACE, // 6
        ],
        token_modifiers: vec![],
    }
}

/// Known bare keywords that appear as `Ident` tokens.
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
) -> SemanticTokensResult {
    let mut result: Vec<(u32, u32, u32, u32)> = Vec::new(); // (line, col, len, type)

    // Collect tokens from the lexer output.
    for tok in tokens {
        if tok.span.file_id != file_id {
            continue;
        }
        let token_type = match &tok.node {
            TokenKind::AtUse | TokenKind::AtReference | TokenKind::AtExtend | TokenKind::AtRender => KEYWORD,
            TokenKind::Ident(name) if KEYWORDS.contains(&name.as_str()) => KEYWORD,
            TokenKind::Ident(_) => VARIABLE,
            TokenKind::StringLit(_) | TokenKind::TemplateLit(_) => STRING,
            TokenKind::Arrow | TokenKind::Plus | TokenKind::Star | TokenKind::Pipe
            | TokenKind::Tilde | TokenKind::Eq | TokenKind::Bang | TokenKind::Slash => OPERATOR,
            TokenKind::Eof => continue,
            // Punctuation tokens — skip.
            _ => continue,
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
