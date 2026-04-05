//! Phase 1: file loading, `@use`/`@reference` resolution, symbol registration.
//!
//! Starting from the entry file, recursively loads all referenced files (DFS),
//! detects `@use` cycles, parses each file, and registers all top-level
//! declarations into the global [`SymbolTable`](crate::symbol_table::SymbolTable).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Export, File, ImportTarget, Item};
use crate::error::{Diagnostic, Diagnostics};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::span::{FileId, SourceMap};
use crate::stdlib;
use crate::symbol_table::{ImportedSymbol, SymbolKind, SymbolTable};

// ---------------------------------------------------------------------------
// Import path classification
// ---------------------------------------------------------------------------

/// Classified import path.
enum ImportSource {
    /// Relative filesystem path (existing behavior).
    Relative(String),
    /// Standard library module, e.g. `"std:ipa"` → module name `"ipa"`.
    Std(String),
    /// Unsupported scheme (e.g. `"https://..."`).
    UnsupportedScheme(String),
}

fn classify_import_path(raw: &str) -> ImportSource {
    if let Some(rest) = raw.strip_prefix("std:") {
        ImportSource::Std(rest.to_string())
    } else if raw.contains("://") {
        ImportSource::UnsupportedScheme(raw.to_string())
    } else {
        ImportSource::Relative(raw.to_string())
    }
}

/// Result of phase 1: all files parsed, symbols registered.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone)]
pub struct Phase1Result {
    pub files: HashMap<FileId, File>,
    pub source_map: SourceMap,
    pub symbol_table: SymbolTable,
    pub diagnostics: Diagnostics,
    /// Map from file path to FileId.
    pub path_to_id: HashMap<PathBuf, FileId>,
}

/// Run phase 1 from a set of import directives (used by `.hut` rendering).
///
/// Creates a virtual entry file that contains only the given imports, then
/// recursively loads all referenced `.hu` files exactly as [`run_phase1`] does.
/// The virtual entry has no local declarations; its scope only contains the
/// imported symbols.
///
/// `base_dir` is used to resolve relative paths in the import directives.
pub fn run_phase1_virtual(
    imports: &[crate::ast::Import],
    base_dir: &Path,
) -> Phase1Result {
    let mut ctx = Phase1Ctx {
        files: HashMap::new(),
        source_map: SourceMap::new(),
        symbol_table: SymbolTable::new(),
        diagnostics: Diagnostics::new(),
        path_to_id: HashMap::new(),
        use_stack: Vec::new(),
        reference_visited: HashSet::new(),
    };

    // Create a virtual entry file with an empty source.
    let virtual_path = base_dir.join("<hut-virtual>");
    let file_id = ctx.source_map.add_file(virtual_path.clone(), String::new());
    ctx.path_to_id.insert(virtual_path, file_id);
    ctx.files.insert(file_id, File { items: Vec::new() });

    // Process each import as a @reference from the virtual file.
    for import in imports {
        let dep_id = ctx.resolve_import(
            &import.path.node, base_dir, false, import.path.span,
        );
        if let Some(dep_id) = dep_id {
            ctx.register_imports(file_id, dep_id, &import.target, false);
        }
    }

    Phase1Result {
        files: ctx.files,
        source_map: ctx.source_map,
        symbol_table: ctx.symbol_table,
        diagnostics: ctx.diagnostics,
        path_to_id: ctx.path_to_id,
    }
}

/// Run phase 1: load files, parse, hoist declarations, register symbols.
pub fn run_phase1(entry_path: &Path) -> Phase1Result {
    let mut ctx = Phase1Ctx {
        files: HashMap::new(),
        source_map: SourceMap::new(),
        symbol_table: SymbolTable::new(),
        diagnostics: Diagnostics::new(),
        path_to_id: HashMap::new(),
        use_stack: Vec::new(),
        reference_visited: HashSet::new(),
    };

    let entry_path = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.to_path_buf());

    ctx.load_file_recursive(&entry_path, true);

    Phase1Result {
        files: ctx.files,
        source_map: ctx.source_map,
        symbol_table: ctx.symbol_table,
        diagnostics: ctx.diagnostics,
        path_to_id: ctx.path_to_id,
    }
}

