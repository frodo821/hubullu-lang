//! Convert hubullu diagnostics to LSP publishDiagnostics notifications.

use lsp_server::Notification;
use lsp_types::{
    DiagnosticRelatedInformation, Location, PublishDiagnosticsParams, Uri,
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
        .map(|d| convert_diagnostic(d, url, source_map))
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
        source: Some("hubullu".into()),
        message: diag.message.clone(),
        related_information: related,
        ..Default::default()
    }
}

fn file_id_to_uri(
    file_id: crate::span::FileId,
    source_map: &SourceMap,
) -> Option<Uri> {
    let path = source_map.path(file_id);
    convert::path_to_uri(path)
}
