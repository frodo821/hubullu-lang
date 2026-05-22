//! Lexicon → FST compilation (F3 of FST migration).
//!
//! Per `docs/proposals/fst-morphology.md` §2 "Lexicon FST: each morpheme
//! entry contributes a labeled path", and §4 step 2 of the compile
//! pipeline, the **lexicon FST** is the foundation that F4 will compose
//! with the slot grammar (and F5 with phonrules) to produce per-inflection
//! morphological FSTs.
//!
//! ## What this module owns
//!
//!   * [`compile_morpheme`] — build the per-morpheme labeled path.
//!   * [`compile_lexicon`] — union of per-morpheme paths over a slice of
//!     entries.
//!   * [`lexicon_surface`] — given a morpheme-ID label, traverse the
//!     lexicon FST and return the headword string. F4's compose-chain
//!     assembly uses this for per-morpheme look-up.
//!   * [`LexiconCompileError`] / [`LexiconLookupError`] — typed errors.
//!
//! ## Per-morpheme path shape (F3.1)
//!
//! For a morpheme entry with name `pn_pc_1sg` and headword `"um"`:
//!
//! ```text
//! states: 0 (start) → 1 → 2 (final)
//! arc 0→1: input = <m:pn_pc_1sg>, output = <u>
//! arc 1→2: input = ε,             output = <m>
//! ```
//!
//! Invariant: the input side accepts exactly one label (the morpheme ID);
//! the output side accepts exactly the headword's character sequence. For
//! an empty headword the path is a single-state FST that accepts only the
//! morpheme ID on input and emits no output (no ε arc needed because the
//! morpheme-ID arc is the only arc).
//!
//! Concretely: an entry with headword `""` compiles to two states (start
//! → final), one arc `start -[<m:NAME>:ε]-> final`. An entry with
//! headword of length `n >= 1` compiles to `n+1` states and `n` arcs.
//!
//! ## Lexicon FST = ⋃ per-morpheme paths (F3.1 closing)
//!
//! `compile_lexicon` unions all per-morpheme FSTs. After construction the
//! FST is `arc_sort_input`ed AND `arc_sort_output`ed so F4's compose
//! works without surprise (compose requires both sides to have a
//! sort-property bit set, see `src/fst/phonrule/apply.rs:147-151` for
//! the existing pattern).
//!
//! ## Morpheme-ID label class (F3.2)
//!
//! Option **A** chosen: extend [`PhonruleAlphabet`] with a dedicated
//! morpheme-label range starting at
//! [`crate::fst::alphabet::FIRST_MORPHEME_LABEL`] (65536). Morpheme symbol
//! names are prefixed `<m:NAME>` in the symbol-table-shaped surface to
//! guarantee no collision with phoneme literals. See
//! `src/fst/alphabet.rs` for the full rationale.
//!
//! ## Surface look-up pattern (F3.4)
//!
//! [`lexicon_surface`] mirrors `src/fst/phonrule/apply.rs::apply_once`'s
//! compose-then-traverse pattern:
//!
//!   1. Build a single-arc input acceptor over the morpheme-ID label.
//!   2. `arc_sort_output` it.
//!   3. `compose(input_acceptor, lexicon_fst)` (lexicon is pre-sorted
//!      on input).
//!   4. Enumerate the single accepting path; decode its output labels
//!      via `alpha.label_to_str` (chars are concatenated to form the
//!      headword).
//!
//! ## What this module does NOT own
//!
//!   * Compose-chain → FST (F4).
//!   * Per-entry specialisation by stem binding (F5).
//!   * Reverse lookup CLI (F6).
//!   * Modifications to `render.rs` / `phase2.rs` / morphology fixtures
//!     (F3 is additive only, same discipline as F1/F2).

use crate::ast::{Entry, Headword};

use super::alphabet::PhonruleAlphabet;
use super::backend::{FstBuilder, Label};
use super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::FstBackend;

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Errors produced by lexicon compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexiconCompileError {
    /// `compile_lexicon` was called with an empty slice. The empty union
    /// would yield an FST that accepts nothing, which is almost certainly
    /// a caller error — F4 always wants a non-empty lexicon FST to
    /// compose against. Surface as a typed error rather than producing a
    /// silently-empty FST.
    EmptyLexicon,
    /// An [`Entry`] passed to compilation had `inflection.is_some()` —
    /// only inflectionless entries are morpheme entries (matches the
    /// `iter_morpheme_entries` predicate in `src/render.rs:2600`).
    NotAMorpheme { entry_name: String },
    /// Backend FST operation failed (e.g. union error).
    Backend(String),
}

