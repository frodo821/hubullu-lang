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

/// A node annotated with source span.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub segments: Vec<TemplateSegment>,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub items: Vec<Spanned<Item>>,
}

/// A top-level item in a `.hu` file.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Use(Import),
    Reference(Import),
    Export(Export),
    TagAxis(TagAxis),
    Extend(Extend),
    Inflection(Inflection),
    Entry(Entry),
    PhonRule(PhonRule),
    Render(RenderConfig),
}

/// Configuration for `.hut` token rendering.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    pub separator: Option<StringLit>,
    pub no_separator_before: Option<StringLit>,
}

// ---------------------------------------------------------------------------
// @use / @reference
// ---------------------------------------------------------------------------

/// An `@use` or `@reference` import statement.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub target: ImportTarget,
    pub path: StringLit,
}

/// What is being imported.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// `*` or `* as ns`
    Glob { alias: Option<Ident> },
    /// Named list, e.g. `tense, aspect as a` or `(tense, aspect as a)`
    Named(Vec<ImportEntry>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: Ident,
    pub alias: Option<Ident>,
}

// ---------------------------------------------------------------------------
// @export
// ---------------------------------------------------------------------------

/// An `@export` directive that re-exports symbols transitively.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagAxis {
    pub name: Ident,
    pub role: Spanned<Role>,
    pub display: DisplayMap,
    pub index: Option<Spanned<IndexKind>>,
}

/// Role of a tag axis.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inflectional,
    Classificatory,
    Structural,
}

/// Kind of search index for a tag axis.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extend {
    pub name: Ident,
    pub target_axis: Ident,
    pub values: Vec<ExtendValue>,
}

/// A single value within an `@extend` block.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonRule {
    pub name: Ident,
    pub classes: Vec<CharClassDef>,
    pub maps: Vec<PhonMapDef>,
    pub rules: Vec<PhonRewriteRule>,
    pub span: Span,
}

/// `class front = ["e", "i"]` or `class V = front | back`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharClassDef {
    pub name: Ident,
    pub body: CharClassBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharClassBody {
    /// Literal list: `["e", "i", "ö", "ü"]`
    List(Vec<StringLit>),
    /// Union of other classes: `front | back`
    Union(Vec<Ident>),
}

