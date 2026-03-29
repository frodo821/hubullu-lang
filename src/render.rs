//! `.hut` file rendering — resolves token lists against compiled databases.
//!
//! Each `.hut` file declares `@reference` directives pointing at `.hu` source
//! files. The renderer compiles those sources (with mtime-based caching) and
//! resolves entry references through namespace-aware lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::ast;
use crate::ast::{HutFile, ImportTarget};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::span::SourceMap;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a `.hut` source string into a [`HutFile`] (references + tokens).
pub fn parse_hut(source: &str, filename: &str) -> Result<HutFile, String> {
    let mut source_map = SourceMap::new();
    let file_id = source_map.add_file(filename.into(), source.to_string());

    let lexer = Lexer::new(source_map.source(file_id), file_id);
    let (tokens, lex_errors) = lexer.tokenize();
    if !lex_errors.is_empty() {
        let msgs: Vec<String> = lex_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    let parser = Parser::new(tokens, file_id);
    let (hut_file, parse_errors) = parser.parse_token_list_to_eof();
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    Ok(hut_file)
}

// ---------------------------------------------------------------------------
// Cached compilation
// ---------------------------------------------------------------------------

/// Compile a `.hu` file to SQLite, returning the path to the database.
///
/// Uses mtime-based caching: if a cached `.sqlite` already exists and is newer
/// than the source file, compilation is skipped.  The cache is stored next to
/// the source as `<name>.hu.cache.sqlite`.
///
/// **Limitation:** transitive dependencies (files loaded via `@use`) are not
/// tracked — only the root `.hu` file's mtime is compared.
pub fn compile_cached(hu_path: &Path) -> Result<PathBuf, String> {
    let hu_path = hu_path
        .canonicalize()
        .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

    let cache_path = hu_path.with_extension("hu.cache.sqlite");

    let needs_compile = if cache_path.exists() {
        let src_meta = std::fs::metadata(&hu_path)
            .map_err(|e| format!("cannot stat '{}': {}", hu_path.display(), e))?;
        let cache_meta = std::fs::metadata(&cache_path)
            .map_err(|e| format!("cannot stat '{}': {}", cache_path.display(), e))?;
        let src_mtime = src_meta
            .modified()
            .map_err(|e| format!("cannot read mtime: {}", e))?;
        let cache_mtime = cache_meta
            .modified()
            .map_err(|e| format!("cannot read mtime: {}", e))?;
        src_mtime > cache_mtime
    } else {
        true
    };

    if needs_compile {
        crate::compile(&hu_path, &cache_path)?;
    }

    Ok(cache_path)
}

// ---------------------------------------------------------------------------
// Entry source — one compiled database with its import rules
// ---------------------------------------------------------------------------

struct EntrySource {
    conn: Connection,
    /// `None` = glob (all entries visible); `Some(map)` = named imports
    /// where key = local name, value = name in db.
    name_map: Option<HashMap<String, String>>,
}

impl EntrySource {
    /// Look up the database-side entry name for a local reference name.
    /// Returns `Some(db_name)` if the entry is visible through this source.
    fn resolve_name<'a>(&'a self, local_name: &'a str) -> Option<&'a str> {
        match &self.name_map {
            None => Some(local_name), // glob — everything visible
            Some(map) => map.get(local_name).map(|s| s.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolve context — namespace-aware multi-database lookup
// ---------------------------------------------------------------------------

/// Holds compiled databases and namespace mappings built from `@reference`
/// directives.
pub struct ResolveContext {
    /// namespace name → entry source
    namespaced: HashMap<String, EntrySource>,
    /// un-namespaced sources, searched in declaration order
    default_sources: Vec<EntrySource>,
}

impl ResolveContext {
    /// Build a [`ResolveContext`] from the `@reference` directives in a `.hut`
    /// file.  `hut_dir` is the directory containing the `.hut` file, used to
    /// resolve relative paths.
    pub fn from_references(
        references: &[ast::Import],
        hut_dir: &Path,
    ) -> Result<Self, String> {
        let mut namespaced: HashMap<String, EntrySource> = HashMap::new();
        let mut default_sources: Vec<EntrySource> = Vec::new();
        // avoid compiling the same file twice
        let mut compiled: HashMap<PathBuf, PathBuf> = HashMap::new();

        for import in references {
            let hu_rel = &import.path.node;
            let hu_path = hut_dir.join(hu_rel);
            let hu_canon = hu_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

            let db_path = match compiled.get(&hu_canon) {
                Some(p) => p.clone(),
                None => {
                    let p = compile_cached(&hu_canon)?;
                    compiled.insert(hu_canon.clone(), p.clone());
                    p
                }
            };

            let conn = Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| format!("cannot open '{}': {}", db_path.display(), e))?;

            let (namespace, name_map) = match &import.target {
                ImportTarget::Glob { alias } => {
                    (alias.as_ref().map(|a| a.node.clone()), None)
                }
                ImportTarget::Named(entries) => {
                    let map: HashMap<String, String> = entries
                        .iter()
                        .map(|e| {
                            let local = e.alias.as_ref().unwrap_or(&e.name).node.clone();
                            let db_name = e.name.node.clone();
                            (local, db_name)
                        })
                        .collect();
                    (None, Some(map))
                }
            };

            let source = EntrySource { conn, name_map };
            match namespace {
                Some(ns) => {
                    namespaced.insert(ns, source);
                }
                None => {
                    default_sources.push(source);
                }
            }
        }

        Ok(ResolveContext {
            namespaced,
            default_sources,
        })
    }

    /// Find the entry source and database-side name for the given reference.
    fn find_entry<'a>(
        &'a self,
        namespace: &[ast::Ident],
        local_name: &'a str,
    ) -> Result<(&'a EntrySource, &'a str), String> {
        if namespace.is_empty() {
            // Search un-namespaced sources in order
            for src in &self.default_sources {
                if let Some(db_name) = src.resolve_name(local_name) {
                    return Ok((src, db_name));
                }
            }
            Err(format!("entry '{}' not found in any @reference", local_name))
        } else {
            // Qualified lookup: first namespace component
            let ns = &namespace[0].node;
            let src = self
                .namespaced
                .get(ns)
                .ok_or_else(|| format!("namespace '{}' not found", ns))?;
            // If there are deeper namespaces we just join them with the entry
            // name — currently only one level is supported.
            if namespace.len() > 1 {
                return Err(format!(
                    "nested namespaces not supported: {}.{}",
                    namespace.iter().map(|i| i.node.as_str()).collect::<Vec<_>>().join("."),
                    local_name
                ));
            }
            match src.resolve_name(local_name) {
                Some(db_name) => Ok((src, db_name)),
                None => Err(format!("entry '{}' not found in namespace '{}'", local_name, ns)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// A resolved piece: either a string part or a glue marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPart {
    Text(String),
    Glue,
}

/// Resolve a list of AST tokens using the [`ResolveContext`].
pub fn resolve(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
) -> Result<Vec<ResolvedPart>, String> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            ast::Token::Glue => {
                parts.push(ResolvedPart::Glue);
            }
            ast::Token::Lit(s) => {
                parts.push(ResolvedPart::Text(s.node.clone()));
            }
            ast::Token::Ref(entry_ref) => {
                let local_name = &entry_ref.entry_id.node;
                let (src, db_name) = ctx.find_entry(&entry_ref.namespace, local_name)?;
                match &entry_ref.form_spec {
                    None => {
                        let headword: String = src
                            .conn
                            .query_row(
                                "SELECT headword FROM entries WHERE name = ?1",
                                [db_name],
                                |row| row.get(0),
                            )
                            .map_err(|e| format!("entry '{}' not found: {}", local_name, e))?;
                        parts.push(ResolvedPart::Text(headword));
                    }
                    Some(form_spec) => {
                        let mut requested: Vec<(String, String)> = form_spec
                            .conditions
                            .iter()
                            .map(|c| (c.axis.node.clone(), c.value.node.clone()))
                            .collect();
                        requested.sort();

                        let mut stmt = src
                            .conn
                            .prepare(
                                "SELECT f.form_str, f.tags FROM forms f \
                                 JOIN entries e ON f.entry_id = e.id \
                                 WHERE e.name = ?1",
                            )
                            .map_err(|e| format!("query failed: {}", e))?;
                        let mut rows = stmt
                            .query([db_name])
                            .map_err(|e| format!("query failed: {}", e))?;

                        let mut found = None;
                        while let Some(row) = rows
                            .next()
                            .map_err(|e| format!("query failed: {}", e))?
                        {
                            let form_str: String =
                                row.get(0).map_err(|e| format!("read failed: {}", e))?;
                            let tags_str: String =
                                row.get(1).map_err(|e| format!("read failed: {}", e))?;
                            let mut stored: Vec<(String, String)> = tags_str
                                .split(',')
                                .filter(|s| !s.is_empty())
                                .filter_map(|pair| {
                                    let mut parts = pair.splitn(2, '=');
                                    Some((parts.next()?.to_string(), parts.next()?.to_string()))
                                })
                                .collect();
                            stored.sort();
                            if stored == requested {
                                found = Some(form_str);
                                break;
                            }
                        }

                        let form_str = found.ok_or_else(|| {
                            let tags_display = form_spec
                                .conditions
                                .iter()
                                .map(|c| format!("{}={}", c.axis.node, c.value.node))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("form '{}[{}]' not found", local_name, tags_display)
                        })?;
                        parts.push(ResolvedPart::Text(form_str));
                    }
                }
            }
        }
    }
    Ok(parts)
}

/// Read render config from the first available database in the context,
/// falling back to defaults.
pub fn read_render_config(ctx: &ResolveContext) -> (String, String) {
    // Try namespaced DBs first, then default sources
    let all_conns = ctx
        .namespaced
        .values()
        .map(|s| &s.conn)
        .chain(ctx.default_sources.iter().map(|s| &s.conn));

    for conn in all_conns {
        let sep = conn.query_row(
            "SELECT value FROM render_config WHERE key = 'separator'",
            [],
            |row| row.get::<_, String>(0),
        );
        let no_sep = conn.query_row(
            "SELECT value FROM render_config WHERE key = 'no_separator_before'",
            [],
            |row| row.get::<_, String>(0),
        );
        if let (Ok(s), Ok(n)) = (sep, no_sep) {
            return (s, n);
        }
    }

    (" ".to_string(), ".,;:!?".to_string())
}

// ---------------------------------------------------------------------------
// Smart join
// ---------------------------------------------------------------------------

/// Join resolved parts using separator, suppressing it before certain characters
/// and around `Glue` markers.
pub fn smart_join(parts: &[ResolvedPart], separator: &str, no_sep_before: &str) -> String {
    let mut result = String::new();
    let mut glue_next = false;
    for part in parts {
        match part {
            ResolvedPart::Glue => {
                glue_next = true;
            }
            ResolvedPart::Text(text) => {
                if !result.is_empty() && !separator.is_empty() && !glue_next {
                    let first_char = text.chars().next();
                    let suppress = first_char
                        .map(|c| no_sep_before.contains(c))
                        .unwrap_or(false);
                    if !suppress {
                        result.push_str(separator);
                    }
                }
                glue_next = false;
                result.push_str(text);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ResolvedPart {
        ResolvedPart::Text(s.to_string())
    }

    #[test]
    fn test_smart_join_basic() {
        let parts = vec![text("La"), text("hundo"), text("dormas"), text(".")];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "La hundo dormas.");
    }

    #[test]
    fn test_smart_join_glue() {
        // mal~bon~a hundo → "malbona hundo"
        let parts = vec![
            text("mal"),
            ResolvedPart::Glue,
            text("bon"),
            ResolvedPart::Glue,
            text("a"),
            text("hundo"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "malbona hundo");
    }

    #[test]
    fn test_smart_join_glue_with_punctuation() {
        // mal~bon~a hundo "."
        let parts = vec![
            text("mal"),
            ResolvedPart::Glue,
            text("bona"),
            text("hundo"),
            text("."),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "malbona hundo.");
    }

    #[test]
    fn test_parse_hut_glue() {
        let hut = parse_hut(r#""mal"~"bona" "hundo""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 4);
        assert!(matches!(hut.tokens[0], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[1], ast::Token::Glue));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[3], ast::Token::Lit(_)));
    }

    #[test]
    fn test_parse_hut_with_reference() {
        let src = r#"@reference * from "lang.hu"
"The" cat walk[tense=present, person=3, number=sg] "."
"#;
        let hut = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 1);
        assert_eq!(hut.references[0].path.node, "lang.hu");
        assert!(hut.tokens.len() >= 3);
    }

    #[test]
    fn test_parse_hut_with_namespaced_reference() {
        let src = r#"@reference * as en from "english.hu"
en.cat en.walk[tense=present] "."
"#;
        let hut = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 1);
        // Check namespace on the entry ref
        if let ast::Token::Ref(r) = &hut.tokens[0] {
            assert_eq!(r.namespace.len(), 1);
            assert_eq!(r.namespace[0].node, "en");
            assert_eq!(r.entry_id.node, "cat");
        } else {
            panic!("expected Ref token");
        }
    }
}