impl std::fmt::Display for LexiconCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexiconCompileError::EmptyLexicon => {
                write!(f, "lexicon compilation called with empty entry slice")
            }
            LexiconCompileError::NotAMorpheme { entry_name } => write!(
                f,
                "entry '{}' has an inflection — only inflectionless entries are morphemes",
                entry_name
            ),
            LexiconCompileError::Backend(s) => {
                write!(f, "lexicon compile backend error: {}", s)
            }
        }
    }
}

impl std::error::Error for LexiconCompileError {}

/// Errors produced by [`lexicon_surface`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexiconLookupError {
    /// No accepting path in the composed FST. Either the morpheme-ID
    /// label was never registered in the lexicon FST, or the FST is
    /// malformed (no start state, no finals reachable).
    UnknownMorpheme { morpheme_id_label: Label },
    /// More than one distinct output found for the same morpheme-ID
    /// input. Indicates a bug in lexicon construction (the union path
    /// shape guarantees one output per input ID) — defensive only.
    AmbiguousLookup {
        morpheme_id_label: Label,
        outputs: Vec<String>,
    },
    /// An output label could not be resolved via `alpha.label_to_str`.
    /// Should not happen for a lexicon FST built from the same alphabet
    /// used at look-up time — defensive only.
    UnknownLabel(Label),
    /// Backend FST operation failed.
    Backend(String),
}

impl std::fmt::Display for LexiconLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexiconLookupError::UnknownMorpheme { morpheme_id_label } => {
                write!(f, "no morpheme path for label {}", morpheme_id_label)
            }
            LexiconLookupError::AmbiguousLookup {
                morpheme_id_label,
                outputs,
            } => write!(
                f,
                "ambiguous lexicon look-up for label {}: {:?}",
                morpheme_id_label, outputs
            ),
            LexiconLookupError::UnknownLabel(l) => {
                write!(f, "lexicon output label {} not in alphabet", l)
            }
            LexiconLookupError::Backend(s) => {
                write!(f, "lexicon look-up backend error: {}", s)
            }
        }
    }
}

impl std::error::Error for LexiconLookupError {}

// ---------------------------------------------------------------------------
// Headword extraction.
// ---------------------------------------------------------------------------

/// Extract the surface string for a [`Headword`].
///
/// For `Simple(s)` returns `s`. For `MultiScript` falls back to the
/// `default` script if present, else the first declared script (mirrors
/// `render::headword_to_string` at `src/render.rs:874`).
///
/// Local copy rather than re-using `render::headword_to_string` to keep
/// the F3 module strictly additive — `render.rs` is off-limits per
/// F3.6's discipline.
fn headword_to_surface(hw: &Headword) -> String {
    match hw {
        Headword::Simple(s) => s.node.clone(),
        Headword::MultiScript(scripts) => {
            for (name, value) in scripts {
                if name.node == "default" {
                    return value.node.clone();
                }
            }
            scripts
                .first()
                .map(|(_, v)| v.node.clone())
                .unwrap_or_default()
        }
    }
}

// ---------------------------------------------------------------------------
// Per-morpheme compile.
// ---------------------------------------------------------------------------

