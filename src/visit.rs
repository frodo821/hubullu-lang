//! AST visitor trait for walking LexDSL syntax trees.
//!
//! Provides a `Visitor` trait with default (no-op) methods for each node type,
//! plus `walk_*` functions that recursively descend. Override only the methods
//! you care about.
//!
//! # Example
//!
//! ```ignore
//! use hubullu::visit::{Visitor, walk_file};
//! use hubullu::ast::*;
//!
//! struct EntryCollector { names: Vec<String> }
//!
//! impl Visitor for EntryCollector {
//!     fn visit_entry(&mut self, entry: &Entry) {
//!         self.names.push(entry.name.node.clone());
//!         // call walk_entry(self, entry) to descend into children
//!     }
//! }
//! ```

use crate::ast::*;

/// Visitor trait for walking AST nodes. All methods have default no-op
/// implementations that call the corresponding `walk_*` function to
/// continue descending.
pub trait Visitor: Sized {
    fn visit_file(&mut self, file: &File) {
        walk_file(self, file);
    }

    fn visit_item(&mut self, item: &Spanned<Item>) {
        walk_item(self, item);
    }

    fn visit_import(&mut self, _import: &Import) {}

    fn visit_tagaxis(&mut self, _tagaxis: &TagAxis) {}

    fn visit_extend(&mut self, extend: &Extend) {
        walk_extend(self, extend);
    }

    fn visit_extend_value(&mut self, _value: &ExtendValue) {}

    fn visit_inflection(&mut self, inflection: &Inflection) {
        walk_inflection(self, inflection);
    }

    fn visit_inflection_rule(&mut self, _rule: &InflectionRule) {}

    fn visit_entry(&mut self, entry: &Entry) {
        walk_entry(self, entry);
    }

    fn visit_stem_def(&mut self, _stem: &StemDef) {}

    fn visit_meaning_entry(&mut self, _meaning: &MeaningEntry) {}

    fn visit_etymology(&mut self, etymology: &Etymology) {
        walk_etymology(self, etymology);
    }

    fn visit_entry_ref(&mut self, _entry_ref: &EntryRef) {}

    fn visit_example(&mut self, example: &Example) {
        walk_example(self, example);
    }

    fn visit_template(&mut self, _template: &Template) {}

    fn visit_ident(&mut self, _ident: &Ident) {}
}

// ---------------------------------------------------------------------------
// Walk functions
// ---------------------------------------------------------------------------

pub fn walk_file<V: Visitor>(visitor: &mut V, file: &File) {
    for item in &file.items {
        visitor.visit_item(item);
    }
}

pub fn walk_item<V: Visitor>(visitor: &mut V, item: &Spanned<Item>) {
    match &item.node {
        Item::Use(import) | Item::Reference(import) => {
            visitor.visit_import(import);
        }
        Item::TagAxis(ta) => {
            visitor.visit_tagaxis(ta);
        }
        Item::Extend(ext) => {
            visitor.visit_extend(ext);
        }
        Item::Inflection(infl) => {
            visitor.visit_inflection(infl);
        }
        Item::Entry(entry) => {
            visitor.visit_entry(entry);
        }
        Item::PhonRule(_) | Item::Render(_) => {
            // PhonRule/Render have no visitor methods yet; skip.
        }
    }
}

pub fn walk_extend<V: Visitor>(visitor: &mut V, extend: &Extend) {
    visitor.visit_ident(&extend.name);
    visitor.visit_ident(&extend.target_axis);
    for val in &extend.values {
        visitor.visit_extend_value(val);
    }
}

pub fn walk_inflection<V: Visitor>(visitor: &mut V, inflection: &Inflection) {
    visitor.visit_ident(&inflection.name);
    for axis in &inflection.axes {
        visitor.visit_ident(axis);
    }
    walk_inflection_body(visitor, &inflection.body);
}

pub fn walk_inflection_body<V: Visitor>(visitor: &mut V, body: &InflectionBody) {
    match body {
        InflectionBody::Rules(rules) => {
            for rule in rules {
                visitor.visit_inflection_rule(rule);
            }
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                for rule in &slot.rules {
                    visitor.visit_inflection_rule(rule);
                }
            }
            for rule in &comp.overrides {
                visitor.visit_inflection_rule(rule);
            }
        }
    }
}

pub fn walk_entry<V: Visitor>(visitor: &mut V, entry: &Entry) {
    visitor.visit_ident(&entry.name);

    for stem in &entry.stems {
        visitor.visit_stem_def(stem);
    }

    if let MeaningDef::Multiple(meanings) = &entry.meaning {
        for m in meanings {
            visitor.visit_meaning_entry(m);
        }
    }

    if let Some(infl) = &entry.inflection {
        match infl {
            EntryInflection::Class(name) => {
                visitor.visit_ident(name);
            }
            EntryInflection::Inline(inline) => {
                for axis in &inline.axes {
                    visitor.visit_ident(axis);
                }
                walk_inflection_body(visitor, &inline.body);
            }
        }
    }

    for rule in &entry.forms_override {
        visitor.visit_inflection_rule(rule);
    }

    if let Some(ety) = &entry.etymology {
        visitor.visit_etymology(ety);
    }

    for example in &entry.examples {
        visitor.visit_example(example);
    }
}

pub fn walk_etymology<V: Visitor>(visitor: &mut V, ety: &Etymology) {
    if let Some(derived) = &ety.derived_from {
        visitor.visit_entry_ref(derived);
    }
    for cognate in &ety.cognates {
        visitor.visit_entry_ref(&cognate.entry);
    }
}

pub fn walk_example<V: Visitor>(visitor: &mut V, example: &Example) {
    for token in &example.tokens {
        if let Token::Ref(entry_ref) = token {
            visitor.visit_entry_ref(entry_ref);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: collect all entry names from a parsed file.
    struct EntryCollector {
        names: Vec<String>,
    }

    impl Visitor for EntryCollector {
        fn visit_entry(&mut self, entry: &Entry) {
            self.names.push(entry.name.node.clone());
            walk_entry(self, entry);
        }
    }

    #[test]
    fn test_visitor_collects_entries() {
        let result = crate::parse_source(
            r#"
            entry foo {
                headword: "foo"
                stems {}
                meaning: "a foo"
            }
            entry bar {
                headword: "bar"
                stems {}
                meaning: "a bar"
            }
            "#,
            "test.hu",
        );
        assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);

        let mut collector = EntryCollector { names: Vec::new() };
        collector.visit_file(&result.file);
        assert_eq!(collector.names, vec!["foo", "bar"]);
    }

    /// Test: collect all identifiers
    struct IdentCollector {
        idents: Vec<String>,
    }

    impl Visitor for IdentCollector {
        fn visit_ident(&mut self, ident: &Ident) {
            self.idents.push(ident.node.clone());
        }
    }

    #[test]
    fn test_visitor_collects_idents() {
        let result = crate::parse_source(
            r#"
            entry foo {
                headword: "foo"
                stems { root: "f" }
                meaning: "a foo"
            }
            "#,
            "test.hu",
        );
        assert!(!result.has_errors(), "parse errors: {:?}", result.diagnostics);

        let mut collector = IdentCollector { idents: Vec::new() };
        collector.visit_file(&result.file);
        // Should contain at least "foo" (entry name)
        assert!(collector.idents.contains(&"foo".to_string()));
    }
}
