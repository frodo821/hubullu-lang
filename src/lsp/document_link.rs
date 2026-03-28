//! Document links handler — makes @use/@reference paths clickable.

use lsp_types::DocumentLink;

use crate::ast::Item;
use crate::phase1::Phase1Result;
use crate::ParseResult;

use super::convert;

/// Produce document links for import paths in a file.
pub fn document_links(
    parse_result: &ParseResult,
    phase1: Option<&Phase1Result>,
) -> Vec<DocumentLink> {
    let source_map = &parse_result.source_map;
    let mut links = Vec::new();

    for item_spanned in &parse_result.file.items {
        let import = match &item_spanned.node {
            Item::Use(imp) | Item::Reference(imp) => imp,
            _ => continue,
        };

        let range = convert::span_to_range(&import.path.span, source_map);

        // Try to resolve the path to an actual file URI.
        let target = phase1.and_then(|p1| {
            resolve_import_target(&import.path.node, p1)
        });

        links.push(DocumentLink {
            range,
            target,
            tooltip: Some(format!("Open {}", import.path.node)),
            data: None,
        });
    }

    links
}

fn resolve_import_target(
    path_str: &str,
    phase1: &Phase1Result,
) -> Option<lsp_types::Uri> {
    for fid in phase1.source_map.file_ids() {
        let file_path = phase1.source_map.path(fid);
        if file_path.to_string_lossy().ends_with(path_str) {
            return convert::path_to_uri(file_path);
        }
    }
    None
}
