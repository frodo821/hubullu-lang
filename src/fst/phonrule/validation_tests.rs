//! F2c5.4 validation harness — strict 100% gate against `phonrule_eval`.
//!
//! The acceptance criterion for F2c5: for every (phonrule, input) pair in
//! the validation corpus, the FST-based engine
//! (`compile_phonrule` → `apply_phonrule_fst`) and the legacy
//! `phonrule_eval::apply_phonrule` produce **byte-identical** output
//! strings.
//!
//! ## Corpus shape
//!
//! Per the plan §6 and the F2c5 task:
//!
//!   * **Golden corpus** — hand-curated rules + inputs covering
//!     - empty input
//!     - no-LHS-occurrence inputs
//!     - single LHS-match with context
//!     - single LHS-match without context
//!     - multiple LHS-matches
//!     - longest-match cases
//!     - deletion (null RHS)
//!     - map application
//!     - boundary markers
//!     - cascading harmony-style rules
//!
//!   * **Fuzz corpus** — for each rule, deterministic-seeded random
//!     inputs over its alphabet. Bounded LCG seeded with the rule's
//!     declared name so failures reproduce exactly.
//!
//! ## Equality contract
//!
//! Both engines operate on `&str` containing literal char data plus
//! `BOUNDARY = '\0'` morpheme separators. We compare the two returned
//! strings byte-for-byte.
//!
//! ## What this module deliberately does NOT do
//!
//!   * Test Turkish harmony / elision **end-to-end** at the renderer
//!     level — that's F4+. We test the rules *in isolation* against
//!     `phonrule_eval` on synthesised inputs.
//!
//!   * Test syllable-aware rules (`%syl[...]%`, `%syl<head>%`, etc.) —
//!     F2 non-goal 5. The corpus excludes them; the rule_seq compile
//!     would emit `UnsupportedLhsShape` for them anyway.

use std::collections::HashMap;

use crate::ast::{
    CharClassBody, CharClassDef, PhonAtom, PhonBodyItem, PhonContext, PhonContextElem,
    PhonMapArm, PhonMapBody, PhonMapDef, PhonMapElse, PhonMapResult, PhonPattern, PhonReplacement,
    PhonRewriteRule, PhonRule, Quantifier, Span, Spanned, StringLit,
};
use crate::phonrule_eval::apply_phonrule;
use crate::span::FileId;

use super::super::alphabet::PhonruleAlphabet;
use super::apply::{apply_phonrule_fst, MAX_PHONRULE_ITER};
use super::rule_seq::compile_phonrule;

// ---------------------------------------------------------------------------
// AST helpers.
// ---------------------------------------------------------------------------

fn sp() -> Span {
    Span { file_id: FileId(0), start: 0, end: 0 }
}

fn ident(s: &str) -> Spanned<String> {
    Spanned::new(s.to_string(), sp())
}

fn lit(s: &str) -> StringLit {
    Spanned::new(s.to_string(), sp())
}

fn ctx_literal(s: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Literal(lit(s)), Quantifier::Exact(1))
}

fn ctx_class(name: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::Class(ident(name)), Quantifier::Exact(1))
}

#[allow(dead_code)]
fn ctx_neg_class(name: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::NegClass(ident(name)), Quantifier::Exact(1))
}

fn ctx_neg_class_star(name: &str) -> PhonContextElem {
    PhonContextElem::Atom(PhonAtom::NegClass(ident(name)), Quantifier::Star)
}

fn ctx_boundary() -> PhonContextElem {
    PhonContextElem::Boundary
}

fn class_list(name: &str, members: &[&str]) -> CharClassDef {
    CharClassDef {
        name: ident(name),
        body: CharClassBody::List(members.iter().map(|m| lit(m)).collect()),
    }
}

/// `class <name> = a | b | ...` — a union of previously-defined classes.
fn class_union(name: &str, parts: &[&str]) -> CharClassDef {
    CharClassDef {
        name: ident(name),
        body: CharClassBody::Union(parts.iter().map(|p| ident(p)).collect()),
    }
}

/// Build a `map <name> = c -> match { from -> to, ..., else -> c }` with an
/// identity (`else -> c`) fallthrough. Each arm rewrites a single literal to a
/// single literal.
fn map_match_else_identity(name: &str, arms: &[(&str, &str)]) -> PhonMapDef {
    PhonMapDef {
        name: ident(name),
        param: ident("c"),
        body: PhonMapBody::Match {
            arms: arms
                .iter()
                .map(|(from, to)| PhonMapArm {
                    from: lit(from),
                    to: PhonMapResult::Literal(lit(to)),
                })
                .collect(),
            else_arm: Some(PhonMapElse::Var(ident("c"))),
        },
    }
}

