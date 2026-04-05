//! `.hut` file rendering — resolves token lists against compiled `.huc` files.
//!
//! Each `.hut` file declares `@reference` directives pointing at `.hu` source
//! files. The renderer either compiles those sources on demand (with
//! mtime-based caching) or uses a pre-compiled `.huc` file supplied via
//! `--huc`, resolving entry references through namespace-aware lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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

/// Compile a `.hu` file to a `.huc` file, returning the path.
///
/// Uses mtime-based caching: if a cached `.huc` already exists and is newer
/// than the source file, compilation is skipped.  The cache is stored next to
/// the source as `<name>.hu.cache.sqlite`.
///
/// **Limitation:** transitive dependencies (files loaded via `@use`) are not
/// tracked — only the root `.hu` file's mtime is compared.
/// Check that a cached .huc file has all required tables.
fn huc_schema_up_to_date(huc_path: &Path) -> bool {
    let conn = match rusqlite::Connection::open_with_flags(
        huc_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Check for the `stems` table and `etymology_proto` column (added after initial schema).
    let ok = conn.prepare("SELECT 1 FROM stems LIMIT 0").is_ok()
        && conn.prepare("SELECT etymology_proto FROM entries LIMIT 0").is_ok();
    ok
}

pub fn compile_cached(hu_path: &Path) -> Result<PathBuf, String> {
    let hu_path = hu_path
        .canonicalize()
        .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

    let cache_dir = hu_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".hubullu-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let cache_path = cache_dir.join(
        hu_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(".hu", ".huc"),
    );

    let needs_compile = if cache_path.exists() {
        if !huc_schema_up_to_date(&cache_path) {
            true
        } else {
            // Run phase1 to discover all transitive source files, then
            // check if any of them is newer than the cached .huc.
            let cache_mtime = std::fs::metadata(&cache_path)
                .and_then(|m| m.modified())
                .map_err(|e| format!("cannot stat '{}': {}", cache_path.display(), e))?;

            let p1 = crate::phase1::run_phase1(&hu_path);
            let any_newer = p1.path_to_id.keys().any(|src_path| {
                std::fs::metadata(src_path)
                    .and_then(|m| m.modified())
                    .map(|t| t > cache_mtime)
                    .unwrap_or(true) // if we can't stat it, assume stale
            });
            any_newer
        }
    } else {
        true
    };

    if needs_compile {
        crate::compile(&hu_path, &cache_path)?;
    }

    Ok(cache_path)
}

// ---------------------------------------------------------------------------
// Entry source — one compiled .huc file with its import rules
// ---------------------------------------------------------------------------

struct EntrySource {
    conn: Rc<Connection>,
    /// `None` = glob (all entries visible); `Some(map)` = named imports
    /// where key = local name, value = name in the .huc file.
    name_map: Option<HashMap<String, String>>,
}

