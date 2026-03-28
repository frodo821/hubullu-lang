//! Folding ranges for code folding in editors.

use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::ParseResult;

use super::convert;

/// Produce folding ranges for all top-level items and brace-delimited blocks.
pub fn folding_ranges(parse_result: &ParseResult) -> Vec<FoldingRange> {
    let source_map = &parse_result.source_map;
    let mut ranges = Vec::new();

    for item_spanned in &parse_result.file.items {
        let range = convert::span_to_range(&item_spanned.span, source_map);
        // Only fold multi-line items.
        if range.start.line < range.end.line {
            ranges.push(FoldingRange {
                start_line: range.start.line,
                start_character: Some(range.start.character),
                end_line: range.end.line,
                end_character: Some(range.end.character),
                kind: Some(FoldingRangeKind::Region),
                collapsed_text: None,
            });
        }
    }

    ranges
}