fn rule_lit_to_lit_no_ctx(from: &str, to: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Literal(lit(to)),
        context: None,
        span: sp(),
    }
}

fn rule_lit_to_lit_with_ctx(
    from: &str,
    to: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Literal(lit(to)),
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

fn rule_lit_to_null(from: &str) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Null,
        context: None,
        span: sp(),
    }
}

fn rule_lit_to_null_with_ctx(
    from: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Literal(lit(from)),
        to: PhonReplacement::Null,
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

fn rule_class_to_lit_with_ctx(
    class_name: &str,
    to: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Class(ident(class_name)),
        to: PhonReplacement::Literal(lit(to)),
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

fn rule_class_to_map_with_ctx(
    class_name: &str,
    map_name: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Class(ident(class_name)),
        to: PhonReplacement::Map(ident(map_name)),
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

fn make_phonrule(
    name: &str,
    classes: Vec<CharClassDef>,
    maps: Vec<PhonMapDef>,
    body: Vec<PhonBodyItem>,
) -> PhonRule {
    PhonRule {
        name: ident(name),
        display: Default::default(),
        derived_from: None,
        syllable: None,
        classes,
        maps,
        body,
        span: sp(),
    }
}

// ---------------------------------------------------------------------------
// Differential harness.
// ---------------------------------------------------------------------------

/// Empty resolver — for phonrules without `apply` chains.
struct EmptyResolver;
impl super::rule_seq::PhonRuleAstResolver for EmptyResolver {
    fn resolve(&self, _name: &str) -> Option<&PhonRule> {
        None
    }
}

/// Build an alphabet pre-populated with the characters used by the
/// corpus inputs. Per the plan §5, the Karttunen `@->` chain needs a
/// **closed** alphabet at compile time; chars that appear in inputs
/// but not in any rule literal must be interned before compilation
/// so they're recognised as Σ members.
fn alpha_for(corpus: &[&str]) -> PhonruleAlphabet {
    let mut alpha = PhonruleAlphabet::empty();
    for s in corpus {
        for ch in s.chars() {
            if ch == crate::phonrule_eval::BOUNDARY {
                continue; // reserved label, not in Σ
            }
            alpha.intern(&ch.to_string());
        }
    }
    alpha
}

/// Run a rule against many inputs; assert byte-identical equality with
/// `phonrule_eval` on every one. Prints diffs on failure and panics if
/// any fail.
///
/// **Performance**: compiles the FST exactly once per rule, then reuses
/// it across all inputs. The alphabet is pre-populated with every char
/// in the corpus so the compiled FST closes over the full Σ.
#[track_caller]
fn assert_all_match(phonrule: &PhonRule, inputs: &[&str]) {
    // Compile once per rule.
    let mut alpha = alpha_for(inputs);
    let resolver = EmptyResolver;
    let fst = match compile_phonrule(phonrule, &resolver, &mut alpha) {
        Ok(fst) => fst,
        Err(e) => panic!("compile_phonrule failed: {}", e),
    };
    let mut failures: Vec<(String, String, String)> = Vec::new();
    for &input in inputs {
        let expected = apply_phonrule(input, phonrule);
        let actual = match apply_phonrule_fst(&fst, input, &mut alpha, MAX_PHONRULE_ITER) {
            Ok(s) => s,
            Err(e) => format!("<FST ERROR: {}>", e),
        };
        if expected != actual {
            failures.push((input.to_string(), expected, actual));
        }
    }
    if !failures.is_empty() {
        let mut msg = format!(
            "phonrule '{}': {}/{} inputs disagreed with phonrule_eval:\n",
            phonrule.name.node,
            failures.len(),
            inputs.len()
        );
        // Cap the printed failures so the panic message stays
        // reasonable on a fuzz blow-up.
        for (input, exp, act) in failures.iter().take(20) {
            msg.push_str(&format!(
                "  input={:?}\n    expected={:?}\n    actual  ={:?}\n",
                input, exp, act
            ));
        }
        if failures.len() > 20 {
            msg.push_str(&format!("  ... and {} more failures\n", failures.len() - 20));
        }
        panic!("{}", msg);
    }
}

/// Same as [`assert_all_match`] but with a resolver for `apply` chains.
#[track_caller]
fn assert_all_match_with_resolver(
    phonrule: &PhonRule,
    resolver: &dyn super::rule_seq::PhonRuleAstResolver,
    inputs: &[&str],
) {
    use crate::phonrule_eval::apply_phonrule_with_resolver;
    use crate::inflection_eval::PhonRuleResolver;
    use crate::phoneme::PhonemeInventory;

    // Bridge: phonrule_eval needs a PhonRuleResolver; we have a
    // PhonRuleAstResolver. Build an ad-hoc bridge.
    struct Bridge<'a>(&'a dyn super::rule_seq::PhonRuleAstResolver);
    impl<'a> PhonRuleResolver for Bridge<'a> {
        fn resolve(&self, name: &str) -> Option<&PhonRule> {
            self.0.resolve(name)
        }
        fn inventory(&self) -> Option<&PhonemeInventory> {
            None
        }
    }

    let bridge = Bridge(resolver);
    // Compile once.
    let mut alpha = alpha_for(inputs);
    let fst = match compile_phonrule(phonrule, resolver, &mut alpha) {
        Ok(fst) => fst,
        Err(e) => panic!("compile_phonrule failed: {}", e),
    };
    let mut failures: Vec<(String, String, String)> = Vec::new();
    for &input in inputs {
        let expected = apply_phonrule_with_resolver(input, phonrule, &bridge)
            .unwrap_or_else(|_| input.to_string());
        let actual = match apply_phonrule_fst(&fst, input, &mut alpha, MAX_PHONRULE_ITER) {
            Ok(s) => s,
            Err(e) => format!("<FST ERROR: {}>", e),
        };
        if expected != actual {
            failures.push((input.to_string(), expected, actual));
        }
    }
    if !failures.is_empty() {
        let mut msg = format!(
            "phonrule '{}' (with resolver): {}/{} inputs disagreed:\n",
            phonrule.name.node,
            failures.len(),
            inputs.len()
        );
        for (input, exp, act) in &failures {
            msg.push_str(&format!(
                "  input={:?}\n    expected={:?}\n    actual  ={:?}\n",
                input, exp, act
            ));
        }
        panic!("{}", msg);
    }
}

// ---------------------------------------------------------------------------
// Deterministic fuzz over a small alphabet.
// ---------------------------------------------------------------------------

/// Simple LCG seeded by a string. Deterministic so failures reproduce.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed_str: &str) -> Self {
        // FNV-1a hash of seed_str for a stable seed.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in seed_str.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Self { state: h.max(1) }
    }
    fn next_u64(&mut self) -> u64 {
        // Numerical Recipes LCG.
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = (self.next_u64() as usize) % items.len();
        &items[idx]
    }
    fn next_len(&mut self, min: usize, max: usize) -> usize {
        let range = (max - min + 1) as u64;
        min + ((self.next_u64() % range) as usize)
    }
}