impl EntrySource {
    /// Look up the .huc-side entry name for a local reference name.
    /// Returns `Some(huc_name)` if the entry is visible through this source.
    fn resolve_name<'a>(&'a self, local_name: &'a str) -> Option<&'a str> {
        match &self.name_map {
            None => Some(local_name), // glob — everything visible
            Some(map) => map.get(local_name).map(|s| s.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolve context — namespace-aware lookup against .huc files
// ---------------------------------------------------------------------------

/// Holds compiled `.huc` connections and namespace mappings built from
/// `@reference` directives.
pub struct ResolveContext {
    /// namespace name → entry source
    namespaced: HashMap<String, EntrySource>,
    /// un-namespaced sources, searched in declaration order
    default_sources: Vec<EntrySource>,
}

impl ResolveContext {
    /// Build a [`ResolveContext`] from the `@reference` directives in a `.hut`
    /// file.  `hut_dir` is the directory containing the `.hut` file, used to
    /// resolve relative paths.  Each referenced `.hu` file is compiled (with
    /// mtime-based caching) to produce a `.huc` file.
    pub fn from_references(
        references: &[ast::Import],
        hut_dir: &Path,
    ) -> Result<Self, String> {
        let mut namespaced: HashMap<String, EntrySource> = HashMap::new();
        let mut default_sources: Vec<EntrySource> = Vec::new();
        // avoid compiling the same file twice
        let mut compiled: HashMap<PathBuf, Rc<Connection>> = HashMap::new();

        for import in references {
            let hu_rel = &import.path.node;
            let hu_path = hut_dir.join(hu_rel);
            let hu_canon = hu_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

            let conn = match compiled.get(&hu_canon) {
                Some(c) => Rc::clone(c),
                None => {
                    let huc_path = compile_cached(&hu_canon)?;
                    let c = Rc::new(
                        Connection::open_with_flags(
                            &huc_path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .map_err(|e| format!("cannot open '{}': {}", huc_path.display(), e))?,
                    );
                    compiled.insert(hu_canon.clone(), Rc::clone(&c));
                    c
                }
            };

            let (namespace, name_map) = match &import.target {
                ImportTarget::Glob { alias } => {
                    (alias.as_ref().map(|a| a.node.clone()), None)
                }
                ImportTarget::Named(entries) => {
                    let map: HashMap<String, String> = entries
                        .iter()
                        .map(|e| {
                            let local = e.alias.as_ref().unwrap_or(&e.name).node.clone();
                            let huc_name = e.name.node.clone();
                            (local, huc_name)
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

    /// Build a [`ResolveContext`] from a pre-compiled `.huc` file.
    ///
    /// Uses the `name_resolution` table inside the `.huc` to scope entry
    /// lookups per `@reference` directive, without re-compiling `.hu` sources.
    pub fn from_huc(
        references: &[ast::Import],
        hut_dir: &Path,
        huc_path: &Path,
    ) -> Result<Self, String> {
        use sha2::{Digest, Sha256};

        let conn = Rc::new(
            Connection::open_with_flags(huc_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("cannot open '{}': {}", huc_path.display(), e))?,
        );

        // Read entry point directory from compile_meta.
        let entry_point_dir: String = conn
            .query_row(
                "SELECT value FROM compile_meta WHERE key = 'entry_point_dir'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot read entry_point_dir from .huc: {}", e))?;
        let entry_point_dir = PathBuf::from(entry_point_dir);

        let mut namespaced: HashMap<String, EntrySource> = HashMap::new();
        let mut default_sources: Vec<EntrySource> = Vec::new();

        for import in references {
            let hu_rel = &import.path.node;
            let hu_path = hut_dir.join(hu_rel);
            // Compute relative path from the entry point directory.
            let hu_canon = hu_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;
            let rel_path = hu_canon
                .strip_prefix(&entry_point_dir)
                .unwrap_or(&hu_canon);
            let file_hash = {
                let mut hasher = Sha256::new();
                hasher.update(rel_path.to_string_lossy().as_bytes());
                format!("{:x}", hasher.finalize())
            };

            // Query name_resolution for all entries visible in this file's scope.
            let scope_names: HashMap<String, String> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT nr.name, e.name FROM name_resolution nr \
                         JOIN entries e ON nr.entry_id = e.id \
                         WHERE nr.file_hash = ?1",
                    )
                    .map_err(|e| format!("query name_resolution failed: {}", e))?;
                let rows = stmt
                    .query_map([&file_hash], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| format!("query name_resolution failed: {}", e))?;
                let mut map = HashMap::new();
                for row in rows {
                    let (local_name, entry_name) =
                        row.map_err(|e| format!("read name_resolution row: {}", e))?;
                    map.insert(local_name, entry_name);
                }
                map
            };

            let (namespace, name_map) = match &import.target {
                ImportTarget::Glob { alias } => {
                    // For glob imports, the scope_names IS the name map.
                    (alias.as_ref().map(|a| a.node.clone()), Some(scope_names))
                }
                ImportTarget::Named(entries) => {
                    // For named imports, filter scope_names to only requested names.
                    let mut map = HashMap::new();
                    for entry in entries {
                        let orig_name = &entry.name.node;
                        let local = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| orig_name.clone());
                        if let Some(huc_name) = scope_names.get(orig_name) {
                            map.insert(local, huc_name.clone());
                        }
                    }
                    (None, Some(map))
                }
            };

            let source = EntrySource {
                conn: Rc::clone(&conn),
                name_map,
            };
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

    /// Find the entry source and .huc-side name for the given reference.
    fn find_entry<'a>(
        &'a self,
        namespace: &[ast::Ident],
        local_name: &'a str,
    ) -> Result<(&'a EntrySource, &'a str), String> {
        if namespace.is_empty() {
            // Search un-namespaced sources in order
            for src in &self.default_sources {
                if let Some(huc_name) = src.resolve_name(local_name) {
                    return Ok((src, huc_name));
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
                Some(huc_name) => Ok((src, huc_name)),
                None => Err(format!("entry '{}' not found in namespace '{}'", local_name, ns)),
            }
        }
    }

    /// Query display texts for tag axes and values.
    ///
    /// Returns a map from `(axis_name, value_name)` to `display_text`,
    /// plus a map from `axis_name` to its own display text (first row's lang).
    /// Searches all sources and merges results.
    pub fn query_tag_display(&self) -> (HashMap<String, String>, HashMap<(String, String), String>) {
        let mut axis_display: HashMap<String, String> = HashMap::new();
        let mut value_display: HashMap<(String, String), String> = HashMap::new();
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT axis_name, value_name, display_text FROM tagaxis_meta",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row in rows.flatten() {
                let (axis, value, display) = row;
                // Use the value display text for axis display if not yet set
                // (axis_name itself doesn't have a separate display row,
                // but we capitalize the axis name as fallback).
                axis_display.entry(axis.clone()).or_insert_with(|| {
                    let mut c = axis.chars();
                    match c.next() {
                        Some(first) => first.to_uppercase().to_string() + c.as_str(),
                        None => axis.clone(),
                    }
                });
                value_display.entry((axis, value)).or_insert(display);
            }
        }
        (axis_display, value_display)
    }

    /// Query all forms for a given entry name.
    ///
    /// Returns a list of `(form_string, tags_string)` pairs, where `tags_string`
    /// is comma-separated `axis=value` pairs (e.g. `"case=nom,number=sg"`).
    /// Searches default sources first, then namespaced sources.
    pub fn query_forms(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT f.form_str, f.tags FROM forms f \
                 JOIN entries e ON f.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY f.tags",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let forms: Vec<(String, String)> = rows.flatten().collect();
            if !forms.is_empty() {
                return forms;
            }
        }
        Vec::new()
    }

    /// Query all meanings for a given entry name.
    ///
    /// Returns a list of `(meaning_id, meaning_text)` pairs from `entry_meanings`.
    /// If the entry uses a single meaning (no `entry_meanings` rows), returns empty.
    pub fn query_meanings(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT m.meaning_id, m.meaning_text FROM entry_meanings m \
                 JOIN entries e ON m.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY m.rowid",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let meanings: Vec<(String, String)> = rows.flatten().collect();
            if !meanings.is_empty() {
                return meanings;
            }
        }
        Vec::new()
    }

    /// Query classificatory tags for a given entry name.
    ///
    /// Returns a list of `(axis, value)` pairs from `entry_tags`.
    pub fn query_entry_tags(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT t.axis, t.value FROM entry_tags t \
                 JOIN entries e ON t.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY t.axis, t.value",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let tags: Vec<(String, String)> = rows.flatten().collect();
            if !tags.is_empty() {
                return tags;
            }
        }
        Vec::new()
    }

    /// Query etymology information for a given entry name.
    ///
    /// Returns `(etymology_proto, etymology_note)` — both optional.
    pub fn query_etymology(&self, entry_name: &str) -> (Option<String>, Option<String>) {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let result = src.conn.query_row(
                "SELECT etymology_proto, etymology_note FROM entries WHERE name = ?1",
                [entry_name],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
            );
            if let Ok((proto, note)) = result {
                if proto.is_some() || note.is_some() {
                    return (proto, note);
                }
            }
        }
        (None, None)
    }

    /// Query the definition order of tag axis values.
    ///
    /// Returns a map from `axis_name` to an ordered list of `value_name`s,
    /// preserving the order they appear in `tagaxis_meta` (by rowid).
    pub fn query_axis_value_order(&self) -> HashMap<String, Vec<String>> {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT DISTINCT axis_name, value_name FROM tagaxis_meta ORDER BY id",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row in rows.flatten() {
                let (axis, value) = row;
                let vals = result.entry(axis).or_default();
                if !vals.contains(&value) {
                    vals.push(value);
                }
            }
        }
        result
    }

    /// Query linked entry names by link type for a given entry name.
    ///
    /// Returns a list of `(dst_entry_name, link_type)` pairs.
    pub fn query_links(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT e2.name, l.link_type FROM links l \
                 JOIN entries e1 ON l.src_entry_id = e1.id \
                 JOIN entries e2 ON l.dst_entry_id = e2.id \
                 WHERE e1.name = ?1 ORDER BY l.link_type, e2.name",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let links: Vec<(String, String)> = rows.flatten().collect();
            if !links.is_empty() {
                return links;
            }
        }
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// A resolved piece: a string part, a glue marker, or a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPart {
    Text(String),
    Glue,
    Newline,
    TagOpen(String, Vec<(String, String)>),
    TagClose(String),
    SelfClosingTag(String, Vec<(String, String)>),
}

/// Metadata about a resolved entry reference (for annotated rendering).
#[derive(Debug, Clone)]
pub struct EntryAnnotation {
    /// Entry name (identifier in the .hu file).
    pub entry_name: String,
    /// The headword of the entry.
    pub headword: String,
    /// The meaning field of the entry.
    pub meaning: String,
    /// Form tags if a specific form was requested, e.g. "tense=present, number=sg".
    pub form_tags: Option<String>,
}

/// A resolved piece with optional entry annotation.
#[derive(Debug, Clone)]
pub enum AnnotatedPart {
    /// Literal text (no entry reference).
    Lit(String),
    /// Text resolved from a dictionary entry, with metadata.
    Entry { text: String, annotation: EntryAnnotation },
    Glue,
    Newline,
    /// Opening XML-like tag: `<em>` → `TagOpen("em", [])`
    TagOpen(String, Vec<(String, String)>),
    /// Closing XML-like tag: `</em>` → `TagClose("em")`
    TagClose(String),
    /// Self-closing XML-like tag: `<br/>` → `SelfClosingTag("br", [])`
    SelfClosingTag(String, Vec<(String, String)>),
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
            ast::Token::Newline => {
                parts.push(ResolvedPart::Newline);
            }
            ast::Token::Lit(s) => {
                parts.push(ResolvedPart::Text(s.node.clone()));
            }
            ast::Token::Ref(entry_ref) => {
                let local_name = &entry_ref.entry_id.node;
                let (src, db_name) = ctx.find_entry(&entry_ref.namespace, local_name)?;

                // Verify the entry exists and get its headword.
                let headword: String = src
                    .conn
                    .query_row(
                        "SELECT headword FROM entries WHERE name = ?1",
                        [db_name],
                        |row| row.get(0),
                    )
                    .map_err(|_| format!("entry '{}' is not defined", local_name))?;

                if let Some(stem_name) = &entry_ref.stem_spec {
                    // Check stem existence with a clear message.
                    let stem_value: String = src
                        .conn
                        .query_row(
                            "SELECT s.stem_value FROM stems s \
                             JOIN entries e ON s.entry_id = e.id \
                             WHERE e.name = ?1 AND s.stem_name = ?2",
                            rusqlite::params![db_name, stem_name.node],
                            |row| row.get(0),
                        )
                        .map_err(|_| {
                            // List available stems for a helpful message.
                            let available = list_stems(src, db_name);
                            if available.is_empty() {
                                format!(
                                    "entry '{}' has no stems defined (requested [$={}])",
                                    local_name, stem_name.node
                                )
                            } else {
                                format!(
                                    "entry '{}' has no stem '{}' (available: {})",
                                    local_name, stem_name.node, available.join(", ")
                                )
                            }
                        })?;
                    parts.push(ResolvedPart::Text(stem_value));
                } else { match &entry_ref.form_spec {
                    None => {
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
                            format!(
                                "entry '{}' has no form matching [{}]",
                                local_name, tags_display
                            )
                        })?;
                        parts.push(ResolvedPart::Text(form_str));
                    }
                } }
            }
            ast::Token::Tag { name, attrs, children, .. } => {
                parts.push(ResolvedPart::TagOpen(name.clone(), attrs.clone()));
                parts.extend(resolve(children, ctx)?);
                parts.push(ResolvedPart::TagClose(name.clone()));
            }
            ast::Token::SelfClosingTag { name, attrs, .. } => {
                parts.push(ResolvedPart::SelfClosingTag(name.clone(), attrs.clone()));
            }
        }
    }
    Ok(parts)
}

