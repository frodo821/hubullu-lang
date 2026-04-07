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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_map(source: &str) -> (SourceMap, FileId) {
        let mut sm = SourceMap::new();
        let id = sm.add_file(PathBuf::from("test.hu"), source.to_string());
        (sm, id)
    }

    #[test]
    fn add_file_returns_sequential_ids() {
        let mut sm = SourceMap::new();
        let id0 = sm.add_file(PathBuf::from("a.hu"), "aaa".into());
        let id1 = sm.add_file(PathBuf::from("b.hu"), "bbb".into());
        assert_eq!(id0.0, 0);
        assert_eq!(id1.0, 1);
        assert_eq!(sm.file_count(), 2);
    }

    #[test]
    fn source_and_path() {
        let (sm, id) = make_map("hello world");
        assert_eq!(sm.source(id), "hello world");
        assert_eq!(sm.path(id), Path::new("test.hu"));
    }

    #[test]
    fn line_col_single_line() {
        let (sm, id) = make_map("abcdef");
        assert_eq!(sm.line_col(id, 0), (1, 1)); // first char
        assert_eq!(sm.line_col(id, 3), (1, 4)); // 'd'
        assert_eq!(sm.line_col(id, 5), (1, 6)); // 'f'
    }

    #[test]
    fn line_col_multi_line() {
        let (sm, id) = make_map("abc\ndef\nghi");
        // line 1: abc\n (offsets 0..3)
        assert_eq!(sm.line_col(id, 0), (1, 1));
        assert_eq!(sm.line_col(id, 2), (1, 3));
        // line 2: def\n (offsets 4..7)
        assert_eq!(sm.line_col(id, 4), (2, 1));
        assert_eq!(sm.line_col(id, 6), (2, 3));
        // line 3: ghi (offsets 8..10)
        assert_eq!(sm.line_col(id, 8), (3, 1));
        assert_eq!(sm.line_col(id, 10), (3, 3));
    }

    #[test]
    fn line_col_multibyte_utf8() {
        // 日本語: each char is 3 bytes in UTF-8
        let (sm, id) = make_map("あいう\nえお");
        // 'あ' = offset 0, 'い' = offset 3, 'う' = offset 6
        assert_eq!(sm.line_col(id, 0), (1, 1));
        assert_eq!(sm.line_col(id, 3), (1, 4));
        assert_eq!(sm.line_col(id, 6), (1, 7));
        // '\n' at offset 9, 'え' at offset 10
        assert_eq!(sm.line_col(id, 10), (2, 1));
    }

    #[test]
    fn line_text_basic() {
        let (sm, id) = make_map("first\nsecond\nthird");
        assert_eq!(sm.line_text(id, 1), "first");
        assert_eq!(sm.line_text(id, 2), "second");
        assert_eq!(sm.line_text(id, 3), "third");
    }

    #[test]
    fn line_text_out_of_bounds() {
        let (sm, id) = make_map("one\ntwo");
        assert_eq!(sm.line_text(id, 99), "");
    }

    #[test]
    fn offset_at_roundtrip() {
        let (sm, id) = make_map("abc\ndef\nghi");
        // (2, 2) should map to offset 5 ('e')
        assert_eq!(sm.offset_at(id, 2, 2), Some(5));
        // roundtrip
        assert_eq!(sm.line_col(id, 5), (2, 2));
    }

    #[test]
    fn offset_at_out_of_bounds() {
        let (sm, id) = make_map("abc");
        assert_eq!(sm.offset_at(id, 0, 1), None); // line 0 invalid
        assert_eq!(sm.offset_at(id, 1, 0), None); // col 0 invalid
        assert_eq!(sm.offset_at(id, 5, 1), None); // line too large
        assert_eq!(sm.offset_at(id, 1, 100), None); // col beyond source
    }

    #[test]
    fn source_slice_valid() {
        let (sm, id) = make_map("hello world");
        assert_eq!(sm.source_slice(id, 0, 5), Some("hello"));
        assert_eq!(sm.source_slice(id, 6, 11), Some("world"));
    }

    #[test]
    fn source_slice_out_of_bounds() {
        let (sm, id) = make_map("abc");
        assert_eq!(sm.source_slice(id, 0, 100), None);
    }

    #[test]
    fn file_ids_iterator() {
        let mut sm = SourceMap::new();
        sm.add_file(PathBuf::from("a.hu"), "a".into());
        sm.add_file(PathBuf::from("b.hu"), "b".into());
        sm.add_file(PathBuf::from("c.hu"), "c".into());
        let ids: Vec<FileId> = sm.file_ids().collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0].0, 0);
        assert_eq!(ids[2].0, 2);
    }

    #[test]
    fn source_len_and_line_count() {
        let (sm, id) = make_map("abc\ndef");
        assert_eq!(sm.source_len(id), 7);
        assert_eq!(sm.line_count(id), 2); // 2 line starts
    }

    #[test]
    fn empty_source() {
        let (sm, id) = make_map("");
        assert_eq!(sm.source(id), "");
        assert_eq!(sm.source_len(id), 0);
        assert_eq!(sm.line_count(id), 1); // one line start at offset 0
        assert_eq!(sm.line_col(id, 0), (1, 1));
        assert_eq!(sm.line_text(id, 1), "");
    }

    #[test]
    fn trailing_newline() {
        let (sm, id) = make_map("abc\n");
        assert_eq!(sm.line_count(id), 2);
        assert_eq!(sm.line_text(id, 1), "abc");
        assert_eq!(sm.line_text(id, 2), "");
    }
}
