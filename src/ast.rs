//! LexDSL Abstract Syntax Tree definitions.
//!
//! Two string types exist in the DSL:
//! - `StringLit` (`"..."`) — plain text, no interpolation
//! - `Template` (`` `...` ``) — interpolation via `{name}` referencing stems/slots

// ---------------------------------------------------------------------------
// Span & common types
// ---------------------------------------------------------------------------

/// Byte offset range into source for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub file_id: crate::span::FileId,
    pub start: usize,
    pub end: usize,
}

/// A node annotated with source span.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub segments: Vec<TemplateSegment>,
    pub span: Span,
}

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub items: Vec<Spanned<Item>>,
}

/// A top-level item in a `.hu` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Use(Import),
    Reference(Import),
    TagAxis(TagAxis),
    Extend(Extend),
    Inflection(Inflection),
    Entry(Entry),
    PhonRule(PhonRule),
    Render(RenderConfig),
}

/// Configuration for `.hut` token rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    pub separator: Option<StringLit>,
    pub no_separator_before: Option<StringLit>,
}

// ---------------------------------------------------------------------------
// @use / @reference
// ---------------------------------------------------------------------------

/// An `@use` or `@reference` import statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub target: ImportTarget,
    pub path: StringLit,
}

/// What is being imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// `*` or `* as ns`
    Glob { alias: Option<Ident> },
    /// Named list, e.g. `tense, aspect as a` or `(tense, aspect as a)`
    Named(Vec<ImportEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: Ident,
    pub alias: Option<Ident>,
}

// ---------------------------------------------------------------------------
// tagaxis
// ---------------------------------------------------------------------------

/// A `tagaxis` declaration defining a grammatical dimension (e.g. tense, number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagAxis {
    pub name: Ident,
    pub role: Spanned<Role>,
    pub display: DisplayMap,
    pub index: Option<Spanned<IndexKind>>,
}

/// Role of a tag axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inflectional,
    Classificatory,
    Structural,
}

/// Kind of search index for a tag axis.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extend {
    pub name: Ident,
    pub target_axis: Ident,
    pub values: Vec<ExtendValue>,
}

/// A single value within an `@extend` block.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonRule {
    pub name: Ident,
    pub classes: Vec<CharClassDef>,
    pub maps: Vec<PhonMapDef>,
    pub rules: Vec<PhonRewriteRule>,
    pub span: Span,
}

/// `class front = ["e", "i"]` or `class V = front | back`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharClassDef {
    pub name: Ident,
    pub body: CharClassBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharClassBody {
    /// Literal list: `["e", "i", "ö", "ü"]`
    List(Vec<StringLit>),
    /// Union of other classes: `front | back`
    Union(Vec<Ident>),
}

