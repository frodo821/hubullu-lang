//! Inflection paradigm evaluator.
//!
//! Expands inflection rules into concrete word forms. Supports:
//! - **Rule-based** paradigms: cartesian product of axis values, best-match rule selection
//! - **Compose** paradigms: agglutinative slot concatenation with override rules
//! - **Delegation**: forwarding to another inflection class with tag/stem remapping

use std::collections::HashMap;

use crate::ast::*;
use crate::error::Diagnostic;

/// A single cell in the paradigm (one combination of axis values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// axis_name -> value_name
    pub tags: HashMap<String, String>,
}

/// Result of evaluating a paradigm for one cell.
#[derive(Debug, Clone)]
pub enum CellResult {
    /// A form string was produced.
    Form(String),
    /// The cell is null (form doesn't exist).
    Null,
}

/// Expanded paradigm: all cells resolved.
#[derive(Debug)]
pub struct ExpandedParadigm {
    pub forms: Vec<(Cell, CellResult)>,
}

/// Enumerate all cells (cartesian product of axis values).
pub fn enumerate_cells(
    axes: &[String],
    axis_values: &HashMap<String, Vec<String>>,
) -> Result<Vec<Cell>, Diagnostic> {
    let mut cells = vec![Cell {
        tags: HashMap::new(),
    }];

    for axis in axes {
        let values = axis_values.get(axis).ok_or_else(|| {
            Diagnostic::error(format!("axis '{}' has no values defined", axis))
        })?;
        if values.is_empty() {
            return Err(Diagnostic::error(format!(
                "axis '{}' has no values",
                axis
            )));
        }
        let mut new_cells = Vec::new();
        for cell in &cells {
            for value in values {
                let mut new_cell = cell.clone();
                new_cell.tags.insert(axis.clone(), value.clone());
                new_cells.push(new_cell);
            }
        }
        cells = new_cells;
    }

    Ok(cells)
}

/// Check if a rule's condition matches a cell.
fn condition_matches(condition: &TagConditionList, cell: &Cell) -> bool {
    for cond in &condition.conditions {
        match cell.tags.get(&cond.axis.node) {
            Some(v) if v == &cond.value.node => {}
            _ => return false,
        }
    }
    true
}

/// Specificity of a rule = number of explicit conditions.
fn specificity(condition: &TagConditionList) -> usize {
    condition.conditions.len()
}

/// Find the best matching rule for a cell.
/// Returns error if ambiguous (multiple rules with same specificity).
fn find_best_match<'a>(
    rules: &'a [InflectionRule],
    cell: &Cell,
) -> Result<Option<&'a InflectionRule>, Diagnostic> {
    let mut best: Option<(usize, &InflectionRule)> = None;
    let mut ambiguous = false;

    for rule in rules {
        if condition_matches(&rule.condition, cell) {
            let spec = specificity(&rule.condition);
            match &best {
                None => {
                    best = Some((spec, rule));
                }
                Some((best_spec, _)) => {
                    if spec > *best_spec {
                        best = Some((spec, rule));
                        ambiguous = false;
                    } else if spec == *best_spec {
                        ambiguous = true;
                    }
                }
            }
        }
    }

    if ambiguous {
        if let Some((_, rule)) = &best {
            return Err(Diagnostic::error("ambiguous rule match")
                .with_label(rule.condition.span, "multiple rules match with same specificity"));
        }
    }

    Ok(best.map(|(_, r)| r))
}

/// Callback for resolving delegate inflections.
pub trait DelegateResolver {
    /// Given a delegate target name, return its axes and body.
    fn resolve(&self, name: &str) -> Option<(Vec<String>, InflectionBody)>;
    /// Given axis name, return its values.
    fn axis_values(&self, axis: &str) -> Vec<String>;
}

/// No-op resolver for contexts without delegation support.
pub struct NullResolver;
impl DelegateResolver for NullResolver {
    fn resolve(&self, _name: &str) -> Option<(Vec<String>, InflectionBody)> { None }
    fn axis_values(&self, _axis: &str) -> Vec<String> { Vec::new() }
}

