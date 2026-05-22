//! Per-slot validation for the `compose + slot ... matching` morphology.
//!
//! After the slot-morphology reshape this module is no longer a top-level
//! grammar parser — the compose chain's `lazy* eager* lazy*` layout is checked
//! statically in phase2 (`validate_compose_layout`), and recursion happens
//! naturally via entry refs (`slot=cl_lex[..][slot=..]`), not via nested slot
//! types. What remains here is the small per-slot kit used by render and by
//! phase2's surface checks:
//!
//! - [`fits`]              — does a morpheme satisfy a `LazyMatching` filter?
//! - [`check_quantifier`]  — are the supplied filler counts within bounds?
//! - [`validate_slot_fill`] — combines fit + quantifier + catch-all warning.
//!
//! The §10.2 catch-all warning fires when a morpheme placed in a `CatchAll`
//! lazy slot would also have fit some eager-core typed slot in the same
//! inflection — suppressed when the morpheme's `is_peripheral` flag is set.
//!
//! Render-time assembly (Phase 4) and forms-emission (Phase 7) will both
//! consume these helpers; for now the module is consumed only by its own unit
//! tests.

use crate::ast::{AxisConstraint, AxisFilter, LazyMatching, SlotQuantifier};
use crate::error::Diagnostic;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One concrete morpheme in the render-time input stream.
///
/// `tags` is a list of `(axis, value)` pairs — the grammatical features this
/// morpheme carries. A morpheme "fits" a `Filter([..])` slot iff it satisfies
/// **every** filter (AND semantics); a `CatchAll` slot accepts any morpheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphemeInstance {
    /// The entry id of the morpheme entry this instance came from.
    pub entry_id: String,
    /// The rendered surface form of the morpheme.
    pub surface: String,
    /// `(axis, value)` feature pairs carried by this morpheme.
    pub tags: Vec<(String, String)>,
    /// Mirrors the morpheme entry's `is_peripheral` flag — suppresses the
    /// catch-all warning for this morpheme even when it carries a slot-defined
    /// axis (proposal §10.2).
    pub is_peripheral: bool,
}

impl MorphemeInstance {
    /// The set of tag axes this morpheme carries.
    fn axes(&self) -> impl Iterator<Item = &String> {
        self.tags.iter().map(|(axis, _)| axis)
    }
}

/// What ended up filling a single slot in the assembled render output.
///
/// Render builds one [`SlotFill`] per slot reference in the compose chain.
/// `Morpheme` is a fixed-count fill (1, for a non-variadic slot); `Stem`
/// covers the stem reference; `Variadic` holds an ordered run of fills for a
/// `*`/`+`/`{n,m}` slot; `Empty` is what a `?`/`*`/`{0,_}` slot yields when
/// it was supplied no fillers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotFill {
    /// A non-variadic slot consumed exactly one morpheme.
    Morpheme(MorphemeInstance),
    /// The stem reference consumed the entry's stem string.
    Stem(String),
    /// A variadic-quantifier slot swallowed an ordered run of fills.
    Variadic(Vec<SlotFill>),
    /// The slot consumed nothing (an unused `?`/`*` slot).
    Empty,
}

// ---------------------------------------------------------------------------
// Per-slot helpers
// ---------------------------------------------------------------------------

/// Whether `morpheme` satisfies `filter`.
///
/// - [`LazyMatching::CatchAll`] — always `true`.
/// - [`LazyMatching::Filter`]   — AND over the per-axis filters:
///   - [`AxisConstraint::Any`]        → morpheme carries *some* value on axis
///   - [`AxisConstraint::Eq(v)`]      → morpheme carries exactly `(axis, v)`
///   - [`AxisConstraint::OneOf(vs)`]  → morpheme carries `(axis, v')` for some `v' ∈ vs`
pub fn fits(morpheme: &MorphemeInstance, filter: &LazyMatching) -> bool {
    match filter {
        LazyMatching::CatchAll => true,
        LazyMatching::Filter(filters) => filters.iter().all(|f| fits_filter(morpheme, f)),
    }
}

fn fits_filter(morpheme: &MorphemeInstance, filter: &AxisFilter) -> bool {
    let axis = &filter.axis.node;
    match &filter.constraint {
        AxisConstraint::Any => morpheme.tags.iter().any(|(a, _)| a == axis),
        AxisConstraint::Eq(v) => morpheme
            .tags
            .iter()
            .any(|(a, val)| a == axis && val == &v.node),
        AxisConstraint::OneOf(vs) => morpheme.tags.iter().any(|(a, val)| {
            a == axis && vs.iter().any(|cand| &cand.node == val)
        }),
    }
}