/// Generate `count` random inputs over the given chars (possibly with
/// BOUNDARY mixed in). Lengths between `min_len` and `max_len`.
fn fuzz_inputs(seed: &str, chars: &[char], count: usize, min_len: usize, max_len: usize) -> Vec<String> {
    let mut rng = Lcg::new(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = rng.next_len(min_len, max_len);
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(*rng.pick(chars));
        }
        out.push(s);
    }
    out
}

// ---------------------------------------------------------------------------
// Test 1 — Simple no-context literal rewrite.
// ---------------------------------------------------------------------------

#[test]
fn val_simple_no_context_a_to_b() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("a", "b"))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "b",
            "ab",
            "ba",
            "aaa",
            "aba",
            "abc",
            "z",
            "zz",
            "azaza",
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Single-context literal rewrite.
// ---------------------------------------------------------------------------

#[test]
fn val_single_context_xay_to_xby() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_with_ctx(
            "a",
            "b",
            vec![ctx_literal("x")],
            vec![ctx_literal("y")],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "xa",
            "ay",
            "xay",
            "xayxay",
            "xaay",
            "xax",
            "yax",
            "zzz",
            "xayz",
            "zxay",
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Class LHS.
// ---------------------------------------------------------------------------

#[test]
fn val_class_lhs_to_lit() {
    let rule = make_phonrule(
        "test",
        vec![class_list("V", &["a", "e", "i"])],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_class_to_lit_with_ctx(
            "V",
            "o",
            vec![ctx_literal("x")],
            vec![ctx_literal("y")],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "xay",
            "xey",
            "xiy",
            "xoy",
            "xby",
            "xayxey",
            "axe",
            "xaeiy",
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 4 — Deletion (null RHS).
// ---------------------------------------------------------------------------

#[test]
fn val_deletion_no_context() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_null("a"))],
    );
    assert_all_match(
        &rule,
        &["", "a", "aa", "aaa", "ba", "ab", "bab", "bababab", "zzz"],
    );
}

// ---------------------------------------------------------------------------
// Test 5 — Deletion with boundary context (Turkish-elision-like).
// ---------------------------------------------------------------------------

