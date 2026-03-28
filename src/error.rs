//! Diagnostic types for error and warning reporting with source locations.

use crate::ast::Span;
use crate::span::SourceMap;

/// Severity level of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A label pointing to a span in the source with a message.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A single diagnostic (error or warning) with optional source labels.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
}

impl Diagnostic {
    /// Create a new error diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            labels: Vec::new(),
        }
    }

    /// Attach a source label to this diagnostic (builder pattern).
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    /// Render this diagnostic to a string with source snippets (rustc-style).
    pub fn render(&self, source_map: &SourceMap) -> String {
        let mut out = String::new();
        let prefix = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        out.push_str(&format!("{}: {}\n", prefix, self.message));

        for label in &self.labels {
            let path = source_map.path(label.span.file_id);
            let (line, col) = source_map.line_col(label.span.file_id, label.span.start);
            out.push_str(&format!(
                "  --> {}:{}:{}\n",
                path.display(),
                line,
                col
            ));
            let line_text = source_map.line_text(label.span.file_id, line);
            let line_num = format!("{}", line);
            let padding = " ".repeat(line_num.len());
            out.push_str(&format!("{} |\n", padding));
            out.push_str(&format!("{} | {}\n", line_num, line_text));

            // Underline
            let (_, end_col) = source_map.line_col(label.span.file_id, label.span.end);
            let underline_start = col - 1;
            let underline_len = if end_col > col { end_col - col } else { 1 };
            out.push_str(&format!(
                "{} | {}{} {}\n",
                padding,
                " ".repeat(underline_start),
                "^".repeat(underline_len),
                label.message
            ));
        }
        out
    }
}

/// Collect diagnostics during compilation.
#[derive(Debug, Default)]
pub struct Diagnostics {
    pub errors: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Create an empty diagnostic collector.
    pub fn new() -> Self {
        Self { errors: Vec::new() }
    }

    /// Add a diagnostic.
    pub fn add(&mut self, diag: Diagnostic) {
        self.errors.push(diag);
    }

    /// Returns `true` if any diagnostic has error severity.
    pub fn has_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Render all diagnostics to a single string.
    pub fn render_all(&self, source_map: &SourceMap) -> String {
        self.errors
            .iter()
            .map(|d| d.render(source_map))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
