//! Phonrule alphabet management (F2a step 1).
//!
//! Per the F2 plan §5: the Karttunen-style rewrite-rule construction (F2b)
//! needs a **closed alphabet** at compile time so it can build `Σ` and
//! `(Σ:Σ)*`. `phonrule_eval` ducks this by inspecting `&str` at scan time;
//! FSTs cannot.
//!
//! This module bundles three things:
//!
//! 1. A shared [`SymbolTable`] used as both input and output alphabet (every
//!    phonrule transducer is over a single common alphabet — there is no
//!    distinct surface vs lexical alphabet at this layer).
//! 2. A fixed reservation of low labels `1..16` for **control markers**:
//!    boundaries, word-edge anchors, and the four Karttunen brackets. These
//!    labels are deliberately interned at construction so phoneme symbols
//!    cannot collide with them.
//! 3. A growing set of **alphabet labels** (`16..`) derived from a
//!    [`PhonemeInventory`] plus ad-hoc symbols added later (class members,
//!    rule literals, map outputs). The "warn on drift" lint policy is
//!    deferred (plan §5.4 / §8 open Q3); F2a silently includes ad-hoc
//!    symbols.
//!
//! The alphabet is intentionally a **handle**: callers `intern` symbols as
//! they meet them while compiling classes / maps / rules. The closed Σ for
//! F2b is taken via [`PhonruleAlphabet::sigma`] **after** all rule
//! compilation has finished — by then every symbol that can appear at FST
//! runtime has been interned.

use std::collections::{HashMap, HashSet};

use crate::phoneme::PhonemeInventory;

use super::backend::{Label, SymbolTable, EPS_LABEL};

// ---------------------------------------------------------------------------
// Reserved control-marker labels.
// ---------------------------------------------------------------------------
//
// Labels 1..=15 are reserved for FST control markers. They are interned at
// `PhonruleAlphabet::new` time with the conventional symbol-table names
// below. Labels 16.. are available for phoneme / literal symbols.
//
// F2a uses only `BOUNDARY_LABEL`, `WORD_START_LABEL`, `WORD_END_LABEL`. The
// four Karttunen brackets (`[+]/[-]/]+/]-`) are reserved here so F2b can use
// them without renumbering; label 8 is the directed-replacement caret
// (F2c4 Strategy A); labels 9..=15 are reserved as opaque future expansion
// slots and have placeholder symbol-table entries.

/// Reserved label for the in-stream boundary marker (matches `phonrule_eval`'s
/// `BOUNDARY` = `'\0'`, `src/phonrule_eval.rs:21`).
pub const BOUNDARY_LABEL: Label = 1;

/// Reserved label for the word-start anchor (`^` in phonrule context).
pub const WORD_START_LABEL: Label = 2;

/// Reserved label for the word-end anchor (`$` in phonrule context).
pub const WORD_END_LABEL: Label = 3;

/// Karttunen `[+]` — opening bracket for an obligatory rewrite site (F2b).
pub const BRACKET_OPEN_OBLIG_LABEL: Label = 4;

/// Karttunen `[-]` — opening bracket for an optional/disallowed site (F2b).
pub const BRACKET_OPEN_OPT_LABEL: Label = 5;

/// Karttunen `]+` — closing bracket for an obligatory rewrite site (F2b).
pub const BRACKET_CLOSE_OBLIG_LABEL: Label = 6;

/// Karttunen `]-` — closing bracket for an optional/disallowed site (F2b).
pub const BRACKET_CLOSE_OPT_LABEL: Label = 7;

/// Karttunen directed-replacement caret `^` (F2c4 Strategy A).
///
/// The "tentative match start" marker introduced by the `Intro` / obligatory
/// caret-insertion step of the directed-replacement construction
/// (Karttunen 1996 Figure 11). It is introduced internally, manipulated by
/// the `NotLeftmost` / `NotInner` filters, and stripped before the
/// transducer's output is returned — it must therefore NOT be a member of Σ
/// (the same discipline as the brackets at labels 4..=7).
pub const CARET_OBLIG_LABEL: Label = 8;

/// First label available for phoneme / literal symbols. Labels 9..=15 are
/// reserved for future expansion.
pub const FIRST_USER_LABEL: Label = 16;

/// Number of reserved low labels (inclusive of label 0 = epsilon and labels
/// 1..=15 = control markers). Phoneme symbols start at `FIRST_USER_LABEL`.
pub const RESERVED_LABEL_COUNT: Label = FIRST_USER_LABEL;