#[test]
fn val_elision_vowel_after_vowel_boundary() {
    // Roughly: V -> null / V + _   where + is the boundary marker.
    let rule = make_phonrule(
        "test",
        vec![class_list("V", &["a", "e", "i", "o", "u"])],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_null_with_ctx(
            "a",
            vec![ctx_class("V"), ctx_boundary()],
            vec![],
        ))],
    );
    // BOUNDARY = '\0'.
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "ba",
            "a\0a",
            "ba\0a",
            "ba\0ax",
            "be\0ax",
            "bi\0ay",
            "bo\0ax",
            "bx\0ax",
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 6 — Map RHS.
// ---------------------------------------------------------------------------

#[test]
fn val_map_rhs_single_arm_else_identity() {
    // map to_e = c -> match { "a" -> "e", else -> c }
    let map_def = PhonMapDef {
        name: ident("to_e"),
        param: ident("c"),
        body: PhonMapBody::Match {
            arms: vec![PhonMapArm {
                from: lit("a"),
                to: PhonMapResult::Literal(lit("e")),
            }],
            else_arm: Some(PhonMapElse::Var(ident("c"))),
        },
    };
    let rule = make_phonrule(
        "test",
        vec![class_list("V", &["a", "e", "i"])],
        vec![map_def],
        vec![PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "V",
            "to_e",
            vec![],
            vec![],
        ))],
    );
    assert_all_match(
        &rule,
        &["", "a", "e", "i", "aei", "xax", "zzz", "aaa", "iae"],
    );
}

// ---------------------------------------------------------------------------
// Test 7 — Sequence of rewrites.
// ---------------------------------------------------------------------------

#[test]
fn val_sequence_two_rewrites() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("a", "b")),
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("b", "c")),
        ],
    );
    assert_all_match(
        &rule,
        &["", "a", "b", "c", "ab", "abc", "aaaa", "xyz"],
    );
}

// ---------------------------------------------------------------------------
// Test 8 — Apply chain.
// ---------------------------------------------------------------------------

#[test]
fn val_apply_chain_two_phonrules() {
    let inner = make_phonrule(
        "inner",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("a", "b"))],
    );
    let outer = make_phonrule(
        "outer",
        vec![],
        vec![],
        vec![
            PhonBodyItem::Apply(crate::ast::PhonApply {
                rule: ident("inner"),
                span: sp(),
            }),
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("b", "c")),
        ],
    );
    let mut resolver: HashMap<String, PhonRule> = HashMap::new();
    resolver.insert("inner".to_string(), inner);
    assert_all_match_with_resolver(
        &outer,
        &resolver,
        &["", "a", "b", "c", "ab", "abc", "aaaa", "xyz"],
    );
}

// ---------------------------------------------------------------------------
// Test 9 — Fuzz: simple rule.
// ---------------------------------------------------------------------------

#[test]
fn val_fuzz_simple_rule_1000_inputs() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_with_ctx(
            "a",
            "b",
            vec![ctx_literal("x")],
            vec![ctx_literal("y")],
        ))],
    );
    let inputs = fuzz_inputs("val_fuzz_simple", &['a', 'b', 'x', 'y', 'z'], 1000, 0, 12);
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 10 — Fuzz: class LHS rule.
// ---------------------------------------------------------------------------

#[test]
fn val_fuzz_class_lhs_500_inputs() {
    let rule = make_phonrule(
        "test",
        vec![class_list("V", &["a", "e", "i"])],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_class_to_lit_with_ctx(
            "V",
            "o",
            vec![ctx_literal("x")],
            vec![ctx_literal("y")],
        ))],
    );
    let inputs = fuzz_inputs("val_fuzz_class", &['a', 'e', 'i', 'o', 'x', 'y', 'z'], 500, 0, 12);
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 11 — Fuzz: deletion rule.
// ---------------------------------------------------------------------------

#[test]
fn val_fuzz_deletion_500_inputs() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_null("a"))],
    );
    let inputs = fuzz_inputs("val_fuzz_delete", &['a', 'b', 'c'], 500, 0, 16);
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 12 — Fuzz: rule sequence.
// ---------------------------------------------------------------------------

#[test]
fn val_fuzz_sequence_500_inputs() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("a", "b")),
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("b", "c")),
            PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("c", "d")),
        ],
    );
    let inputs = fuzz_inputs("val_fuzz_seq", &['a', 'b', 'c', 'd', 'e'], 500, 0, 12);
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 13 — Negated class context (Turkish-harmony-like piece).
// ---------------------------------------------------------------------------

