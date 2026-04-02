//! Convert hubullu diagnostics to LSP publishDiagnostics notifications.

use lsp_server::Notification;
use lsp_types::{
    DiagnosticRelatedInformation, Location, NumberOrString, PublishDiagnosticsParams, Uri,
};

use crate::error::Diagnostic;
use crate::span::SourceMap;

use super::convert;

/// Build an LSP `publishDiagnostics` notification for a document.
pub fn publish_notification(
    url: &Uri,
    diagnostics: &[&Diagnostic],
    source_map: &SourceMap,
) -> Notification {
    let lsp_diags = diagnostics
        .iter()
        .map(|d| convert_diagnostic(d, None, url, source_map))
        .collect();

    Notification::new(
        "textDocument/publishDiagnostics".into(),
        PublishDiagnosticsParams {
            uri: url.clone(),
            diagnostics: lsp_diags,
            version: None,
        },
    )
}

/// Build a notification combining diagnostics from two different source maps.
///
/// This is needed because parse diagnostics use the document's source map (single
/// file) while project-level diagnostics (phase1 errors, lint warnings) use the
/// project's source map (multi-file).
pub fn publish_combined_notification(
    url: &Uri,
    parse_diags: &[&Diagnostic],
    parse_source_map: &SourceMap,
    proj_diags: &[&Diagnostic],
    lint_diags: &[&crate::lint::LintDiagnostic],
    proj_source_map: Option<&SourceMap>,
    extra_lsp_diags: &[lsp_types::Diagnostic],
) -> Notification {
    let mut lsp_diags: Vec<lsp_types::Diagnostic> = parse_diags
        .iter()
        .map(|d| convert_diagnostic(d, None, url, parse_source_map))
        .collect();

    if let Some(psm) = proj_source_map {
        lsp_diags.extend(
            proj_diags
                .iter()
                .map(|d| convert_diagnostic(d, None, url, psm)),
        );
        lsp_diags.extend(
            lint_diags
                .iter()
                .map(|ld| convert_diagnostic(&ld.diagnostic, Some(ld.rule), url, psm)),
        );
    }

    lsp_diags.extend_from_slice(extra_lsp_diags);

    Notification::new(
        "textDocument/publishDiagnostics".into(),
        PublishDiagnosticsParams {
            uri: url.clone(),
            diagnostics: lsp_diags,
            version: None,
        },
    )
}

/// Build an empty publishDiagnostics notification (clears stale markers).
pub fn clear_notification(url: &Uri) -> Notification {
    Notification::new(
        "textDocument/publishDiagnostics".into(),
        PublishDiagnosticsParams {
            uri: url.clone(),
            diagnostics: vec![],
            version: None,
        },
    )
}

fn convert_diagnostic(
    diag: &Diagnostic,
    code: Option<&str>,
    doc_url: &Uri,
    source_map: &SourceMap,
) -> lsp_types::Diagnostic {
    // Primary range: first label, or fallback to start of file.
    let range = diag
        .labels
        .first()
        .map(|l| convert::span_to_range(&l.span, source_map))
        .unwrap_or_default();

    // Additional labels become relatedInformation.
    let related = if diag.labels.len() > 1 {
        Some(
            diag.labels[1..]
                .iter()
                .map(|l| {
                    let label_url = file_id_to_uri(l.span.file_id, source_map)
                        .unwrap_or_else(|| doc_url.clone());
                    DiagnosticRelatedInformation {
                        location: Location {
                            uri: label_url,
                            range: convert::span_to_range(&l.span, source_map),
                        },
                        message: l.message.clone(),
                    }
                })
                .collect(),
        )
    } else {
        None
    };

    lsp_types::Diagnostic {
        range,
        severity: Some(convert::severity_to_lsp(diag.severity)),
        code: code.map(|c| NumberOrString::String(c.to_string())),
        source: Some("hubullu".into()),
        message: diag.message.clone(),
        related_information: related,
        ..Default::default()
    }
}

/// Convert a `LintDiagnostic` to an `lsp_types::Diagnostic`, including the rule
/// name as the diagnostic code.
pub fn lint_to_lsp_diagnostic(
    ld: &crate::lint::LintDiagnostic,
    doc_url: &Uri,
    source_map: &SourceMap,
) -> lsp_types::Diagnostic {
    convert_diagnostic(&ld.diagnostic, Some(ld.rule), doc_url, source_map)
}

fn file_id_to_uri(
    file_id: crate::span::FileId,
    source_map: &SourceMap,
) -> Option<Uri> {
    let path = source_map.path(file_id);
    convert::path_to_uri(path)
}
