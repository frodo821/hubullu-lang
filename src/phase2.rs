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
    collect_referenced_axes, enumerate_cells, evaluate_compose, evaluate_rules_with_overrides,
    CellResult, DelegateResolver, PhonRuleResolver,
};
use crate::phase1::Phase1Result;
use crate::phoneme::{resolve_inventory, PhonemeInventory};
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
    /// All resolved phonrule metadata (display, derived_from).
    /// Empty default for backward compat with old caches.
    #[cfg_attr(feature = "serialization", serde(default))]
    pub phonrules: Vec<ResolvedPhonRule>,
    /// Resolved phoneme inventory (name → terminal set + multigraph alphabet).
    /// Empty default for backward compat with old caches.
    #[cfg_attr(feature = "serialization", serde(default, skip))]
    pub phonemes: PhonemeInventory,
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

/// Resolved phonrule metadata for emission (display + derived_from).
/// Informational only; the compiler does not use these fields for evaluation.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct ResolvedPhonRule {
    pub name: String,
    pub display: Vec<(String, String)>,
    pub derived_from: Option<String>,
    /// F2c: name of the `syllable NAME` declaration this phonrule references,
    /// if any. Mirrors `PhonRule.syllable` flattened to a string for storage.
    pub syllable_ref: Option<String>,
}

/// Run phase 2: resolve extends, validate inflections, expand entries, check DAG.
pub fn run_phase2(p1: &Phase1Result) -> Phase2Result {
    let mut ctx = Phase2Ctx {
        p1,
        axes: HashMap::new(),
        inflections: Vec::new(),
        phonemes: PhonemeInventory::default(),
        entries: Vec::new(),
        diagnostics: Diagnostics::new(),
        deferred_infl_errors: Vec::new(),
    };

    log::debug!("phase2: resolving phonemes");
    ctx.resolve_phonemes();
    log::debug!("phase2: resolving extends");
    ctx.resolve_extends();
    log::debug!("phase2: validating phonrules");
    ctx.validate_phonrules();
    log::debug!("phase2: validating syllables");
    ctx.validate_syllables();
    log::debug!("phase2: validating inflections");
    ctx.validate_inflections();
    ctx.collect_inflections();
    log::debug!("phase2: resolving entries");
    ctx.resolve_entries();
    ctx.flush_deferred_infl_errors();
    log::debug!("phase2: checking DAG");
    ctx.check_dag();

    let render_config = ctx.collect_render_config();

    Phase2Result {
        axes: ctx.axes,
        inflections: ctx.inflections,
        phonrules: collect_phonrules(p1),
        phonemes: ctx.phonemes,
        entries: ctx.entries,
        render_config,
        diagnostics: ctx.diagnostics,
    }
}

/// Run phase 2 with incremental entry resolution.
///
/// `entries_to_resolve` specifies which `(source_path, entry_name)` pairs need
/// fresh expansion.  `cached_entries` provides pre-resolved entries whose
/// Merkle hashes have not changed.
/// Schema validation (extends, inflections, phonrules) always runs fully.
pub fn run_phase2_incremental(
    p1: &Phase1Result,
    entries_to_resolve: &HashSet<(std::path::PathBuf, String)>,
    cached_entries: Vec<ResolvedEntry>,
) -> Phase2Result {
    let mut ctx = Phase2Ctx {
        p1,
        axes: HashMap::new(),
        inflections: Vec::new(),
        phonemes: PhonemeInventory::default(),
        entries: Vec::new(),
        diagnostics: Diagnostics::new(),
        deferred_infl_errors: Vec::new(),
    };

    ctx.resolve_phonemes();
    ctx.resolve_extends();
    ctx.validate_phonrules();
    ctx.validate_syllables();
    ctx.validate_inflections();
    ctx.collect_inflections();
    ctx.resolve_entries_by_merkle(entries_to_resolve, cached_entries);
    ctx.flush_deferred_infl_errors();
    ctx.check_dag();

    let render_config = ctx.collect_render_config();

    Phase2Result {
        axes: ctx.axes,
        inflections: ctx.inflections,
        phonrules: collect_phonrules(p1),
        phonemes: ctx.phonemes,
        entries: ctx.entries,
        render_config,
        diagnostics: ctx.diagnostics,
    }
}