/// First label used for **morpheme-identity symbols** (F3 of the FST
/// migration — `docs/proposals/fst-morphology.md` §6.7 "input alphabet …
/// morpheme-identity symbols").
///
/// Morpheme IDs and phoneme/literal symbols share one numeric label space
/// (single `PhonruleAlphabet`, single `SymbolTable`), but the morpheme range
/// starts at `65536` to leave a generous gap above any plausible phoneme
/// inventory. Phoneme symbols are allocated upward from
/// [`FIRST_USER_LABEL`] (16); morpheme symbols are allocated upward from
/// [`FIRST_MORPHEME_LABEL`] (65536). The chosen gap of 65520 labels for
/// phonemes is several orders of magnitude beyond what any natural-language
/// phoneme inventory needs — hubullu's largest grammar to date uses ~50
/// phonemes — and the assertion in [`PhonruleAlphabet::intern_morpheme`]
/// catches an inventory accidentally crossing the boundary.
///
/// Morpheme symbol-table names are prefixed `<m:NAME>` so they can never
/// collide with phoneme literals (which never contain `<` or `:`). The
/// prefix is also the disambiguator on look-up (`lookup_morpheme` /
/// `intern_morpheme` work on bare names; the `<m:>` prefix is internal).
pub const FIRST_MORPHEME_LABEL: Label = 65536;

// Symbol-table names for the reserved markers. These are deliberately ugly
// strings unlikely to collide with any real phoneme literal.
const SYM_BOUNDARY: &str = "<bdy>";
const SYM_WORD_START: &str = "<^>";
const SYM_WORD_END: &str = "<$>";
const SYM_BRACKET_OPEN_OBLIG: &str = "<[+]>";
const SYM_BRACKET_OPEN_OPT: &str = "<[->";
const SYM_BRACKET_CLOSE_OBLIG: &str = "<]+>";
const SYM_BRACKET_CLOSE_OPT: &str = "<]->";
const SYM_CARET_OBLIG: &str = "<caret>";

// Placeholder names for labels 8..=15 (`<rsv8>` .. `<rsv15>`). Interned
// eagerly so future readers can see all reserved slots in the symbol table
// and so the user-label range starts cleanly at 16.
fn reserved_placeholder_name(label: Label) -> String {
    format!("<rsv{}>", label)
}

// ---------------------------------------------------------------------------
// PhonruleAlphabet — the F2a public type.
// ---------------------------------------------------------------------------

/// Closed-alphabet handle for phonrule FST compilation.
///
/// Owns a [`SymbolTable`] populated with:
///   * `<eps>` at label 0 (from `SymbolTable::new`)
///   * `<bdy>`, `<^>`, `<$>`, and the four Karttunen-bracket placeholders at
///     labels 1..=7
///   * `<rsv8>` .. `<rsv15>` reserved placeholders at labels 8..=15
///   * Every terminal in the supplied [`PhonemeInventory`] at consecutive
///     labels from [`FIRST_USER_LABEL`] upward
///   * Any further ad-hoc symbol [`intern`](Self::intern)ed during class /
///     map / rule compilation
///
/// Symbols are deduplicated; calling `intern` on a known symbol returns the
/// existing label.
#[derive(Debug, Clone)]
pub struct PhonruleAlphabet {
    symtab: SymbolTable,
    /// Labels considered part of the **runtime alphabet Σ** for F2b's
    /// `(Σ:Σ)*` constructions. Excludes epsilon and reserved control markers
    /// (boundary/word-edge/brackets) — those are added to the FST via their
    /// own dedicated arcs and must not be members of the wildcard alphabet.
    sigma: Vec<Label>,
    /// Set membership companion to `sigma`, for O(1) "is this in Σ" checks.
    sigma_set: HashSet<Label>,
    /// Morpheme-name → morpheme-label map (F3). Morphemes live in a
    /// dedicated label range starting at [`FIRST_MORPHEME_LABEL`]; their
    /// symbol-table names are prefixed `<m:NAME>` to guarantee no collision
    /// with phoneme literals. Tracked in a separate map so `sigma()` /
    /// `is_in_sigma()` continue to mean "phoneme-side runtime alphabet";
    /// morpheme labels are NEVER part of Σ.
    morphemes: HashMap<String, Label>,
    /// Reverse map: morpheme-label → prefixed name (`<m:NAME>`). Cached
    /// once at intern time so [`label_to_str`](Self::label_to_str) can
    /// return a borrowed `&str` for morpheme labels.
    morpheme_prefixed_names: HashMap<Label, String>,
    /// Next morpheme label to hand out. Starts at [`FIRST_MORPHEME_LABEL`];
    /// monotonically incremented by [`intern_morpheme`](Self::intern_morpheme).
    next_morpheme_label: Label,
}

