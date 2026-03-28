//! Conversion utilities between hubullu spans and LSP positions/URIs.

use std::path::Path;

use lsp_types::{DiagnosticSeverity, Position, Range, Uri};

use crate::ast::Span;
use crate::error::Severity;
use crate::span::{FileId, SourceMap};

/// Convert a hubullu `Span` to an LSP `Range`.
///
/// SourceMap returns 1-based line/col; LSP uses 0-based.
/// LSP columns are UTF-16 code units; we convert from byte offsets.
pub fn span_to_range(span: &Span, source_map: &SourceMap) -> Range {
    Range {
        start: offset_to_position(span.file_id, span.start, source_map),
        end: offset_to_position(span.file_id, span.end, source_map),
    }
}

/// Convert a byte offset to an LSP `Position` (0-based line, 0-based UTF-16 column).
pub fn offset_to_position(file_id: FileId, offset: usize, source_map: &SourceMap) -> Position {
    let (line_1based, col_1based) = source_map.line_col(file_id, offset);
    let line_text = source_map.line_text(file_id, line_1based);
    // Convert byte column to UTF-16 code units
    let byte_col = col_1based - 1;
    let utf16_col = byte_col_to_utf16(line_text, byte_col);
    Position {
        line: (line_1based - 1) as u32,
        character: utf16_col as u32,
    }
}

/// Convert an LSP `Position` to a byte offset.
pub fn position_to_offset(
    position: &Position,
    file_id: FileId,
    source_map: &SourceMap,
) -> Option<usize> {
    let line_1based = position.line as usize + 1;
    let line_text = source_map.line_text(file_id, line_1based);
    let byte_col = utf16_col_to_byte(line_text, position.character as usize);
    source_map.offset_at(file_id, line_1based, byte_col + 1)
}

/// Convert hubullu severity to LSP severity.
pub fn severity_to_lsp(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// Convert byte column offset to UTF-16 code unit offset within a line.
fn byte_col_to_utf16(line: &str, byte_col: usize) -> usize {
    let prefix = &line[..byte_col.min(line.len())];
    prefix.encode_utf16().count()
}

/// Convert UTF-16 code unit offset to byte offset within a line.
fn utf16_col_to_byte(line: &str, utf16_col: usize) -> usize {
    let mut utf16_count = 0;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_count >= utf16_col {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
    }
    line.len()
}

/// Create a `file://` URI from a filesystem path.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let abs = if path.is_absolute() {
        path.to_string_lossy().to_string()
    } else {
        path.canonicalize().ok()?.to_string_lossy().to_string()
    };
    let uri_string = format!("file://{}", abs);
    uri_string.parse().ok()
}

/// Extract a filesystem path from a `file://` URI.
pub fn uri_to_path(uri: &Uri) -> Option<std::path::PathBuf> {
    let s = uri.as_str();
    if let Some(path) = s.strip_prefix("file://") {
        Some(std::path::PathBuf::from(path))
    } else {
        None
    }
}

/// Get the URI string suitable for use as a filename.
pub fn uri_to_filename(uri: &Uri) -> String {
    uri_to_path(uri)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| uri.as_str().to_string())
}
