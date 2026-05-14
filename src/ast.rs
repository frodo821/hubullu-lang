//! Hubullu Abstract Syntax Tree definitions.
//!
//! Two string types exist in the DSL:
//! - `StringLit` (`"..."`) — plain text, no interpolation
//! - `Template` (`` `...` ``) — interpolation via `{name}` referencing stems/slots

// ---------------------------------------------------------------------------
// Span & common types
// ---------------------------------------------------------------------------

/// Byte offset range into source for error reporting.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub file_id: crate::span::FileId,
    pub start: usize,
    pub end: usize,
}

/// Span is intentionally excluded from AST hashing — source positions must not
/// affect Merkle cache keys.
impl std::hash::Hash for Span {
    fn hash<H: std::hash::Hasher>(&self, _state: &mut H) {}
}

/// A node annotated with source span.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

/// An identifier with source span.
pub type Ident = Spanned<String>;

/// Plain string literal (`"..."`).
pub type StringLit = Spanned<String>;

/// Template literal (`` `...` ``), containing segments.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Template {
    pub segments: Vec<TemplateSegment>,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TemplateSegment {
    /// Literal text between interpolations.
    Lit(String),
    /// `{stem_name}` — reference to a stem.
    Stem(Ident),
    /// `{ident.slot}` — reference to a structural stem's slot.
    Slot { stem: Ident, slot: Ident },
}

// ---------------------------------------------------------------------------
// File (top-level)
// ---------------------------------------------------------------------------

/// A parsed `.hu` file — the root AST node.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct File {
    pub items: Vec<Spanned<Item>>,
}

/// A top-level item in a `.hu` file.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    Use(Import),
    Reference(Import),
    Export(Export),
    TagAxis(TagAxis),
    Extend(Extend),
    Inflection(Inflection),
    Entry(Box<Entry>),
    PhonRule(PhonRule),
    Phoneme(Phoneme),
    Syllable(Syllable),
    Render(RenderConfig),
}

/// Configuration for `.hut` token rendering.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderConfig {
    pub separator: Option<StringLit>,
    pub no_separator_before: Option<StringLit>,
}

// ---------------------------------------------------------------------------
// phoneme
// ---------------------------------------------------------------------------

/// A top-level `phoneme NAME { ... }` declaration.
///
/// Phonemes name a set of phonological surface units. Members may be literal
/// strings or references to other phonemes; the effective inventory is the
/// union of all transitively reachable literal members. Multigraphs are
/// supported via longest-match-first tokenization during phonrule evaluation.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Phoneme {
    pub name: Ident,
    pub members: Vec<PhonemeMember>,
    pub span: Span,
}

/// A single member of a `phoneme` block: either a literal surface form
/// (e.g. `"a"`, `"ng"`) or a reference to another phoneme.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonemeMember {
    Lit(StringLit),
    Ref(Ident),
}

// ---------------------------------------------------------------------------
// syllable
// ---------------------------------------------------------------------------

/// A top-level `syllable NAME { ... }` declaration describing how surface
/// strings are decomposed into syllables.
///
/// See proposal F2b: the template (e.g. `(C) V (C) (C)`) plus a nucleus
/// phoneme drive a greedy left-to-right syllabifier built on top of F2a's
/// `PhonemeInventory` longest-match tokenizer. `onset_priority` chooses
/// between maximal- and minimal-onset interpretations at ambiguous joints,
/// while `unknown` (with optional per-grapheme `unknown_overrides`) governs
/// what happens when the input contains characters outside the inventory.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Syllable {
    pub name: Ident,
    pub template: SyllableTemplate,
    /// Phoneme whose terminals act as syllable nuclei.
    pub nucleus: Ident,
    pub onset_max: Option<u32>,
    pub coda_max: Option<u32>,
    pub onset_priority: OnsetPriority,
    pub unknown: UnknownMode,
    /// Per-grapheme overrides for the default `unknown` mode. Stored as a
    /// sorted `Vec` (not `HashMap`) so that AST hashing is deterministic.
    pub unknown_overrides: Vec<(String, UnknownMode)>,
    pub span: Span,
}

