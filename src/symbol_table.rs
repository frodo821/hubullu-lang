//! Symbol table with per-file scopes for name resolution.
//!
//! Each file gets a `Scope` containing locally defined symbols and symbols
//! brought in via `@use` / `@reference`. The `SymbolTable` holds all scopes
//! keyed by `FileId`.

use std::collections::HashMap;

use crate::ast::Span;
use crate::error::Diagnostic;
use crate::span::FileId;

/// Kind of symbol in the symbol table.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    TagAxis,
    Extend,
    Inflection,
    Entry,
    PhonRule,
}

/// A registered symbol.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub span: Span,
    /// Index into the source file's item list.
    pub item_index: usize,
}

/// Imported symbol with optional alias and namespace.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ImportedSymbol {
    pub original_name: String,
    pub local_name: String,
    pub namespace: Option<String>,
    pub kind: SymbolKind,
    pub source_file: FileId,
    pub span: Span,
    pub item_index: usize,
}

/// Per-file scope for name resolution.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone)]
pub struct Scope {
    /// Symbols defined locally in this file.
    pub locals: HashMap<String, Symbol>,
    /// Symbols imported via @use or @reference.
    pub imports: Vec<ImportedSymbol>,
    /// Symbols re-exported via @export.
    pub exports: Vec<ImportedSymbol>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a name in this scope.
    /// Returns all matching symbols (for ambiguity detection).
    pub fn resolve(&self, name: &str) -> Vec<ResolvedSymbol> {
        let mut results = Vec::new();

        // Check locals first
        if let Some(sym) = self.locals.get(name) {
            results.push(ResolvedSymbol {
                name: sym.name.clone(),
                kind: sym.kind,
                file_id: sym.file_id,
                span: sym.span,
                item_index: sym.item_index,
            });
        }

        // Check imports
        for imp in &self.imports {
            if imp.local_name == name {
                results.push(ResolvedSymbol {
                    name: imp.original_name.clone(),
                    kind: imp.kind,
                    file_id: imp.source_file,
                    span: imp.span,
                    item_index: imp.item_index,
                });
            }
        }

        results
    }

    /// Resolve a namespaced name like `ns.name`.
    pub fn resolve_qualified(&self, namespace: &str, name: &str) -> Vec<ResolvedSymbol> {
        let mut results = Vec::new();
        for imp in &self.imports {
            if imp.namespace.as_deref() == Some(namespace)
                && imp.original_name == name
            {
                results.push(ResolvedSymbol {
                    name: imp.original_name.clone(),
                    kind: imp.kind,
                    file_id: imp.source_file,
                    span: imp.span,
                    item_index: imp.item_index,
                });
            }
        }
        results
    }
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub span: Span,
    pub item_index: usize,
}

/// Global symbol table spanning all files.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone)]
pub struct SymbolTable {
    pub scopes: HashMap<FileId, Scope>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scope_mut(&mut self, file_id: FileId) -> &mut Scope {
        self.scopes.entry(file_id).or_default()
    }

    pub fn scope(&self, file_id: FileId) -> Option<&Scope> {
        self.scopes.get(&file_id)
    }

    /// Register a local symbol. Returns error diagnostic if duplicate.
    pub fn register_local(
        &mut self,
        file_id: FileId,
        name: String,
        kind: SymbolKind,
        span: Span,
        item_index: usize,
    ) -> Result<(), Diagnostic> {
        let scope = self.scope_mut(file_id);
        if let Some(existing) = scope.locals.get(&name) {
            return Err(Diagnostic::error(format!("duplicate definition of '{}'", name))
                .with_label(span, "redefined here")
                .with_label(existing.span, "first defined here"));
        }
        scope.locals.insert(
            name.clone(),
            Symbol {
                name,
                kind,
                file_id,
                span,
                item_index,
            },
        );
        Ok(())
    }
}
