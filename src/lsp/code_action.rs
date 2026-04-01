//! LSP code actions for hubullu.
//!
//! Provides quick-fix actions such as inserting `# @suppress next-line:` or
//! `# @suppress entire-file:` comments to silence lint warnings.

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri,
    WorkspaceEdit,
};
use std::collections::HashMap;

use crate::lint::LintDiagnostic;
use crate::span::{FileId, SourceMap};

use super::convert;

/// Build code actions for lint diagnostics whose primary label falls within the
/// requested range.
pub fn code_actions(
    uri: &Uri,
    range: &Range,
    file_id: FileId,
    lint_diagnostics: &[LintDiagnostic],
    source_map: &SourceMap,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();

    for ld in lint_diagnostics {
        let label = match ld.diagnostic.labels.first() {
            Some(l) if l.span.file_id == file_id => l,
            _ => continue,
        };

        let diag_range = convert::span_to_range(&label.span, source_map);

        // Check if the diagnostic overlaps with the requested range.
        if diag_range.end < range.start || diag_range.start > range.end {
            continue;
        }

        let lsp_diag = super::diagnostics::lint_to_lsp_diagnostic(ld, uri, source_map);

        // --- Action 1: suppress next-line ---
        {
            let (line_1based, _) = source_map.line_col(label.span.file_id, label.span.start);
            let line_text = source_map.line_text(file_id, line_1based);

            let indent: String = line_text
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();

            let suppress_line = format!("{}# @suppress next-line: {}\n", indent, ld.rule);

            let insert_pos = lsp_types::Position {
                line: (line_1based - 1) as u32,
                character: 0,
            };

            let mut changes = HashMap::new();
            changes.insert(uri.clone(), vec![TextEdit {
                range: Range { start: insert_pos, end: insert_pos },
                new_text: suppress_line,
            }]);

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Suppress `{}`", ld.rule),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![lsp_diag.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }

        // --- Action 2: suppress entire-file ---
        {
            let source = source_map.source(file_id);
            // Check if there is already an entire-file suppress for this rule.
            let already_suppressed = source.lines().any(|line| {
                let t = line.trim();
                t.starts_with('#')
                    && t.contains("@suppress")
                    && t.contains("entire-file")
                    && t.contains(ld.rule)
            });

            if !already_suppressed {
                let suppress_line = format!("# @suppress entire-file: {}\n", ld.rule);
                let insert_pos = lsp_types::Position { line: 0, character: 0 };

                let mut changes = HashMap::new();
                changes.insert(uri.clone(), vec![TextEdit {
                    range: Range { start: insert_pos, end: insert_pos },
                    new_text: suppress_line,
                }]);

                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Suppress `{}` for entire file", ld.rule),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![lsp_diag]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }
        }
    }

    actions
}
