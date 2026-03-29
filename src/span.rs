//! Source map for multi-file span resolution.
//!
//! Tracks source text and file paths for all loaded files, and provides
//! efficient byte-offset → line/column conversion.

use std::path::{Path, PathBuf};

/// Opaque file identifier for multi-file span resolution.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// Maps FileId → source text and file path.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
struct SourceFile {
    pub(crate) path: PathBuf,
    pub(crate) source: String,
    /// Byte offset of each line start (for line/col calculation).
    pub(crate) line_starts: Vec<usize>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Add a file and return its FileId.
    pub fn add_file(&mut self, path: PathBuf, source: String) -> FileId {
        let line_starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile {
            path,
            source,
            line_starts,
        });
        id
    }

    /// Get the full source text for a file.
    pub fn source(&self, id: FileId) -> &str {
        &self.files[id.0 as usize].source
    }

    /// Get the file path for a file.
    pub fn path(&self, id: FileId) -> &Path {
        &self.files[id.0 as usize].path
    }

    /// Convert byte offset to (1-based line, 1-based column).
    pub fn line_col(&self, id: FileId, offset: usize) -> (usize, usize) {
        let file = &self.files[id.0 as usize];
        let line = file
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let col = offset - file.line_starts[line];
        (line + 1, col + 1)
    }

    /// Get the source text of a given line (1-based).
    pub fn line_text(&self, id: FileId, line: usize) -> &str {
        let file = &self.files[id.0 as usize];
        let idx = line - 1;
        if idx >= file.line_starts.len() {
            return "";
        }
        let start = file.line_starts[idx];
        let end = file
            .line_starts
            .get(idx + 1)
            .copied()
            .unwrap_or(file.source.len());
        file.source[start..end].trim_end_matches('\n')
    }

    /// Convert (1-based line, 1-based column) to byte offset.
    /// Returns `None` if out of bounds.
    pub fn offset_at(&self, id: FileId, line: usize, col: usize) -> Option<usize> {
        let file = &self.files[id.0 as usize];
        let idx = line.checked_sub(1)?;
        let line_start = *file.line_starts.get(idx)?;
        let offset = line_start + col.checked_sub(1)?;
        if offset <= file.source.len() {
            Some(offset)
        } else {
            None
        }
    }

    /// Get a slice of source text for a file by byte offsets.
    pub fn source_slice(&self, id: FileId, start: usize, end: usize) -> Option<&str> {
        let source = &self.files[id.0 as usize].source;
        source.get(start..end)
    }

    /// Number of files in this source map.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Iterate over all file IDs.
    pub fn file_ids(&self) -> impl Iterator<Item = FileId> {
        (0..self.files.len() as u32).map(FileId)
    }

    /// Length of source text for a file (in bytes).
    pub fn source_len(&self, id: FileId) -> usize {
        self.files[id.0 as usize].source.len()
    }

    /// Number of lines in a file.
    pub fn line_count(&self, id: FileId) -> usize {
        self.files[id.0 as usize].line_starts.len()
    }
}
