//! Document links handler — makes @use/@reference paths clickable.

use lsp_types::DocumentLink;

use crate::ast::Item;
use crate::phase1::Phase1Result;
use crate::ParseResult;

use super::convert;

/// Produce document links for import paths in a `.hu` file.
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

        let target = phase1.and_then(|p1| resolve_import_target(&import.path.node, p1));

        if let Some(uri) = target {
            links.push(DocumentLink {
                range,
                target: Some(uri),
                tooltip: Some(format!("Open {}", import.path.node)),
                data: None,
            });
        }
    }

    links
}

/// Produce document links for `@reference` paths in a `.hut` file.
///
/// Uses the project's phase1 source map to resolve the target file URIs.
pub fn hut_reference_links(
    hut_text: &str,
    hut_filename: &str,
    phase1: Option<&Phase1Result>,
) -> Vec<DocumentLink> {
    let hut_file = match crate::render::parse_hut(hut_text, hut_filename) {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };

    let mut source_map = crate::span::SourceMap::new();
    let _file_id = source_map.add_file(hut_filename.into(), hut_text.to_string());

    // Match each @reference's path to its token span.
    let mut links = Vec::new();
    for reference in &hut_file.references {
        let range = convert::span_to_range(&reference.path.span, &source_map);
        let target = phase1.and_then(|p1| resolve_import_target(&reference.path.node, p1));
        if let Some(uri) = target {
            links.push(DocumentLink {
                range,
                target: Some(uri),
                tooltip: Some(format!("Open {}", reference.path.node)),
                data: None,
            });
        }
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