/// Verify that `fillers.len()` is inside the bounds permitted by `q`.
///
/// Returns `Ok(())` when in range, or an error diagnostic (without a source
/// span — callers attach the slot's span).
pub fn check_quantifier(
    fillers_len: usize,
    q: SlotQuantifier,
    slot_name: &str,
) -> Result<(), Diagnostic> {
    let n = fillers_len as u32;
    let min = q.min();
    if n < min {
        return Err(Diagnostic::error(format!(
            "slot '{}': received {} filler{} but quantifier requires at least {}",
            slot_name,
            n,
            if n == 1 { "" } else { "s" },
            min
        )));
    }
    if let Some(max) = q.max() {
        if n > max {
            return Err(Diagnostic::error(format!(
                "slot '{}': received {} filler{} but quantifier allows at most {}",
                slot_name,
                n,
                if n == 1 { "" } else { "s" },
                max
            )));
        }
    }
    Ok(())
}

/// Combined per-slot fill validation: every supplied morpheme must `fits` the
/// lazy filter, the count must satisfy the compose-chain quantifier, and any
/// fill landing in a `CatchAll` slot that ALSO satisfies one of the inflection's
/// other eager-core typed filters earns a catch-all warning (suppressed when
/// `morpheme.is_peripheral`).
///
/// `eager_core_typed_axes` is the union of axes covered by the eager-core
/// typed slots in this inflection — used to detect over-supply that fell
/// through to the periphery.
pub fn validate_slot_fill(
    slot_name: &str,
    filter: &LazyMatching,
    quantifier: SlotQuantifier,
    fillers: &[MorphemeInstance],
    eager_core_typed_axes: &[String],
) -> ValidationOutcome {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for filler in fillers {
        if !fits(filler, filter) {
            errors.push(Diagnostic::error(format!(
                "slot '{}': morpheme '{}' does not satisfy the slot's `matching` filter",
                slot_name, filler.entry_id
            )));
        }
    }

    if let Err(d) = check_quantifier(fillers.len(), quantifier, slot_name) {
        errors.push(d);
    }

    // Catch-all warning: a CatchAll lazy slot that absorbs a morpheme bearing
    // an axis the inflection's eager core would normally structure.
    if matches!(filter, LazyMatching::CatchAll) {
        for filler in fillers {
            if filler.is_peripheral {
                continue;
            }
            let captured_axis = filler
                .axes()
                .find(|a| eager_core_typed_axes.iter().any(|core| core == *a));
            if let Some(axis) = captured_axis {
                warnings.push(Diagnostic::warning(format!(
                    "slot '{}': morpheme '{}' carries axis '{}' which an \
                     eager-core slot of this inflection structures — over-supply \
                     captured by the catch-all (set `is_peripheral: true` on \
                     the entry to suppress this warning)",
                    slot_name, filler.entry_id, axis
                )));
            }
        }
    }

    ValidationOutcome { errors, warnings }
}