/// `map to_back = c -> match { "e" -> "a", else -> c }`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonMapDef {
    pub name: Ident,
    pub param: Ident,
    pub body: PhonMapBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapBody {
    Match {
        arms: Vec<PhonMapArm>,
        else_arm: Option<PhonMapElse>,
    },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonMapArm {
    pub from: StringLit,
    pub to: PhonMapResult,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapResult {
    Literal(StringLit),
    Var(Ident),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapElse {
    Literal(StringLit),
    Var(Ident),
}

/// A phonological rewrite rule: `V -> to_back / back !back* + !back* _`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonRewriteRule {
    pub from: PhonPattern,
    pub to: PhonReplacement,
    pub context: Option<PhonContext>,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonPattern {
    Class(Ident),
    Literal(StringLit),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonReplacement {
    Map(Ident),
    Literal(StringLit),
    Null,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonContext {
    pub left: Vec<PhonContextElem>,
    pub right: Vec<PhonContextElem>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonContextElem {
    Class(Ident),
    NegClass(Ident),
    Boundary,
    WordStart,
    WordEnd,
    Literal(StringLit),
    Repeat(Box<PhonContextElem>),
    Alt(Vec<PhonContextElem>),
}

// ---------------------------------------------------------------------------
// inflection
// ---------------------------------------------------------------------------

/// An inflection class defining paradigm rules.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemReq {
    pub name: Ident,
    /// Optional constraint, e.g. `root1[stem_type=consonantal_3]`
    pub constraint: Vec<TagCondition>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflectionBody {
    /// Simple rule list, optionally with an `apply` phonrule wrapper.
    Rules(RulesBody),
    /// Agglutinative: `compose root + sfx1 + sfx2` with slots and optional overrides.
    Compose(ComposeBody),
}

/// Body for rule-based inflections.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesBody {
    /// Optional `apply harmony(cell)` — phonrule applied to every non-delegate cell.
    pub apply: Option<ApplyExpr>,
    pub rules: Vec<InflectionRule>,
}

/// Expression tree for `apply` at the inflection level.
///
/// `apply harmony(elision(cell))` → `PhonApply { rule: harmony, inner: PhonApply { rule: elision, inner: Cell } }`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyExpr {
    /// Terminal: the evaluated cell result.
    Cell,
    /// Phonological rule application wrapping an inner expression.
    PhonApply { rule: Ident, inner: Box<ApplyExpr> },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeBody {
    /// Compose expression: `harmony(root + sfx1 + sfx2)` or `root + sfx1 + sfx2`.
    pub chain: ComposeExpr,
    pub slots: Vec<SlotDef>,
    pub overrides: Vec<InflectionRule>,
}

/// Expression tree for compose chains, supporting phonrule application.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeExpr {
    /// A single slot reference: `root`, `sfx1`
    Slot(Ident),
    /// Concatenation of elements: `root + sfx1 + sfx2`
    Concat(Vec<ComposeExpr>),
    /// Phonological rule application: `harmony(root + sfx1 + sfx2)`
    PhonApply { rule: Ident, inner: Box<ComposeExpr> },
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDef {
    pub name: Ident,
    pub rules: Vec<InflectionRule>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InflectionRule {
    pub condition: TagConditionList,
    pub rhs: Spanned<RuleRhs>,
}

/// Tag condition list: `[tense=present, person=1, _]`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagConditionList {
    pub conditions: Vec<TagCondition>,
    /// Whether `_` (wildcard) is present at the end.
    pub wildcard: bool,
    pub span: Span,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCondition {
    pub axis: Ident,
    pub value: Ident,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    /// Target inflection name.
    pub target: Ident,
    /// Tag arguments: mix of fixed (`case=nominative`) and pass-through (`case`).
    pub tags: Vec<DelegateTag>,
    /// `with stems { nom: nom_f, ... }`
    pub stem_mapping: Vec<StemMapping>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegateTag {
    /// `case=nominative` — fixed value.
    Fixed(TagCondition),
    /// `case` — pass-through from caller.
    PassThrough(Ident),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemMapping {
    /// Stem name in the delegate target.
    pub target_stem: Ident,
    /// Source: a stem reference or a literal string value.
    pub source: StemSource,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Headword {
    /// Simple: `headword: "faren"`
    Simple(StringLit),
    /// Multi-script: `headword { default: "食べる", kana: "たべる" }`
    MultiScript(Vec<(Ident, StringLit)>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemDef {
    pub name: Ident,
    pub value: StringLit,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryInflection {
    /// `inflection_class: strong_I`
    Class(Ident),
    /// Inline `inflect for {axes} { rules }`
    Inline(InlineInflection),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineInflection {
    pub axes: Vec<Ident>,
    pub body: InflectionBody,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeaningDef {
    /// Single meaning: `meaning: "to go"`
    Single(StringLit),
    /// Multiple meanings: `meanings { motion { "to go" } progress { "to proceed" } }`
    Multiple(Vec<MeaningEntry>),
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeaningEntry {
    pub ident: Ident,
    pub text: StringLit,
}

// ---------------------------------------------------------------------------
// entry — etymology
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Etymology {
    pub proto: Option<StringLit>,
    pub cognates: Vec<Cognate>,
    pub derived_from: Option<EntryRef>,
    pub note: Option<StringLit>,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cognate {
    pub entry: EntryRef,
    pub note: StringLit,
}

// ---------------------------------------------------------------------------
// entry — examples
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    pub tokens: Vec<Token>,
    pub translation: StringLit,
}

#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

// ---------------------------------------------------------------------------
// Entry reference (shared)
// ---------------------------------------------------------------------------

/// Parsed `.hut` file: `@reference` imports followed by a token list.
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HutFile {
    pub references: Vec<Import>,
    pub tokens: Vec<Token>,
}

/// Fully qualified entry reference:
/// `(<namespace>.)* <entry_id> (#<meaning>)? ([<form_spec>])? | ([$=<stem>])?`
#[cfg_attr(feature = "serialization", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRef {
    pub namespace: Vec<Ident>,
    pub entry_id: Ident,
    pub meaning: Option<Ident>,
    pub form_spec: Option<TagConditionList>,
    /// `[$=stem_name]` — extract a raw stem value instead of an inflected form.
    pub stem_spec: Option<Ident>,
    pub span: Span,
}
