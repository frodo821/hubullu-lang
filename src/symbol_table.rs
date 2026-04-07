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

        // Check imports and exports
        for imp in self.imports.iter().chain(self.exports.iter()) {
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
        for imp in self.imports.iter().chain(self.exports.iter()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_span() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn span_at(start: usize, end: usize) -> Span {
        Span { file_id: FileId(0), start, end }
    }

    #[test]
    fn register_local_and_resolve() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        st.register_local(file_id, "foo".into(), SymbolKind::Entry, dummy_span(), 0)
            .unwrap();

        let scope = st.scope(file_id).unwrap();
        let results = scope.resolve("foo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "foo");
        assert_eq!(results[0].kind, SymbolKind::Entry);
    }

    #[test]
    fn resolve_nonexistent_returns_empty() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        st.register_local(file_id, "foo".into(), SymbolKind::Entry, dummy_span(), 0)
            .unwrap();

        let scope = st.scope(file_id).unwrap();
        assert!(scope.resolve("bar").is_empty());
    }

    #[test]
    fn duplicate_registration_returns_error() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        st.register_local(file_id, "foo".into(), SymbolKind::Entry, span_at(0, 3), 0)
            .unwrap();

        let err = st
            .register_local(file_id, "foo".into(), SymbolKind::TagAxis, span_at(10, 13), 1)
            .unwrap_err();
        assert!(err.message.contains("duplicate definition of 'foo'"));
    }

    #[test]
    fn resolve_includes_imports() {
        let file_id = FileId(0);
        let source_file = FileId(1);
        let mut st = SymbolTable::new();
        let scope = st.scope_mut(file_id);
        scope.imports.push(ImportedSymbol {
            original_name: "bar".into(),
            local_name: "bar".into(),
            namespace: None,
            kind: SymbolKind::Inflection,
            source_file,
            span: dummy_span(),
            item_index: 5,
        });

        let results = scope.resolve("bar");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Inflection);
        assert_eq!(results[0].file_id, source_file);
        assert_eq!(results[0].item_index, 5);
    }

    #[test]
    fn resolve_returns_both_local_and_import() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        st.register_local(file_id, "x".into(), SymbolKind::Entry, dummy_span(), 0)
            .unwrap();
        let scope = st.scope_mut(file_id);
        scope.imports.push(ImportedSymbol {
            original_name: "x".into(),
            local_name: "x".into(),
            namespace: None,
            kind: SymbolKind::TagAxis,
            source_file: FileId(1),
            span: dummy_span(),
            item_index: 1,
        });

        let results = scope.resolve("x");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn resolve_qualified() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        let scope = st.scope_mut(file_id);
        scope.imports.push(ImportedSymbol {
            original_name: "vowel".into(),
            local_name: "vowel".into(),
            namespace: Some("ipa".into()),
            kind: SymbolKind::PhonRule,
            source_file: FileId(1),
            span: dummy_span(),
            item_index: 0,
        });
        scope.imports.push(ImportedSymbol {
            original_name: "consonant".into(),
            local_name: "consonant".into(),
            namespace: Some("ipa".into()),
            kind: SymbolKind::PhonRule,
            source_file: FileId(1),
            span: dummy_span(),
            item_index: 1,
        });

        // Qualified lookup
        let results = scope.resolve_qualified("ipa", "vowel");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "vowel");

        // Unqualified lookup also finds namespaced imports by local_name
        let results = scope.resolve("vowel");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn resolve_qualified_wrong_namespace() {
        let file_id = FileId(0);
        let mut st = SymbolTable::new();
        let scope = st.scope_mut(file_id);
        scope.imports.push(ImportedSymbol {
            original_name: "foo".into(),
            local_name: "foo".into(),
            namespace: Some("ns1".into()),
            kind: SymbolKind::Entry,
            source_file: FileId(1),
            span: dummy_span(),
            item_index: 0,
        });

        assert!(scope.resolve_qualified("ns2", "foo").is_empty());
    }

    #[test]
    fn scope_for_missing_file_returns_none() {
        let st = SymbolTable::new();
        assert!(st.scope(FileId(99)).is_none());
    }

    #[test]
    fn scope_mut_creates_on_demand() {
        let mut st = SymbolTable::new();
        let scope = st.scope_mut(FileId(5));
        assert!(scope.locals.is_empty());
        assert!(st.scope(FileId(5)).is_some());
    }

    #[test]
    fn resolve_includes_exports() {
        let mut scope = Scope::new();
        scope.exports.push(ImportedSymbol {
            original_name: "exported_entry".into(),
            local_name: "exported_entry".into(),
            namespace: None,
            kind: SymbolKind::Entry,
            source_file: FileId(2),
            span: dummy_span(),
            item_index: 3,
        });

        let results = scope.resolve("exported_entry");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_id, FileId(2));
    }
}