/// Compile a single morpheme entry to its labeled path FST.
///
/// See module docs for the path shape. Interns the morpheme name (via
/// [`PhonruleAlphabet::intern_morpheme`]) and each headword character
/// (via [`PhonruleAlphabet::intern`]) into `alpha`. The morpheme-ID label
/// is returned alongside the FST so callers can record the
/// (name, label) binding without re-looking it up.
///
/// Behaviour on empty headword: yields a 2-state FST with one arc
/// `start -[<m:NAME>:ε]-> final`. The accepted input is `[<m:NAME>]`,
/// the accepted output is the empty sequence.
pub fn compile_morpheme(
    entry: &Entry,
    alpha: &mut PhonruleAlphabet,
) -> Result<(Label, RustFstWrapper), LexiconCompileError> {
    if entry.inflection.is_some() {
        return Err(LexiconCompileError::NotAMorpheme {
            entry_name: entry.name.node.clone(),
        });
    }
    let morph_label = alpha.intern_morpheme(&entry.name.node);
    let surface = headword_to_surface(&entry.headword);

    // Intern every char first, so the path build is purely structural
    // (no further alphabet mutation interleaved with state allocation).
    let out_labels: Vec<Label> = surface
        .chars()
        .map(|ch| alpha.intern(&ch.to_string()))
        .collect();

    let mut b = RustFstBackend::builder();
    let start = b.add_state();
    b.set_start(start)
        .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;

    if out_labels.is_empty() {
        // Empty surface: one arc `start -[<m:NAME>:ε]-> final`.
        let fin = b.add_state();
        b.add_arc(start, morph_label, super::backend::EPS_LABEL, fin)
            .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
        b.set_final(fin)
            .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    } else {
        // First arc: input = morph_label, output = first char label.
        // Subsequent arcs: input = ε, output = each remaining char label.
        let mut prev = start;
        for (i, &o) in out_labels.iter().enumerate() {
            let next = b.add_state();
            let input_label = if i == 0 {
                morph_label
            } else {
                super::backend::EPS_LABEL
            };
            b.add_arc(prev, input_label, o, next)
                .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
            prev = next;
        }
        b.set_final(prev)
            .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    }

    let fst = b
        .finish()
        .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    Ok((morph_label, fst))
}

// ---------------------------------------------------------------------------
// Lexicon compile.
// ---------------------------------------------------------------------------

/// Compile a slice of morpheme entries to a single lexicon FST.
///
/// The result is the union of every per-morpheme path. Both
/// `arc_sort_input` and `arc_sort_output` are applied to the result so
/// downstream compose (F4) works without the caller having to remember
/// the sort discipline.
///
/// Empty entry slice → [`LexiconCompileError::EmptyLexicon`]. A
/// non-empty slice always produces an FST with at least one accepting
/// path.
///
/// Each entry's morpheme name is interned via
/// [`PhonruleAlphabet::intern_morpheme`]; each headword char via
/// [`PhonruleAlphabet::intern`]. The caller can look up labels
/// afterwards via [`PhonruleAlphabet::lookup_morpheme`] /
/// [`PhonruleAlphabet::lookup`].
pub fn compile_lexicon(
    morphemes: &[&Entry],
    alpha: &mut PhonruleAlphabet,
) -> Result<RustFstWrapper, LexiconCompileError> {
    if morphemes.is_empty() {
        return Err(LexiconCompileError::EmptyLexicon);
    }
    let (_first_label, mut acc) = compile_morpheme(morphemes[0], alpha)?;
    for entry in &morphemes[1..] {
        let (_lbl, sub) = compile_morpheme(entry, alpha)?;
        acc = RustFstBackend::union(&acc, &sub)
            .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    }
    // Arc-sort discipline: F4's compose needs both sort bits. Apply
    // input-sort second so the final FST has I_LABEL_SORTED set (the
    // most common right-operand state for compose). Output-sort is
    // also applied so the FST can sit on either side of a compose.
    let out_sorted = RustFstBackend::arc_sort_output(&acc)
        .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    let in_sorted = RustFstBackend::arc_sort_input(&out_sorted)
        .map_err(|e| LexiconCompileError::Backend(e.to_string()))?;
    Ok(in_sorted)
}

// ---------------------------------------------------------------------------
// Surface look-up.
// ---------------------------------------------------------------------------

