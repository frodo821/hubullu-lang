//! Document formatting — normalize indentation and sort imports.
//!
//! This formatter adjusts leading indentation based on brace nesting depth and
//! sorts contiguous blocks of `@use` / `@reference` imports alphabetically.

use lsp_types::{Position, Range, TextEdit};

const INDENT: &str = "    ";

/// Returns `true` if `trimmed` begins with an import directive (`@use` or `@reference`).
fn is_import_line(trimmed: &str) -> bool {
    trimmed.starts_with("@use ") || trimmed.starts_with("@reference ")
}

/// Sort contiguous blocks of import lines and emit replacement edits.
fn sort_imports(text: &str, edits: &mut Vec<TextEdit>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if !is_import_line(lines[i].trim()) {
            i += 1;
            continue;
        }
        // Found the start of an import block.
        let block_start = i;
        while i < lines.len() && is_import_line(lines[i].trim()) {
            i += 1;
        }
        let block_end = i; // exclusive

        let mut sorted: Vec<&str> = lines[block_start..block_end].to_vec();
        sorted.sort_by(|a, b| a.trim().cmp(b.trim()));

        for (offset, &original) in lines[block_start..block_end].iter().enumerate() {
            let idx = block_start + offset;
            let target = sorted[offset];
            if original != target {
                edits.push(TextEdit {
                    range: Range {
                        start: Position { line: idx as u32, character: 0 },
                        end: Position { line: idx as u32, character: original.len() as u32 },
                    },
                    new_text: target.to_string(),
                });
            }
        }
    }
}

/// Format an entire document by normalizing indentation and sorting imports.
pub fn format_document(text: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut depth: usize = 0;

    for (line_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Leading closing brace decreases depth before indenting this line.
        if trimmed.starts_with('}') {
            depth = depth.saturating_sub(1);
        }

        let expected_indent = INDENT.repeat(depth);
        let current_indent = &line[..line.len() - line.trim_start().len()];

        if current_indent != expected_indent {
            edits.push(TextEdit {
                range: Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: current_indent.len() as u32,
                    },
                },
                new_text: expected_indent,
            });
        }

        // Count braces on this line to update depth for subsequent lines.
        // Skip the leading '}' if already handled above to avoid double-counting.
        let count_from = if trimmed.starts_with('}') { 1 } else { 0 };
        for ch in trimmed[count_from..].chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                '"' => break, // Don't count braces inside strings.
                '#' => break, // Don't count braces in comments.
                _ => {}
            }
        }
    }

    sort_imports(text, &mut edits);

    edits
}