impl PhonruleAlphabet {
    /// Build a fresh alphabet from a phoneme inventory.
    ///
    /// Steps:
    ///   1. Create a fresh symbol table; reserve `<eps>` at 0 (implicit) and
    ///      the seven named control markers at 1..=7.
    ///   2. Reserve placeholder names for labels 8..=15.
    ///   3. Intern every terminal in `inventory.all_terminals` (sorted for
    ///      deterministic label assignment). These become Σ members.
    pub fn from_inventory(inventory: &PhonemeInventory) -> Self {
        let mut alpha = Self::empty();
        // Sort terminals for deterministic label order — `HashSet` iteration
        // is non-deterministic, but the compiled FST and label numbering
        // should be reproducible across runs (matters for test snapshots,
        // potential on-disk caches, debuggability).
        let mut terms: Vec<&String> = inventory.all_terminals.iter().collect();
        terms.sort();
        for t in terms {
            alpha.intern(t);
        }
        alpha
    }

    /// Build an alphabet with only the reserved markers (no phonemes). Useful
    /// for tests and for use cases where the alphabet is built up entirely
    /// from class / rule literals.
    pub fn empty() -> Self {
        let mut symtab = SymbolTable::new();
        // Intern reserved markers in label order. SymbolTable::intern returns
        // labels in insertion order starting at 1 (label 0 is <eps>).
        let l_bdy = symtab.intern(SYM_BOUNDARY);
        debug_assert_eq!(l_bdy, BOUNDARY_LABEL);
        let l_ws = symtab.intern(SYM_WORD_START);
        debug_assert_eq!(l_ws, WORD_START_LABEL);
        let l_we = symtab.intern(SYM_WORD_END);
        debug_assert_eq!(l_we, WORD_END_LABEL);
        let l_boo = symtab.intern(SYM_BRACKET_OPEN_OBLIG);
        debug_assert_eq!(l_boo, BRACKET_OPEN_OBLIG_LABEL);
        let l_boo2 = symtab.intern(SYM_BRACKET_OPEN_OPT);
        debug_assert_eq!(l_boo2, BRACKET_OPEN_OPT_LABEL);
        let l_bco = symtab.intern(SYM_BRACKET_CLOSE_OBLIG);
        debug_assert_eq!(l_bco, BRACKET_CLOSE_OBLIG_LABEL);
        let l_bco2 = symtab.intern(SYM_BRACKET_CLOSE_OPT);
        debug_assert_eq!(l_bco2, BRACKET_CLOSE_OPT_LABEL);
        let l_caret = symtab.intern(SYM_CARET_OBLIG);
        debug_assert_eq!(l_caret, CARET_OBLIG_LABEL);
        // Reserve placeholders for labels 9..=15.
        for l in (CARET_OBLIG_LABEL + 1)..FIRST_USER_LABEL {
            let assigned = symtab.intern(&reserved_placeholder_name(l));
            debug_assert_eq!(assigned, l);
        }
        debug_assert_eq!(symtab.len() as Label, FIRST_USER_LABEL);
        Self {
            symtab,
            sigma: Vec::new(),
            sigma_set: HashSet::new(),
            morphemes: HashMap::new(),
            morpheme_prefixed_names: HashMap::new(),
            next_morpheme_label: FIRST_MORPHEME_LABEL,
        }
    }

    // ----- reserved-marker accessors -----

    /// Label for the in-stream boundary marker. F2a uses this; F2b's context
    /// compilation will need it too.
    pub fn boundary_label(&self) -> Label {
        BOUNDARY_LABEL
    }

    /// Label for the word-start anchor.
    pub fn word_start_label(&self) -> Label {
        WORD_START_LABEL
    }

    /// Label for the word-end anchor.
    pub fn word_end_label(&self) -> Label {
        WORD_END_LABEL
    }

