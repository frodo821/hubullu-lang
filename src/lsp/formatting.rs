//! Document formatting — normalize indentation within brace-delimited blocks.
//!
//! This is a whitespace-only formatter: it adjusts leading indentation based on
//! brace nesting depth without altering content or line breaks.

use lsp_types::{Position, Range, TextEdit};

const INDENT: &str = "    ";

/// Format an entire document by normalizing indentation.
pub fn format_document(text: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut depth: usize = 0;

    for (line_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Closing brace decreases depth before indenting this line.
        if trimmed.starts_with('}') || trimmed.starts_with(']') {
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

        // Count braces/brackets on this line to update depth.
        for ch in trimmed.chars() {
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => depth = depth.saturating_sub(1),
                '"' => break, // Don't count braces inside strings.
                '#' => break, // Don't count braces in comments.
                _ => {}
            }
        }
    }

    edits
}