#[test]
fn val_neg_class_context_simple() {
    // Class V = {a, e, i, o}.
    // rule: "i" -> "u" / "o" !V* _
    //   i.e., rewrite `i` to `u` when preceded by `o` followed by any number
    //   of non-vowels.
    let rule = make_phonrule(
        "test",
        vec![class_list("V", &["a", "e", "i", "o"])],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_with_ctx(
            "i",
            "u",
            vec![ctx_literal("o"), ctx_neg_class_star("V")],
            vec![],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "i",
            "oi",
            "oki",
            "okxi",
            "oai",  // o followed by V then i: !V* must consume only non-V
            "oki ki", // multi-token
            "abi",  // no preceding o
            "ki",   // no preceding o
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 14 — Boundary-edge cases with BOUNDARY chars.
// ---------------------------------------------------------------------------

#[test]
fn val_fuzz_with_boundary_chars() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("a", "b"))],
    );
    // Mix BOUNDARY ('\0') into the input alphabet.
    let inputs = fuzz_inputs(
        "val_fuzz_bdy",
        &['a', 'b', 'c', '\0'],
        500,
        0,
        12,
    );
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 15a — Turkish elision (real-world golden rule).
// ---------------------------------------------------------------------------

/// Mirror of `examples/turkish/profile.hu`'s `elision` phonrule:
///
///   class V = ["a", "e", "ı", "i", "o", "ö", "u", "ü"]
///   V -> null / V + _
#[test]
fn val_turkish_elision_real_rule() {
    let rule = make_phonrule(
        "elision",
        vec![class_list(
            "V",
            &["a", "e", "ı", "i", "o", "ö", "u", "ü"],
        )],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_class_to_null_with_ctx(
            "V",
            vec![ctx_class("V"), ctx_boundary()],
            vec![],
        ))],
    );
    // Plausible Turkish forms with morpheme boundaries.
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "e\0i",
            "ev\0i",
            "kapı\0ı",   // kapı+ı → kapı (acc, elision fires)
            "araba\0a",  // araba+a → araba
            "su\0u",     // su+u → su (deletes the suffix vowel)
            "kalem\0i",  // kalem+i → kalemi (no elision, stem ends in C)
            "ev",
            "ev\0ler",
            "kapı\0lar",
            "ev\0lar\0e", // multiple boundaries
        ],
    );
}

/// Helper: class-LHS-to-null with context.
fn rule_class_to_null_with_ctx(
    class_name: &str,
    left: Vec<PhonContextElem>,
    right: Vec<PhonContextElem>,
) -> PhonRewriteRule {
    PhonRewriteRule {
        from: PhonPattern::Class(ident(class_name)),
        to: PhonReplacement::Null,
        context: Some(PhonContext { left, right }),
        span: sp(),
    }
}

// ---------------------------------------------------------------------------
// Test 15b — Turkish elision fuzz over real alphabet.
// ---------------------------------------------------------------------------

#[test]
fn val_turkish_elision_fuzz_500_inputs() {
    let rule = make_phonrule(
        "elision",
        vec![class_list(
            "V",
            &["a", "e", "ı", "i", "o", "ö", "u", "ü"],
        )],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_class_to_null_with_ctx(
            "V",
            vec![ctx_class("V"), ctx_boundary()],
            vec![],
        ))],
    );
    // Mix vowels, consonants, and morpheme boundary chars.
    let chars: Vec<char> = "aeıiouöü\0klmnsrv".chars().collect();
    let inputs = fuzz_inputs("val_turkish_elision_fuzz", &chars, 500, 0, 14);
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    assert_all_match(&rule, &refs);
}

// ---------------------------------------------------------------------------
// Test 15b' — Simpler cascading test case.
// ---------------------------------------------------------------------------