/// `map to_back = c -> match { "e" -> "a", else -> c }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonMapDef {
    pub name: Ident,
    pub param: Ident,
    pub body: PhonMapBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapBody {
    Match {
        arms: Vec<PhonMapArm>,
        else_arm: Option<PhonMapElse>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonMapArm {
    pub from: StringLit,
    pub to: PhonMapResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapResult {
    Literal(StringLit),
    Var(Ident),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonMapElse {
    Literal(StringLit),
    Var(Ident),
}

/// A phonological rewrite rule: `V -> to_back / back !back* + !back* _`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonRewriteRule {
    pub from: PhonPattern,
    pub to: PhonReplacement,
    pub context: Option<PhonContext>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonPattern {
    Class(Ident),
    Literal(StringLit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonReplacement {
    Map(Ident),
    Literal(StringLit),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonContext {
    pub left: Vec<PhonContextElem>,
    pub right: Vec<PhonContextElem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonContextElem {
    Class(Ident),
    NegClass(Ident),
    Boundary,
    Literal(StringLit),
    Repeat(Box<PhonContextElem>),
}

// ---------------------------------------------------------------------------
// inflection
// ---------------------------------------------------------------------------

/// An inflection class defining paradigm rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inflection {
    pub name: Ident,
    /// `for {tense, person, number}`
    pub axes: Vec<Ident>,
    /// `requires stems: pres, past`
    pub required_stems: Vec<StemReq>,
    pub body: InflectionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemReq {
    pub name: Ident,
    /// Optional constraint, e.g. `root1[stem_type=consonantal_3]`
    pub constraint: Vec<TagCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflectionBody {
    /// Simple rule list.
    Rules(Vec<InflectionRule>),
    /// Agglutinative: `compose root + sfx1 + sfx2` with slots and optional overrides.
    Compose(ComposeBody),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeBody {
    /// Compose expression: `harmony(root + sfx1 + sfx2)` or `root + sfx1 + sfx2`.
    pub chain: ComposeExpr,
    pub slots: Vec<SlotDef>,
    pub overrides: Vec<InflectionRule>,
}

/// Expression tree for compose chains, supporting phonrule application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeExpr {
    /// A single slot reference: `root`, `sfx1`
    Slot(Ident),
    /// Concatenation of elements: `root + sfx1 + sfx2`
    Concat(Vec<ComposeExpr>),
    /// Phonological rule application: `harmony(root + sfx1 + sfx2)`
    PhonApply { rule: Ident, inner: Box<ComposeExpr> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDef {
    pub name: Ident,
    pub rules: Vec<InflectionRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InflectionRule {
    pub condition: TagConditionList,
    pub rhs: Spanned<RuleRhs>,
}

/// Tag condition list: `[tense=present, person=1, _]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagConditionList {
    pub conditions: Vec<TagCondition>,
    /// Whether `_` (wildcard) is present at the end.
    pub wildcard: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCondition {
    pub axis: Ident,
    pub value: Ident,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    /// Target inflection name.
    pub target: Ident,
    /// Tag arguments: mix of fixed (`case=nominative`) and pass-through (`case`).
    pub tags: Vec<DelegateTag>,
    /// `with stems { nom: nom_f, ... }`
    pub stem_mapping: Vec<StemMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegateTag {
    /// `case=nominative` — fixed value.
    Fixed(TagCondition),
    /// `case` — pass-through from caller.
    PassThrough(Ident),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemMapping {
    /// Stem name in the delegate target.
    pub target_stem: Ident,
    /// Stem name in the caller.
    pub source_stem: Ident,
}

// ---------------------------------------------------------------------------
// entry
// ---------------------------------------------------------------------------

/// A dictionary entry definition.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Headword {
    /// Simple: `headword: "faren"`
    Simple(StringLit),
    /// Multi-script: `headword { default: "食べる", kana: "たべる" }`
    MultiScript(Vec<(Ident, StringLit)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemDef {
    pub name: Ident,
    pub value: StringLit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryInflection {
    /// `inflection_class: strong_I`
    Class(Ident),
    /// Inline `inflect for {axes} { rules }`
    Inline(InlineInflection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineInflection {
    pub axes: Vec<Ident>,
    pub body: InflectionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeaningDef {
    /// Single meaning: `meaning: "to go"`
    Single(StringLit),
    /// Multiple meanings: `meanings { motion { "to go" } progress { "to proceed" } }`
    Multiple(Vec<MeaningEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeaningEntry {
    pub ident: Ident,
    pub text: StringLit,
}

// ---------------------------------------------------------------------------
// entry — etymology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Etymology {
    pub proto: Option<StringLit>,
    pub cognates: Vec<Cognate>,
    pub derived_from: Option<EntryRef>,
    pub note: Option<StringLit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cognate {
    pub entry: EntryRef,
    pub note: StringLit,
}

// ---------------------------------------------------------------------------
// entry — examples
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    pub tokens: Vec<Token>,
    pub translation: StringLit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Entry reference with optional form spec: `faren[tense=present, ...]`
    Ref(EntryRef),
    /// Plain string: `"."`, `"die Tür"`
    Lit(StringLit),
}

// ---------------------------------------------------------------------------
// Entry reference (shared)
// ---------------------------------------------------------------------------

/// Fully qualified entry reference:
/// `(<namespace>.)* <entry_id> (#<meaning>)? ([<form_spec>])?`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRef {
    pub namespace: Vec<Ident>,
    pub entry_id: Ident,
    pub meaning: Option<Ident>,
    pub form_spec: Option<TagConditionList>,
    pub span: Span,
}
