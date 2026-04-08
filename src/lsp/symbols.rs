//! Document symbols and workspace symbols handlers.

use lsp_types::{DocumentSymbol, SymbolKind, SymbolInformation, WorkspaceSymbolResponse, Location};

use crate::ast::{self, Item};
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::ParseResult;

use super::convert;

/// Produce document symbols (outline) for a single file.
#[allow(deprecated)] // DocumentSymbol.deprecated field
pub fn document_symbols(parse_result: &ParseResult) -> Vec<DocumentSymbol> {
    let source_map = &parse_result.source_map;
    let file_id = parse_result.file_id;

    parse_result
        .file
        .items
        .iter()
        .map(|item_spanned| {
            let range = convert::span_to_range(&item_spanned.span, source_map);
            match &item_spanned.node {
                Item::Entry(e) => DocumentSymbol {
                    name: e.name.node.clone(),
                    detail: Some(entry_detail(e)),
                    kind: SymbolKind::CLASS,
                    range,
                    selection_range: convert::span_to_range(&e.name.span, source_map),
                    children: Some(entry_children(e, file_id, source_map)),
                    tags: None,
                    deprecated: None,
                },
                Item::Inflection(i) => DocumentSymbol {
                    name: i.name.node.clone(),
                    detail: Some(inflection_detail(i)),
                    kind: SymbolKind::FUNCTION,
                    range,
                    selection_range: convert::span_to_range(&i.name.span, source_map),
                    children: None,
                    tags: None,
                    deprecated: None,
                },
                Item::TagAxis(t) => DocumentSymbol {
                    name: t.name.node.clone(),
                    detail: Some(format!("{:?}", t.role.node).to_lowercase()),
                    kind: SymbolKind::ENUM,
                    range,
                    selection_range: convert::span_to_range(&t.name.span, source_map),
                    children: None,
                    tags: None,
                    deprecated: None,
                },
                Item::Extend(ext) => DocumentSymbol {
                    name: ext.name.node.clone(),
                    detail: Some(format!("on {}", ext.target_axis.node)),
                    kind: SymbolKind::MODULE,
                    range,
                    selection_range: convert::span_to_range(&ext.name.span, source_map),
                    children: Some(extend_children(ext, source_map)),
                    tags: None,
                    deprecated: None,
                },
                Item::PhonRule(p) => DocumentSymbol {
                    name: p.name.node.clone(),
                    detail: Some(format!(
                        "{} rules",
                        p.rules.len()
                    )),
                    kind: SymbolKind::OPERATOR,
                    range,
                    selection_range: convert::span_to_range(&p.name.span, source_map),
                    children: None,
                    tags: None,
                    deprecated: None,
                },
                Item::Use(imp) | Item::Reference(imp) => {
                    let kind_str = if matches!(&item_spanned.node, Item::Use(_)) {
                        "@use"
                    } else {
                        "@reference"
                    };
                    DocumentSymbol {
                        name: format!("{} \"{}\"", kind_str, imp.path.node),
                        detail: None,
                        kind: SymbolKind::NAMESPACE,
                        range,
                        selection_range: convert::span_to_range(&imp.path.span, source_map),
                        children: None,
                        tags: None,
                        deprecated: None,
                    }
                }
                Item::Export(exp) => {
                    let sub = if exp.is_use { "use" } else { "reference" };
                    let name = if let Some(ref path) = exp.path {
                        format!("@export {} \"{}\"", sub, path.node)
                    } else {
                        format!("@export {}", sub)
                    };
                    DocumentSymbol {
                        name,
                        detail: None,
                        kind: SymbolKind::NAMESPACE,
                        range,
                        selection_range: range,
                        children: None,
                        tags: None,
                        deprecated: None,
                    }
                }
                Item::Render(_) => DocumentSymbol {
                    name: "@render".into(),
                    detail: None,
                    kind: SymbolKind::PROPERTY,
                    range,
                    selection_range: range,
                    children: None,
                    tags: None,
                    deprecated: None,
                },
            }
        })
        .collect()
}

/// Produce workspace symbols from the project-level symbol table.
pub fn workspace_symbols(
    query: &str,
    phase1: &Phase1Result,
) -> WorkspaceSymbolResponse {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for (file_id, scope) in &phase1.symbol_table.scopes {
        for sym in scope.locals.values() {
            if query.is_empty() || sym.name.to_lowercase().contains(&query_lower) {
                let path = phase1.source_map.path(*file_id);
                if let Some(uri) = convert::path_to_uri(path) {
                    let range = convert::span_to_range(&sym.span, &phase1.source_map);
                    #[allow(deprecated)]
                    results.push(SymbolInformation {
                        name: sym.name.clone(),
                        kind: symbol_kind_to_lsp(sym.kind),
                        location: Location { uri, range },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
            }
        }
    }

    WorkspaceSymbolResponse::Flat(results)
}

fn symbol_kind_to_lsp(kind: crate::symbol_table::SymbolKind) -> SymbolKind {
    match kind {
        crate::symbol_table::SymbolKind::Entry => SymbolKind::CLASS,
        crate::symbol_table::SymbolKind::Inflection => SymbolKind::FUNCTION,
        crate::symbol_table::SymbolKind::TagAxis => SymbolKind::ENUM,
        crate::symbol_table::SymbolKind::Extend => SymbolKind::MODULE,
        crate::symbol_table::SymbolKind::PhonRule => SymbolKind::OPERATOR,
    }
}

fn entry_detail(e: &ast::Entry) -> String {
    let hw = match &e.headword {
        ast::Headword::Simple(s) => format!("\"{}\"", s.node),
        ast::Headword::MultiScript(scripts) => {
            scripts
                .first()
                .map(|(_, v)| format!("\"{}\"", v.node))
                .unwrap_or_default()
        }
    };
    hw
}

fn inflection_detail(i: &ast::Inflection) -> String {
    let axes: Vec<_> = i.axes.iter().map(|a| a.node.as_str()).collect();
    format!("for {{{}}}", axes.join(", "))
}

#[allow(deprecated)]
fn entry_children(
    e: &ast::Entry,
    _file_id: FileId,
    source_map: &crate::span::SourceMap,
) -> Vec<DocumentSymbol> {
    let mut children = Vec::new();

    for stem in &e.stems {
        children.push(DocumentSymbol {
            name: stem.name.node.clone(),
            detail: Some(format!("\"{}\"", stem.value.node)),
            kind: SymbolKind::FIELD,
            range: convert::span_to_range(&stem.name.span, source_map),
            selection_range: convert::span_to_range(&stem.name.span, source_map),
            children: None,
            tags: None,
            deprecated: None,
        });
    }

    children
}

#[allow(deprecated)]
fn extend_children(
    ext: &ast::Extend,
    source_map: &crate::span::SourceMap,
) -> Vec<DocumentSymbol> {
    ext.values
        .iter()
        .map(|v| DocumentSymbol {
            name: v.name.node.clone(),
            detail: None,
            kind: SymbolKind::ENUM_MEMBER,
            range: convert::span_to_range(&v.name.span, source_map),
            selection_range: convert::span_to_range(&v.name.span, source_map),
            children: None,
            tags: None,
            deprecated: None,
        })
        .collect()
}