/// Resolve a list of AST tokens with entry annotations (for HTML rendering).
pub fn resolve_annotated(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
) -> Result<Vec<AnnotatedPart>, String> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            ast::Token::Glue => {
                parts.push(AnnotatedPart::Glue);
            }
            ast::Token::Newline => {
                parts.push(AnnotatedPart::Newline);
            }
            ast::Token::Lit(s) => {
                parts.push(AnnotatedPart::Lit(s.node.clone()));
            }
            ast::Token::Ref(entry_ref) => {
                let local_name = &entry_ref.entry_id.node;
                let (src, db_name) = ctx.find_entry(&entry_ref.namespace, local_name)?;

                let (headword, meaning): (String, String) = src
                    .conn
                    .query_row(
                        "SELECT headword, meaning FROM entries WHERE name = ?1",
                        [db_name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|_| format!("entry '{}' is not defined", local_name))?;

                if let Some(stem_name) = &entry_ref.stem_spec {
                    let stem_value: String = src
                        .conn
                        .query_row(
                            "SELECT s.stem_value FROM stems s \
                             JOIN entries e ON s.entry_id = e.id \
                             WHERE e.name = ?1 AND s.stem_name = ?2",
                            rusqlite::params![db_name, stem_name.node],
                            |row| row.get(0),
                        )
                        .map_err(|_| {
                            let available = list_stems(src, db_name);
                            if available.is_empty() {
                                format!(
                                    "entry '{}' has no stems defined (requested [$={}])",
                                    local_name, stem_name.node
                                )
                            } else {
                                format!(
                                    "entry '{}' has no stem '{}' (available: {})",
                                    local_name, stem_name.node, available.join(", ")
                                )
                            }
                        })?;
                    parts.push(AnnotatedPart::Entry {
                        text: stem_value,
                        annotation: EntryAnnotation {
                            entry_name: db_name.to_string(),
                            headword: headword.clone(),
                            meaning: meaning.clone(),
                            form_tags: Some(format!("$={}", stem_name.node)),
                        },
                    });
                } else {
                    match &entry_ref.form_spec {
                        None => {
                            parts.push(AnnotatedPart::Entry {
                                text: headword.clone(),
                                annotation: EntryAnnotation {
                                    entry_name: db_name.to_string(),
                                    headword,
                                    meaning,
                                    form_tags: None,
                                },
                            });
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
                                        Some((
                                            parts.next()?.to_string(),
                                            parts.next()?.to_string(),
                                        ))
                                    })
                                    .collect();
                                stored.sort();
                                if stored == requested {
                                    found = Some(form_str);
                                    break;
                                }
                            }

                            let tags_display = form_spec
                                .conditions
                                .iter()
                                .map(|c| format!("{}={}", c.axis.node, c.value.node))
                                .collect::<Vec<_>>()
                                .join(", ");

                            let form_str = found.ok_or_else(|| {
                                format!(
                                    "entry '{}' has no form matching [{}]",
                                    local_name, tags_display
                                )
                            })?;
                            parts.push(AnnotatedPart::Entry {
                                text: form_str,
                                annotation: EntryAnnotation {
                                    entry_name: db_name.to_string(),
                                    headword,
                                    meaning,
                                    form_tags: Some(tags_display),
                                },
                            });
                        }
                    }
                }
            }
            ast::Token::Tag { name, attrs, children, .. } => {
                parts.push(AnnotatedPart::TagOpen(name.clone(), attrs.clone()));
                parts.extend(resolve_annotated(children, ctx)?);
                parts.push(AnnotatedPart::TagClose(name.clone()));
            }
            ast::Token::SelfClosingTag { name, attrs, .. } => {
                parts.push(AnnotatedPart::SelfClosingTag(name.clone(), attrs.clone()));
            }
        }
    }
    Ok(parts)
}

