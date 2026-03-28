//! Parse `# @source` directives from `.hut` files.
//!
//! A `.hut` file can declare its source `.hu` project entry point:
//!
//! ```text
//! # @source ../main.hu
//! "The" cat walk[tense=present] "."
//! ```
//!
//! The directive must appear in a comment line (starting with `#`) and is
//! resolved relative to the `.hut` file's directory.

use std::path::{Path, PathBuf};

/// Extract the `@source` path from a `.hut` file's source text.
///
/// Scans the first 20 lines for a `# @source <path>` directive.
pub fn parse_source_directive(source: &str) -> Option<String> {
    for line in source.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim();
            if let Some(path) = rest.strip_prefix("@source") {
                let path = path.trim();
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
        // Stop scanning after a non-empty, non-comment line.
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            break;
        }
    }
    None
}

/// Resolve a `@source` path relative to the `.hut` file's directory.
pub fn resolve_source_path(hut_file: &Path, source_path: &str) -> PathBuf {
    let dir = hut_file.parent().unwrap_or(Path::new("."));
    let resolved = dir.join(source_path);
    // Canonicalize if possible, otherwise return as-is.
    resolved.canonicalize().unwrap_or(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source_directive() {
        let source = "# @source ../main.hu\n\"The\" cat walk[tense=present] \".\"\n";
        assert_eq!(
            parse_source_directive(source),
            Some("../main.hu".to_string())
        );
    }

    #[test]
    fn test_parse_source_directive_with_comment() {
        let source = "# This is a sentence file\n# @source ./lang.hu\ncat walk\n";
        assert_eq!(
            parse_source_directive(source),
            Some("./lang.hu".to_string())
        );
    }

    #[test]
    fn test_no_directive() {
        let source = "\"The\" cat walk[tense=present] \".\"\n";
        assert_eq!(parse_source_directive(source), None);
    }

    #[test]
    fn test_resolve_source_path() {
        let hut = Path::new("/project/examples/sentences.hut");
        let resolved = resolve_source_path(hut, "../main.hu");
        assert!(resolved.to_string_lossy().contains("main.hu"));
    }
}