    /// Label for the directed-replacement caret marker (`^`, F2c4 Strategy A).
    /// Not a member of Σ; introduced and stripped internally by the
    /// directed-replacement construction.
    pub fn caret_label(&self) -> Label {
        CARET_OBLIG_LABEL
    }

    // ----- intern / lookup -----

    /// Get-or-create the label for a user symbol.
    ///
    /// New symbols are appended to Σ (the runtime alphabet used by F2b's
    /// `(Σ:Σ)*`). Known symbols return their existing label without
    /// modifying Σ membership.
    ///
    /// Symbols whose name happens to collide with a reserved-marker
    /// placeholder name (`<eps>`, `<bdy>`, etc.) return the reserved label
    /// and do NOT become Σ members. In practice class / rule literals are
    /// real phoneme strings ("a", "ng", ...) so this branch is academic;
    /// it's correct behaviour either way.
    pub fn intern(&mut self, symbol: &str) -> Label {
        if let Some(existing) = self.symtab.label(symbol) {
            return existing;
        }
        let label = self.symtab.intern(symbol);
        debug_assert!(label >= FIRST_USER_LABEL);
        self.sigma.push(label);
        self.sigma_set.insert(label);
        label
    }

    /// Look up the label for a symbol without interning. Returns `None` if
    /// the symbol is unknown.
    pub fn lookup(&self, symbol: &str) -> Option<Label> {
        self.symtab.label(symbol)
    }

    /// Reverse lookup: label → symbol name. Returns `None` for unknown
    /// labels.
    ///
    /// Phoneme / control-marker labels (`< FIRST_MORPHEME_LABEL`) resolve
    /// via the symbol table; morpheme labels (`>= FIRST_MORPHEME_LABEL`)
    /// resolve to the prefixed name `<m:NAME>` via the morpheme map. Use
    /// [`label_to_morpheme_str`](Self::label_to_morpheme_str) to get the
    /// bare morpheme name (without the `<m:>` prefix).
    pub fn label_to_str(&self, label: Label) -> Option<&str> {
        if label >= FIRST_MORPHEME_LABEL {
            return self.label_to_morpheme_prefixed(label);
        }
        self.symtab.name(label)
    }

    // ----- morpheme namespace (F3) -----

    /// Get-or-create the label for a morpheme identity symbol.
    ///
    /// Morphemes live in their own label range starting at
    /// [`FIRST_MORPHEME_LABEL`] (65536). Returns the existing label if
    /// `name` was previously interned; otherwise allocates the next
    /// morpheme label and records it.
    ///
    /// Implementation note: morpheme labels live in `self.morphemes`, NOT
    /// in the symbol table's contiguous id space. Phoneme symbol-table
    /// labels grow upward from `FIRST_USER_LABEL` (16); morpheme labels
    /// grow upward from `FIRST_MORPHEME_LABEL` (65536). The gap is large
    /// enough that the assertion below cannot fire under any plausible
    /// inventory (the largest natural-language phoneme set is ~140
    /// symbols; the gap is 65520).
    pub fn intern_morpheme(&mut self, name: &str) -> Label {
        if let Some(&existing) = self.morphemes.get(name) {
            return existing;
        }
        // Defensive: ensure the phoneme symbol-table hasn't crossed into
        // the morpheme range. With FIRST_MORPHEME_LABEL = 65536 this is
        // unreachable in any real grammar.
        debug_assert!(
            (self.symtab.len() as Label) < FIRST_MORPHEME_LABEL,
            "phoneme/literal symbol table overflowed into morpheme label range"
        );
        let label = self.next_morpheme_label;
        self.next_morpheme_label += 1;
        self.morphemes.insert(name.to_string(), label);
        self.morpheme_prefixed_names
            .insert(label, format!("<m:{}>", name));
        label
    }

    /// Look up the label for a morpheme by bare name, without interning.
    /// Returns `None` if the morpheme is unknown.
    pub fn lookup_morpheme(&self, name: &str) -> Option<Label> {
        self.morphemes.get(name).copied()
    }