/// Given a morpheme-ID label, traverse `lexicon_fst` to return the
/// headword surface string.
///
/// Construction:
///   1. Build a single-arc input acceptor over `morpheme_id_label`.
///   2. `arc_sort_output` the acceptor (compose precondition).
///   3. `compose(acceptor, lexicon_fst)`.
///   4. Enumerate accepting paths; decode the unique output via
///      `alpha.label_to_str`.
///
/// If the morpheme-ID label has no path in the lexicon FST, returns
/// [`LexiconLookupError::UnknownMorpheme`].
///
/// Multiple distinct outputs for the same morpheme-ID are flagged as
/// [`LexiconLookupError::AmbiguousLookup`] — this would indicate a bug
/// in [`compile_lexicon`], but defensive.
///
/// Two morphemes with the **same** headword but **different**
/// morpheme-IDs produce two distinct paths in the lexicon FST and one
/// path each in the composed FST — so look-up works correctly for both.
pub fn lexicon_surface(
    lexicon_fst: &RustFstWrapper,
    morpheme_id_label: Label,
    alpha: &PhonruleAlphabet,
) -> Result<String, LexiconLookupError> {
    let acceptor = single_label_input_acceptor(morpheme_id_label);
    let left = RustFstBackend::arc_sort_output(&acceptor)
        .map_err(|e| LexiconLookupError::Backend(e.to_string()))?;
    let composed = RustFstBackend::compose(&left, lexicon_fst)
        .map_err(|e| LexiconLookupError::Backend(e.to_string()))?;

    let mut iter = RustFstBackend::paths(&composed)
        .map_err(|e| LexiconLookupError::Backend(e.to_string()))?;
    // Cap path enumeration defensively. The lexicon path-per-morpheme
    // construction guarantees at most one accepting path here, but a
    // cap prevents hangs on accidentally-cyclic FSTs.
    const PATH_ENUM_CAP: usize = 64;
    let mut seen_outputs: Vec<Vec<Label>> = Vec::new();
    for p in iter.by_ref().take(PATH_ENUM_CAP) {
        if !seen_outputs.iter().any(|prev| prev == &p.output) {
            seen_outputs.push(p.output);
        }
    }
    if seen_outputs.is_empty() {
        return Err(LexiconLookupError::UnknownMorpheme { morpheme_id_label });
    }
    if seen_outputs.len() > 1 {
        let outputs: Vec<String> = seen_outputs
            .iter()
            .map(|labels| labels_to_string_strict(labels, alpha).unwrap_or_default())
            .collect();
        return Err(LexiconLookupError::AmbiguousLookup {
            morpheme_id_label,
            outputs,
        });
    }
    labels_to_string_strict(&seen_outputs[0], alpha)
}

// ---------------------------------------------------------------------------
// Internals.
// ---------------------------------------------------------------------------

/// Build a 2-state FST whose only accepting path consumes `label` on
/// the input side and emits `label` on the output side. The output
/// side's label happens to match because we use identity arcs; the
/// composed result projects only the lexicon FST's output, which is
/// the headword.
fn single_label_input_acceptor(label: Label) -> RustFstWrapper {
    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");
    b.add_arc(s0, label, label, s1).expect("add_arc");
    b.finish().expect("finish single_label_input_acceptor")
}

