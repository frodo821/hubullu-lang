//! Built-in standard library modules for `std:` imports.
//!
//! Modules are embedded as source text via `include_str!` and parsed lazily
//! on first use.  Synthetic [`PathBuf`] values (e.g. `<std:ipa>`) are used as
//! keys in `path_to_id` and `source_map` so they cannot collide with real
//! filesystem paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Return the source text for a standard library module, or `None`.
pub fn lookup(module_name: &str) -> Option<&'static str> {
    registry().get(module_name).copied()
}

/// Return the synthetic `PathBuf` used for a std module in source_map / path_to_id.
pub fn synthetic_path(module_name: &str) -> PathBuf {
    PathBuf::from(format!("<std:{}>", module_name))
}

/// Check if a path is a synthetic std module path.
pub fn is_std_path(path: &Path) -> bool {
    path.to_string_lossy().starts_with("<std:")
}

/// List all available standard library module names.
pub fn available_modules() -> Vec<&'static str> {
    let mut names: Vec<&str> = registry().keys().copied().collect();
    names.sort_unstable();
    names
}

static REGISTRY: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

fn registry() -> &'static HashMap<&'static str, &'static str> {
    REGISTRY.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("_test", include_str!("../std/_test.hu"));
        m
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_existing_module() {
        assert!(lookup("_test").is_some());
        assert!(lookup("_test").unwrap().contains("std_test_axis"));
    }

    #[test]
    fn lookup_missing_module() {
        assert!(lookup("nonexistent").is_none());
    }

    #[test]
    fn synthetic_path_format() {
        assert_eq!(synthetic_path("ipa"), PathBuf::from("<std:ipa>"));
    }

    #[test]
    fn is_std_path_detection() {
        assert!(is_std_path(Path::new("<std:ipa>")));
        assert!(!is_std_path(Path::new("./foo.hu")));
        assert!(!is_std_path(Path::new("/abs/path.hu")));
    }
}