/// Evaluate a simple rule-based paradigm.
pub fn evaluate_rules(
    rules: &[InflectionRule],
    cells: &[Cell],
    stems: &HashMap<String, String>,
    struct_stems: &HashMap<String, HashMap<String, String>>,
    resolver: &dyn DelegateResolver,
) -> Result<ExpandedParadigm, Vec<Diagnostic>> {
    let mut forms = Vec::new();
    let mut errors = Vec::new();

    for cell in cells {
        match find_best_match(rules, cell) {
            Ok(Some(rule)) => match &rule.rhs.node {
                RuleRhs::Template(tmpl) => {
                    match render_template(tmpl, stems, struct_stems) {
                        Ok(s) => forms.push((cell.clone(), CellResult::Form(s))),
                        Err(e) => errors.push(e),
                    }
                }
                RuleRhs::Null => {
                    forms.push((cell.clone(), CellResult::Null));
                }
                RuleRhs::Delegate(deleg) => {
                    match resolve_delegate(deleg, cell, stems, struct_stems, resolver) {
                        Ok(result) => forms.push((cell.clone(), result)),
                        Err(e) => errors.push(e),
                    }
                }
            },
            Ok(None) => {
                let tag_desc = cell
                    .tags
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ");
                errors.push(Diagnostic::error(format!(
                    "no rule matches cell [{}]",
                    tag_desc
                )));
            }
            Err(e) => errors.push(e),
        }
    }

    if errors.is_empty() {
        Ok(ExpandedParadigm { forms })
    } else {
        Err(errors)
    }
}

/// Resolve a delegation for a single cell.
fn resolve_delegate(
    deleg: &Delegate,
    cell: &Cell,
    stems: &HashMap<String, String>,
    struct_stems: &HashMap<String, HashMap<String, String>>,
    resolver: &dyn DelegateResolver,
) -> Result<CellResult, Diagnostic> {
    let target_name = &deleg.target.node;

    let (target_axes, target_body) = resolver.resolve(target_name).ok_or_else(|| {
        Diagnostic::error(format!("delegate target '{}' not found", target_name))
            .with_label(deleg.target.span, "not found")
    })?;

    // Build the delegate cell: map tags from caller to target
    let mut delegate_cell = Cell { tags: HashMap::new() };
    for tag in &deleg.tags {
        match tag {
            DelegateTag::Fixed(tc) => {
                delegate_cell.tags.insert(tc.axis.node.clone(), tc.value.node.clone());
            }
            DelegateTag::PassThrough(axis) => {
                if let Some(val) = cell.tags.get(&axis.node) {
                    delegate_cell.tags.insert(axis.node.clone(), val.clone());
                }
            }
        }
    }

    // Build delegate stems: map from caller stems
    let mut delegate_stems = HashMap::new();
    for mapping in &deleg.stem_mapping {
        if let Some(val) = stems.get(&mapping.source_stem.node) {
            delegate_stems.insert(mapping.target_stem.node.clone(), val.clone());
        }
    }

    // Evaluate the target body for this single cell
    let cells = vec![delegate_cell];
    let result = match &target_body {
        InflectionBody::Rules(rules) => {
            evaluate_rules(rules, &cells, &delegate_stems, struct_stems, resolver)
        }
        InflectionBody::Compose(comp) => {
            evaluate_compose(comp, &cells, &delegate_stems, struct_stems)
        }
    };

    match result {
        Ok(paradigm) => {
            if let Some((_, cell_result)) = paradigm.forms.into_iter().next() {
                Ok(cell_result)
            } else {
                Err(Diagnostic::error(format!(
                    "delegate '{}' produced no result",
                    target_name
                )))
            }
        }
        Err(errs) => Err(errs.into_iter().next().unwrap_or_else(|| {
            Diagnostic::error(format!("delegate '{}' evaluation failed", target_name))
        })),
    }
}

/// Evaluate a compose-based paradigm.
pub fn evaluate_compose(
    compose: &ComposeBody,
    cells: &[Cell],
    stems: &HashMap<String, String>,
    struct_stems: &HashMap<String, HashMap<String, String>>,
) -> Result<ExpandedParadigm, Vec<Diagnostic>> {
    let mut forms = Vec::new();
    let mut errors = Vec::new();

    for cell in cells {
        // Check overrides first
        if let Ok(Some(rule)) = find_best_match(&compose.overrides, cell) {
            match &rule.rhs.node {
                RuleRhs::Template(tmpl) => {
                    match render_template(tmpl, stems, struct_stems) {
                        Ok(s) => forms.push((cell.clone(), CellResult::Form(s))),
                        Err(e) => errors.push(e),
                    }
                    continue;
                }
                RuleRhs::Null => {
                    forms.push((cell.clone(), CellResult::Null));
                    continue;
                }
                _ => {}
            }
        }

        // Compose the chain
        let mut composed = String::new();
        let mut all_resolved = true;

        for slot_ref in &compose.chain {
            let slot_name = &slot_ref.node;

            // Check if it's a stem
            if let Some(stem_val) = stems.get(slot_name) {
                composed.push_str(stem_val);
                continue;
            }

            // Find the slot definition
            if let Some(slot_def) = compose.slots.iter().find(|s| s.name.node == *slot_name) {
                match find_best_match(&slot_def.rules, cell) {
                    Ok(Some(rule)) => match &rule.rhs.node {
                        RuleRhs::Template(tmpl) => {
                            match render_template(tmpl, stems, struct_stems) {
                                Ok(s) => composed.push_str(&s),
                                Err(e) => {
                                    errors.push(e);
                                    all_resolved = false;
                                }
                            }
                        }
                        RuleRhs::Null => {
                            forms.push((cell.clone(), CellResult::Null));
                            all_resolved = false;
                            break;
                        }
                        _ => {
                            all_resolved = false;
                        }
                    },
                    Ok(None) => {
                        let tag_desc = cell
                            .tags
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ");
                        errors.push(Diagnostic::error(format!(
                            "no rule matches slot '{}' for cell [{}]",
                            slot_name, tag_desc
                        )));
                        all_resolved = false;
                    }
                    Err(e) => {
                        errors.push(e);
                        all_resolved = false;
                    }
                }
            } else {
                errors.push(Diagnostic::error(format!(
                    "slot '{}' not defined",
                    slot_name
                )));
                all_resolved = false;
            }
        }

        if all_resolved && !forms.iter().any(|(c, _)| c == cell) {
            forms.push((cell.clone(), CellResult::Form(composed)));
        }
    }

    if errors.is_empty() {
        Ok(ExpandedParadigm { forms })
    } else {
        Err(errors)
    }
}

