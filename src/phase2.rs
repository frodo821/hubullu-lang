//! Phase 2: `@extend` resolution, inflection validation, entry expansion.
//!
//! Takes the [`Phase1Result`](crate::phase1::Phase1Result) and resolves all `@extend` blocks to populate
//! axis values, validates inflection rules against declared axes, expands
//! each entry's paradigm, and checks for cyclic `derived_from` links.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::ast::*;
use crate::dag;
use crate::error::{Diagnostic, Diagnostics};
use crate::inflection_eval::{
    enumerate_cells, evaluate_compose, evaluate_rules_with_overrides, CellResult, DelegateResolver,
    PhonRuleResolver,
};
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::symbol_table::SymbolKind;

/// Resolved extend: axis name → list of values.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone)]
pub struct ResolvedAxis {
    pub values: Vec<String>,
    pub display: HashMap<String, Vec<(String, String)>>,
    /// slots per value (for structural axes)
    pub slots: HashMap<String, Vec<String>>,
}

/// Resolved render configuration.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedRenderConfig {
    pub separator: String,
    pub no_separator_before: String,
}

impl Default for ResolvedRenderConfig {
    fn default() -> Self {
        Self {
            separator: " ".to_string(),
            no_separator_before: ".,;:!?".to_string(),
        }
    }
}

/// Result of phase 2.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone)]
pub struct Phase2Result {
    /// All resolved axis values.
    pub axes: HashMap<String, ResolvedAxis>,
    /// All resolved inflection class metadata.
    pub inflections: Vec<ResolvedInflection>,
    /// All expanded entry data ready for SQLite emission.
    pub entries: Vec<ResolvedEntry>,
    /// Render configuration from `@render` directive.
    pub render_config: ResolvedRenderConfig,
    pub diagnostics: Diagnostics,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub name: String,
    pub source_file: PathBuf,
    pub headword: String,
    pub headword_scripts: HashMap<String, String>,
    pub tags: Vec<(String, String)>,
    pub inflection_class: Option<String>,
    pub meaning: String,
    pub meanings: Vec<(String, String)>,
    #[cfg_attr(feature = "serialization", serde(default))]
    pub stems: HashMap<String, String>,
    pub forms: Vec<ResolvedForm>,
    pub links: Vec<ResolvedLink>,
    pub etymology_proto: Option<String>,
    pub etymology_note: Option<String>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedForm {
    pub form_str: String,
    pub tags: Vec<(String, String)>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    pub dst_entry_id: String,
    pub link_type: String,
}

/// Resolved inflection class metadata for emission.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedInflection {
    pub name: String,
    pub display: Vec<(String, String)>,
    pub axes: Vec<String>,
}

/// Run phase 2: resolve extends, validate inflections, expand entries, check DAG.
pub fn run_phase2(p1: &Phase1Result) -> Phase2Result {
    let mut ctx = Phase2Ctx {
        p1,
        axes: HashMap::new(),
        inflections: Vec::new(),
        entries: Vec::new(),
        diagnostics: Diagnostics::new(),
        deferred_infl_errors: Vec::new(),
    };

    ctx.resolve_extends();
    ctx.validate_phonrules();
    ctx.validate_inflections();
    ctx.collect_inflections();
    ctx.resolve_entries();
    ctx.flush_deferred_infl_errors();
    ctx.check_dag();

    let render_config = ctx.collect_render_config();

    Phase2Result {
        axes: ctx.axes,
        inflections: ctx.inflections,
        entries: ctx.entries,
        render_config,
        diagnostics: ctx.diagnostics,
    }
}