    /// Reverse lookup: morpheme label → bare morpheme name. Returns `None`
    /// if the label is not in the morpheme range or is unknown.
    ///
    /// For the full prefixed form `<m:NAME>` (useful for diagnostics that
    /// want to match the symbol-table style of [`label_to_str`]), use
    /// [`label_to_morpheme_prefixed`](Self::label_to_morpheme_prefixed).
    pub fn label_to_morpheme(&self, label: Label) -> Option<&str> {
        if label < FIRST_MORPHEME_LABEL {
            return None;
        }
        // Strip the `<m:` prefix and trailing `>` from the cached
        // prefixed form. We could keep a second reverse map of bare
        // names, but the prefixed cache already exists and the slice
        // arithmetic is trivial.
        self.morpheme_prefixed_names.get(&label).map(|s| {
            // s = "<m:NAME>"; strip 3-byte "<m:" prefix and 1-byte ">"
            // suffix. Both are ASCII so byte-slicing is safe.
            let bytes = s.as_bytes();
            debug_assert!(s.starts_with("<m:") && s.ends_with('>'));
            std::str::from_utf8(&bytes[3..bytes.len() - 1])
                .expect("morpheme name was originally a valid &str")
        })
    }

    /// Like [`label_to_morpheme`](Self::label_to_morpheme) but returns the
    /// `<m:NAME>` prefixed form. Used by [`label_to_str`] so morpheme
    /// labels round-trip through the same accessor as phoneme labels.
    /// The prefixed string is cached at intern time in
    /// `morpheme_prefixed_names`, so this is O(1).
    pub fn label_to_morpheme_prefixed(&self, label: Label) -> Option<&str> {
        if label < FIRST_MORPHEME_LABEL {
            return None;
        }
        // Same linear scan; returns the cached prefixed name from the
        // dedicated map populated at intern time.
        self.morpheme_prefixed_names
            .get(&label)
            .map(|s| s.as_str())
    }

