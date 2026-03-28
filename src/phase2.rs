//! Phase 2: `@extend` resolution, inflection validation, entry expansion.
//!
//! Takes the [`Phase1Result`](crate::phase1::Phase1Result) and resolves all `@extend` blocks to populate
//! axis values, validates inflection rules against declared axes, expands
//! each entry's paradigm, and checks for cyclic `derived_from` links.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::dag;
use crate::error::{Diagnostic, Diagnostics};
use crate::inflection_eval::{
    enumerate_cells, evaluate_compose, evaluate_rules_with_overrides, CellResult, DelegateResolver,
};
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::symbol_table::SymbolKind;

/// Resolved extend: axis name → list of values.
#[derive(Debug, Default, Clone)]
pub struct ResolvedAxis {
    pub values: Vec<String>,
    pub display: HashMap<String, Vec<(String, String)>>,
    /// slots per value (for structural axes)
    pub slots: HashMap<String, Vec<String>>,
}

/// Result of phase 2.
pub struct Phase2Result {
    /// All resolved axis values.
    pub axes: HashMap<String, ResolvedAxis>,
    /// All expanded entry data ready for SQLite emission.
    pub entries: Vec<ResolvedEntry>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug)]
pub struct ResolvedEntry {
    pub entry_id: String,
    pub headword: String,
    pub headword_scripts: HashMap<String, String>,
    pub tags: Vec<(String, String)>,
    pub meaning: String,
    pub meanings: Vec<(String, String)>,
    pub forms: Vec<ResolvedForm>,
    pub links: Vec<ResolvedLink>,
}

#[derive(Debug)]
pub struct ResolvedForm {
    pub form_str: String,
    pub tags: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct ResolvedLink {
    pub dst_entry_id: String,
    pub link_type: String,
}

/// Run phase 2: resolve extends, validate inflections, expand entries, check DAG.
pub fn run_phase2(p1: &Phase1Result) -> Phase2Result {
    let mut ctx = Phase2Ctx {
        p1,
        axes: HashMap::new(),
        entries: Vec::new(),
        diagnostics: Diagnostics::new(),
    };

    ctx.resolve_extends();
    ctx.validate_inflections();
    ctx.resolve_entries();
    ctx.check_dag();

    Phase2Result {
        axes: ctx.axes,
        entries: ctx.entries,
        diagnostics: ctx.diagnostics,
    }
}

struct Phase2Ctx<'a> {
    p1: &'a Phase1Result,
    axes: HashMap<String, ResolvedAxis>,
    entries: Vec<ResolvedEntry>,
    diagnostics: Diagnostics,
}

impl<'a> Phase2Ctx<'a> {
    // -----------------------------------------------------------------------
    // @extend resolution
    // -----------------------------------------------------------------------

    fn resolve_extends(&mut self) {
        // First collect all tagaxis definitions
        for (_file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::TagAxis(ta) = &item.node {
                    self.axes.entry(ta.name.node.clone()).or_default();
                }
            }
        }

        // Track which extends have been applied and detect conflicts
        let mut value_provenance: HashMap<(String, String), (String, FileId)> = HashMap::new();