/// A parsed syllable template — a sequence of phoneme-class slots.
///
/// Each slot is a phoneme class identifier plus an `optional` flag for
/// whether it appeared in parentheses (`(C)` vs `C`). Slots have no
/// `+`/`*`-style quantification beyond optionality.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyllableTemplate {
    pub slots: Vec<SyllableTemplateSlot>,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyllableTemplateSlot {
    /// Phoneme class referenced by this slot (e.g. `C` or `V`).
    pub class: Ident,
    /// `true` when wrapped in parentheses in the source.
    pub optional: bool,
}

/// Whether the syllabifier prefers a maximal or minimal onset at ambiguous
/// `... V C V ...` joints.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OnsetPriority {
    /// Maximal-onset principle: push intervocalic consonants to the next
    /// syllable's onset (subject to `onset_max`).
    Max,
    /// Minimal onset: keep intervocalic consonants in the prior syllable's
    /// coda (subject to `coda_max`).
    Min,
}

/// How the syllabifier handles input characters that do not belong to any
/// phoneme in the inventory.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UnknownMode {
    /// Silently pass through; the unknown chunk stays inside the current syllable.
    Ignore,
    /// Treat the unknown chunk as a syllable boundary.
    Skip,
    /// Like `Skip`, but emit a warning diagnostic.
    #[default]
    Warn,
    /// Emit an error diagnostic; syllabification still completes (treating the
    /// unknown chunk like `Skip`).
    Error,
}

// ---------------------------------------------------------------------------
// @use / @reference
// ---------------------------------------------------------------------------

/// An `@use` or `@reference` import statement.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    pub target: ImportTarget,
    pub path: StringLit,
}

/// What is being imported.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImportTarget {
    /// `*` or `* as ns`
    Glob { alias: Option<Ident> },
    /// Named list, e.g. `tense, aspect as a` or `(tense, aspect as a)`
    Named(Vec<ImportEntry>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportEntry {
    pub name: Ident,
    pub alias: Option<Ident>,
}

// ---------------------------------------------------------------------------
// @export
// ---------------------------------------------------------------------------

/// An `@export` directive that re-exports symbols transitively.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Export {
    /// `true` for `@export use`, `false` for `@export reference`.
    pub is_use: bool,
    /// What to export: `*`, `* as ns`, or named list.
    pub target: ImportTarget,
    /// Source file path. Present for form 2 (`from "file"`), absent for form 1.
    pub path: Option<StringLit>,
}

// ---------------------------------------------------------------------------
// tagaxis
// ---------------------------------------------------------------------------

/// A `tagaxis` declaration defining a grammatical dimension (e.g. tense, number).
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagAxis {
    pub name: Ident,
    pub role: Spanned<Role>,
    pub display: DisplayMap,
    pub index: Option<Spanned<IndexKind>>,
}

/// Role of a tag axis.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Inflectional,
    Classificatory,
    Structural,
}

/// Kind of search index for a tag axis.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexKind {
    Exact,
    Fulltext,
}

/// `{ ja: "品詞", en: "Part of Speech" }`
pub type DisplayMap = Vec<(Ident, StringLit)>;

// ---------------------------------------------------------------------------
// @extend
// ---------------------------------------------------------------------------

/// An `@extend` block that adds values to a tag axis.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Extend {
    pub name: Ident,
    pub target_axis: Ident,
    pub values: Vec<ExtendValue>,
}

/// A single value within an `@extend` block.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExtendValue {
    pub name: Ident,
    pub display: DisplayMap,
    /// `slots: [C1, C2, C3]` — only meaningful for structural axes.
    pub slots: Vec<Ident>,
}

// ---------------------------------------------------------------------------
// phonrule
// ---------------------------------------------------------------------------

/// A `phonrule` block defining phonological rewrite rules.
///
/// Class and map definitions are order-independent (lookup tables establishing
/// names available to rules in this block). Rewrite rules and `apply` statements
/// share declaration order — they live together in `body` so that mixing them
/// preserves intent (e.g. `class V = [...]; apply Q; "œ" -> "e"` runs Q before
/// the rewrite).
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonRule {
    pub name: Ident,
    /// Optional human-readable display names. Informational metadata only.
    pub display: DisplayMap,
    /// `derived_from: proto` — informational only, not used by the compiler.
    pub derived_from: Option<Ident>,
    /// `syllable: NAME` — references a top-level `syllable` declaration (F2c).
    /// Required whenever the body uses a syllable-aware macro context element
    /// (`%syl<head>%` / `%syl<tail>%` / `%syl[...]%` / `%syl<#N>%`).
    /// Omitted phonrules cannot use syllable-aware context.
    pub syllable: Option<Ident>,
    pub classes: Vec<CharClassDef>,
    pub maps: Vec<PhonMapDef>,
    /// Ordered body items: rewrite rules and `apply <other>` statements,
    /// interleaved in declaration order.
    pub body: Vec<PhonBodyItem>,
    pub span: Span,
}