struct Phase1Ctx {
    files: HashMap<FileId, File>,
    source_map: SourceMap,
    symbol_table: SymbolTable,
    diagnostics: Diagnostics,
    path_to_id: HashMap<PathBuf, FileId>,
    /// Stack for @use cycle detection (DFS).
    use_stack: Vec<PathBuf>,
    /// Set of files visited via @reference (dedup, no cycle error).
    reference_visited: HashSet<PathBuf>,
}

impl Phase1Ctx {
    fn load_file_recursive(
        &mut self,
        path: &Path,
        is_use: bool,
    ) -> Option<FileId> {
        self.load_file_recursive_inner(path, is_use, None)
    }

    fn load_file_recursive_with_span(
        &mut self,
        path: &Path,
        is_use: bool,
        import_span: crate::ast::Span,
    ) -> Option<FileId> {
        self.load_file_recursive_inner(path, is_use, Some(import_span))
    }

    /// Classify an import path and dispatch to the appropriate loader.
    fn resolve_import(
        &mut self,
        raw_path: &str,
        base_dir: &Path,
        is_use: bool,
        span: crate::ast::Span,
    ) -> Option<FileId> {
        match classify_import_path(raw_path) {
            ImportSource::Relative(rel) => {
                let import_path = base_dir.join(&rel);
                if is_use {
                    self.load_file_recursive_with_span(&import_path, true, span)
                } else {
                    if !self.reference_visited.contains(&import_path) {
                        self.reference_visited.insert(import_path.clone());
                        self.load_file_recursive_with_span(&import_path, false, span)
                    } else {
                        self.path_to_id.get(&import_path).copied()
                    }
                }
            }
            ImportSource::Std(module_name) => {
                self.load_std_module(&module_name, span)
            }
            ImportSource::UnsupportedScheme(scheme) => {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "unsupported import scheme: '{}'",
                        scheme,
                    ))
                    .with_label(span, "scheme not supported"),
                );
                None
            }
        }
    }

    /// Load a standard library module by name, returning its [`FileId`].
    ///
    /// On first load, the embedded source is lexed, parsed, and symbols are
    /// registered. Subsequent loads return the cached [`FileId`].
    fn load_std_module(
        &mut self,
        module_name: &str,
        span: crate::ast::Span,
    ) -> Option<FileId> {
        let synthetic = stdlib::synthetic_path(module_name);

        // Already loaded?
        if let Some(&id) = self.path_to_id.get(&synthetic) {
            return Some(id);
        }

        let source = match stdlib::lookup(module_name) {
            Some(s) => s.to_string(),
            None => {
                let available = stdlib::available_modules();
                let hint = if available.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (available: {})",
                        available
                            .iter()
                            .filter(|n| !n.starts_with('_'))
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                };
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "unknown standard library module '{}'{}",
                        module_name, hint,
                    ))
                    .with_label(span, "not found"),
                );
                return None;
            }
        };

        let file_id = self.source_map.add_file(synthetic.clone(), source);
        self.path_to_id.insert(synthetic, file_id);

        // Lex & parse
        let lexer = Lexer::new(self.source_map.source(file_id), file_id);
        let (tokens, lex_errors) = lexer.tokenize();
        for e in lex_errors {
            self.diagnostics.add(e);
        }

        let parser = Parser::new(tokens, file_id);
        let (file, parse_errors) = parser.parse();
        for e in parse_errors {
            self.diagnostics.add(e);
        }

        // Register local symbols
        for (idx, item) in file.items.iter().enumerate() {
            let (name, kind) = match &item.node {
                Item::TagAxis(ta) => (ta.name.node.clone(), SymbolKind::TagAxis),
                Item::Extend(ext) => (ext.name.node.clone(), SymbolKind::Extend),
                Item::Inflection(infl) => (infl.name.node.clone(), SymbolKind::Inflection),
                Item::Entry(entry) => (entry.name.node.clone(), SymbolKind::Entry),
                Item::PhonRule(pr) => (pr.name.node.clone(), SymbolKind::PhonRule),
                Item::Use(_) | Item::Reference(_) | Item::Export(_) | Item::Render(_) => continue,
            };
            if let Err(diag) = self.symbol_table.register_local(
                file_id, name, kind, item.span, idx,
            ) {
                self.diagnostics.add(diag);
            }
        }

        // Process imports within the std module (must use std: scheme, not relative paths)
        let imports: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Use(imp) => Some((true, imp.clone())),
                Item::Reference(imp) => Some((false, imp.clone())),
                _ => None,
            })
            .collect();

        self.files.insert(file_id, file);

        for (is_use_import, import) in imports {
            let dep_id = self.resolve_import(
                &import.path.node,
                // base_dir is irrelevant for std modules — relative imports
                // within std will fail at canonicalize, which is fine since
                // inter-std deps should use std: scheme.
                Path::new("<std>"),
                is_use_import,
                import.path.span,
            );
            if let Some(dep_id) = dep_id {
                self.register_imports(file_id, dep_id, &import.target, is_use_import);
            }
        }

        Some(file_id)
    }

    fn load_file_recursive_inner(
        &mut self,
        path: &Path,
        is_use: bool,
        import_span: Option<crate::ast::Span>,
    ) -> Option<FileId> {
        let path = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());

        // Check if already loaded
        if let Some(&id) = self.path_to_id.get(&path) {
            if is_use && self.use_stack.contains(&path) {
                let mut diag = Diagnostic::error(format!(
                    "circular @use detected: {}",
                    self.use_stack
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ));
                if let Some(span) = import_span {
                    diag = diag.with_label(span, "imported here");
                }
                self.diagnostics.add(diag);
                return None;
            }
            return Some(id);
        }

        if is_use && self.use_stack.contains(&path) {
            let mut diag = Diagnostic::error(format!(
                "circular @use detected: {}",
                self.use_stack
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ));
            if let Some(span) = import_span {
                diag = diag.with_label(span, "imported here");
            }
            self.diagnostics.add(diag);
            return None;
        }

        // Read source
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                let mut diag = Diagnostic::error(
                    format!("cannot read file '{}': {}", path.display(), e),
                );
                if let Some(span) = import_span {
                    diag = diag.with_label(span, "referenced here");
                }
                self.diagnostics.add(diag);
                return None;
            }
        };

        let file_id = self.source_map.add_file(path.clone(), source.clone());
        self.path_to_id.insert(path.clone(), file_id);

        // Lex & parse
        let lexer = Lexer::new(self.source_map.source(file_id), file_id);
        let (tokens, lex_errors) = lexer.tokenize();
        for e in lex_errors {
            self.diagnostics.add(e);
        }

        let parser = Parser::new(tokens, file_id);
        let (file, parse_errors) = parser.parse();
        for e in parse_errors {
            self.diagnostics.add(e);
        }

        // Register local symbols (hoisted declarations)
        for (idx, item) in file.items.iter().enumerate() {
            let (name, kind) = match &item.node {
                Item::TagAxis(ta) => (ta.name.node.clone(), SymbolKind::TagAxis),
                Item::Extend(ext) => (ext.name.node.clone(), SymbolKind::Extend),
                Item::Inflection(infl) => (infl.name.node.clone(), SymbolKind::Inflection),
                Item::Entry(entry) => (entry.name.node.clone(), SymbolKind::Entry),
                Item::PhonRule(pr) => (pr.name.node.clone(), SymbolKind::PhonRule),
                Item::Use(_) | Item::Reference(_) | Item::Export(_) | Item::Render(_) => continue,
            };
            if let Err(diag) = self.symbol_table.register_local(
                file_id, name, kind, item.span, idx,
            ) {
                self.diagnostics.add(diag);
            }
        }

        // Process @use directives (recursive, with cycle detection)
        if is_use {
            self.use_stack.push(path.clone());
        }

        let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

        // Collect imports and exports to process (to avoid borrow conflicts)
        let imports: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Use(imp) => Some((true, imp.clone())),
                Item::Reference(imp) => Some((false, imp.clone())),
                _ => None,
            })
            .collect();

        let exports: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Export(exp) => Some(exp.clone()),
                _ => None,
            })
            .collect();

        self.files.insert(file_id, file);

        for (is_use_import, import) in imports {
            let dep_id = self.resolve_import(
                &import.path.node, &base_dir, is_use_import, import.path.span,
            );
            if let Some(dep_id) = dep_id {
                // Register imported symbols into this file's scope
                self.register_imports(file_id, dep_id, &import.target, is_use_import);
            }
        }

        // Process @export directives (after all imports are resolved)
        for export in exports {
            self.process_export(file_id, &base_dir, &export);
        }

        if is_use {
            self.use_stack.pop();
        }

        Some(file_id)
    }

    /// Build the unified available-symbol map from a source file's locals + exports,
    /// filtered by `allowed_kind`. Locals take priority over exports of the same name.
    fn collect_available_symbols(
        source_scope: &crate::symbol_table::Scope,
        allowed_kind: impl Fn(SymbolKind) -> bool,
    ) -> HashMap<String, ImportedSymbol> {
        let mut available: HashMap<String, ImportedSymbol> = HashMap::new();

        // Add locals
        for sym in source_scope.locals.values().filter(|s| allowed_kind(s.kind)) {
            available.insert(sym.name.clone(), ImportedSymbol {
                local_name: sym.name.clone(),
                original_name: sym.name.clone(),
                namespace: None,
                kind: sym.kind,
                source_file: sym.file_id,
                span: sym.span,
                item_index: sym.item_index,
            });
        }

        // Add exports (re-exported symbols from transitive deps); locals take priority
        for exp in source_scope.exports.iter().filter(|s| allowed_kind(s.kind)) {
            available.entry(exp.local_name.clone()).or_insert_with(|| exp.clone());
        }

        available
    }

    fn register_imports(
        &mut self,
        into_file: FileId,
        from_file: FileId,
        target: &ImportTarget,
        is_use: bool,
    ) {
        let source_scope = match self.symbol_table.scope(from_file) {
            Some(s) => s,
            None => return,
        };

        let allowed_kind = if is_use {
            |k: SymbolKind| k != SymbolKind::Entry
        } else {
            |k: SymbolKind| k == SymbolKind::Entry
        };

        // Collect available symbols from source (locals + exports) BEFORE mutating
        let available = Self::collect_available_symbols(source_scope, allowed_kind);

        // Now we can drop the immutable borrow and do mutations
        match target {
            ImportTarget::Glob { alias } => {
                let namespace = alias.as_ref().map(|a| a.node.clone());
                let scope = self.symbol_table.scope_mut(into_file);
                for sym in available.values() {
                    scope.imports.push(ImportedSymbol {
                        local_name: sym.original_name.clone(),
                        original_name: sym.original_name.clone(),
                        namespace: namespace.clone(),
                        kind: sym.kind,
                        source_file: sym.source_file,
                        span: sym.span,
                        item_index: sym.item_index,
                    });
                }
            }
            ImportTarget::Named(entries) => {
                for entry in entries {
                    let name = &entry.name.node;
                    if let Some(sym) = available.get(name) {
                        let local_name = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| name.clone());
                        let scope = self.symbol_table.scope_mut(into_file);
                        scope.imports.push(ImportedSymbol {
                            local_name,
                            original_name: sym.original_name.clone(),
                            namespace: None,
                            kind: sym.kind,
                            source_file: sym.source_file,
                            span: sym.span,
                            item_index: sym.item_index,
                        });
                    } else {
                        // Check if the symbol exists but is the wrong kind
                        let source_scope = self.symbol_table.scope(from_file);
                        let exists_wrong_kind = source_scope.map_or(false, |s| {
                            s.locals.contains_key(name) || s.exports.iter().any(|e| e.local_name == *name)
                        });
                        if exists_wrong_kind {
                            let what = if is_use { "entry" } else { "declaration" };
                            self.diagnostics.add(
                                Diagnostic::error(format!(
                                    "cannot import {} '{}' via @{}",
                                    what,
                                    name,
                                    if is_use { "use" } else { "reference" }
                                ))
                                .with_label(entry.name.span, "imported here"),
                            );
                        } else {
                            self.diagnostics.add(
                                Diagnostic::error(format!(
                                    "symbol '{}' not found in imported file",
                                    name
                                ))
                                .with_label(entry.name.span, "not found"),
                            );
                        }
                    }
                }
            }
        }
    }

    fn process_export(&mut self, file_id: FileId, base_dir: &Path, export: &Export) {
        if let Some(ref path_lit) = export.path {
            // Form 2: combined import + re-export from file
            let dep_id = self.resolve_import(
                &path_lit.node, base_dir, export.is_use, path_lit.span,
            );
            if let Some(dep_id) = dep_id {
                // Import into this file's scope (so the file itself can use them)
                self.register_imports(file_id, dep_id, &export.target, export.is_use);
                // Also register as exports (so downstream importers can see them)
                self.register_exports_from_file(file_id, dep_id, &export.target, export.is_use);
            }
        } else {
            // Form 1: re-export already-imported symbols
            self.register_exports_from_scope(file_id, &export.target, export.is_use);
        }
    }

    fn register_exports_from_file(
        &mut self,
        into_file: FileId,
        from_file: FileId,
        target: &ImportTarget,
        is_use: bool,
    ) {
        let source_scope = match self.symbol_table.scope(from_file) {
            Some(s) => s,
            None => return,
        };

        let allowed_kind = if is_use {
            |k: SymbolKind| k != SymbolKind::Entry
        } else {
            |k: SymbolKind| k == SymbolKind::Entry
        };

        let available = Self::collect_available_symbols(source_scope, allowed_kind);

        match target {
            ImportTarget::Glob { .. } => {
                // Export all available symbols (strip namespace — downstream applies its own)
                let scope = self.symbol_table.scope_mut(into_file);
                for sym in available.values() {
                    scope.exports.push(ImportedSymbol {
                        local_name: sym.original_name.clone(),
                        original_name: sym.original_name.clone(),
                        namespace: None,
                        kind: sym.kind,
                        source_file: sym.source_file,
                        span: sym.span,
                        item_index: sym.item_index,
                    });
                }
            }
            ImportTarget::Named(entries) => {
                // Collect into temp vec to avoid borrow issues
                let mut to_export = Vec::new();
                for entry in entries {
                    let name = &entry.name.node;
                    if let Some(sym) = available.get(name) {
                        let export_name = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| sym.original_name.clone());
                        to_export.push(ImportedSymbol {
                            local_name: export_name,
                            original_name: sym.original_name.clone(),
                            namespace: None,
                            kind: sym.kind,
                            source_file: sym.source_file,
                            span: sym.span,
                            item_index: sym.item_index,
                        });
                    } else {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "symbol '{}' not found in imported file",
                                name
                            ))
                            .with_label(entry.name.span, "not found"),
                        );
                    }
                }
                let scope = self.symbol_table.scope_mut(into_file);
                scope.exports.extend(to_export);
            }
        }
    }

    fn register_exports_from_scope(
        &mut self,
        file_id: FileId,
        target: &ImportTarget,
        is_use: bool,
    ) {
        let scope = match self.symbol_table.scope(file_id) {
            Some(s) => s,
            None => return,
        };

        let allowed_kind = if is_use {
            |k: SymbolKind| k != SymbolKind::Entry
        } else {
            |k: SymbolKind| k == SymbolKind::Entry
        };

        match target {
            ImportTarget::Glob { .. } => {
                // Re-export all matching imports and locals
                let mut to_export: Vec<ImportedSymbol> = Vec::new();

                for imp in scope.imports.iter().filter(|i| allowed_kind(i.kind)) {
                    to_export.push(ImportedSymbol {
                        local_name: imp.original_name.clone(),
                        original_name: imp.original_name.clone(),
                        namespace: None,
                        kind: imp.kind,
                        source_file: imp.source_file,
                        span: imp.span,
                        item_index: imp.item_index,
                    });
                }

                for sym in scope.locals.values().filter(|s| allowed_kind(s.kind)) {
                    to_export.push(ImportedSymbol {
                        local_name: sym.name.clone(),
                        original_name: sym.name.clone(),
                        namespace: None,
                        kind: sym.kind,
                        source_file: sym.file_id,
                        span: sym.span,
                        item_index: sym.item_index,
                    });
                }

                let scope = self.symbol_table.scope_mut(file_id);
                scope.exports.extend(to_export);
            }
            ImportTarget::Named(entries) => {
                // Find each named symbol in imports or locals
                let existing_imports: Vec<_> = scope.imports.clone();
                let existing_locals: HashMap<String, _> = scope
                    .locals
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                let mut to_export = Vec::new();
                for entry in entries {
                    let name = &entry.name.node;
                    // Look in imports first
                    let found = existing_imports
                        .iter()
                        .find(|imp| imp.local_name == *name && allowed_kind(imp.kind));
                    if let Some(imp) = found {
                        let export_name = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| imp.original_name.clone());
                        to_export.push(ImportedSymbol {
                            local_name: export_name,
                            original_name: imp.original_name.clone(),
                            namespace: None,
                            kind: imp.kind,
                            source_file: imp.source_file,
                            span: imp.span,
                            item_index: imp.item_index,
                        });
                    } else if let Some(sym) = existing_locals.get(name) {
                        if allowed_kind(sym.kind) {
                            let export_name = entry
                                .alias
                                .as_ref()
                                .map(|a| a.node.clone())
                                .unwrap_or_else(|| sym.name.clone());
                            to_export.push(ImportedSymbol {
                                local_name: export_name,
                                original_name: sym.name.clone(),
                                namespace: None,
                                kind: sym.kind,
                                source_file: sym.file_id,
                                span: sym.span,
                                item_index: sym.item_index,
                            });
                        } else {
                            let what = if is_use { "entry" } else { "declaration" };
                            self.diagnostics.add(
                                Diagnostic::error(format!(
                                    "cannot export {} '{}' via @export {}",
                                    what,
                                    name,
                                    if is_use { "use" } else { "reference" }
                                ))
                                .with_label(entry.name.span, "here"),
                            );
                        }
                    } else {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "symbol '{}' not found in scope for @export",
                                name
                            ))
                            .with_label(entry.name.span, "not found"),
                        );
                    }
                }
                let scope = self.symbol_table.scope_mut(file_id);
                scope.exports.extend(to_export);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_relative_path() {
        assert!(matches!(classify_import_path("./foo.hu"), ImportSource::Relative(p) if p == "./foo.hu"));
        assert!(matches!(classify_import_path("../bar.hu"), ImportSource::Relative(p) if p == "../bar.hu"));
        assert!(matches!(classify_import_path("profile.hu"), ImportSource::Relative(p) if p == "profile.hu"));
    }

    #[test]
    fn classify_std_scheme() {
        assert!(matches!(classify_import_path("std:ipa"), ImportSource::Std(m) if m == "ipa"));
        assert!(matches!(classify_import_path("std:_test"), ImportSource::Std(m) if m == "_test"));
    }

    #[test]
    fn classify_unsupported_scheme() {
        assert!(matches!(classify_import_path("https://example.com/foo.hu"), ImportSource::UnsupportedScheme(_)));
        assert!(matches!(classify_import_path("http://example.com/foo.hu"), ImportSource::UnsupportedScheme(_)));
    }
}