/// Synthesised micro-cascade: rule `e -> a / a _` on input `a\0e\0e`.
///
/// Eval iter 1: only the first `e` matches (preceded by `a`); result
/// `a\0a\0e`. Iter 2: second `e` now matches; result `a\0a\0a`.
/// The FST apply driver must reproduce this two-iter convergence.
#[test]
fn val_cascading_rewrite_two_iterations() {
    let rule = make_phonrule(
        "cascading",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_with_ctx(
            "e",
            "a",
            vec![ctx_literal("a")],
            vec![],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "e",
            "ae",
            "aee",
            "aeae",
            "a\0e",
            "a\0e\0e",
            "ae\0e",
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 15c — Harmony-shape rule with map RHS (small variant).
// ---------------------------------------------------------------------------

/// Smaller variant of the Turkish harmony rule that captures the
/// **shape** (class LHS + map RHS + boundary context) but with a tiny
/// alphabet so the constraint construction is tractable. The
/// production-scale Turkish harmony rule uses `back !V* + !V* _`
/// (neg-class-star bracketing the boundary), which produces a
/// constraint FST that exceeds the 10-minute compile budget on this
/// F2c5 Strategy B construction — documented as a known perf
/// limitation; production cutover will need a faster leftmost filter
/// (Strategy A / Foma `rewr_notleftmost`) and/or per-rule pruning
/// before the full rule becomes practical.
///
/// This test exercises:
///
///   * class LHS (`low`),
///   * map RHS with else-identity,
///   * boundary context (`+`),
///   * (no `!V*` quantifier — that's the perf-killer.)
#[test]
fn val_harmony_shape_class_map_boundary() {
    let map_def = PhonMapDef {
        name: ident("to_a"),
        param: ident("c"),
        body: PhonMapBody::Match {
            arms: vec![PhonMapArm {
                from: lit("e"),
                to: PhonMapResult::Literal(lit("a")),
            }],
            else_arm: Some(PhonMapElse::Var(ident("c"))),
        },
    };
    let rule = make_phonrule(
        "harmony_shape",
        vec![
            class_list("back", &["a", "o"]),
            class_list("low", &["a", "e", "o"]),
        ],
        vec![map_def],
        vec![PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "low",
            "to_a",
            vec![ctx_class("back"), ctx_boundary()],
            vec![],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "a\0e",  // back=a, +, low=e → low harmonises to a → "a\0a"
            "o\0e",  // back=o, +, low=e → "o\0a"
            "k\0e",  // no back, no rewrite → "k\0e"
            "a\0o",  // back=a, +, low=o → maps via else (identity) → "a\0o"
            "ae",    // no boundary, no rewrite → "ae"
        ],
    );
}

// ---------------------------------------------------------------------------
// Test 15d/e/f — FULL Turkish vowel harmony (F2c4-#6).
//
// These are the *real* `harmony` phonrule rules from
// `examples/turkish/profile.hu` (lines 73-116), routed through the PRODUCTION
// dispatch path (`compile_phonrule` → `compile_rewrite_rule_dispatch`, which
// defaults to Strategy A and the `resolve_class_members` class-name bridge).
//
// The distinguishing feature versus the `val_harmony_shape_*` stand-in above is
// the `!V*` neg-class-star bracketing of the morpheme boundary
// (`back !V* + !V* _`): Strategy B could not compile this in <10 min, which is
// why the full rule was absent from the validation corpus until Strategy A. We
// assert byte-identical equality to `phonrule_eval` over the real Turkish
// vowel + consonant inventory.
//
// Classes/maps mirror the `.hu` source exactly:
//   class front = ["e","i","ö","ü"];  class back = ["a","ı","o","u"]
//   class V = front | back            (union — exercises resolve_class_members)
//   class high = ["i","ı","u","ü"];   class low = ["e","a","ö","o"]
//   class back_unrounded = ["a","ı"]; back_rounded = ["o","u"]; front_rounded = ["ö","ü"]
// ---------------------------------------------------------------------------

/// `low -> to_back_low / back !V* + !V* _`
///
/// 2-way low-vowel harmony: a low vowel at the start of a suffix harmonises to
/// its back counterpart when the preceding morpheme's last vowel is back,
/// regardless of how many (non-vowel) consonants intervene on either side of
/// the boundary.
#[test]
fn val_turkish_harmony_low() {
    let rule = make_phonrule(
        "harmony",
        vec![
            class_list("front", &["e", "i", "ö", "ü"]),
            class_list("back", &["a", "ı", "o", "u"]),
            class_union("V", &["front", "back"]),
            class_list("low", &["e", "a", "ö", "o"]),
        ],
        vec![map_match_else_identity("to_back_low", &[("e", "a"), ("ö", "a")])],
        vec![PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "low",
            "to_back_low",
            vec![
                ctx_class("back"),
                ctx_neg_class_star("V"),
                ctx_boundary(),
                ctx_neg_class_star("V"),
            ],
            vec![],
        ))],
    );
    assert_all_match(
        &rule,
        &[
            // --- empty / trivial ---
            "",
            "a",
            "e",
            // --- back vowel licenses harmony across the boundary ---
            "a\0e",     // back a, +, low e → "a\0a"
            "o\0e",     // back o, +, low e → "o\0a"
            "a\0ö",     // back a, +, low ö → "a\0a"
            "ı\0e",     // back ı, +, low e → "ı\0a"
            "u\0e",     // back u, +, low e → "u\0a"
            // --- consonant runs bracketing the boundary (!V*) ---
            "ak\0le",   // back a, !V*=k, +, !V*=l, low e → "ak\0la"
            "a\0klle",  // back a, +, !V*=kll, low e → "a\0klla"
            "ok\0te",   // back o, k, +, t, low e → "ok\0ta"
            "kol\0ler", // back o (last stem vowel), l, +, l, low e → "kol\0lar"
            // --- no harmony: front vowel does NOT license ---
            "e\0e",     // front e, no back → unchanged
            "i\0e",     // front i (also high), no back → low e unchanged
            "ö\0e",     // front ö, + , low e → unchanged (ö is front)
            // --- no harmony: a vowel intervenes (breaks !V*) ---
            "ae\0e",    // back a, then V=e before boundary → !V* can't span it
            "a\0ee",    // back a, +, V=e then low e: first suffix vowel only
            // --- no boundary at all ---
            "ae",       // no boundary → unchanged
            "ako",      // no boundary → unchanged
            // --- low LHS only: high vowel `i` is NOT low ---
            "a\0i",     // i is high, not low → unchanged by this rule
            // --- multiple morphemes / cascade ---
            "o\0len\0e",   // run over two morphemes
            "o\0e\0e",     // cascade across boundaries
            "e\0a\0e",     // front then back; harmony only after back licensed
            "kalem\0e",    // consonant-final stem, back? no (e is front) → unchanged
            "araba\0e",    // araba ends in back a, +, low e → "araba\0a"
        ],
    );
}

/// The three 4-way high-vowel harmony rules, as a single phonrule body (exactly
/// as the `.hu` source lists them in sequence):
///
///   high -> to_back_unrounded_high / back_unrounded !V* + !V* _
///   high -> to_back_rounded_high   / back_rounded   !V* + !V* _
///   high -> to_front_rounded_high  / front_rounded  !V* + !V* _
///
/// Each map has multiple from→to arms plus an else-identity fallthrough.
#[test]
fn val_turkish_harmony_high_4way() {
    let high_rule = |class_name: &str, map_name: &str| {
        PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "high",
            map_name,
            vec![
                ctx_class(class_name),
                ctx_neg_class_star("V"),
                ctx_boundary(),
                ctx_neg_class_star("V"),
            ],
            vec![],
        ))
    };
    let rule = make_phonrule(
        "harmony",
        vec![
            class_list("front", &["e", "i", "ö", "ü"]),
            class_list("back", &["a", "ı", "o", "u"]),
            class_union("V", &["front", "back"]),
            class_list("high", &["i", "ı", "u", "ü"]),
            class_list("back_unrounded", &["a", "ı"]),
            class_list("back_rounded", &["o", "u"]),
            class_list("front_rounded", &["ö", "ü"]),
        ],
        vec![
            map_match_else_identity(
                "to_back_unrounded_high",
                &[("i", "ı"), ("ü", "ı"), ("u", "ı")],
            ),
            map_match_else_identity(
                "to_back_rounded_high",
                &[("i", "u"), ("ı", "u"), ("ü", "u")],
            ),
            map_match_else_identity(
                "to_front_rounded_high",
                &[("i", "ü"), ("ı", "ü"), ("u", "ü")],
            ),
        ],
        vec![
            high_rule("back_unrounded", "to_back_unrounded_high"),
            high_rule("back_rounded", "to_back_rounded_high"),
            high_rule("front_rounded", "to_front_rounded_high"),
        ],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "i",
            // --- back_unrounded (a, ı) → high becomes ı ---
            "a\0i",      // a, +, high i → "a\0ı"
            "ı\0i",      // ı, +, high i → "ı\0ı"
            "a\0ü",      // a, +, high ü → "a\0ı"
            "ak\0li",    // a, k, +, l, high i → "ak\0lı"
            // --- back_rounded (o, u) → high becomes u ---
            "o\0i",      // o, +, high i → "o\0u"
            "u\0i",      // u, +, high i → "u\0u"
            "o\0ı",      // o, +, high ı → "o\0u"
            "ok\0ti",    // o, k, +, t, high i → "ok\0tu"
            // --- front_rounded (ö, ü) → high becomes ü ---
            "ö\0i",      // ö, +, high i → "ö\0ü"
            "ü\0i",      // ü, +, high i → "ü\0ü"
            "ö\0ı",      // ö, +, high ı → "ö\0ü"
            // --- front_unrounded (e, i) does NOT license any high rule ---
            "e\0i",      // e is front but not in any licensing class → unchanged
            "i\0i",      // i likewise → unchanged
            // --- low LHS is not high → untouched ---
            "a\0e",      // e is low, not high → unchanged
            "o\0a",      // a is low, not high → unchanged
            // --- vowel breaks !V* ---
            "ae\0i",     // intervening V before boundary → no match
            "a\0ei",     // intervening suffix V → first vowel is low e, not high
            // --- no boundary ---
            "ai",
            "oi",
            // --- cascade / multi-morpheme ---
            "o\0ti\0i",  // o licenses ti→tu? t is C; high i → u over two morphemes
            "ak\0lı\0i", // chained suffixes
            "kalem\0i",  // consonant stem ending in front e → e doesn't license → unchanged
            "kapı\0i",   // kapı ends in back ı (back_unrounded) → high i → ı → "kapı\0ı"
        ],
    );
}