    /// Iterate over (name, label) pairs for every interned morpheme.
    /// Order is unspecified (HashMap iteration).
    pub fn morphemes(&self) -> impl Iterator<Item = (&str, Label)> + '_ {
        self.morphemes.iter().map(|(k, &v)| (k.as_str(), v))
    }

    /// Number of interned morphemes.
    pub fn morpheme_count(&self) -> usize {
        self.morphemes.len()
    }

    /// Whether `label` is a morpheme-identity label (i.e. in the
    /// `>= FIRST_MORPHEME_LABEL` range).
    pub fn is_morpheme_label(&self, label: Label) -> bool {
        label >= FIRST_MORPHEME_LABEL
    }

    // ----- Σ enumeration -----

    /// Iterate over Σ — the runtime alphabet used in `(Σ:Σ)*` constructions.
    ///
    /// Excludes epsilon and all reserved control markers (`BOUNDARY_LABEL`,
    /// word-edge anchors, Karttunen brackets). Iteration order is insertion
    /// order: phoneme terminals first (sorted at construction), then ad-hoc
    /// interns in the order they were first seen.
    pub fn sigma(&self) -> impl Iterator<Item = Label> + '_ {
        self.sigma.iter().copied()
    }

    /// Whether `label` is a member of Σ (i.e. a user symbol, not epsilon or a
    /// reserved marker).
    pub fn is_in_sigma(&self, label: Label) -> bool {
        self.sigma_set.contains(&label)
    }

    /// Number of Σ members. Useful for tests and for sizing alphabet-wide
    /// constructions in F2b.
    pub fn sigma_len(&self) -> usize {
        self.sigma.len()
    }

    /// Borrow the underlying symbol table. Backends and downstream FST code
    /// that needs to round-trip a label through a `SymbolTable` (e.g.
    /// diagnostics, serialised output) should use this.
    pub fn symbol_table(&self) -> &SymbolTable {
        &self.symtab
    }

    /// Epsilon label, re-exported for convenience so callers don't need to
    /// reach into the backend.
    pub const EPS: Label = EPS_LABEL;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet as Set;

    fn inv_with(terms: &[&str]) -> PhonemeInventory {
        let mut inv = PhonemeInventory::default();
        inv.all_terminals = terms.iter().map(|s| s.to_string()).collect();
        inv
    }

    #[test]
    fn empty_reserves_low_labels() {
        let a = PhonruleAlphabet::empty();
        // Reserved markers occupy 1..=7; placeholders 8..=15.
        assert_eq!(a.boundary_label(), 1);
        assert_eq!(a.word_start_label(), 2);
        assert_eq!(a.word_end_label(), 3);
        // <eps> still at 0.
        assert_eq!(a.label_to_str(0), Some("<eps>"));
        // First user label is 16; sigma is empty.
        assert_eq!(a.sigma_len(), 0);
        assert_eq!(a.sigma().count(), 0);
    }

    #[test]
    fn from_inventory_assigns_user_labels_from_16() {
        let inv = inv_with(&["a", "b", "c"]);
        let a = PhonruleAlphabet::from_inventory(&inv);
        assert_eq!(a.sigma_len(), 3);
        // Sorted insertion order means "a" < "b" < "c" get 16, 17, 18.
        assert_eq!(a.lookup("a"), Some(16));
        assert_eq!(a.lookup("b"), Some(17));
        assert_eq!(a.lookup("c"), Some(18));
        // All three are in Σ.
        let sig: Set<Label> = a.sigma().collect();
        assert_eq!(sig, [16, 17, 18].iter().copied().collect());
    }

    #[test]
    fn intern_is_idempotent_and_extends_sigma_once() {
        let mut a = PhonruleAlphabet::empty();
        let x1 = a.intern("x");
        let x2 = a.intern("x");
        assert_eq!(x1, x2);
        assert_eq!(a.sigma_len(), 1);
        let y = a.intern("y");
        assert_ne!(x1, y);
        assert_eq!(a.sigma_len(), 2);
    }

    #[test]
    fn reserved_markers_not_in_sigma() {
        let a = PhonruleAlphabet::empty();
        assert!(!a.is_in_sigma(0)); // eps
        assert!(!a.is_in_sigma(a.boundary_label()));
        assert!(!a.is_in_sigma(a.word_start_label()));
        assert!(!a.is_in_sigma(a.word_end_label()));
        for l in 4..FIRST_USER_LABEL {
            assert!(!a.is_in_sigma(l), "reserved label {} must not be in Σ", l);
        }
    }

    #[test]
    fn caret_marker_reserved_at_label_8() {
        let a = PhonruleAlphabet::empty();
        assert_eq!(a.caret_label(), 8);
        assert_eq!(CARET_OBLIG_LABEL, 8);
        assert_eq!(a.label_to_str(CARET_OBLIG_LABEL), Some("<caret>"));
        // Not part of Σ — introduced/stripped internally like the brackets.
        assert!(!a.is_in_sigma(a.caret_label()));
        // User symbols still start cleanly at 16.
        let mut a2 = PhonruleAlphabet::empty();
        assert_eq!(a2.intern("a"), FIRST_USER_LABEL);
    }

    #[test]
    fn label_to_str_round_trip_for_user_symbols() {
        let mut a = PhonruleAlphabet::empty();
        let l = a.intern("ng");
        assert_eq!(a.label_to_str(l), Some("ng"));
    }

    // ----- morpheme namespace (F3) -----

    #[test]
    fn morpheme_labels_live_in_dedicated_range() {
        let mut a = PhonruleAlphabet::empty();
        let l1 = a.intern_morpheme("pn_pc_1sg");
        let l2 = a.intern_morpheme("tns_pc");
        assert!(l1 >= FIRST_MORPHEME_LABEL);
        assert!(l2 >= FIRST_MORPHEME_LABEL);
        assert_ne!(l1, l2);
        // Phoneme interns continue from FIRST_USER_LABEL untouched.
        let pa = a.intern("a");
        assert_eq!(pa, FIRST_USER_LABEL);
    }

    #[test]
    fn morpheme_intern_is_idempotent() {
        let mut a = PhonruleAlphabet::empty();
        let l1 = a.intern_morpheme("neg_pc");
        let l2 = a.intern_morpheme("neg_pc");
        assert_eq!(l1, l2);
        assert_eq!(a.morpheme_count(), 1);
    }

    #[test]
    fn morpheme_label_round_trips_via_label_to_str() {
        let mut a = PhonruleAlphabet::empty();
        let l = a.intern_morpheme("pn_pst_1pl");
        // label_to_str returns the prefixed form for diagnostics symmetry.
        assert_eq!(a.label_to_str(l), Some("<m:pn_pst_1pl>"));
        // label_to_morpheme returns the bare name.
        assert_eq!(a.label_to_morpheme(l), Some("pn_pst_1pl"));
        assert_eq!(a.lookup_morpheme("pn_pst_1pl"), Some(l));
    }

    #[test]
    fn morphemes_are_not_in_sigma() {
        let mut a = PhonruleAlphabet::empty();
        let l = a.intern_morpheme("m1");
        assert!(!a.is_in_sigma(l));
        assert!(a.is_morpheme_label(l));
    }
}