/// Run phase 2 with incremental entry resolution.
///
/// `files_to_resolve` specifies which files need fresh entry expansion.
/// `cached_entries` provides pre-resolved entries from unchanged files.
/// Schema validation (extends, inflections, phonrules) always runs fully.
pub fn run_phase2_incremental(
    p1: &Phase1Result,
    files_to_resolve: &HashSet<FileId>,
    cached_entries: Vec<ResolvedEntry>,
) -> Phase2Result {
    let mut ctx = Phase2Ctx {
        p1,
        axes: HashMap::new(),
        inflections: Vec::new(),
        entries: Vec::new(),
        diagnostics: Diagnostics::new(),
        deferred_infl_errors: Vec::new(),
    };

    ctx.resolve_extends();
    ctx.validate_phonrules();
    ctx.validate_inflections();
    ctx.collect_inflections();
    ctx.resolve_entries_selective(files_to_resolve, cached_entries);
    ctx.flush_deferred_infl_errors();
    ctx.check_dag();

    let render_config = ctx.collect_render_config();

    Phase2Result {
        axes: ctx.axes,
        inflections: ctx.inflections,
        entries: ctx.entries,
        render_config,
        diagnostics: ctx.diagnostics,
    }
}

struct Phase2Ctx<'a> {
    p1: &'a Phase1Result,
    axes: HashMap<String, ResolvedAxis>,
    inflections: Vec<ResolvedInflection>,
    entries: Vec<ResolvedEntry>,
    diagnostics: Diagnostics,
    /// Inflection errors deferred for grouping by (message, infl_span).
    /// Each element: (base diagnostic, inflection def span, entry name ident).
    deferred_infl_errors: Vec<(Diagnostic, Option<Span>, Ident)>,
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
            InflectionBody::Rules(body) => {
                for rule in &body.rules {
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
    // inflection collection
    // -----------------------------------------------------------------------

    fn collect_inflections(&mut self) {
        for (_file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::Inflection(infl) = &item.node {
                    self.inflections.push(ResolvedInflection {
                        name: infl.name.node.clone(),
                        display: infl
                            .display
                            .iter()
                            .map(|(k, v)| (k.node.clone(), v.node.clone()))
                            .collect(),
                        axes: infl.axes.iter().map(|a| a.node.clone()).collect(),
                    });
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // phonrule validation
    // -----------------------------------------------------------------------

    fn validate_phonrules(&mut self) {
        for (_file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::PhonRule(pr) = &item.node {
                    self.validate_phonrule(pr);
                }
            }
        }
    }

    fn validate_phonrule(&mut self, pr: &PhonRule) {
        let class_names: HashSet<_> = pr.classes.iter().map(|c| &c.name.node).collect();

        // Validate union references
        for cls in &pr.classes {
            if let CharClassBody::Union(members) = &cls.body {
                for member in members {
                    if !class_names.contains(&member.node) {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "phonrule '{}': class union references undefined class '{}'",
                                pr.name.node, member.node
                            ))
                            .with_label(member.span, "undefined class"),
                        );
                    }
                }
            }
        }

        let map_names: HashSet<_> = pr.maps.iter().map(|m| &m.name.node).collect();

        // Validate rewrite rules
        for rule in &pr.rules {
            // FROM references
            if let PhonPattern::Class(name) = &rule.from {
                if !class_names.contains(&name.node) {
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': rewrite rule references undefined class '{}'",
                            pr.name.node, name.node
                        ))
                        .with_label(name.span, "undefined class"),
                    );
                }
            }

            // TO references
            if let PhonReplacement::Map(name) = &rule.to {
                if !map_names.contains(&name.node) {
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': rewrite rule references undefined map '{}'",
                            pr.name.node, name.node
                        ))
                        .with_label(name.span, "undefined map"),
                    );
                }
            }

            // Context references
            if let Some(ctx) = &rule.context {
                for elem in ctx.left.iter().chain(ctx.right.iter()) {
                    self.validate_context_elem(pr, elem, &class_names);
                }
            }
        }
    }

    fn validate_context_elem(
        &mut self,
        pr: &PhonRule,
        elem: &PhonContextElem,
        class_names: &HashSet<&String>,
    ) {
        match elem {
            PhonContextElem::Class(name) | PhonContextElem::NegClass(name) => {
                if !class_names.contains(&name.node) {
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': context references undefined class '{}'",
                            pr.name.node, name.node
                        ))
                        .with_label(name.span, "undefined class"),
                    );
                }
            }
            PhonContextElem::Repeat(inner) => {
                self.validate_context_elem(pr, inner, class_names);
            }
            PhonContextElem::Alt(alts) => {
                for alt in alts {
                    self.validate_context_elem(pr, alt, class_names);
                }
            }
            PhonContextElem::Boundary | PhonContextElem::WordStart | PhonContextElem::WordEnd | PhonContextElem::Literal(_) => {}
        }
    }

    fn find_phonrule(&self, name: &str, file_id: FileId) -> Option<&PhonRule> {
        if let Some(scope) = self.p1.symbol_table.scope(file_id) {
            let resolved = scope.resolve(name);
            for sym in resolved {
                if sym.kind == SymbolKind::PhonRule {
                    if let Some(file) = self.p1.files.get(&sym.file_id) {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let Item::PhonRule(pr) = &item.node {
                                return Some(pr);
                            }
                        }
                    }
                }
            }
        }

        None
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

    /// Resolve entries only from the specified files; use cached entries for the rest.
    fn resolve_entries_selective(
        &mut self,
        files_to_resolve: &HashSet<FileId>,
        cached: Vec<ResolvedEntry>,
    ) {
        self.entries = cached;

        let entries: Vec<(FileId, Entry)> = self
            .p1
            .files
            .iter()
            .filter(|(&fid, _)| files_to_resolve.contains(&fid))
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
            let (axes, body, stem_reqs, infl_span) = match infl {
                EntryInflection::Class(class_name) => {
                    // Find the inflection class
                    if let Some(infl_def) = self.find_inflection(&class_name.node, file_id) {
                        let axes: Vec<String> =
                            infl_def.axes.iter().map(|a| a.node.clone()).collect();
                        (axes, Some(infl_def.body.clone()), infl_def.required_stems.clone(), Some(infl_def.name.span))
                    } else {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "inflection class '{}' not found",
                                class_name.node
                            ))
                            .with_label(class_name.span, "not found"),
                        );
                        (Vec::new(), None, Vec::new(), None)
                    }
                }
                EntryInflection::Inline(inline) => {
                    let axes: Vec<String> =
                        inline.axes.iter().map(|a| a.node.clone()).collect();
                    (axes, Some(inline.body.clone()), Vec::new(), None)
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
                        // Build struct_stems from required_stems constraints + axis slots
                        let mut struct_stems: HashMap<String, HashMap<String, String>> = HashMap::new();
                        for req in &stem_reqs {
                            if req.constraint.is_empty() { continue; }
                            let stem_val = match stems.get(&req.name.node) {
                                Some(v) => v,
                                None => continue,
                            };
                            for cond in &req.constraint {
                                if let Some(axis) = self.axes.get(&cond.axis.node) {
                                    if let Some(slot_names) = axis.slots.get(&cond.value.node) {
                                        if slot_names.is_empty() { continue; }
                                        let chars: Vec<String> = stem_val.chars().map(|c| c.to_string()).collect();
                                        if chars.len() != slot_names.len() {
                                            self.diagnostics.add(
                                                Diagnostic::error(format!(
                                                    "stem '{}' has {} characters but axis value '{}' expects {} slots",
                                                    req.name.node, chars.len(), cond.value.node, slot_names.len()
                                                ))
                                                .with_label(req.name.span, "stem length mismatch"),
                                            );
                                            continue;
                                        }
                                        let slot_map: HashMap<String, String> = slot_names.iter()
                                            .zip(chars.iter())
                                            .map(|(name, ch)| (name.clone(), ch.clone()))
                                            .collect();
                                        struct_stems.insert(req.name.node.clone(), slot_map);
                                    }
                                }
                            }
                        }
                        let resolver = Phase2Resolver { ctx: self, file_id };
                        let phon_resolver = Phase2PhonResolver { ctx: self, file_id };
                        let result = match &body {
                            InflectionBody::Rules(body) => {
                                evaluate_rules_with_overrides(
                                    &body.rules, &entry.forms_override, body.apply.as_ref(), &cells, &stems, &struct_stems, &resolver, &phon_resolver,
                                )
                            }
                            InflectionBody::Compose(comp) => {
                                evaluate_compose(comp, &entry.forms_override, &cells, &stems, &struct_stems, &phon_resolver)
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
                                    if e.labels.is_empty() {
                                        // Defer label-less errors for grouping across entries.
                                        self.deferred_infl_errors.push((e, infl_span, entry.name.clone()));
                                    } else {
                                        let mut e = e;
                                        e.message = format!(
                                            "entry '{}': {}", entry.name.node, e.message,
                                        );
                                        self.diagnostics.add(e);
                                    }
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

        // Track inflection class name
        let inflection_class = match &entry.inflection {
            Some(EntryInflection::Class(class_name)) => Some(class_name.node.clone()),
            _ => None,
        };

        // Collect etymology text and links
        let etymology_proto = entry.etymology.as_ref().and_then(|e| e.proto.as_ref().map(|s| s.node.clone()));
        let etymology_note = entry.etymology.as_ref().and_then(|e| e.note.as_ref().map(|s| s.node.clone()));

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
            name: entry.name.node.clone(),
            source_file: self.p1.source_map.path(file_id).to_path_buf(),
            headword,
            headword_scripts,
            tags,
            inflection_class,
            meaning,
            meanings,
            stems,
            forms,
            links,
            etymology_proto,
            etymology_note,
        });
    }

    /// Emit deferred inflection errors, grouping identical errors across entries.
    ///
    /// Each unique error message (with its inflection span) is emitted once,
    /// with up to 10 "required by this entry" labels. If more than 10 entries
    /// triggered the same error, the remainder is summarised as "and N more".
    fn flush_deferred_infl_errors(&mut self) {
        // Group by (message, infl_span) → Vec<entry Ident>
        let mut groups: Vec<(String, Option<Span>, Vec<Ident>)> = Vec::new();
        for (diag, ispan, entry_name) in std::mem::take(&mut self.deferred_infl_errors) {
            if let Some(group) = groups.iter_mut().find(|(m, s, _)| *m == diag.message && *s == ispan) {
                if !group.2.iter().any(|e| e.span == entry_name.span) {
                    group.2.push(entry_name);
                }
            } else {
                groups.push((diag.message, ispan, vec![entry_name]));
            }
        }

        const MAX_ENTRIES: usize = 10;
        for (message, ispan, entries) in groups {
            let mut diag = Diagnostic::error(&message);
            if let Some(ispan) = ispan {
                diag = diag.with_label(ispan, "in this inflection class");
            }
            for entry_name in entries.iter().take(MAX_ENTRIES) {
                diag = diag.with_label(entry_name.span, format!("required by '{}'", entry_name.node));
            }
            if entries.len() > MAX_ENTRIES {
                diag.message = format!("{} (and {} more entries)", diag.message, entries.len() - MAX_ENTRIES);
            }
            self.diagnostics.add(diag);
        }
    }

    fn find_inflection(&self, name: &str, file_id: FileId) -> Option<&Inflection> {
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

        None
    }

    // -----------------------------------------------------------------------
    // @render config collection
    // -----------------------------------------------------------------------

    fn collect_render_config(&self) -> ResolvedRenderConfig {
        let mut config = ResolvedRenderConfig::default();
        for (_file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::Render(rc) = &item.node {
                    if let Some(sep) = &rc.separator {
                        config.separator = sep.node.clone();
                    }
                    if let Some(nsb) = &rc.no_separator_before {
                        config.no_separator_before = nsb.node.clone();
                    }
                }
            }
        }
        config
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
                    .map(|l| (e.name.clone(), l.dst_entry_id.clone()))
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

/// PhonRuleResolver implementation for Phase2.
struct Phase2PhonResolver<'a, 'b> {
    ctx: &'a Phase2Ctx<'b>,
    file_id: FileId,
}

impl<'a, 'b> PhonRuleResolver for Phase2PhonResolver<'a, 'b> {
    fn resolve(&self, name: &str) -> Option<&PhonRule> {
        self.ctx.find_phonrule(name, self.file_id)
    }
}