/// List available stem names for an entry (for error messages).
fn list_stems(src: &EntrySource, db_name: &str) -> Vec<String> {
    let mut stmt = match src.conn.prepare(
        "SELECT s.stem_name FROM stems s \
         JOIN entries e ON s.entry_id = e.id \
         WHERE e.name = ?1 ORDER BY s.stem_name",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([db_name], |row| row.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.flatten().collect()
}

/// Read render config from the first available `.huc` source in the context,
/// falling back to defaults.
pub fn read_render_config(ctx: &ResolveContext) -> (String, String) {
    // Try namespaced sources first, then default sources
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

// ---------------------------------------------------------------------------
// PartRenderer trait — format-agnostic rendering of AnnotatedPart sequences
// ---------------------------------------------------------------------------

/// Trait for rendering a sequence of resolved [`AnnotatedPart`]s into a
/// target format (HTML, plain text, etc.).
///
/// Library users can implement this trait to add custom output formats
/// (e.g. LaTeX, EPUB) without modifying the core resolution pipeline.
pub trait PartRenderer {
    /// Render annotated parts into the target format.
    ///
    /// * `separator` — default token separator (e.g. `" "`)
    /// * `no_sep_before` — characters that suppress the preceding separator
    ///   (e.g. `".,;:!?"`)
    fn render(&self, parts: &[AnnotatedPart], separator: &str, no_sep_before: &str) -> String;
}

/// Plain-text renderer: strips tags and joins text content with separators.
pub struct PlainTextRenderer;

impl PartRenderer for PlainTextRenderer {
    fn render(&self, parts: &[AnnotatedPart], separator: &str, no_sep_before: &str) -> String {
        let mut result = String::new();
        let mut glue_next = false;
        let mut newline_next = false;
        let mut has_content = false;

        for part in parts {
            match part {
                AnnotatedPart::Glue => {
                    glue_next = true;
                }
                AnnotatedPart::Newline => {
                    newline_next = true;
                    glue_next = false;
                }
                AnnotatedPart::Lit(text) | AnnotatedPart::Entry { text, .. } => {
                    if newline_next {
                        result.push('\n');
                        newline_next = false;
                    } else if has_content && !separator.is_empty() && !glue_next {
                        let suppress = text
                            .chars()
                            .next()
                            .map(|c| no_sep_before.contains(c))
                            .unwrap_or(false);
                        if !suppress {
                            result.push_str(separator);
                        }
                    }
                    glue_next = false;
                    has_content = true;
                    result.push_str(text);
                }
                // Tags are stripped in plain-text output.
                AnnotatedPart::TagOpen(..)
                | AnnotatedPart::TagClose(_)
                | AnnotatedPart::SelfClosingTag(..) => {}
            }
        }
        result
    }
}

/// Join resolved parts using separator, suppressing it before certain characters
/// and around `Glue` markers.
pub fn smart_join(parts: &[ResolvedPart], separator: &str, no_sep_before: &str) -> String {
    let mut result = String::new();
    let mut glue_next = false;
    let mut newline_next = false;
    for part in parts {
        match part {
            ResolvedPart::Glue => {
                glue_next = true;
            }
            ResolvedPart::Newline => {
                newline_next = true;
                glue_next = false;
            }
            ResolvedPart::Text(text) => {
                if newline_next {
                    result.push('\n');
                    newline_next = false;
                } else if !result.is_empty() && !separator.is_empty() && !glue_next {
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
            // Tags are structural markers for HTML; plain-text join ignores them.
            ResolvedPart::TagOpen(..)
            | ResolvedPart::TagClose(_)
            | ResolvedPart::SelfClosingTag(..) => {}
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
    fn test_smart_join_newline() {
        // "hello" // "world" → "hello\nworld"
        let parts = vec![
            text("hello"),
            ResolvedPart::Newline,
            text("world"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "hello\nworld");
    }

    #[test]
    fn test_smart_join_newline_no_extra_separator() {
        // Newline should replace separator, not add one
        let parts = vec![
            text("line1"),
            text("word"),
            ResolvedPart::Newline,
            text("line2"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "line1 word\nline2");
    }

    #[test]
    fn test_parse_hut_newline() {
        let hut = parse_hut(r#""hello" // "world""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 3);
        assert!(matches!(hut.tokens[0], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[1], ast::Token::Newline));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
    }

    #[test]
    fn test_parse_hut_stem_spec() {
        let hut = parse_hut(r#"gelmek[$=root]~"iyor""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 3);
        if let ast::Token::Ref(r) = &hut.tokens[0] {
            assert_eq!(r.entry_id.node, "gelmek");
            assert!(r.form_spec.is_none());
            assert_eq!(r.stem_spec.as_ref().unwrap().node, "root");
        } else {
            panic!("expected Ref token");
        }
        assert!(matches!(hut.tokens[1], ast::Token::Glue));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
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
