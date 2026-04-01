//! Linter for `.hu` files.
//!
//! Runs after phase 1 and phase 2 to detect warnings and style issues that are
//! not compilation errors. Supports `--fix` for mechanically fixable diagnostics.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::*;
use crate::error::{Diagnostic, Diagnostics, Severity};
use crate::inflection_eval;
use crate::phase1::{self, Phase1Result};
use crate::span::{FileId, SourceMap};
use crate::visit::{self, Visitor};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A source-level text edit (byte-offset based).
#[derive(Debug, Clone)]
pub struct SourceEdit {
    pub file_id: FileId,
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// A lint diagnostic with an optional auto-fix.
#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    pub rule: &'static str,
    pub diagnostic: Diagnostic,
    pub fix: Option<Vec<SourceEdit>>,
}

/// Result of running the linter.
#[derive(Debug)]
pub struct LintResult {
    pub lints: Vec<LintDiagnostic>,
    /// Compilation errors encountered while loading (not lint issues).
    pub compile_errors: Diagnostics,
    pub source_map: crate::span::SourceMap,
}

impl LintResult {
    pub fn has_lints(&self) -> bool {
        !self.lints.is_empty()
    }

    /// Render all lint diagnostics to a string.
    pub fn render_all(&self) -> String {
        let mut out = String::new();
        for lint in &self.lints {
            out.push_str(&format!("[{}] ", lint.rule));
            out.push_str(&lint.diagnostic.render(&self.source_map));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Suppress comments
// ---------------------------------------------------------------------------

/// Per-file suppressions.
///
/// * Line-specific: maps 1-based line number → set of suppressed rule names.
/// * File-wide (`entire-file`): stored under key `0`.
type Suppressions = HashMap<usize, HashSet<String>>;

/// Key used for `entire-file` suppressions.
const ENTIRE_FILE_KEY: usize = 0;

/// Parse `# @suppress next-line: rule1, rule2` and
/// `# @suppress entire-file: rule1, rule2` comments from source text.
fn parse_suppressions(source: &str) -> Suppressions {
    let mut suppressions = Suppressions::new();
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        // Strip leading '#' characters
        let rest = match trimmed.strip_prefix('#') {
            Some(r) => r.trim_start_matches('#').trim_start(),
            None => continue,
        };
        let rest = match rest.strip_prefix("@suppress") {
            Some(r) => r.trim_start(),
            None => continue,
        };

        let target_line;
        let rest = if let Some(r) = rest.strip_prefix("next-line") {
            // line_idx is 0-based; the suppressed line is the next one, 1-based
            target_line = line_idx + 2;
            r.trim_start()
        } else if let Some(r) = rest.strip_prefix("entire-file") {
            target_line = ENTIRE_FILE_KEY;
            r.trim_start()
        } else {
            continue;
        };

        let rest = match rest.strip_prefix(':') {
            Some(r) => r,
            None => continue,
        };
        for rule_name in rest.split(',') {
            let rule_name = rule_name.trim();
            if !rule_name.is_empty() {
                suppressions
                    .entry(target_line)
                    .or_default()
                    .insert(rule_name.to_string());
            }
        }
    }
    suppressions
}

/// Build per-file suppressions for all files in the phase1 result.
fn build_suppressions(p1: &Phase1Result) -> HashMap<FileId, Suppressions> {
    let mut all = HashMap::new();
    for &file_id in p1.files.keys() {
        let source = p1.source_map.source(file_id);
        let supps = parse_suppressions(source);
        if !supps.is_empty() {
            all.insert(file_id, supps);
        }
    }
    all
}

/// Check if a lint is suppressed by a `@suppress` comment (next-line or entire-file).
fn is_suppressed(
    lint: &LintDiagnostic,
    file_suppressions: &HashMap<FileId, Suppressions>,
    source_map: &SourceMap,
) -> bool {
    let label = match lint.diagnostic.labels.first() {
        Some(l) => l,
        None => return false,
    };
    let supps = match file_suppressions.get(&label.span.file_id) {
        Some(s) => s,
        None => return false,
    };
    // Check entire-file suppression first.
    if supps.get(&ENTIRE_FILE_KEY).is_some_and(|rules| rules.contains(lint.rule)) {
        return true;
    }
    // Then check line-specific suppression.
    let (line, _) = source_map.line_col(label.span.file_id, label.span.start);
    supps.get(&line).is_some_and(|rules| rules.contains(lint.rule))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run all lint rules using an already-computed Phase1Result.
///
/// This is useful for the LSP server which already has the phase1 result.
pub fn run_lint_from_phase1(p1: &Phase1Result) -> Vec<LintDiagnostic> {
    let mut lints = Vec::new();

    for (&file_id, file) in &p1.files {
        let source = p1.source_map.source(file_id);
        lint_single_file(file_id, file, source, &mut lints);
        lint_source_format(file_id, source, &mut lints);
    }

    lint_cross_file(p1, &mut lints);

    if !p1.diagnostics.has_errors() {
        lint_with_resolved_axes(p1, &mut lints);
    }

    // Filter suppressed lints
    let suppressions = build_suppressions(p1);
    if !suppressions.is_empty() {
        lints.retain(|lint| !is_suppressed(lint, &suppressions, &p1.source_map));
    }

    lints.sort_by(|a, b| {
        let span_a = a.diagnostic.labels.first().map(|l| (l.span.file_id.0, l.span.start));
        let span_b = b.diagnostic.labels.first().map(|l| (l.span.file_id.0, l.span.start));
        span_a.cmp(&span_b)
    });

    lints
}

/// Run all lint rules on a project starting from the given entry file.
pub fn run_lint(entry_path: &Path) -> LintResult {
    let p1 = phase1::run_phase1(entry_path);

    let mut lints = Vec::new();

    // Run single-file lints on each file
    for (&file_id, file) in &p1.files {
        let source = p1.source_map.source(file_id);
        lint_single_file(file_id, file, source, &mut lints);
        lint_source_format(file_id, source, &mut lints);
    }

    // Run cross-file lints using phase1 symbol table
    lint_cross_file(&p1, &mut lints);

    // Run lints that need resolved axis values (lightweight, no full phase2)
    if !p1.diagnostics.has_errors() {
        lint_with_resolved_axes(&p1, &mut lints);
    }

    // Filter suppressed lints
    let suppressions = build_suppressions(&p1);
    if !suppressions.is_empty() {
        lints.retain(|lint| !is_suppressed(lint, &suppressions, &p1.source_map));
    }

    // Sort lints by file and position for stable output
    lints.sort_by(|a, b| {
        let span_a = a.diagnostic.labels.first().map(|l| (l.span.file_id.0, l.span.start));
        let span_b = b.diagnostic.labels.first().map(|l| (l.span.file_id.0, l.span.start));
        span_a.cmp(&span_b)
    });

    LintResult {
        lints,
        compile_errors: p1.diagnostics,
        source_map: p1.source_map,
    }
}

/// Fix priority for a lint rule. Lower number = higher priority.
///
/// When multiple fixable rules target overlapping byte ranges, the
/// higher-priority fix wins and the lower-priority fix is silently skipped.
///
/// Priority phases:
///   0 — Semantic deletions (e.g. `unused-import`): removing dead code takes
///       precedence over rearranging it.
///   1 — Structural rearrangements (e.g. `import-order`): moving code around.
///   2 — Formatting (e.g. `trailing-whitespace`, `consecutive-blank-lines`,
///       `trailing-newline`): whitespace-only changes.
fn fix_priority(rule: &str) -> u8 {
    match rule {
        "unused-import" => 0,
        "import-order" => 1,
        _ => 2,
    }
}

/// Apply fixes to source files on disk.
///
/// Fixes are grouped by originating lint rule.  When edits from different
/// rules overlap, the entire fix group with the **lower** priority (higher
/// number from [`fix_priority`]) is dropped — its edits are interdependent,
/// so partial application would be incorrect.
pub fn apply_fixes(
    lints: &[LintDiagnostic],
    source_map: &crate::span::SourceMap,
) -> Result<usize, String> {
    // A group of edits from a single lint diagnostic.
    struct FixGroup<'a> {
        priority: u8,
        edits: &'a [SourceEdit],
    }

    // Collect fix groups per file.
    let mut groups_by_file: HashMap<FileId, Vec<FixGroup<'_>>> = HashMap::new();
    for lint in lints {
        if let Some(fixes) = &lint.fix {
            if fixes.is_empty() {
                continue;
            }
            let file_id = fixes[0].file_id;
            groups_by_file
                .entry(file_id)
                .or_default()
                .push(FixGroup {
                    priority: fix_priority(lint.rule),
                    edits: fixes,
                });
        }
    }

    let mut total_fixes = 0;

    for (file_id, mut groups) in groups_by_file {
        // Higher priority (lower number) first.
        groups.sort_by_key(|g| g.priority);

        // Greedily accept non-overlapping fix groups.
        let mut accepted: Vec<&SourceEdit> = Vec::new();

        for group in &groups {
            let overlaps = group.edits.iter().any(|edit| {
                accepted
                    .iter()
                    .any(|acc| edit.start < acc.end && edit.end > acc.start)
            });
            if !overlaps {
                accepted.extend(group.edits.iter());
            }
        }

        // Sort accepted edits in reverse order so earlier offsets aren't shifted.
        accepted.sort_by(|a, b| b.start.cmp(&a.start));

        let mut source = source_map.source(file_id).to_string();
        for edit in &accepted {
            source.replace_range(edit.start..edit.end, &edit.replacement);
            total_fixes += 1;
        }

        let path = source_map.path(file_id);
        std::fs::write(path, &source)
            .map_err(|e| format!("cannot write '{}': {}", path.display(), e))?;
    }

    Ok(total_fixes)
}

// ---------------------------------------------------------------------------
// Fix helpers
// ---------------------------------------------------------------------------

/// Expand a byte range to cover complete lines in the source.
/// Returns `(line_start, line_end)` where `line_end` includes the trailing `\n`.
fn line_range(source: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = source[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = source[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(source.len());
    (line_start, line_end)
}

/// Create a `SourceEdit` that removes the full line(s) containing an item span.
fn item_removal_edit(span: Span, source: &str) -> SourceEdit {
    let (ls, le) = line_range(source, span.start, span.end);
    SourceEdit {
        file_id: span.file_id,
        start: ls,
        end: le,
        replacement: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Source-level format lints (raw text, no AST)
// ---------------------------------------------------------------------------

fn lint_source_format(file_id: FileId, source: &str, lints: &mut Vec<LintDiagnostic>) {
    lint_trailing_whitespace(file_id, source, lints);
    lint_consecutive_blank_lines(file_id, source, lints);
    lint_trailing_newline(file_id, source, lints);
}

/// `trailing-whitespace`: lines with trailing spaces or tabs.
fn lint_trailing_whitespace(file_id: FileId, source: &str, lints: &mut Vec<LintDiagnostic>) {
    let mut edits = Vec::new();
    let mut pos = 0;

    for line in source.split('\n') {
        let trimmed_len = line.trim_end_matches(|c: char| c == ' ' || c == '\t').len();
        if trimmed_len < line.len() {
            edits.push(SourceEdit {
                file_id,
                start: pos + trimmed_len,
                end: pos + line.len(),
                replacement: String::new(),
            });
        }
        pos += line.len() + 1; // +1 for '\n'
    }

    if !edits.is_empty() {
        let count = edits.len();
        lints.push(LintDiagnostic {
            rule: "trailing-whitespace",
            diagnostic: Diagnostic::warning(format!(
                "{} line(s) with trailing whitespace",
                count
            ))
            .with_label(
                Span {
                    file_id,
                    start: edits[0].start,
                    end: edits[0].end,
                },
                "first occurrence",
            ),
            fix: Some(edits),
        });
    }
}

/// `consecutive-blank-lines`: 2+ consecutive blank lines (3+ consecutive newlines).
///
/// Only fires for interior runs; trailing newlines at end-of-file are handled
/// by `trailing-newline`.
fn lint_consecutive_blank_lines(file_id: FileId, source: &str, lints: &mut Vec<LintDiagnostic>) {
    let mut edits = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let run_start = i;
            while i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
            let count = i - run_start;
            // 3+ consecutive newlines = 2+ blank lines; skip if it reaches EOF
            if count >= 3 && i < bytes.len() {
                edits.push(SourceEdit {
                    file_id,
                    start: run_start,
                    end: i,
                    replacement: "\n\n".to_string(),
                });
            }
        } else {
            i += 1;
        }
    }

    if !edits.is_empty() {
        let count = edits.len();
        lints.push(LintDiagnostic {
            rule: "consecutive-blank-lines",
            diagnostic: Diagnostic::warning(format!(
                "{} region(s) with consecutive blank lines",
                count
            ))
            .with_label(
                Span {
                    file_id,
                    start: edits[0].start,
                    end: edits[0].end,
                },
                "first occurrence",
            ),
            fix: Some(edits),
        });
    }
}

/// `trailing-newline`: file should end with exactly one newline.
fn lint_trailing_newline(file_id: FileId, source: &str, lints: &mut Vec<LintDiagnostic>) {
    if source.is_empty() {
        return;
    }

    if !source.ends_with('\n') {
        lints.push(LintDiagnostic {
            rule: "trailing-newline",
            diagnostic: Diagnostic::warning("file does not end with a newline").with_label(
                Span {
                    file_id,
                    start: source.len(),
                    end: source.len(),
                },
                "end of file",
            ),
            fix: Some(vec![SourceEdit {
                file_id,
                start: source.len(),
                end: source.len(),
                replacement: "\n".to_string(),
            }]),
        });
    } else {
        let trimmed_len = source.trim_end_matches('\n').len();
        let trailing_count = source.len() - trimmed_len;
        if trailing_count > 1 {
            lints.push(LintDiagnostic {
                rule: "trailing-newline",
                diagnostic: Diagnostic::warning("file has multiple trailing newlines")
                    .with_label(
                        Span {
                            file_id,
                            start: trimmed_len + 1,
                            end: source.len(),
                        },
                        "extra newlines",
                    ),
                fix: Some(vec![SourceEdit {
                    file_id,
                    start: trimmed_len + 1,
                    end: source.len(),
                    replacement: String::new(),
                }]),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Single-file lints (AST only)
// ---------------------------------------------------------------------------

fn lint_single_file(file_id: FileId, file: &File, source: &str, lints: &mut Vec<LintDiagnostic>) {
    lint_single_meaning_multiple(file, lints);
    lint_import_order(file_id, file, source, lints);
    lint_duplicate_condition(file, lints);
}

/// `single-meaning-multiple`: `meanings { }` block with only one entry.
fn lint_single_meaning_multiple(file: &File, lints: &mut Vec<LintDiagnostic>) {
    for item in &file.items {
        if let Item::Entry(entry) = &item.node {
            if let MeaningDef::Multiple(meanings) = &entry.meaning {
                if meanings.len() == 1 {
                    let m = &meanings[0];
                    lints.push(LintDiagnostic {
                        rule: "single-meaning-multiple",
                        diagnostic: Diagnostic::warning(
                            "meanings block contains only one meaning; use `meaning:` instead",
                        )
                        .with_label(m.ident.span, "only one meaning defined"),
                        fix: None,
                    });
                }
            }
        }
    }
}

/// `import-order`: @use and @reference should come before other items.
fn lint_import_order(file_id: FileId, file: &File, source: &str, lints: &mut Vec<LintDiagnostic>) {
    let mut first_non_import_span: Option<Span> = None;
    let mut misplaced: Vec<&Spanned<Item>> = Vec::new();

    for item in &file.items {
        match &item.node {
            Item::Use(_) | Item::Reference(_) | Item::Export(_) => {
                if first_non_import_span.is_some() {
                    misplaced.push(item);
                }
            }
            _ => {
                if first_non_import_span.is_none() {
                    first_non_import_span = Some(item.span);
                }
            }
        }
    }

    if misplaced.is_empty() {
        return;
    }

    // Build a combined fix: remove each misplaced import and insert all at target
    let insert_pos = first_non_import_span.unwrap().start;
    let (insert_line_start, _) = line_range(source, insert_pos, insert_pos);

    let mut edits = Vec::new();
    let mut insert_text = String::new();

    for import_item in &misplaced {
        let (ls, le) = line_range(source, import_item.span.start, import_item.span.end);
        insert_text.push_str(&source[ls..le]);
        edits.push(SourceEdit {
            file_id,
            start: ls,
            end: le,
            replacement: String::new(),
        });
    }

    // Insert all imports at target position
    edits.push(SourceEdit {
        file_id,
        start: insert_line_start,
        end: insert_line_start,
        replacement: insert_text,
    });

    // First diagnostic carries the fix, rest get None
    let mut first = true;
    for import_item in &misplaced {
        lints.push(LintDiagnostic {
            rule: "import-order",
            diagnostic: Diagnostic::warning(
                "import should appear before other declarations",
            )
            .with_label(import_item.span, "import after non-import item"),
            fix: if first { first = false; Some(edits.clone()) } else { None },
        });
    }
}

/// `duplicate-condition`: same condition set appears multiple times in an inflection.
fn lint_duplicate_condition(file: &File, lints: &mut Vec<LintDiagnostic>) {
    for item in &file.items {
        match &item.node {
            Item::Inflection(infl) => {
                check_body_duplicate_conditions(&infl.body, lints);
            }
            Item::Entry(entry) => {
                if !entry.forms_override.is_empty() {
                    check_duplicate_conditions_inner(&entry.forms_override, lints);
                }
                if let Some(EntryInflection::Inline(inline)) = &entry.inflection {
                    check_body_duplicate_conditions(&inline.body, lints);
                }
            }
            _ => {}
        }
    }
}

fn check_body_duplicate_conditions(body: &InflectionBody, lints: &mut Vec<LintDiagnostic>) {
    match body {
        InflectionBody::Rules(rules) => {
            check_duplicate_conditions_inner(rules, lints);
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                check_duplicate_conditions_inner(&slot.rules, lints);
            }
            check_duplicate_conditions_inner(&comp.overrides, lints);
        }
    }
}

fn check_duplicate_conditions_inner(rules: &[InflectionRule], lints: &mut Vec<LintDiagnostic>) {
    let mut seen: Vec<(ConditionKey, Span)> = Vec::new();

    for rule in rules {
        let key = condition_key(&rule.condition);
        if let Some((_, prev_span)) = seen.iter().find(|(k, _)| k == &key) {
            lints.push(LintDiagnostic {
                rule: "duplicate-condition",
                diagnostic: Diagnostic::warning("duplicate rule condition")
                    .with_label(rule.condition.span, "duplicate condition")
                    .with_label(*prev_span, "first defined here"),
                fix: None,
            });
        } else {
            seen.push((key, rule.condition.span));
        }
    }
}

/// Normalized representation of a condition for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConditionKey {
    /// Sorted (axis, value) pairs.
    conditions: Vec<(String, String)>,
    wildcard: bool,
}

fn condition_key(cond: &TagConditionList) -> ConditionKey {
    let mut conditions: Vec<(String, String)> = cond
        .conditions
        .iter()
        .map(|c| (c.axis.node.clone(), c.value.node.clone()))
        .collect();
    conditions.sort();
    ConditionKey {
        conditions,
        wildcard: cond.wildcard,
    }
}

// ---------------------------------------------------------------------------
// Cross-file lints (need Phase1Result)
// ---------------------------------------------------------------------------

fn lint_cross_file(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    lint_unused_import(p1, lints);
    lint_glob_import(p1, lints);
    lint_unused_definitions(p1, lints);
}

/// `glob-import`: `@use *` / `@export use *` imports everything — prefer named imports.
fn lint_glob_import(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    for file in p1.files.values() {
        for item in &file.items {
            let (target, msg) = match &item.node {
                Item::Use(import) => (&import.target, "prefer named imports over `@use *`"),
                Item::Export(export) if export.is_use => {
                    (&export.target, "prefer named exports over `@export use *`")
                }
                _ => continue,
            };
            if let ImportTarget::Glob { alias: None } = target {
                lints.push(LintDiagnostic {
                    rule: "glob-import",
                    diagnostic: Diagnostic::warning(msg)
                        .with_label(item.span, "glob import"),
                    fix: None,
                });
            }
        }
    }
}

/// `unused-import`: imported symbol never referenced in the importing file.
fn lint_unused_import(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    for (&file_id, file) in &p1.files {
        if p1.symbol_table.scope(file_id).is_none() {
            continue;
        }

        let source = p1.source_map.source(file_id);

        // Collect all names actually used in this file's AST
        let used_names = collect_used_names(file);

        // Check each non-glob named import
        for item in &file.items {
            let import = match &item.node {
                Item::Use(imp) | Item::Reference(imp) => imp,
                _ => continue,
            };

            if let ImportTarget::Named(entries) = &import.target {
                // Count how many entries are unused
                let unused_count = entries
                    .iter()
                    .filter(|e| {
                        let local = e.alias.as_ref().unwrap_or(&e.name);
                        !used_names.contains(&local.node)
                    })
                    .count();

                for entry in entries {
                    let local = entry.alias.as_ref().unwrap_or(&entry.name);
                    if !used_names.contains(&local.node) {
                        // Auto-fix: remove the whole @use line when all entries are unused
                        // or when there's only one entry.
                        let fix = if unused_count == entries.len() || entries.len() == 1 {
                            Some(vec![item_removal_edit(item.span, source)])
                        } else {
                            None
                        };
                        lints.push(LintDiagnostic {
                            rule: "unused-import",
                            diagnostic: Diagnostic::warning(format!(
                                "unused import: `{}`",
                                local.node
                            ))
                            .with_label(local.span, "imported but never used"),
                            fix,
                        });
                    }
                }
            }
        }
    }
}

/// `unused-tagaxis`, `unused-inflection`: definitions not referenced anywhere.
fn lint_unused_definitions(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    for file in p1.files.values() {
        for item in &file.items {
            match &item.node {
                Item::TagAxis(ta) => {
                    if !is_tagaxis_used(&ta.name.node, p1) {
                        lints.push(LintDiagnostic {
                            rule: "unused-tagaxis",
                            diagnostic: Diagnostic::warning(format!(
                                "tagaxis `{}` is never used",
                                ta.name.node
                            ))
                            .with_label(ta.name.span, "defined here"),
                            fix: None,
                        });
                    }
                }
                Item::Inflection(infl) => {
                    if !is_inflection_used(&infl.name.node, p1) {
                        lints.push(LintDiagnostic {
                            rule: "unused-inflection",
                            diagnostic: Diagnostic::warning(format!(
                                "inflection `{}` is never referenced by any entry",
                                infl.name.node
                            ))
                            .with_label(infl.name.span, "defined here"),
                            fix: None,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_tagaxis_used(name: &str, p1: &Phase1Result) -> bool {
    for file in p1.files.values() {
        for item in &file.items {
            match &item.node {
                Item::Extend(ext) => {
                    if ext.target_axis.node == name {
                        return true;
                    }
                }
                Item::Inflection(infl) => {
                    if infl.axes.iter().any(|a| a.node == name) {
                        return true;
                    }
                }
                Item::Entry(entry) => {
                    if entry.tags.iter().any(|t| t.axis.node == name) {
                        return true;
                    }
                    if let Some(EntryInflection::Inline(inline)) = &entry.inflection {
                        if inline.axes.iter().any(|a| a.node == name) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    false
}

fn is_inflection_used(name: &str, p1: &Phase1Result) -> bool {
    for file in p1.files.values() {
        for item in &file.items {
            match &item.node {
                Item::Entry(entry) => {
                    if let Some(EntryInflection::Class(cls)) = &entry.inflection {
                        if cls.node == name {
                            return true;
                        }
                    }
                }
                Item::Inflection(infl) => {
                    if references_inflection_name(&infl.body, name) {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

fn references_inflection_name(body: &InflectionBody, name: &str) -> bool {
    match body {
        InflectionBody::Rules(rules) => rules_reference_inflection(rules, name),
        InflectionBody::Compose(comp) => rules_reference_inflection(&comp.overrides, name),
    }
}

fn rules_reference_inflection(rules: &[InflectionRule], name: &str) -> bool {
    rules.iter().any(|rule| match &rule.rhs.node {
        RuleRhs::Delegate(d) => d.target.node == name,
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// Phase 2 lints (need resolved axes)
// ---------------------------------------------------------------------------

/// Resolve axis values from phase1 AST (lightweight, no full phase2 needed).
fn resolve_axis_values(p1: &Phase1Result) -> HashMap<String, Vec<String>> {
    let mut axes: HashMap<String, Vec<String>> = HashMap::new();

    // Register tagaxis names
    for file in p1.files.values() {
        for item in &file.items {
            if let Item::TagAxis(ta) = &item.node {
                axes.entry(ta.name.node.clone()).or_default();
            }
        }
    }

    // Collect @extend values
    for file in p1.files.values() {
        for item in &file.items {
            if let Item::Extend(ext) = &item.node {
                if let Some(values) = axes.get_mut(&ext.target_axis.node) {
                    for val in &ext.values {
                        if !values.contains(&val.name.node) {
                            values.push(val.name.node.clone());
                        }
                    }
                }
            }
        }
    }

    axes
}

fn lint_with_resolved_axes(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    let axis_values = resolve_axis_values(p1);
    lint_unused_extend_value(p1, lints);
    lint_unused_stem(p1, lints);
    lint_shadowed_rule(p1, &axis_values, lints);
    lint_incomplete_coverage(p1, &axis_values, lints);
}

/// `unused-extend-value`: a value added by @extend is never used in any rule condition.
fn lint_unused_extend_value(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    // Collect all (axis, value) pairs referenced in rule conditions and entry tags
    let mut used_values: HashSet<(String, String)> = HashSet::new();
    for file in p1.files.values() {
        collect_used_tag_values(file, &mut used_values);
    }

    for file in p1.files.values() {
        for item in &file.items {
            if let Item::Extend(ext) = &item.node {
                let axis = &ext.target_axis.node;
                for val in &ext.values {
                    if !used_values.contains(&(axis.clone(), val.name.node.clone())) {
                        lints.push(LintDiagnostic {
                            rule: "unused-extend-value",
                            diagnostic: Diagnostic::warning(format!(
                                "value `{}` of axis `{}` is never used in any rule or tag",
                                val.name.node, axis
                            ))
                            .with_label(val.name.span, "defined but never referenced"),
                            fix: None,
                        });
                    }
                }
            }
        }
    }
}

/// Collect all (axis, value) pairs used in rule conditions and entry tags.
fn collect_used_tag_values(file: &File, used: &mut HashSet<(String, String)>) {
    for item in &file.items {
        match &item.node {
            Item::Inflection(infl) => {
                collect_body_tag_values(&infl.body, used);
            }
            Item::Entry(entry) => {
                for tag in &entry.tags {
                    used.insert((tag.axis.node.clone(), tag.value.node.clone()));
                }
                for rule in &entry.forms_override {
                    collect_rule_tag_values(rule, used);
                }
                if let Some(EntryInflection::Inline(inline)) = &entry.inflection {
                    collect_body_tag_values(&inline.body, used);
                }
            }
            _ => {}
        }
    }
}

fn collect_body_tag_values(body: &InflectionBody, used: &mut HashSet<(String, String)>) {
    match body {
        InflectionBody::Rules(rules) => {
            for rule in rules {
                collect_rule_tag_values(rule, used);
            }
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                for rule in &slot.rules {
                    collect_rule_tag_values(rule, used);
                }
            }
            for rule in &comp.overrides {
                collect_rule_tag_values(rule, used);
            }
        }
    }
}

fn collect_rule_tag_values(rule: &InflectionRule, used: &mut HashSet<(String, String)>) {
    for cond in &rule.condition.conditions {
        used.insert((cond.axis.node.clone(), cond.value.node.clone()));
    }
}

/// `unused-stem`: entry defines a stem that is never referenced by its inflection.
fn lint_unused_stem(p1: &Phase1Result, lints: &mut Vec<LintDiagnostic>) {
    // Build a map of inflection name -> set of stem names used in templates
    let mut infl_stem_refs: HashMap<String, HashSet<String>> = HashMap::new();
    for file in p1.files.values() {
        for item in &file.items {
            if let Item::Inflection(infl) = &item.node {
                let stems = collect_template_stem_refs(&infl.body);
                infl_stem_refs.insert(infl.name.node.clone(), stems);
            }
        }
    }

    for file in p1.files.values() {
        for item in &file.items {
            if let Item::Entry(entry) = &item.node {
                let referenced_stems = match &entry.inflection {
                    Some(EntryInflection::Class(cls)) => {
                        infl_stem_refs.get(&cls.node).cloned().unwrap_or_default()
                    }
                    Some(EntryInflection::Inline(inline)) => {
                        let mut stems = collect_template_stem_refs(&inline.body);
                        // Also check forms_override templates
                        collect_rules_template_stems(&entry.forms_override, &mut stems);
                        stems
                    }
                    None => {
                        // No inflection — stems in forms_override only
                        let mut stems = HashSet::new();
                        collect_rules_template_stems(&entry.forms_override, &mut stems);
                        stems
                    }
                };

                for stem in &entry.stems {
                    if !referenced_stems.contains(&stem.name.node) {
                        lints.push(LintDiagnostic {
                            rule: "unused-stem",
                            diagnostic: Diagnostic::warning(format!(
                                "stem `{}` is defined but never referenced by the inflection",
                                stem.name.node
                            ))
                            .with_label(stem.name.span, "unused stem"),
                            fix: None,
                        });
                    }
                }
            }
        }
    }
}

/// Collect all stem names referenced in templates within an inflection body.
fn collect_template_stem_refs(body: &InflectionBody) -> HashSet<String> {
    let mut stems = HashSet::new();
    match body {
        InflectionBody::Rules(rules) => {
            collect_rules_template_stems(rules, &mut stems);
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                collect_rules_template_stems(&slot.rules, &mut stems);
            }
            collect_rules_template_stems(&comp.overrides, &mut stems);
        }
    }
    stems
}

fn collect_rules_template_stems(rules: &[InflectionRule], stems: &mut HashSet<String>) {
    for rule in rules {
        collect_rhs_template_stems(&rule.rhs.node, stems);
    }
}

fn collect_rhs_template_stems(rhs: &RuleRhs, stems: &mut HashSet<String>) {
    match rhs {
        RuleRhs::Template(tmpl) => {
            for seg in &tmpl.segments {
                match seg {
                    TemplateSegment::Stem(ident) => {
                        stems.insert(ident.node.clone());
                    }
                    TemplateSegment::Slot { stem, .. } => {
                        stems.insert(stem.node.clone());
                    }
                    TemplateSegment::Lit(_) => {}
                }
            }
        }
        RuleRhs::PhonApply { inner, .. } => {
            collect_rhs_template_stems(&inner.node, stems);
        }
        RuleRhs::Delegate(d) => {
            // Stems passed via `with stems { target: source }` — source is used
            for mapping in &d.stem_mapping {
                if let StemSource::Stem(ident) = &mapping.source {
                    stems.insert(ident.node.clone());
                }
            }
        }
        RuleRhs::Null => {}
    }
}

/// `shadowed-rule`: a rule that can never win because a higher-specificity rule
/// matches every cell it would match.
fn lint_shadowed_rule(
    p1: &Phase1Result,
    axis_values: &HashMap<String, Vec<String>>,
    lints: &mut Vec<LintDiagnostic>,
) {
    for file in p1.files.values() {
        for item in &file.items {
            match &item.node {
                Item::Inflection(infl) => {
                    let axes: Vec<String> = infl.axes.iter().map(|a| a.node.clone()).collect();
                    check_shadowed_in_body(&infl.body, &axes, axis_values, lints);
                }
                Item::Entry(entry) => {
                    if let Some(EntryInflection::Inline(inline)) = &entry.inflection {
                        let axes: Vec<String> = inline.axes.iter().map(|a| a.node.clone()).collect();
                        check_shadowed_in_body(&inline.body, &axes, axis_values, lints);
                    }
                }
                _ => {}
            }
        }
    }
}

fn check_shadowed_in_body(
    body: &InflectionBody,
    axes: &[String],
    axis_values: &HashMap<String, Vec<String>>,
    lints: &mut Vec<LintDiagnostic>,
) {
    match body {
        InflectionBody::Rules(rules) => {
            check_shadowed_rules(rules, axes, axis_values, lints);
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                check_shadowed_rules(&slot.rules, axes, axis_values, lints);
            }
            check_shadowed_rules(&comp.overrides, axes, axis_values, lints);
        }
    }
}

fn check_shadowed_rules(
    rules: &[InflectionRule],
    axes: &[String],
    axis_values: &HashMap<String, Vec<String>>,
    lints: &mut Vec<LintDiagnostic>,
) {

    let cells = match inflection_eval::enumerate_cells(axes, &axis_values) {
        Ok(c) => c,
        Err(_) => return,
    };

    // For each rule, check if every cell it matches is also matched by a
    // strictly higher-specificity rule.
    for (i, rule) in rules.iter().enumerate() {
        let spec_i = rule.condition.conditions.len();

        // Find all cells this rule matches
        let matching_cells: Vec<&inflection_eval::Cell> = cells
            .iter()
            .filter(|cell| condition_matches_cell(&rule.condition, cell))
            .collect();

        if matching_cells.is_empty() {
            continue;
        }

        // Check if every matching cell has a higher-specificity match from another rule
        let all_shadowed = matching_cells.iter().all(|cell| {
            rules.iter().enumerate().any(|(j, other)| {
                j != i
                    && other.condition.conditions.len() > spec_i
                    && condition_matches_cell(&other.condition, cell)
            })
        });

        if all_shadowed {
            lints.push(LintDiagnostic {
                rule: "shadowed-rule",
                diagnostic: Diagnostic::warning(
                    "rule is shadowed: every matching cell is covered by a higher-specificity rule",
                )
                .with_label(rule.condition.span, "this rule never wins"),
                fix: None,
            });
        }
    }
}

/// `incomplete-coverage`: inflection rules do not cover all cells in the paradigm.
fn lint_incomplete_coverage(
    p1: &Phase1Result,
    axis_values: &HashMap<String, Vec<String>>,
    lints: &mut Vec<LintDiagnostic>,
) {
    for file in p1.files.values() {
        for item in &file.items {
            if let Item::Inflection(infl) = &item.node {
                if let InflectionBody::Rules(rules) = &infl.body {
                    let axes: Vec<String> = infl.axes.iter().map(|a| a.node.clone()).collect();
                    check_incomplete_coverage(rules, &axes, axis_values, &infl.name, lints);
                }
            }
            if let Item::Entry(entry) = &item.node {
                if let Some(EntryInflection::Inline(inline)) = &entry.inflection {
                    if let InflectionBody::Rules(rules) = &inline.body {
                        let axes: Vec<String> = inline.axes.iter().map(|a| a.node.clone()).collect();
                        check_incomplete_coverage(rules, &axes, axis_values, &entry.name, lints);
                    }
                }
            }
        }
    }
}

fn check_incomplete_coverage(
    rules: &[InflectionRule],
    axes: &[String],
    axis_values: &HashMap<String, Vec<String>>,
    name: &Ident,
    lints: &mut Vec<LintDiagnostic>,
) {

    let cells = match inflection_eval::enumerate_cells(axes, &axis_values) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut uncovered: Vec<String> = Vec::new();
    for cell in &cells {
        let matched = rules
            .iter()
            .any(|rule| condition_matches_cell(&rule.condition, cell));
        if !matched {
            let desc: Vec<String> = axes
                .iter()
                .filter_map(|a| cell.tags.get(a).map(|v| format!("{}={}", a, v)))
                .collect();
            uncovered.push(desc.join(", "));
        }
    }

    if !uncovered.is_empty() {
        let max_show = 3;
        let shown: Vec<&str> = uncovered.iter().take(max_show).map(|s| s.as_str()).collect();
        let mut msg = format!(
            "inflection `{}` has {} uncovered cell(s): [{}]",
            name.node,
            uncovered.len(),
            shown.join("], [")
        );
        if uncovered.len() > max_show {
            msg.push_str(&format!(" and {} more", uncovered.len() - max_show));
        }
        lints.push(LintDiagnostic {
            rule: "incomplete-coverage",
            diagnostic: Diagnostic::warning(msg).with_label(name.span, "defined here"),
            fix: None,
        });
    }
}

/// Check if a rule's condition matches a cell (reimplemented here to avoid
/// depending on inflection_eval internals).
fn condition_matches_cell(condition: &TagConditionList, cell: &inflection_eval::Cell) -> bool {
    for cond in &condition.conditions {
        match cell.tags.get(&cond.axis.node) {
            Some(v) if v == &cond.value.node => {}
            _ => return false,
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Name collection helpers
// ---------------------------------------------------------------------------

struct NameCollector {
    used: HashSet<String>,
}

impl NameCollector {
    fn new() -> Self {
        Self {
            used: HashSet::new(),
        }
    }
}

impl Visitor for NameCollector {
    fn visit_extend(&mut self, extend: &Extend) {
        self.used.insert(extend.target_axis.node.clone());
        visit::walk_extend(self, extend);
    }

    fn visit_inflection(&mut self, inflection: &Inflection) {
        for axis in &inflection.axes {
            self.used.insert(axis.node.clone());
        }
        visit::walk_inflection(self, inflection);
    }

    fn visit_inflection_rule(&mut self, rule: &InflectionRule) {
        for cond in &rule.condition.conditions {
            self.used.insert(cond.axis.node.clone());
        }
        match &rule.rhs.node {
            RuleRhs::Delegate(d) => {
                self.used.insert(d.target.node.clone());
            }
            RuleRhs::PhonApply { rule: pr, .. } => {
                self.used.insert(pr.node.clone());
            }
            _ => {}
        }
    }

    fn visit_entry(&mut self, entry: &Entry) {
        if let Some(EntryInflection::Class(cls)) = &entry.inflection {
            self.used.insert(cls.node.clone());
        }
        for tag in &entry.tags {
            self.used.insert(tag.axis.node.clone());
        }
        visit::walk_entry(self, entry);
    }

    fn visit_entry_ref(&mut self, entry_ref: &EntryRef) {
        self.used.insert(entry_ref.entry_id.node.clone());
    }

    fn visit_export(&mut self, export: &Export) {
        // Names referenced in @export (form 1, no path) count as "used"
        // so that the backing @use/@reference is not flagged as unused.
        if export.path.is_none() {
            if let ImportTarget::Named(entries) = &export.target {
                for entry in entries {
                    self.used.insert(entry.name.node.clone());
                }
            }
        }
    }
}

fn collect_used_names(file: &File) -> HashSet<String> {
    let mut collector = NameCollector::new();
    collector.visit_file(file);
    collector.used
}

// ---------------------------------------------------------------------------
// Diagnostic helper
// ---------------------------------------------------------------------------

impl Diagnostic {
    /// Create a warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            labels: Vec::new(),
        }
    }
}