        for (file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::Extend(ext) = &item.node {
                    let axis_name = &ext.target_axis.node;
                    if !self.axes.contains_key(axis_name) {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "@extend targets unknown tagaxis '{}'",
                                axis_name
                            ))
                            .with_label(ext.target_axis.span, "unknown axis"),
                        );
                        continue;
                    }

                    for val in &ext.values {
                        let key = (axis_name.clone(), val.name.node.clone());
                        if let Some((prev_extend, _)) = value_provenance.get(&key) {
                            self.diagnostics.add(
                                Diagnostic::error(format!(
                                    "value '{}' for axis '{}' is added by multiple @extends ('{}' and '{}')",
                                    val.name.node, axis_name, prev_extend, ext.name.node
                                ))
                                .with_label(val.name.span, "conflicting addition"),
                            );
                            continue;
                        }
                        value_provenance.insert(key, (ext.name.node.clone(), *file_id));

                        let axis = self.axes.get_mut(axis_name).unwrap();
                        axis.values.push(val.name.node.clone());

                        // Collect display
                        let display_entries: Vec<(String, String)> = val
                            .display
                            .iter()
                            .map(|(k, v)| (k.node.clone(), v.node.clone()))
                            .collect();
                        axis.display
                            .insert(val.name.node.clone(), display_entries);

                        // Collect slots
                        if !val.slots.is_empty() {
                            let slot_names: Vec<String> =
                                val.slots.iter().map(|s| s.node.clone()).collect();
                            axis.slots.insert(val.name.node.clone(), slot_names);
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // inflection validation
    // -----------------------------------------------------------------------

    fn validate_inflections(&mut self) {
        for (_file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::Inflection(infl) = &item.node {
                    self.validate_inflection(infl);
                }
            }
        }
    }

    fn validate_inflection(&mut self, infl: &Inflection) {
        // Check that all axes in `for {}` are defined
        for axis in &infl.axes {
            if !self.axes.contains_key(&axis.node) {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "inflection '{}' references undeclared axis '{}'",
                        infl.name.node, axis.node
                    ))
                    .with_label(axis.span, "unknown axis"),
                );
            }
        }

        // Validate rules reference only declared axes
        let declared_axes: HashSet<_> = infl.axes.iter().map(|a| &a.node).collect();
        self.validate_body_axes(&infl.body, &declared_axes, &infl.name.node);
    }

    fn validate_body_axes(
        &mut self,
        body: &InflectionBody,
        declared: &HashSet<&String>,
        infl_name: &str,
    ) {
        match body {
            InflectionBody::Rules(rules) => {
                for rule in rules {
                    self.validate_rule_axes(rule, declared, infl_name);
                }
            }
            InflectionBody::Compose(comp) => {
                for slot in &comp.slots {
                    for rule in &slot.rules {
                        self.validate_rule_axes(rule, declared, infl_name);
                    }
                }
                for rule in &comp.overrides {
                    self.validate_rule_axes(rule, declared, infl_name);
                }
            }
        }
    }

    fn validate_rule_axes(
        &mut self,
        rule: &InflectionRule,
        declared: &HashSet<&String>,
        infl_name: &str,
    ) {
        for cond in &rule.condition.conditions {
            if !declared.contains(&cond.axis.node) {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "inflection '{}': axis '{}' not in for {{}} declaration",
                        infl_name, cond.axis.node
                    ))
                    .with_label(cond.axis.span, "undeclared axis"),
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // entry resolution
    // -----------------------------------------------------------------------

    fn resolve_entries(&mut self) {
        // Collect entries from all files
        let entries: Vec<(FileId, Entry)> = self
            .p1
            .files
            .iter()
            .flat_map(|(&fid, file)| {
                file.items.iter().filter_map(move |item| {
                    if let Item::Entry(e) = &item.node {
                        Some((fid, e.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect();

        for (file_id, entry) in entries {
            self.resolve_entry(file_id, &entry);
        }
    }

    fn resolve_entry(&mut self, file_id: FileId, entry: &Entry) {
        let headword = match &entry.headword {
            Headword::Simple(s) => s.node.clone(),
            Headword::MultiScript(scripts) => {
                scripts
                    .iter()
                    .find(|(k, _)| k.node == "default")
                    .map(|(_, v)| v.node.clone())
                    .unwrap_or_else(|| {
                        scripts.first().map(|(_, v)| v.node.clone()).unwrap_or_default()
                    })
            }
        };

        let headword_scripts = match &entry.headword {
            Headword::Simple(_) => HashMap::new(),
            Headword::MultiScript(scripts) => scripts
                .iter()
                .map(|(k, v)| (k.node.clone(), v.node.clone()))
                .collect(),
        };

        let tags: Vec<(String, String)> = entry
            .tags
            .iter()
            .map(|tc| (tc.axis.node.clone(), tc.value.node.clone()))
            .collect();

        let (meaning, meanings) = match &entry.meaning {
            MeaningDef::Single(s) => (s.node.clone(), Vec::new()),
            MeaningDef::Multiple(entries) => {
                let first = entries
                    .first()
                    .map(|e| e.text.node.clone())
                    .unwrap_or_default();
                let all: Vec<(String, String)> = entries
                    .iter()
                    .map(|e| (e.ident.node.clone(), e.text.node.clone()))
                    .collect();
                (first, all)
            }
        };

        // Build stems map
        let stems: HashMap<String, String> = entry
            .stems
            .iter()
            .map(|s| (s.name.node.clone(), s.value.node.clone()))
            .collect();

        // Expand forms
        let mut forms = Vec::new();

        if let Some(infl) = &entry.inflection {
            let (axes, body) = match infl {
                EntryInflection::Class(class_name) => {
                    // Find the inflection class
                    if let Some(infl_def) = self.find_inflection(&class_name.node, file_id) {
                        let axes: Vec<String> =
                            infl_def.axes.iter().map(|a| a.node.clone()).collect();
                        (axes, Some(infl_def.body.clone()))
                    } else {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "inflection class '{}' not found",
                                class_name.node
                            ))
                            .with_label(class_name.span, "not found"),
                        );
                        (Vec::new(), None)
                    }
                }
                EntryInflection::Inline(inline) => {
                    let axes: Vec<String> =
                        inline.axes.iter().map(|a| a.node.clone()).collect();
                    (axes, Some(inline.body.clone()))
                }
            };

            if let Some(body) = body {
                let axis_values: HashMap<String, Vec<String>> = axes
                    .iter()
                    .map(|a| {
                        let vals = self
                            .axes
                            .get(a)
                            .map(|ra| ra.values.clone())
                            .unwrap_or_default();
                        (a.clone(), vals)
                    })
                    .collect();

                match enumerate_cells(&axes, &axis_values) {
                    Ok(cells) => {
                        let struct_stems = HashMap::new();
                        let resolver = Phase2Resolver { ctx: self, file_id };
                        let result = match &body {
                            InflectionBody::Rules(rules) => {
                                evaluate_rules_with_overrides(
                                    rules, &entry.forms_override, &cells, &stems, &struct_stems, &resolver,
                                )
                            }
                            InflectionBody::Compose(comp) => {
                                evaluate_compose(comp, &entry.forms_override, &cells, &stems, &struct_stems)
                            }
                        };

                        match result {
                            Ok(paradigm) => {
                                for (cell, cell_result) in paradigm.forms {
                                    if let CellResult::Form(form_str) = cell_result {
                                        let cell_tags: Vec<(String, String)> =
                                            cell.tags.into_iter().collect();
                                        forms.push(ResolvedForm {
                                            form_str,
                                            tags: cell_tags,
                                        });
                                    }
                                }
                            }
                            Err(errors) => {
                                for e in errors {
                                    self.diagnostics.add(e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        self.diagnostics.add(e);
                    }
                }
            }
        }

        // Collect links
        let mut links = Vec::new();
        if let Some(ety) = &entry.etymology {
            if let Some(derived) = &ety.derived_from {
                links.push(ResolvedLink {
                    dst_entry_id: derived.entry_id.node.clone(),
                    link_type: "derived_from".to_string(),
                });
            }
            for cognate in &ety.cognates {
                links.push(ResolvedLink {
                    dst_entry_id: cognate.entry.entry_id.node.clone(),
                    link_type: "cognate".to_string(),
                });
            }
        }
        for example in &entry.examples {
            for token in &example.tokens {
                if let crate::ast::Token::Ref(entry_ref) = token {
                    links.push(ResolvedLink {
                        dst_entry_id: entry_ref.entry_id.node.clone(),
                        link_type: "example".to_string(),
                    });
                }
            }
        }

        self.entries.push(ResolvedEntry {
            entry_id: entry.name.node.clone(),
            headword,
            headword_scripts,
            tags,
            meaning,
            meanings,
            forms,
            links,
        });
    }

    fn find_inflection(&self, name: &str, file_id: FileId) -> Option<&Inflection> {
        // Search in local file first, then imports
        if let Some(scope) = self.p1.symbol_table.scope(file_id) {
            let resolved = scope.resolve(name);
            for sym in resolved {
                if sym.kind == SymbolKind::Inflection {
                    if let Some(file) = self.p1.files.get(&sym.file_id) {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let Item::Inflection(infl) = &item.node {
                                return Some(infl);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: search all files
        for (_, file) in &self.p1.files {
            for item in &file.items {
                if let Item::Inflection(infl) = &item.node {
                    if infl.name.node == name {
                        return Some(infl);
                    }
                }
            }
        }

        None
    }

    // -----------------------------------------------------------------------
    // DAG check (derived_from links)
    // -----------------------------------------------------------------------

    fn check_dag(&mut self) {
        let edges: Vec<(String, String)> = self
            .entries
            .iter()
            .flat_map(|e| {
                e.links
                    .iter()
                    .filter(|l| l.link_type == "derived_from")
                    .map(|l| (e.entry_id.clone(), l.dst_entry_id.clone()))
            })
            .collect();

        if let Err(cycle_nodes) = dag::check_dag(&edges) {
            self.diagnostics.add(Diagnostic::error(format!(
                "cyclic derived_from relationship detected among: {:?}",
                cycle_nodes
            )));
        }
    }
}

/// DelegateResolver implementation that looks up inflections from Phase2Ctx.
struct Phase2Resolver<'a, 'b> {
    ctx: &'a Phase2Ctx<'b>,
    file_id: FileId,
}

impl<'a, 'b> DelegateResolver for Phase2Resolver<'a, 'b> {
    fn resolve(&self, name: &str) -> Option<(Vec<String>, InflectionBody)> {
        let infl = self.ctx.find_inflection(name, self.file_id)?;
        let axes: Vec<String> = infl.axes.iter().map(|a| a.node.clone()).collect();
        Some((axes, infl.body.clone()))
    }

    fn axis_values(&self, axis: &str) -> Vec<String> {
        self.ctx
            .axes
            .get(axis)
            .map(|ra| ra.values.clone())
            .unwrap_or_default()
    }
}