struct Phase2Ctx<'a> {
    p1: &'a Phase1Result,
    axes: HashMap<String, ResolvedAxis>,
    inflections: Vec<ResolvedInflection>,
    phonemes: PhonemeInventory,
    entries: Vec<ResolvedEntry>,
    diagnostics: Diagnostics,
    /// Inflection errors deferred for grouping by (message, infl_span).
    /// Each element: (base diagnostic, inflection def span, entry name ident).
    deferred_infl_errors: Vec<(Diagnostic, Option<Span>, Ident)>,
}

/// Collect phonrule metadata (display, derived_from) from all source files.
/// Pure helper used by both `run_phase2` and `run_phase2_incremental`.
fn collect_phonrules(p1: &Phase1Result) -> Vec<ResolvedPhonRule> {
    let mut out = Vec::new();
    for file in p1.files.values() {
        for item in &file.items {
            if let Item::PhonRule(pr) = &item.node {
                let display = pr
                    .display
                    .iter()
                    .map(|(k, v)| (k.node.clone(), v.node.clone()))
                    .collect();
                out.push(ResolvedPhonRule {
                    name: pr.name.node.clone(),
                    display,
                    derived_from: pr.derived_from.as_ref().map(|i| i.node.clone()),
                    syllable_ref: pr.syllable.as_ref().map(|i| i.node.clone()),
                });
            }
        }
    }
    out
}

/// Free function: resolve a phonrule by name in the scope of `file_id`.
/// Returns the phonrule and the file_id where it is defined. Borrows directly
/// from `p1` so that callers can keep the borrow alive across `&mut self`
/// access on a Phase2Ctx (the borrow comes from p1, not from self).
fn find_phonrule_in<'a>(
    p1: &'a Phase1Result,
    name: &str,
    file_id: FileId,
) -> Option<&'a PhonRule> {
    let scope = p1.symbol_table.scope(file_id)?;
    for sym in scope.resolve(name) {
        if sym.kind == SymbolKind::PhonRule {
            if let Some(file) = p1.files.get(&sym.file_id) {
                if let Some(item) = file.items.get(sym.item_index) {
                    if let Item::PhonRule(pr) = &item.node {
                        return Some(pr);
                    }
                }
            }
        }
    }
    None
}

/// Free function (F2c): resolve a syllable declaration by name in the scope of
/// `file_id`. Mirrors [`find_phonrule_in`] for the Syllable symbol kind.
pub(crate) fn find_syllable_in<'a>(
    p1: &'a Phase1Result,
    name: &str,
    file_id: FileId,
) -> Option<&'a Syllable> {
    let scope = p1.symbol_table.scope(file_id)?;
    for sym in scope.resolve(name) {
        if sym.kind == SymbolKind::Syllable {
            if let Some(file) = p1.files.get(&sym.file_id) {
                if let Some(item) = file.items.get(sym.item_index) {
                    if let Item::Syllable(syl) = &item.node {
                        return Some(syl);
                    }
                }
            }
        }
    }
    None
}

impl<'a> Phase2Ctx<'a> {
    // -----------------------------------------------------------------------
    // phoneme resolution
    // -----------------------------------------------------------------------