/// The result of [`validate_slot_fill`]: any errors block the render; warnings
/// surface alongside.
#[derive(Debug, Clone, Default)]
pub struct ValidationOutcome {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, Span, Spanned};
    use crate::span::FileId;

    fn span() -> Span {
        Span {
            file_id: FileId(0),
            start: 0,
            end: 0,
        }
    }

    fn ident(s: &str) -> Ident {
        Spanned::new(s.to_string(), span())
    }

    fn morpheme(id: &str, tags: &[(&str, &str)], peripheral: bool) -> MorphemeInstance {
        MorphemeInstance {
            entry_id: id.to_string(),
            surface: id.to_string(),
            tags: tags
                .iter()
                .map(|(a, v)| (a.to_string(), v.to_string()))
                .collect(),
            is_peripheral: peripheral,
        }
    }

    fn any_filter(axis: &str) -> AxisFilter {
        AxisFilter {
            axis: ident(axis),
            constraint: AxisConstraint::Any,
        }
    }

    fn eq_filter(axis: &str, val: &str) -> AxisFilter {
        AxisFilter {
            axis: ident(axis),
            constraint: AxisConstraint::Eq(ident(val)),
        }
    }

    fn oneof_filter(axis: &str, vals: &[&str]) -> AxisFilter {
        AxisFilter {
            axis: ident(axis),
            constraint: AxisConstraint::OneOf(vals.iter().map(|v| ident(v)).collect()),
        }
    }

    // ----- fits ------------------------------------------------------------

    #[test]
    fn fits_catch_all_always_true() {
        let m = morpheme("x", &[], false);
        assert!(fits(&m, &LazyMatching::CatchAll));
    }

    #[test]
    fn fits_filter_any() {
        let m = morpheme("x", &[("tense", "past")], false);
        let f = LazyMatching::Filter(vec![any_filter("tense")]);
        assert!(fits(&m, &f));
        let m2 = morpheme("y", &[("person", "1")], false);
        assert!(!fits(&m2, &f));
    }

    #[test]
    fn fits_filter_eq() {
        let m_past = morpheme("p", &[("tense", "past")], false);
        let m_pres = morpheme("q", &[("tense", "present")], false);
        let f = LazyMatching::Filter(vec![eq_filter("tense", "past")]);
        assert!(fits(&m_past, &f));
        assert!(!fits(&m_pres, &f));
    }

    #[test]
    fn fits_filter_oneof() {
        let m1 = morpheme("a", &[("number", "sg")], false);
        let m2 = morpheme("b", &[("number", "du")], false);
        let m3 = morpheme("c", &[("number", "pl")], false);
        let f = LazyMatching::Filter(vec![oneof_filter("number", &["sg", "pl"])]);
        assert!(fits(&m1, &f));
        assert!(!fits(&m2, &f));
        assert!(fits(&m3, &f));
    }

    #[test]
    fn fits_filter_and_semantics() {
        // Filter has 2 axis filters; both must match.
        let f = LazyMatching::Filter(vec![any_filter("number"), eq_filter("tense", "past")]);
        let ok = morpheme("ok", &[("number", "sg"), ("tense", "past")], false);
        let no_tense = morpheme("nt", &[("number", "sg")], false);
        let no_number = morpheme("nn", &[("tense", "past")], false);
        let wrong_tense = morpheme("wt", &[("number", "sg"), ("tense", "present")], false);
        assert!(fits(&ok, &f));
        assert!(!fits(&no_tense, &f));
        assert!(!fits(&no_number, &f));
        assert!(!fits(&wrong_tense, &f));
    }

    // ----- check_quantifier ------------------------------------------------

    #[test]
    fn quantifier_one_requires_exactly_one() {
        assert!(check_quantifier(1, SlotQuantifier::One, "s").is_ok());
        assert!(check_quantifier(0, SlotQuantifier::One, "s").is_err());
        assert!(check_quantifier(2, SlotQuantifier::One, "s").is_err());
    }

    #[test]
    fn quantifier_zero_or_one() {
        assert!(check_quantifier(0, SlotQuantifier::ZeroOrOne, "s").is_ok());
        assert!(check_quantifier(1, SlotQuantifier::ZeroOrOne, "s").is_ok());
        assert!(check_quantifier(2, SlotQuantifier::ZeroOrOne, "s").is_err());
    }

    #[test]
    fn quantifier_zero_or_more() {
        for n in 0..5 {
            assert!(check_quantifier(n, SlotQuantifier::ZeroOrMore, "s").is_ok());
        }
    }

    #[test]
    fn quantifier_one_or_more() {
        assert!(check_quantifier(0, SlotQuantifier::OneOrMore, "s").is_err());
        for n in 1..5 {
            assert!(check_quantifier(n, SlotQuantifier::OneOrMore, "s").is_ok());
        }
    }

    #[test]
    fn quantifier_bounded() {
        let q = SlotQuantifier::Bounded { min: 2, max: 4 };
        assert!(check_quantifier(1, q, "s").is_err());
        assert!(check_quantifier(2, q, "s").is_ok());
        assert!(check_quantifier(4, q, "s").is_ok());
        assert!(check_quantifier(5, q, "s").is_err());
    }

    // ----- validate_slot_fill ---------------------------------------------

    #[test]
    fn validate_fit_error() {
        // Filler doesn't satisfy the filter → error.
        let f = LazyMatching::Filter(vec![eq_filter("tense", "past")]);
        let bad = morpheme("bad", &[("tense", "present")], false);
        let out = validate_slot_fill("s", &f, SlotQuantifier::One, &[bad], &[]);
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.warnings.len(), 0);
    }

    #[test]
    fn validate_quantifier_error() {
        // 0 fillers under a `One` quantifier → error.
        let f = LazyMatching::CatchAll;
        let out = validate_slot_fill("s", &f, SlotQuantifier::One, &[], &[]);
        assert_eq!(out.errors.len(), 1);
    }

    #[test]
    fn validate_catch_all_warning_on_structural_axis() {
        // CatchAll absorbs a morpheme that carries an eager-core axis → warning.
        let f = LazyMatching::CatchAll;
        let m = morpheme("os", &[("person", "3")], false);
        let out = validate_slot_fill(
            "enclitics",
            &f,
            SlotQuantifier::ZeroOrMore,
            &[m],
            &["person".to_string()],
        );
        assert!(out.errors.is_empty());
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn validate_catch_all_suppressed_by_is_peripheral() {
        // Same case, but the morpheme is is_peripheral → no warning.
        let f = LazyMatching::CatchAll;
        let m = morpheme("os", &[("person", "3")], true);
        let out = validate_slot_fill(
            "enclitics",
            &f,
            SlotQuantifier::ZeroOrMore,
            &[m],
            &["person".to_string()],
        );
        assert!(out.errors.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn validate_catch_all_axis_not_in_core_no_warning() {
        // A morpheme in a CatchAll whose tag axis is not covered by any
        // eager-core typed slot: no warning. (Quiet particle / clitic.)
        let f = LazyMatching::CatchAll;
        let m = morpheme("particle", &[("topic", "wa")], false);
        let out = validate_slot_fill(
            "enclitics",
            &f,
            SlotQuantifier::ZeroOrMore,
            &[m],
            &["person".to_string(), "tense".to_string()],
        );
        assert!(out.errors.is_empty());
        assert!(out.warnings.is_empty());
    }
}
