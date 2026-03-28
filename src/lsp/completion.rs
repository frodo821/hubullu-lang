//! Completion handler.

use lsp_types::{CompletionItem, CompletionItemKind, CompletionList, CompletionResponse};

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
) -> CompletionResponse {
    let ctx = determine_context(tokens, file_id, offset);
    let mut items = Vec::new();

    match ctx {
        Context::TopLevel => {
            // Offer top-level keywords.
            for kw in TOP_LEVEL_KEYWORDS {
                items.push(CompletionItem {
                    label: kw.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..Default::default()
                });
            }
        }
        Context::InflectionClass => {
            // Offer inflection symbols.
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::Inflection, p1);
            }
        }
        Context::TagAxis => {
            // Offer tag axis symbols.
            if let Some(p1) = phase1 {
                add_symbols_of_kind(&mut items, file_id, SymbolKind::TagAxis, p1);
            }
        }
        Context::TagValue(axis_name) => {
            // Offer extend values for the given axis.
            if let Some(p1) = phase1 {
                add_extend_values(&mut items, &axis_name, file_id, p1);
            }
        }
        Context::General => {
            // Offer all symbols in scope.
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

const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "@use", "@reference", "@extend", "@render",
    "entry", "tagaxis", "inflection", "phonrule",
];

#[derive(Debug)]
enum Context {
    TopLevel,
    InflectionClass,
    TagAxis,
    TagValue(String),
    General,
}

/// Scan tokens backwards from cursor to determine completion context.
fn determine_context(tokens: &[Token], file_id: FileId, offset: usize) -> Context {
    // Find the last token before or at the cursor.
    let preceding: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.span.file_id == file_id && t.span.end <= offset)
        .collect();

    if preceding.is_empty() {
        return Context::TopLevel;
    }

    // Check the last few tokens for context patterns.
    let len = preceding.len();

    // After `inflection_class:` -> offer inflection names
    if len >= 2 {
        if let TokenKind::Colon = &preceding[len - 1].node {
            if let TokenKind::Ident(name) = &preceding[len - 2].node {
                if name == "inflection_class" {
                    return Context::InflectionClass;
                }
            }
        }
    }

    // After `[axis=` -> offer values for that axis
    if len >= 2 {
        if let TokenKind::Eq = &preceding[len - 1].node {
            if let TokenKind::Ident(axis) = &preceding[len - 2].node {
                return Context::TagValue(axis.clone());
            }
        }
    }

    // After `[` -> offer axis names
    if let TokenKind::LBracket = &preceding[len - 1].node {
        return Context::TagAxis;
    }

    // After `on` (in @extend context) -> offer axis names
    if let TokenKind::Ident(name) = &preceding[len - 1].node {
        if name == "on" {
            return Context::TagAxis;
        }
    }

    // Check if we're at top level (between items): if the last meaningful token
    // is a RBrace or if there are no preceding tokens in this scope.
    if let TokenKind::RBrace = &preceding[len - 1].node {
        return Context::TopLevel;
    }

    // Beginning of file or after directives
    if len == 0 {
        return Context::TopLevel;
    }

    Context::General
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
                    ..Default::default()
                });
            }
        }
        for imp in &scope.imports {
            if imp.kind == kind {
                items.push(CompletionItem {
                    label: imp.local_name.clone(),
                    kind: Some(symbol_kind_to_completion(kind)),
                    ..Default::default()
                });
            }
        }
    }
}

fn add_extend_values(
    items: &mut Vec<CompletionItem>,
    axis_name: &str,
    _file_id: FileId,
    phase1: &Phase1Result,
) {
    // Walk all files for @extend blocks targeting this axis.
    for file_ast in phase1.files.values() {
        for item_spanned in &file_ast.items {
            if let crate::ast::Item::Extend(ext) = &item_spanned.node {
                if ext.target_axis.node == axis_name {
                    for val in &ext.values {
                        items.push(CompletionItem {
                            label: val.name.node.clone(),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
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
                ..Default::default()
            });
        }
        for imp in &scope.imports {
            items.push(CompletionItem {
                label: imp.local_name.clone(),
                kind: Some(symbol_kind_to_completion(imp.kind)),
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