/// Render a template with stem values.
pub fn render_template(
    template: &Template,
    stems: &HashMap<String, String>,
    struct_stems: &HashMap<String, HashMap<String, String>>,
) -> Result<String, Diagnostic> {
    let mut result = String::new();
    for seg in &template.segments {
        match seg {
            TemplateSegment::Lit(s) => result.push_str(s),
            TemplateSegment::Stem(name) => {
                if let Some(val) = stems.get(&name.node) {
                    result.push_str(val);
                } else {
                    return Err(Diagnostic::error(format!(
                        "undefined stem '{}'",
                        name.node
                    ))
                    .with_label(name.span, "not found in stems"));
                }
            }
            TemplateSegment::Slot { stem, slot } => {
                if let Some(slots) = struct_stems.get(&stem.node) {
                    if let Some(val) = slots.get(&slot.node) {
                        result.push_str(val);
                    } else {
                        return Err(Diagnostic::error(format!(
                            "undefined slot '{}.{}'",
                            stem.node, slot.node
                        ))
                        .with_label(slot.span, "slot not found"));
                    }
                } else {
                    return Err(Diagnostic::error(format!(
                        "undefined structural stem '{}'",
                        stem.node
                    ))
                    .with_label(stem.span, "not found"));
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    fn make_span() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn make_ident(s: &str) -> Ident {
        Spanned::new(s.to_string(), make_span())
    }

    #[test]
    fn test_enumerate_cells() {
        let axes = vec!["tense".to_string(), "number".to_string()];
        let mut axis_values = HashMap::new();
        axis_values.insert("tense".to_string(), vec!["present".to_string(), "past".to_string()]);
        axis_values.insert("number".to_string(), vec!["sg".to_string(), "pl".to_string()]);

        let cells = enumerate_cells(&axes, &axis_values).unwrap();
        assert_eq!(cells.len(), 4);
    }

    #[test]
    fn test_evaluate_simple_rules() {
        let span = make_span();
        let rules = vec![
            InflectionRule {
                condition: TagConditionList {
                    conditions: vec![TagCondition {
                        axis: make_ident("tense"),
                        value: make_ident("present"),
                    }],
                    wildcard: true,
                    span,
                },
                rhs: Spanned::new(
                    RuleRhs::Template(Template {
                        segments: vec![
                            TemplateSegment::Stem(make_ident("root")),
                            TemplateSegment::Lit("s".to_string()),
                        ],
                        span,
                    }),
                    span,
                ),
            },
            InflectionRule {
                condition: TagConditionList {
                    conditions: vec![TagCondition {
                        axis: make_ident("tense"),
                        value: make_ident("past"),
                    }],
                    wildcard: true,
                    span,
                },
                rhs: Spanned::new(RuleRhs::Null, span),
            },
        ];

        let cells = vec![
            Cell { tags: [("tense".to_string(), "present".to_string())].into() },
            Cell { tags: [("tense".to_string(), "past".to_string())].into() },
        ];

        let stems: HashMap<String, String> = [("root".to_string(), "walk".to_string())].into();
        let struct_stems = HashMap::new();

        let result = evaluate_rules(&rules, &cells, &stems, &struct_stems, &NullResolver).unwrap();
        assert_eq!(result.forms.len(), 2);
        assert!(matches!(&result.forms[0].1, CellResult::Form(s) if s == "walks"));
        assert!(matches!(&result.forms[1].1, CellResult::Null));
    }
}