/// A single ordered body item of a phonrule.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonBodyItem {
    Rewrite(PhonRewriteRule),
    Apply(PhonApply),
}

/// `apply OTHER` statement inside a phonrule body. Composes another phonrule
/// into this one at the position the statement appears in body declaration
/// order.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonApply {
    pub rule: Ident,
    pub span: Span,
}

impl PhonRule {
    /// Convenience iterator yielding only the rewrite rules in declaration
    /// order, skipping any `apply` statements. Useful for places that don't
    /// care about composition semantics (e.g. validation passes that only
    /// inspect rewrite refs).
    pub fn rewrite_rules(&self) -> impl Iterator<Item = &PhonRewriteRule> {
        self.body.iter().filter_map(|it| match it {
            PhonBodyItem::Rewrite(r) => Some(r),
            PhonBodyItem::Apply(_) => None,
        })
    }

    /// Convenience iterator yielding only the apply statements in declaration
    /// order.
    pub fn applies(&self) -> impl Iterator<Item = &PhonApply> {
        self.body.iter().filter_map(|it| match it {
            PhonBodyItem::Rewrite(_) => None,
            PhonBodyItem::Apply(a) => Some(a),
        })
    }
}

/// `class front = ["e", "i"]` or `class V = front | back`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CharClassDef {
    pub name: Ident,
    pub body: CharClassBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CharClassBody {
    /// Literal list: `["e", "i", "ö", "ü"]`
    List(Vec<StringLit>),
    /// Union of other classes: `front | back`
    Union(Vec<Ident>),
}

