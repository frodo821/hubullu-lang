//! Phase 1: file loading, `@use`/`@reference` resolution, symbol registration.
//!
//! Starting from the entry file, recursively loads all referenced files (DFS),
//! detects `@use` cycles, parses each file, and registers all top-level
//! declarations into the global [`SymbolTable`](crate::symbol_table::SymbolTable).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{File, ImportTarget, Item};
use crate::error::{Diagnostic, Diagnostics};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::span::{FileId, SourceMap};
use crate::symbol_table::{ImportedSymbol, SymbolKind, SymbolTable};

/// Result of phase 1: all files parsed, symbols registered.
pub struct Phase1Result {
    pub files: HashMap<FileId, File>,
    pub source_map: SourceMap,
    pub symbol_table: SymbolTable,
    pub diagnostics: Diagnostics,
    /// Map from file path to FileId.
    pub path_to_id: HashMap<PathBuf, FileId>,
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
    fn load_file_recursive(&mut self, path: &Path, is_use: bool) -> Option<FileId> {
        let path = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());

        // Check if already loaded
        if let Some(&id) = self.path_to_id.get(&path) {
            if is_use && self.use_stack.contains(&path) {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "circular @use detected: {}",
                        self.use_stack
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    )),
                );
                return None;
            }
            return Some(id);
        }

        if is_use && self.use_stack.contains(&path) {
            self.diagnostics.add(
                Diagnostic::error(format!(
                    "circular @use detected: {}",
                    self.use_stack
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                )),
            );
            return None;
        }

        // Read source
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.diagnostics.add(
                    Diagnostic::error(format!("cannot read file '{}': {}", path.display(), e)),
                );
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
                Item::Use(_) | Item::Reference(_) => continue,
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

        // Collect imports to process (to avoid borrow conflicts)
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
            let import_path = base_dir.join(&import.path.node);
            let dep_id = if is_use_import {
                self.load_file_recursive(&import_path, true)
            } else {
                if !self.reference_visited.contains(&import_path) {
                    self.reference_visited.insert(import_path.clone());
                    self.load_file_recursive(&import_path, false)
                } else {
                    self.path_to_id.get(&import_path).copied()
                }
            };

            if let Some(dep_id) = dep_id {
                // Register imported symbols into this file's scope
                self.register_imports(file_id, dep_id, &import.target, is_use_import);
            }
        }

        if is_use {
            self.use_stack.pop();
        }

        Some(file_id)
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

        // Collect all data we need from source scope BEFORE mutating
        let symbols_to_import: Vec<_> = source_scope
            .locals
            .values()
            .filter(|s| allowed_kind(s.kind))
            .cloned()
            .collect();

        // Also collect the full locals map for named lookups
        let source_locals: HashMap<String, _> = source_scope
            .locals
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Now we can drop the immutable borrow and do mutations
        match target {
            ImportTarget::Glob { alias } => {
                let namespace = alias.as_ref().map(|a| a.node.clone());
                let scope = self.symbol_table.scope_mut(into_file);
                for sym in symbols_to_import {
                    scope.imports.push(ImportedSymbol {
                        local_name: sym.name.clone(),
                        original_name: sym.name.clone(),
                        namespace: namespace.clone(),
                        kind: sym.kind,
                        source_file: sym.file_id,
                        span: sym.span,
                        item_index: sym.item_index,
                    });
                }
            }
            ImportTarget::Named(entries) => {
                for entry in entries {
                    let name = &entry.name.node;
                    if let Some(sym) = source_locals.get(name) {
                        if !allowed_kind(sym.kind) {
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
                            continue;
                        }
                        let local_name = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| name.clone());
                        let scope = self.symbol_table.scope_mut(into_file);
                        scope.imports.push(ImportedSymbol {
                            local_name,
                            original_name: sym.name.clone(),
                            namespace: None,
                            kind: sym.kind,
                            source_file: sym.file_id,
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
            }
        }
    }
}