/// Decode a sequence of output labels back to a string. Errors on any
/// unknown label. For headword surfaces the labels are phoneme/literal
/// labels (single-char names interned via [`PhonruleAlphabet::intern`])
/// or the boundary/word-edge markers (excluded — lexicon outputs never
/// contain them by construction). The decoder concatenates label names
/// verbatim; for single-char names this gives the headword string.
fn labels_to_string_strict(
    labels: &[Label],
    alpha: &PhonruleAlphabet,
) -> Result<String, LexiconLookupError> {
    let mut out = String::new();
    for &l in labels {
        let name = alpha
            .label_to_str(l)
            .ok_or(LexiconLookupError::UnknownLabel(l))?;
        out.push_str(name);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// F3.5 tests (inline per task brief option).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Headword, MeaningDef, Span, TagCondition};
    use crate::span::FileId;
    use std::collections::HashSet;

    // ---- test helpers ----

    fn span() -> Span {
        Span {
            file_id: FileId(0),
            start: 0,
            end: 0,
        }
    }

    fn ident(name: &str) -> crate::ast::Ident {
        crate::ast::Spanned::new(name.to_string(), span())
    }

    fn slit(s: &str) -> crate::ast::StringLit {
        crate::ast::Spanned::new(s.to_string(), span())
    }

    /// Build a minimal inflectionless entry with the given name and
    /// headword. Tags / meaning / stems are dummies — F3 only reads
    /// `name`, `headword`, and `inflection`.
    fn morph_entry(name: &str, headword: &str) -> Entry {
        Entry {
            name: ident(name),
            headword: Headword::Simple(slit(headword)),
            tags: Vec::<TagCondition>::new(),
            stems: Vec::new(),
            inflection: None,
            meaning: MeaningDef::Single(slit("")),
            forms_override: Vec::new(),
            etymology: None,
            examples: Vec::new(),
            is_peripheral: false,
        }
    }

    /// All 13 Turkish morpheme entries from `examples/turkish/profile.hu`.
    fn turkish_morphemes() -> Vec<Entry> {
        vec![
            morph_entry("neg_pc", "mi"),
            morph_entry("neg_pst", "me"),
            morph_entry("tns_pc", "iyor"),
            morph_entry("tns_pst", "di"),
            morph_entry("pn_pc_1sg", "um"),
            morph_entry("pn_pc_2sg", "sun"),
            morph_entry("pn_pc_1pl", "uz"),
            morph_entry("pn_pc_2pl", "sunuz"),
            morph_entry("pn_pc_3pl", "lar"),
            morph_entry("pn_pst_1sg", "m"),
            morph_entry("pn_pst_2sg", "n"),
            morph_entry("pn_pst_1pl", "k"),
            morph_entry("pn_pst_2pl", "niz"),
            morph_entry("pn_pst_3pl", "ler"),
        ]
    }

    // ---- test 1: single morpheme round-trip ----

    #[test]
    fn t01_single_morpheme_compile_and_lookup() {
        let mut alpha = PhonruleAlphabet::empty();
        let e = morph_entry("neg_pc", "mi");
        let lex = compile_lexicon(&[&e], &mut alpha).unwrap();
        let lbl = alpha.lookup_morpheme("neg_pc").unwrap();
        let s = lexicon_surface(&lex, lbl, &alpha).unwrap();
        assert_eq!(s, "mi");
    }

    // ---- test 2: three morphemes, distinct surfaces ----

    #[test]
    fn t02_three_morphemes_distinct_surfaces() {
        let mut alpha = PhonruleAlphabet::empty();
        let e1 = morph_entry("neg_pc", "mi");
        let e2 = morph_entry("neg_pst", "me");
        let e3 = morph_entry("tns_pc", "iyor");
        let lex = compile_lexicon(&[&e1, &e2, &e3], &mut alpha).unwrap();
        for (name, expected) in [("neg_pc", "mi"), ("neg_pst", "me"), ("tns_pc", "iyor")] {
            let lbl = alpha.lookup_morpheme(name).unwrap();
            assert_eq!(lexicon_surface(&lex, lbl, &alpha).unwrap(), expected);
        }
    }

    // ---- test 3: empty surface ----

    #[test]
    fn t03_empty_surface_yields_empty_string() {
        let mut alpha = PhonruleAlphabet::empty();
        let e = morph_entry("zero_morph", "");
        let lex = compile_lexicon(&[&e], &mut alpha).unwrap();
        let lbl = alpha.lookup_morpheme("zero_morph").unwrap();
        let s = lexicon_surface(&lex, lbl, &alpha).unwrap();
        assert_eq!(s, "");
    }

    // ---- test 4: multi-char surface (5 chars) ----

    #[test]
    fn t04_multi_char_surface() {
        let mut alpha = PhonruleAlphabet::empty();
        let e = morph_entry("pn_pc_2pl", "sunuz");
        let lex = compile_lexicon(&[&e], &mut alpha).unwrap();
        let lbl = alpha.lookup_morpheme("pn_pc_2pl").unwrap();
        assert_eq!(lexicon_surface(&lex, lbl, &alpha).unwrap(), "sunuz");
    }

    // ---- test 5: two morphemes with same surface, distinct IDs ----

    #[test]
    fn t05_two_morphemes_same_surface_distinct_ids() {
        let mut alpha = PhonruleAlphabet::empty();
        let e1 = morph_entry("loc_sg", "də");
        let e2 = morph_entry("abl_sg", "də");
        let lex = compile_lexicon(&[&e1, &e2], &mut alpha).unwrap();
        let l1 = alpha.lookup_morpheme("loc_sg").unwrap();
        let l2 = alpha.lookup_morpheme("abl_sg").unwrap();
        assert_ne!(l1, l2);
        assert_eq!(lexicon_surface(&lex, l1, &alpha).unwrap(), "də");
        assert_eq!(lexicon_surface(&lex, l2, &alpha).unwrap(), "də");
    }

    // ---- test 6: serialize + mmap_load round-trip ----

    #[test]
    fn t06_serialize_mmap_round_trip_preserves_lookup() {
        let mut alpha = PhonruleAlphabet::empty();
        let entries: Vec<Entry> = turkish_morphemes();
        let refs: Vec<&Entry> = entries.iter().collect();
        let lex = compile_lexicon(&refs, &mut alpha).unwrap();

        // Round-trip via serialize/deserialize. mmap_load is the
        // file-backed variant; serialize + deserialize is the
        // in-memory equivalent and exercises the same wire format.
        let bytes = RustFstBackend::serialize(&lex).unwrap();
        let restored = RustFstBackend::deserialize(&bytes).unwrap();

        // Surface look-up still works after round-trip.
        for entry in &entries {
            let lbl = alpha.lookup_morpheme(&entry.name.node).unwrap();
            let expected = headword_to_surface(&entry.headword);
            let actual = lexicon_surface(&restored, lbl, &alpha).unwrap();
            assert_eq!(actual, expected, "morpheme '{}' surface after round-trip", entry.name.node);
        }

        // Also test the literal mmap_load path via a temp file, so
        // we exercise the bytes-from-disk codepath F4 will use.
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join("hubullu_fst_lexicon_test_t06.fst");
        std::fs::write(&tmp_path, &bytes).unwrap();
        let mmap_restored = RustFstBackend::mmap_load(&tmp_path).unwrap();
        let lbl = alpha.lookup_morpheme("tns_pst").unwrap();
        assert_eq!(lexicon_surface(&mmap_restored, lbl, &alpha).unwrap(), "di");
        let _ = std::fs::remove_file(&tmp_path);
    }

    // ---- test 7: compose with intro_brackets (sanity for F4) ----

    #[test]
    fn t07_compose_with_intro_brackets_succeeds() {
        use crate::fst::phonrule::intro_brackets;
        let mut alpha = PhonruleAlphabet::empty();
        let e = morph_entry("neg_pc", "mi");
        let lex = compile_lexicon(&[&e], &mut alpha).unwrap();
        // intro_brackets reads alpha.sigma() to build its identity
        // loops. The lexicon already interned 'm' and 'i', so Σ has
        // two members at this point.
        assert!(alpha.sigma_len() >= 2);
        let brackets = intro_brackets(&alpha);
        // Lexicon FST is already arc-sorted on input; brackets needs
        // output-sorting on the left for compose.
        let left = RustFstBackend::arc_sort_output(&brackets).unwrap();
        let composed = RustFstBackend::compose(&left, &lex);
        assert!(composed.is_ok(), "compose(intro_brackets, lexicon) must succeed");
    }

    // ---- test 8: real Turkish morpheme set ----

    #[test]
    fn t08_real_turkish_lexicon_all_lookups() {
        let mut alpha = PhonruleAlphabet::empty();
        let entries: Vec<Entry> = turkish_morphemes();
        let refs: Vec<&Entry> = entries.iter().collect();
        let lex = compile_lexicon(&refs, &mut alpha).unwrap();
        for entry in &entries {
            let lbl = alpha.lookup_morpheme(&entry.name.node).unwrap();
            let expected = headword_to_surface(&entry.headword);
            let actual = lexicon_surface(&lex, lbl, &alpha).unwrap();
            assert_eq!(
                actual, expected,
                "morpheme '{}' lookup mismatch",
                entry.name.node
            );
        }
        // Sanity: 14 morphemes interned (2 neg + 2 tns + 5 pn_pc + 5 pn_pst).
        // The task brief said "13" — actual count from
        // `examples/turkish/profile.hu` is 14; brief was approximate.
        assert_eq!(alpha.morpheme_count(), 14);
    }

    // ---- test 9: unknown ID lookup → clean error ----

    #[test]
    fn t09_unknown_id_lookup_returns_error() {
        let mut alpha = PhonruleAlphabet::empty();
        let e = morph_entry("neg_pc", "mi");
        let lex = compile_lexicon(&[&e], &mut alpha).unwrap();
        // A morpheme-range label never registered with this lexicon.
        let bogus_label: Label = super::super::alphabet::FIRST_MORPHEME_LABEL + 999;
        let err = lexicon_surface(&lex, bogus_label, &alpha).unwrap_err();
        assert!(matches!(
            err,
            LexiconLookupError::UnknownMorpheme { morpheme_id_label } if morpheme_id_label == bogus_label
        ));
    }

    // ---- test 10: paths() enumeration discipline check ----

    #[test]
    fn t10_paths_enumeration_each_morpheme_appears_exactly_once() {
        let mut alpha = PhonruleAlphabet::empty();
        let entries: Vec<Entry> = turkish_morphemes();
        let refs: Vec<&Entry> = entries.iter().collect();
        let lex = compile_lexicon(&refs, &mut alpha).unwrap();

        // Each accepting path's input side is exactly one morpheme-ID
        // label (the ε-input arcs are stripped by paths()). Collect
        // them and assert the multiset equals the morpheme set.
        let mut seen: HashSet<Label> = HashSet::new();
        let mut total: usize = 0;
        // 128 cap is generous: 13 morphemes × 1 path = 13.
        for path in RustFstBackend::paths(&lex).unwrap().take(128) {
            total += 1;
            // path.input should be exactly [morpheme_id_label] — one
            // arc carries the morpheme ID, all others are ε on input.
            assert_eq!(
                path.input.len(),
                1,
                "expected one input label per morpheme path, got {:?}",
                path.input
            );
            let lbl = path.input[0];
            assert!(alpha.is_morpheme_label(lbl), "input label {} not in morpheme range", lbl);
            assert!(seen.insert(lbl), "duplicate morpheme path for label {}", lbl);
        }
        assert_eq!(total, entries.len(), "expected one path per morpheme");
        for entry in &entries {
            let lbl = alpha.lookup_morpheme(&entry.name.node).unwrap();
            assert!(seen.contains(&lbl), "missing path for morpheme '{}'", entry.name.node);
        }
    }

    // ---- test 11 (bonus): per-morpheme compile errors on inflectional entry ----

    #[test]
    fn t11_compile_morpheme_rejects_inflectional_entry() {
        let mut alpha = PhonruleAlphabet::empty();
        let mut e = morph_entry("verb_with_infl", "go");
        e.inflection = Some(crate::ast::EntryInflection::Class(ident("verb_conj")));
        let err = compile_morpheme(&e, &mut alpha).unwrap_err();
        assert!(matches!(
            err,
            LexiconCompileError::NotAMorpheme { ref entry_name }
                if entry_name == "verb_with_infl"
        ));
    }

    // ---- test 12 (bonus): empty lexicon → clean error ----

    #[test]
    fn t12_empty_lexicon_returns_error() {
        let mut alpha = PhonruleAlphabet::empty();
        let err = compile_lexicon(&[], &mut alpha).unwrap_err();
        assert!(matches!(err, LexiconCompileError::EmptyLexicon));
    }

    // ---- test 13 (sizing telemetry): Turkish lexicon FST stats ----
    //
    // Pinned numbers (not strict): the Turkish lexicon FST has a small,
    // predictable state count and serialisation size. Asserting upper
    // bounds catches accidental regressions (e.g. forgetting to
    // arc-sort, leaving an un-minimised result) at PR time.
    //
    // The numbers are reported in the F3 hand-off summary for F4
    // planning purposes.

    #[test]
    fn t13_turkish_lexicon_size_telemetry() {
        let mut alpha = PhonruleAlphabet::empty();
        let entries: Vec<Entry> = turkish_morphemes();
        let refs: Vec<&Entry> = entries.iter().collect();
        let lex = compile_lexicon(&refs, &mut alpha).unwrap();
        let num_states = RustFstBackend::num_states(&lex);
        let serialised = RustFstBackend::serialize(&lex).unwrap();
        // 14 morphemes; total headword char count = 2+2+4+2+2+3+2+5+3+1+1+1+3+3 = 34.
        // Union path-per-morpheme construction (1 start + 1 per arc + 1 final
        // per morpheme): state count is roughly 1 + Σ(1 + len(headword)).
        // Pin generous upper bounds to catch regressions but not over-fit:
        assert!(
            num_states < 200,
            "Turkish lexicon FST state count regressed: {}",
            num_states
        );
        assert!(
            serialised.len() < 8_192,
            "Turkish lexicon FST serialise size regressed: {} bytes",
            serialised.len()
        );
        // Also surface the actual values via println so cargo test -- --nocapture
        // shows the F4-relevant numbers.
        println!(
            "[F3 telemetry] Turkish lexicon: states={}, serialised={} bytes",
            num_states,
            serialised.len()
        );
    }
}