/// `map to_back = c -> match { "e" -> "a", else -> c }`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonMapDef {
    pub name: Ident,
    pub param: Ident,
    pub body: PhonMapBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonMapBody {
    Match {
        arms: Vec<PhonMapArm>,
        else_arm: Option<PhonMapElse>,
    },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonMapArm {
    pub from: StringLit,
    pub to: PhonMapResult,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonMapResult {
    Literal(StringLit),
    Var(Ident),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonMapElse {
    Literal(StringLit),
    Var(Ident),
}

/// A phonological rewrite rule: `V -> to_back / back !back* + !back* _`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonRewriteRule {
    pub from: PhonPattern,
    pub to: PhonReplacement,
    pub context: Option<PhonContext>,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonPattern {
    Class(Ident),
    Literal(StringLit),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonReplacement {
    Map(Ident),
    Literal(StringLit),
    Null,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhonContext {
    pub left: Vec<PhonContextElem>,
    pub right: Vec<PhonContextElem>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhonContextElem {
    Class(Ident),
    NegClass(Ident),
    Boundary,
    WordStart,
    WordEnd,
    Literal(StringLit),
    Repeat(Box<PhonContextElem>),
    Alt(Vec<PhonContextElem>),
    /// `%syl<head>%` — current position is at the start of a syllable (M, was F2c `σ[`).
    /// Requires the enclosing phonrule to have a `syllable: NAME` field.
    SylHead,
    /// `%syl<tail>%` — current position is at the end of a syllable (M, was F2c `]σ`).
    /// Requires the enclosing phonrule to have a `syllable: NAME` field.
    SylTail,
    /// `%syl<#N>%` / `%syl<#{a..b}>%` — syllable index anchor (M parses, F7 evaluates).
    /// Requires the enclosing phonrule to have a `syllable: NAME` field.
    /// The numeric spec is parsed and stored here, but evaluation is deferred
    /// to F7; phase2 currently rejects its use with a "not yet implemented" error.
    SylIndex(SylSpec),
}

/// Numeric/range spec inside a `%syl<#...>%` macro (M parses, F7 evaluates).
///
/// Bounds are 1-indexed; positive values count from the word start and negative
/// values count from the word end (`-1` = last syllable). Ranges are inclusive
/// on both ends.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SylSpec {
    /// `%syl<#N>%` — a single syllable index.
    Index(i64),
    /// `%syl<#{a..b}>%` — an inclusive range; either bound may be omitted.
    Range {
        lo: Option<i64>,
        hi: Option<i64>,
    },
}

// ---------------------------------------------------------------------------
// inflection
// ---------------------------------------------------------------------------

/// An inflection class defining paradigm rules.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Inflection {
    pub name: Ident,
    pub display: DisplayMap,
    /// `for {tense, person, number}`
    pub axes: Vec<Ident>,
    /// `requires stems: pres, past`
    pub required_stems: Vec<StemReq>,
    pub body: InflectionBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StemReq {
    pub name: Ident,
    /// Optional constraint, e.g. `root1[stem_type=consonantal_3]`
    pub constraint: Vec<TagCondition>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InflectionBody {
    /// Simple rule list, optionally with an `apply` phonrule wrapper.
    Rules(RulesBody),
    /// Agglutinative: `compose root + sfx1 + sfx2` with slots and optional overrides.
    Compose(ComposeBody),
}

/// Body for rule-based inflections.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RulesBody {
    /// Optional `apply harmony(cell)` — phonrule applied to every non-delegate cell.
    pub apply: Option<ApplyExpr>,
    pub rules: Vec<InflectionRule>,
}

/// Expression tree for `apply` at the inflection level.
///
/// `apply harmony(elision(cell))` → `PhonApply { rule: harmony, inner: PhonApply { rule: elision, inner: Cell } }`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ApplyExpr {
    /// Terminal: the evaluated cell result.
    Cell,
    /// Phonological rule application wrapping an inner expression.
    PhonApply { rule: Ident, inner: Box<ApplyExpr> },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComposeBody {
    /// Compose expression: `harmony(root + sfx1 + sfx2)` or `root + sfx1 + sfx2`.
    pub chain: ComposeExpr,
    pub slots: Vec<SlotDef>,
    pub overrides: Vec<InflectionRule>,
}

/// Expression tree for compose chains, supporting phonrule application.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComposeExpr {
    /// A single slot reference: `root`, `sfx1`
    Slot(Ident),
    /// Concatenation of elements: `root + sfx1 + sfx2`
    Concat(Vec<ComposeExpr>),
    /// Phonological rule application: `harmony(root + sfx1 + sfx2)`
    PhonApply { rule: Ident, inner: Box<ComposeExpr> },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlotDef {
    pub name: Ident,
    pub rules: Vec<InflectionRule>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InflectionRule {
    pub condition: TagConditionList,
    pub rhs: Spanned<RuleRhs>,
}

/// Tag condition list: `[tense=present, person=1, _]`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagConditionList {
    pub conditions: Vec<TagCondition>,
    /// Whether `_` (wildcard) is present at the end.
    pub wildcard: bool,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagCondition {
    pub axis: Ident,
    pub value: Ident,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuleRhs {
    /// Template literal: `` `{pres}e` ``
    Template(Template),
    /// `null` — form does not exist.
    Null,
    /// Delegation to another inflection.
    Delegate(Delegate),
    /// Phonological rule application: `harmony(`{root}ler`)`
    PhonApply {
        rule: Ident,
        inner: Box<Spanned<RuleRhs>>,
    },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Delegate {
    /// Target inflection name.
    pub target: Ident,
    /// Tag arguments: mix of fixed (`case=nominative`) and pass-through (`case`).
    pub tags: Vec<DelegateTag>,
    /// `with stems { nom: nom_f, ... }`
    pub stem_mapping: Vec<StemMapping>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DelegateTag {
    /// `case=nominative` — fixed value.
    Fixed(TagCondition),
    /// `case` — pass-through from caller.
    PassThrough(Ident),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StemMapping {
    /// Stem name in the delegate target.
    pub target_stem: Ident,
    /// Source: a stem reference or a literal string value.
    pub source: StemSource,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StemSource {
    /// Reference to a stem in the caller.
    Stem(Ident),
    /// Literal string value.
    Literal(StringLit),
}

// ---------------------------------------------------------------------------
// entry
// ---------------------------------------------------------------------------

/// A dictionary entry definition.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    pub name: Ident,
    pub headword: Headword,
    pub tags: Vec<TagCondition>,
    pub stems: Vec<StemDef>,
    pub inflection: Option<EntryInflection>,
    pub meaning: MeaningDef,
    pub forms_override: Vec<InflectionRule>,
    pub etymology: Option<Etymology>,
    pub examples: Vec<Example>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Headword {
    /// Simple: `headword: "faren"`
    Simple(StringLit),
    /// Multi-script: `headword { default: "食べる", kana: "たべる" }`
    MultiScript(Vec<(Ident, StringLit)>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StemDef {
    pub name: Ident,
    pub value: StringLit,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryInflection {
    /// `inflection_class: strong_I`
    Class(Ident),
    /// Inline `inflect for {axes} { rules }`
    Inline(InlineInflection),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InlineInflection {
    pub axes: Vec<Ident>,
    pub body: InflectionBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MeaningDef {
    /// Single meaning: `meaning: "to go"`
    Single(StringLit),
    /// Multiple meanings: `meanings { motion { "to go" } progress { "to proceed" } }`
    Multiple(Vec<MeaningEntry>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MeaningEntry {
    pub ident: Ident,
    pub text: StringLit,
}

// ---------------------------------------------------------------------------
// entry — etymology
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Etymology {
    pub proto: Option<StringLit>,
    pub cognates: Vec<Cognate>,
    pub derived_from: Option<EntryRef>,
    pub note: Option<StringLit>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cognate {
    pub entry: EntryRef,
    pub note: StringLit,
}

// ---------------------------------------------------------------------------
// entry — examples
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Example {
    pub tokens: Vec<Token>,
    pub translation: StringLit,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    /// Entry reference with optional form spec: `faren[tense=present, ...]`
    Ref(EntryRef),
    /// Plain string: `"."`, `"die Tür"`
    Lit(StringLit),
    /// `~` — glue marker: suppresses separator between adjacent tokens.
    Glue,
    /// `//` — newline marker: inserts a line break in rendered output.
    Newline,
    /// `<em>...</em>` — XML-like tag wrapping child tokens.
    Tag { name: String, attrs: Vec<(String, String)>, children: Vec<Token>, span: Span },
    /// `<br/>` — self-closing XML-like tag.
    SelfClosingTag { name: String, attrs: Vec<(String, String)>, span: Span },
    /// `f(token_seq)` — inline phonrule call (F1c). The inner sequence is
    /// recursively resolved and then `rule` is applied to each emerging
    /// phonological word. The outer `@apply` stack is **not** propagated into
    /// the inner sequence: the explicit `rule` overrides any ambient applies.
    PhonCall { rule: Ident, inner: Vec<Token>, span: Span },
    /// `@apply IDENT { token_seq }` — scoped `@apply` block (F1c). While
    /// evaluating `inner`, `rule` is pushed onto the active apply stack so
    /// that file-level + nested `@apply` rules are chained left-to-right.
    ApplyBlock { rule: Ident, inner: Vec<Token>, span: Span },
}

// ---------------------------------------------------------------------------
// Entry reference (shared)
// ---------------------------------------------------------------------------

/// Parsed `.hut` file: leading `@reference` / `@use` / `@apply` directives
/// (in any order) followed by a token list.
///
/// `apply_chain` records the file-level `@apply IDENT` directives in
/// declaration order (F1b). At render time each phonological word (a maximal
/// run of `~`-connected tokens) is passed through the chain via
/// [`apply_phonrule_with_resolver`]. Empty `apply_chain` preserves legacy
/// `.hut` semantics exactly.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HutFile {
    pub references: Vec<Import>,
    pub uses: Vec<Import>,
    pub apply_chain: Vec<Ident>,
    pub tokens: Vec<Token>,
    /// F4: Top-level item declarations (phonrule / phoneme / syllable) parsed
    /// inline from the `.hut` file or from `-e` eval sources. These are
    /// injected into the virtual file's symbol scope by [`HutPhonContext::build`].
    /// Empty for legacy `.hut` files.
    #[cfg_attr(feature = "serialization", serde(default))]
    pub inline_items: Vec<Spanned<Item>>,
}

/// Fully qualified entry reference:
/// `(<namespace>.)* <entry_id> (#<meaning>)? ([<form_spec>])? | ([$=<stem>])?`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryRef {
    pub namespace: Vec<Ident>,
    pub entry_id: Ident,
    pub meaning: Option<Ident>,
    pub form_spec: Option<TagConditionList>,
    /// `[$=stem_name]` — extract a raw stem value instead of an inflected form.
    pub stem_spec: Option<Ident>,
    pub span: Span,
}