/// Fuzz both the low rule and the 4-way high rules over the real Turkish
/// alphabet (8 vowels + a few consonants + the morpheme boundary).
#[test]
fn val_turkish_harmony_fuzz() {
    use std::time::Instant;

    // --- low rule ---
    let low_rule = make_phonrule(
        "harmony_low",
        vec![
            class_list("front", &["e", "i", "ö", "ü"]),
            class_list("back", &["a", "ı", "o", "u"]),
            class_union("V", &["front", "back"]),
            class_list("low", &["e", "a", "ö", "o"]),
        ],
        vec![map_match_else_identity("to_back_low", &[("e", "a"), ("ö", "a")])],
        vec![PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "low",
            "to_back_low",
            vec![
                ctx_class("back"),
                ctx_neg_class_star("V"),
                ctx_boundary(),
                ctx_neg_class_star("V"),
            ],
            vec![],
        ))],
    );

    // --- 4-way high rules ---
    let high_ctx = |class_name: &str, map_name: &str| {
        PhonBodyItem::Rewrite(rule_class_to_map_with_ctx(
            "high",
            map_name,
            vec![
                ctx_class(class_name),
                ctx_neg_class_star("V"),
                ctx_boundary(),
                ctx_neg_class_star("V"),
            ],
            vec![],
        ))
    };
    let high_rule = make_phonrule(
        "harmony_high",
        vec![
            class_list("front", &["e", "i", "ö", "ü"]),
            class_list("back", &["a", "ı", "o", "u"]),
            class_union("V", &["front", "back"]),
            class_list("high", &["i", "ı", "u", "ü"]),
            class_list("back_unrounded", &["a", "ı"]),
            class_list("back_rounded", &["o", "u"]),
            class_list("front_rounded", &["ö", "ü"]),
        ],
        vec![
            map_match_else_identity(
                "to_back_unrounded_high",
                &[("i", "ı"), ("ü", "ı"), ("u", "ı")],
            ),
            map_match_else_identity(
                "to_back_rounded_high",
                &[("i", "u"), ("ı", "u"), ("ü", "u")],
            ),
            map_match_else_identity(
                "to_front_rounded_high",
                &[("i", "ü"), ("ı", "ü"), ("u", "ü")],
            ),
        ],
        vec![
            high_ctx("back_unrounded", "to_back_unrounded_high"),
            high_ctx("back_rounded", "to_back_rounded_high"),
            high_ctx("front_rounded", "to_front_rounded_high"),
        ],
    );

    // Real Turkish alphabet: all 8 vowels + a few consonants + boundary.
    let chars: Vec<char> = "aeıiouöü\0klrtn".chars().collect();

    // 500+ deterministic inputs each. Distinct seeds so the two rules don't
    // share the exact same stream.
    let low_inputs = fuzz_inputs("val_turkish_harmony_fuzz_low", &chars, 600, 0, 14);
    let high_inputs = fuzz_inputs("val_turkish_harmony_fuzz_high", &chars, 600, 0, 14);
    let low_refs: Vec<&str> = low_inputs.iter().map(|s| s.as_str()).collect();
    let high_refs: Vec<&str> = high_inputs.iter().map(|s| s.as_str()).collect();

    let t0 = Instant::now();
    assert_all_match(&low_rule, &low_refs);
    assert_all_match(&high_rule, &high_refs);
    let elapsed = t0.elapsed();

    // Sane time bound: compile (sub-second each per Strategy A) + apply over
    // 1200 inputs should be well under 60s on any CI box.
    assert!(
        elapsed.as_secs() < 60,
        "turkish harmony fuzz too slow: {:?} (compile+apply over {} inputs)",
        elapsed,
        low_refs.len() + high_refs.len()
    );
    eprintln!(
        "val_turkish_harmony_fuzz: {} low + {} high inputs in {:?}",
        low_refs.len(),
        high_refs.len(),
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Test 16 — Multi-char LHS.
// ---------------------------------------------------------------------------

#[test]
fn val_multi_char_lhs_ab_to_xy() {
    let rule = make_phonrule(
        "test",
        vec![],
        vec![],
        vec![PhonBodyItem::Rewrite(rule_lit_to_lit_no_ctx("ab", "xy"))],
    );
    assert_all_match(
        &rule,
        &[
            "",
            "a",
            "b",
            "ab",
            "ba",
            "abab",
            "aab",   // single match, "aab" -> "a" + "xy"
            "abb",
            "ababab",
            "aabb",
            "zzz",
        ],
    );
}