    /// Resolve all `phoneme` declarations across all loaded files into a single
    /// global inventory. Reference cycles and undefined references emit
    /// diagnostics. Duplicate declarations are caught earlier by the
    /// symbol table — a later declaration with the same name silently
    /// overwrites the earlier one in the resolver pool here.
    fn resolve_phonemes(&mut self) {
        let mut all: Vec<&'a Phoneme> = Vec::new();
        for file in self.p1.files.values() {
            for item in &file.items {
                if let Item::Phoneme(ph) = &item.node {
                    all.push(ph);
                }
            }
        }
        match resolve_inventory(&all) {
            Ok(inv) => self.phonemes = inv,
            Err(diags) => {
                for d in diags {
                    self.diagnostics.add(d);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // syllable validation
    // -----------------------------------------------------------------------

    /// Validate every `syllable` declaration: `nucleus` and every template
    /// slot's class must resolve to a declared phoneme, and `onset_max` /
    /// `coda_max` must not exceed the number of optional pre-/post-nucleus
    /// slots in the template.
    fn validate_syllables(&mut self) {
        // The full @use-scoped phoneme name set per file is overkill for this
        // pass; the inventory already aggregates every declared phoneme by
        // name (we resolved it before this fn runs), so we can ask it
        // directly via `has_phoneme`.
        for file in self.p1.files.values() {
            for item in &file.items {
                if let Item::Syllable(syl) = &item.node {
                    self.validate_syllable(syl);
                }
            }
        }
    }

    fn validate_syllable(&mut self, syl: &Syllable) {
        // 1. nucleus must resolve to a declared phoneme.
        if !self.phonemes.has_phoneme(&syl.nucleus.node) {
            self.diagnostics.add(
                Diagnostic::error(format!(
                    "syllable '{}' references unknown phoneme '{}' as nucleus",
                    syl.name.node, syl.nucleus.node
                ))
                .with_label(syl.nucleus.span, "unknown phoneme"),
            );
        }

        // 2. each template slot's class must resolve.
        for slot in &syl.template.slots {
            if !self.phonemes.has_phoneme(&slot.class.node) {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "syllable '{}' template references unknown phoneme '{}'",
                        syl.name.node, slot.class.node
                    ))
                    .with_label(slot.class.span, "unknown phoneme"),
                );
            }
        }

        // 3. template / onset_max / coda_max consistency. Count optional
        //    non-nucleus slots before vs after the first nucleus slot.
        let mut pre = 0u32;
        let mut post = 0u32;
        let mut seen_nucleus = false;
        for slot in &syl.template.slots {
            if slot.class.node == syl.nucleus.node {
                seen_nucleus = true;
                continue;
            }
            if seen_nucleus {
                post += 1;
            } else {
                pre += 1;
            }
        }
        if !seen_nucleus {
            self.diagnostics.add(
                Diagnostic::error(format!(
                    "syllable '{}' template does not contain the nucleus phoneme '{}'",
                    syl.name.node, syl.nucleus.node
                ))
                .with_label(syl.template.span, "template missing nucleus"),
            );
        }
        if let Some(om) = syl.onset_max {
            if om > pre {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "syllable '{}': onset_max={} exceeds template's {} pre-nucleus slot(s)",
                        syl.name.node, om, pre
                    ))
                    .with_label(syl.template.span, "template too small"),
                );
            }
        }
        if let Some(cm) = syl.coda_max {
            if cm > post {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "syllable '{}': coda_max={} exceeds template's {} post-nucleus slot(s)",
                        syl.name.node, cm, post
                    ))
                    .with_label(syl.template.span, "template too small"),
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // @extend resolution
    // -----------------------------------------------------------------------

    fn resolve_extends(&mut self) {
        // First collect all tagaxis definitions
        for file in self.p1.files.values() {
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
        for file in self.p1.files.values() {
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
        for file in self.p1.files.values() {
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
        for (file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::PhonRule(pr) = &item.node {
                    self.validate_phonrule(pr, *file_id);
                }
            }
        }

        // Cycle detection across `apply` references.
        self.detect_phonrule_cycles();
    }

    fn validate_phonrule(&mut self, pr: &PhonRule, file_id: FileId) {
        let class_names: HashSet<_> = pr.classes.iter().map(|c| &c.name.node).collect();

        // Precompute the set of phoneme names visible from this file's scope
        // so we can answer "is this name a phoneme?" without re-borrowing self
        // in the diagnostic-emitting loops below.
        let phoneme_names: HashSet<String> = self.phoneme_names_in_scope(file_id);

        let is_class_like = |name: &String| -> bool {
            class_names.contains(name) || phoneme_names.contains(name)
        };

        // F2c: `syllable: NAME` must resolve to a top-level `syllable`
        // declaration visible from this file's scope. The actual syllable-macro usage
        // requirement is checked per context-element below.
        if let Some(syl_ref) = &pr.syllable {
            if find_syllable_in(self.p1, &syl_ref.node, file_id).is_none() {
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "phonrule '{}': syllable: references undefined syllable '{}'",
                        pr.name.node, syl_ref.node
                    ))
                    .with_label(syl_ref.span, "undefined syllable"),
                );
            }
        }

        // Validate union references
        for cls in &pr.classes {
            if let CharClassBody::Union(members) = &cls.body {
                for member in members {
                    if !is_class_like(&member.node) {
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

        // Validate body items: rewrite rules and apply statements.
        for item in &pr.body {
            match item {
                PhonBodyItem::Rewrite(rule) => {
                    // FROM references
                    if let PhonPattern::Class(name) = &rule.from {
                        if !is_class_like(&name.node) {
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
                            self.validate_context_elem(pr, elem, &class_names, &phoneme_names);
                        }
                    }
                }
                PhonBodyItem::Apply(apply) => {
                    // The referenced phonrule must resolve in this file's scope.
                    if self.find_phonrule(&apply.rule.node, file_id).is_none() {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "phonrule '{}': apply references undefined phonrule '{}'",
                                pr.name.node, apply.rule.node
                            ))
                            .with_label(apply.rule.span, "undefined phonrule"),
                        );
                    }
                }
            }
        }
    }

    /// Detect cycles among `apply` references. Each phonrule is the source
    /// scope for its own `apply` lookups, so the same name may resolve to
    /// different phonrules depending on the importing file. We DFS from every
    /// phonrule, threading the source file_id through resolution.
    fn detect_phonrule_cycles(&mut self) {
        // Collect all (file_id, phonrule_ref) pairs. The borrow lives via
        // self.p1 (lifetime 'a), independent of self, so the subsequent
        // &mut self call into dfs_phonrule is fine.
        let mut all: Vec<(FileId, &'a PhonRule)> = Vec::new();
        for (file_id, file) in &self.p1.files {
            for item in &file.items {
                if let Item::PhonRule(pr) = &item.node {
                    all.push((*file_id, pr));
                }
            }
        }

        // Walk each as a root; report cycles using the visited stack.
        // We key cycle membership by (file_id, phonrule pointer) to avoid
        // reporting cycles multiple times — once reported, mark seen-cycle pairs.
        let mut reported: HashSet<(FileId, *const PhonRule)> = HashSet::new();

        for (file_id, root) in all {
            let mut stack: Vec<(FileId, *const PhonRule, String, Span)> = Vec::new();
            let mut on_stack: HashSet<(FileId, *const PhonRule)> = HashSet::new();
            self.dfs_phonrule(file_id, root, &mut stack, &mut on_stack, &mut reported);
        }
    }

    fn dfs_phonrule(
        &mut self,
        file_id: FileId,
        pr: &'a PhonRule,
        stack: &mut Vec<(FileId, *const PhonRule, String, Span)>,
        on_stack: &mut HashSet<(FileId, *const PhonRule)>,
        reported: &mut HashSet<(FileId, *const PhonRule)>,
    ) {
        let key = (file_id, pr as *const PhonRule);
        if on_stack.contains(&key) {
            // Cycle: emit diagnostic if not already reported for this entry node.
            if !reported.contains(&key) {
                let names: Vec<&str> = stack
                    .iter()
                    .skip_while(|(f, p, _, _)| (*f, *p) != key)
                    .map(|(_, _, n, _)| n.as_str())
                    .collect();
                let mut cycle = names.join(" -> ");
                if !cycle.is_empty() {
                    cycle.push_str(&format!(" -> {}", pr.name.node));
                }
                // Use the span of the apply statement that closed the cycle if available.
                let label_span = stack
                    .last()
                    .map(|(_, _, _, sp)| *sp)
                    .unwrap_or(pr.name.span);
                self.diagnostics.add(
                    Diagnostic::error(format!(
                        "phonrule cycle detected: {}",
                        cycle
                    ))
                    .with_label(label_span, "cycle"),
                );
                reported.insert(key);
            }
            return;
        }

        on_stack.insert(key);
        stack.push((file_id, pr as *const PhonRule, pr.name.node.clone(), pr.name.span));

        for apply in pr.applies() {
            // Resolve directly from p1 so the &PhonRule borrow is independent
            // of `self` (avoids borrow checker conflict with `&mut self`).
            if let Some(target) = find_phonrule_in(self.p1, &apply.rule.node, file_id) {
                // Cycle detection threads the current file_id as the resolution
                // scope. This matches eval-time behavior (Phase2PhonResolver
                // is constructed per-entry with that entry's file_id).
                let saved_last_span = stack.last_mut().map(|t| {
                    let prev = t.3;
                    t.3 = apply.rule.span;
                    prev
                });
                self.dfs_phonrule(file_id, target, stack, on_stack, reported);
                if let Some(prev) = saved_last_span {
                    if let Some(t) = stack.last_mut() {
                        t.3 = prev;
                    }
                }
            }
            // If unresolved, that's already reported by validate_phonrule.
        }

        stack.pop();
        on_stack.remove(&key);
    }

    fn validate_context_elem(
        &mut self,
        pr: &PhonRule,
        elem: &PhonContextElem,
        class_names: &HashSet<&String>,
        phoneme_names: &HashSet<String>,
    ) {
        match elem {
            // F6: a quantifiable atom. The quantifier itself is validated by
            // the parser (`{n,m}` with n>m is a parse error); here we only
            // recurse into the atom to check class references.
            PhonContextElem::Atom(atom, _quant) => {
                self.validate_context_atom(pr, atom, class_names, phoneme_names);
            }
            // M (was F2c): syllable-aware macro context elements require an
            // enclosing `syllable: NAME` field; otherwise we have no
            // syllabification strategy to consult.
            PhonContextElem::SylHead | PhonContextElem::SylTail => {
                if pr.syllable.is_none() {
                    let macro_form = if matches!(elem, PhonContextElem::SylHead) {
                        "%syl<head>%"
                    } else {
                        "%syl<tail>%"
                    };
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': context uses '{}' but no 'syllable:' field is set",
                            pr.name.node, macro_form
                        ))
                        .with_label(pr.name.span, "add 'syllable: NAME' to this phonrule"),
                    );
                }
            }
            // F7: `%syl<#N>%` / `%syl<#{a..b}>%` syllable-index anchors. Like
            // the head/tail anchors they require an enclosing `syllable: NAME`
            // field. We additionally reject statically-invalid specs: index 0
            // (1-indexed origin) and ranges that are provably empty.
            PhonContextElem::SylIndex(spec) => {
                if pr.syllable.is_none() {
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': context uses a '%syl<#...>%' index macro \
                             but no 'syllable:' field is set",
                            pr.name.node
                        ))
                        .with_label(pr.name.span, "add 'syllable: NAME' to this phonrule"),
                    );
                }
                match spec {
                    SylSpec::Index(0) => {
                        self.diagnostics.add(
                            Diagnostic::error(format!(
                                "phonrule '{}': '%syl<#0>%' is invalid — syllable \
                                 indices are 1-indexed (use '#1' for the first \
                                 syllable, '#-1' for the last)",
                                pr.name.node
                            ))
                            .with_label(pr.name.span, "syllable index 0 is not allowed"),
                        );
                    }
                    SylSpec::Index(_) => {}
                    SylSpec::Range { lo, hi } => {
                        if matches!(lo, Some(0)) || matches!(hi, Some(0)) {
                            self.diagnostics.add(
                                Diagnostic::error(format!(
                                    "phonrule '{}': '%syl<#{{...}}>%' range bound 0 is \
                                     invalid — syllable indices are 1-indexed",
                                    pr.name.node
                                ))
                                .with_label(pr.name.span, "syllable index 0 is not allowed"),
                            );
                        }
                        // A range is provably empty only when both bounds have
                        // the same sign (so end-relative normalisation can't
                        // reorder them) and `lo > hi`.
                        if let (Some(lo), Some(hi)) = (lo, hi) {
                            let same_sign = (*lo > 0) == (*hi > 0);
                            if same_sign && lo > hi {
                                self.diagnostics.add(
                                    Diagnostic::warning(format!(
                                        "phonrule '{}': '%syl<#{{{}..{}}}>%' is an empty \
                                         range (lo > hi) — this context never matches",
                                        pr.name.node, lo, hi
                                    ))
                                    .with_label(pr.name.span, "empty syllable-index range"),
                                );
                            }
                        }
                    }
                }
            }
            PhonContextElem::Boundary | PhonContextElem::WordStart | PhonContextElem::WordEnd => {}
        }
    }

    /// Validate an F6 quantifiable atom: check class references resolve, and
    /// recurse into alternations.
    fn validate_context_atom(
        &mut self,
        pr: &PhonRule,
        atom: &PhonAtom,
        class_names: &HashSet<&String>,
        phoneme_names: &HashSet<String>,
    ) {
        match atom {
            PhonAtom::Class(name) | PhonAtom::NegClass(name) => {
                let known = class_names.contains(&name.node)
                    || phoneme_names.contains(&name.node);
                if !known {
                    self.diagnostics.add(
                        Diagnostic::error(format!(
                            "phonrule '{}': context references undefined class '{}'",
                            pr.name.node, name.node
                        ))
                        .with_label(name.span, "undefined class"),
                    );
                }
            }
            PhonAtom::Alt(alts) => {
                for alt in alts {
                    self.validate_context_elem(pr, alt, class_names, phoneme_names);
                }
            }
            // Literals and the wildcard `.` need no name resolution.
            PhonAtom::Literal(_) | PhonAtom::Wildcard => {}
        }
    }

    /// Collect the names of all phonemes visible from `file_id` (locals plus
    /// `@use` / `@reference` imports), restricted to those actually present in
    /// the resolved inventory.
    fn phoneme_names_in_scope(&self, file_id: FileId) -> HashSet<String> {
        let mut out = HashSet::new();
        let Some(scope) = self.p1.symbol_table.scope(file_id) else {
            return out;
        };
        for sym in scope.locals.values() {
            if sym.kind == SymbolKind::Phoneme && self.phonemes.has_phoneme(&sym.name) {
                out.insert(sym.name.clone());
            }
        }
        for imp in scope.imports.iter().chain(scope.exports.iter()) {
            if imp.kind == SymbolKind::Phoneme
                && self.phonemes.has_phoneme(&imp.original_name)
            {
                out.insert(imp.local_name.clone());
            }
        }
        out
    }

    fn find_phonrule(&self, name: &str, file_id: FileId) -> Option<&'a PhonRule> {
        find_phonrule_in(self.p1, name, file_id)
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
                        Some((fid, (**e).clone()))
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

    /// Resolve entries selectively based on Merkle hash changes.
    ///
    /// Only entries in `entries_to_resolve` (keyed by `(source_path, entry_name)`)
    /// are freshly expanded; everything else comes from `cached`.
    fn resolve_entries_by_merkle(
        &mut self,
        entries_to_resolve: &HashSet<(std::path::PathBuf, String)>,
        cached: Vec<ResolvedEntry>,
    ) {
        self.entries = cached;

        let entries: Vec<(FileId, Entry)> = self
            .p1
            .files
            .iter()
            .flat_map(|(&fid, file)| {
                let path = self.p1.source_map.path(fid).to_path_buf();
                file.items.iter().filter_map(move |item| {
                    if let Item::Entry(e) = &item.node {
                        let key = (path.clone(), e.name.node.clone());
                        if entries_to_resolve.contains(&key) {
                            Some((fid, (**e).clone()))
                        } else {
                            None
                        }
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
        let (headword, headword_scripts) = Self::extract_headword(entry);

        let tags: Vec<(String, String)> = entry
            .tags
            .iter()
            .map(|tc| (tc.axis.node.clone(), tc.value.node.clone()))
            .collect();

        let (meaning, meanings) = Self::extract_meanings(entry);

        let stems: HashMap<String, String> = entry
            .stems
            .iter()
            .map(|s| (s.name.node.clone(), s.value.node.clone()))
            .collect();

        let forms = self.expand_inflection_forms(file_id, entry, &stems);

        let inflection_class = match &entry.inflection {
            Some(EntryInflection::Class(class_name)) => Some(class_name.node.clone()),
            _ => None,
        };

        let etymology_proto = entry.etymology.as_ref().and_then(|e| e.proto.as_ref().map(|s| s.node.clone()));
        let etymology_note = entry.etymology.as_ref().and_then(|e| e.note.as_ref().map(|s| s.node.clone()));
        let links = Self::collect_links(entry);

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

    /// Extract the default headword and per-script headword map from an entry.
    fn extract_headword(entry: &Entry) -> (String, HashMap<String, String>) {
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

        (headword, headword_scripts)
    }

    /// Extract the primary meaning and the full meanings list from an entry.
    fn extract_meanings(entry: &Entry) -> (String, Vec<(String, String)>) {
        match &entry.meaning {
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
        }
    }

    /// Expand inflection forms for an entry, returning resolved forms.
    fn expand_inflection_forms(
        &mut self,
        file_id: FileId,
        entry: &Entry,
        stems: &HashMap<String, String>,
    ) -> Vec<ResolvedForm> {
        let mut forms = Vec::new();

        let infl = match &entry.inflection {
            Some(infl) => infl,
            None => return forms,
        };

        let (axes, body, stem_reqs, infl_span) = match infl {
            EntryInflection::Class(class_name) => {
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
                    return forms;
                }
            }
            EntryInflection::Inline(inline) => {
                let axes: Vec<String> =
                    inline.axes.iter().map(|a| a.node.clone()).collect();
                (axes, Some(inline.body.clone()), Vec::new(), None)
            }
        };

        let body = match body {
            Some(b) => b,
            None => return forms,
        };

        // Filter out wildcard axes not referenced in any rule condition or delegate.
        let referenced = collect_referenced_axes(&body, &entry.forms_override);
        let effective_axes: Vec<String> = axes
            .iter()
            .filter(|a| referenced.contains(a.as_str()))
            .cloned()
            .collect();

        let axis_values: HashMap<String, Vec<String>> = effective_axes
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

        let cells = match enumerate_cells(&effective_axes, &axis_values) {
            Ok(cells) => cells,
            Err(e) => {
                self.diagnostics.add(e);
                return forms;
            }
        };

        let struct_stems = self.build_struct_stems(&stem_reqs, stems);

        let resolver = Phase2Resolver { ctx: self, file_id };
        let phon_resolver = Phase2PhonResolver { ctx: self, file_id };
        let result = match &body {
            InflectionBody::Rules(body) => {
                evaluate_rules_with_overrides(
                    &body.rules, &entry.forms_override, body.apply.as_ref(), &cells, stems, &struct_stems, &resolver, &phon_resolver,
                )
            }
            InflectionBody::Compose(comp) => {
                evaluate_compose(comp, &entry.forms_override, &cells, stems, &struct_stems, &phon_resolver)
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

        forms
    }

    /// Build structural stems mapping from required_stems constraints and axis slots.
    fn build_struct_stems(
        &mut self,
        stem_reqs: &[StemReq],
        stems: &HashMap<String, String>,
    ) -> HashMap<String, HashMap<String, String>> {
        let mut struct_stems: HashMap<String, HashMap<String, String>> = HashMap::new();
        for req in stem_reqs {
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
        struct_stems
    }

    /// Collect all links (derived_from, cognates, examples) from an entry.
    fn collect_links(entry: &Entry) -> Vec<ResolvedLink> {
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
        links
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
        for file in self.p1.files.values() {
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

    fn inventory(&self) -> Option<&crate::phoneme::PhonemeInventory> {
        Some(&self.ctx.phonemes)
    }

    fn resolve_syllable(&self, name: &str) -> Option<&Syllable> {
        find_syllable_in(self.ctx.p1, name, self.file_id)
    }
}
