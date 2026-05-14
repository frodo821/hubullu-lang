#![cfg(feature = "sqlite")]
//! Integration tests for F1c: inline `phon_call(...)` and `@apply IDENT { ... }`
//! blocks in `.hut` token streams.
//!
//! Pipeline exercised:
//!   parse_hut -> resolve -> apply_phonrule_chain (via HutPhonContext) -> smart_join
//!
//! Semantics being verified:
//!   * `f(token_seq)` collapses `token_seq` into ONE phonological word and
//!     applies only `f` (the outer apply stack is ignored: explicit phon_call
//!     overrides any ambient `@apply`).
//!   * Nested phon_calls are innermost-first: `f(g(x))` runs `g` then `f`.
//!   * `@apply X { token_seq }` pushes `X` onto the active apply stack while
//!     evaluating `token_seq`, then pops at the closing `}`. Inside, the
//!     effective chain is `file_level_chain ++ active_block_rules`.
//!   * Nested `@apply` blocks chain in declaration order.
//!   * An inline `phon_call` inside an `@apply` block still ignores the block
//!     (and the file-level chain).
//!   * Referring to an undefined phonrule produces a render-time error with
//!     the directive's source location.

use hubullu::render::{
    apply_phonrule_chain, parse_hut, read_render_config, resolve, smart_join, HutPhonContext,
    ResolveContext, ResolvedPart,
};

/// Render a `.hut` source against a temp dir containing `phon.hu`.
fn render_hut_with_phon(hut_src: &str, phon_hu: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("phon.hu"), phon_hu).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let parts = resolve(&hut_file.tokens, &ctx, &source_map)?;

    // Mirror main.rs's gate: dispatch to the phonrule pipeline if we have a
    // file-level chain OR any F1c marker.
    let has_f1c = parts.iter().any(|p| {
        matches!(
            p,
            ResolvedPart::PhonCallStart(_) | ResolvedPart::ApplyBlockStart(_)
        )
    });
    let parts = if hut_file.apply_chain.is_empty() && !has_f1c {
        parts
    } else {
        let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };

    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

// =========================================================================
// Parser: F1c AST shapes.
// =========================================================================

/// `f(...)` parses as a `PhonCall` token, not a sequence of `Ref` + parens.
#[test]
fn test_parse_phon_call_ast_shape() {
    let src = r#"@use f from "phon.hu"
f("cat")
"#;
    let (hut_file, _) = parse_hut(src, "t.hut").expect("parse");
    let call = hut_file
        .tokens
        .iter()
        .find(|t| matches!(t, hubullu::ast::Token::PhonCall { .. }))
        .expect("expected PhonCall in tokens");
    if let hubullu::ast::Token::PhonCall { rule, inner, .. } = call {
        assert_eq!(rule.node, "f");
        assert!(inner.iter().any(|t| matches!(t, hubullu::ast::Token::Lit(_))));
    }
}

/// `@apply IDENT { ... }` parses as an `ApplyBlock`, distinct from the
/// file-level `@apply IDENT` directive (which has no `{`).
#[test]
fn test_parse_apply_block_ast_shape() {
    let src = r#"@use g from "phon.hu"
@apply g { "cat" }
"#;
    let (hut_file, _) = parse_hut(src, "t.hut").expect("parse");
    // The file-level apply chain stays empty — only a block-scoped apply was
    // declared.
    assert!(
        hut_file.apply_chain.is_empty(),
        "block @apply should not populate file-level chain"
    );
    // First non-newline token should be the ApplyBlock.
    let block = hut_file
        .tokens
        .iter()
        .find(|t| matches!(t, hubullu::ast::Token::ApplyBlock { .. }))
        .expect("ApplyBlock token");
    if let hubullu::ast::Token::ApplyBlock { rule, inner, .. } = block {
        assert_eq!(rule.node, "g");
        assert!(inner.iter().any(|t| matches!(t, hubullu::ast::Token::Lit(_))));
    }
}

// =========================================================================
// Inline phon_call: only the call's rule applies.
// =========================================================================

/// Bare `f("cat")` with no file-level `@apply` runs `f` and produces "cad".
#[test]
fn test_phon_call_inline_basic() {
    let phon_hu = r#"
phonrule f {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use f from "phon.hu"
f("cat")
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cad");
}

/// `f("cat" ~ "ate")` forces the inner sequence into one phon-word
/// ("cat\0ate") and applies only `f`. With `t -> d`, all three `t`s flip,
/// yielding "cadade".
#[test]
fn test_phon_call_collapses_inner_to_one_word() {
    let phon_hu = r#"
phonrule f {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use f from "phon.hu"
f("cat" ~ "ate")
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cadade");
}

/// An inline phon_call **ignores** the surrounding file-level `@apply` chain.
/// File-level `b: e -> i` would normally fire, but `f(...)` overrides it and
/// only `f: t -> d` runs.
#[test]
fn test_phon_call_overrides_file_level_apply() {
    let phon_hu = r#"
phonrule f {
  "t" -> "d"
}
phonrule b {
  "e" -> "i"
}
"#;
    let hut_src = r#"@use f, b from "phon.hu"
@apply b
f("cat" ~ "ate")
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // If b had applied, we would see "cadadi". phon_call overrides => "cadade".
    assert_eq!(out, "cadade");
}

/// Nested phon_calls: `f(g(x))` runs `g` first (innermost), then `f` over
/// the result. With `g: a -> A` and `f: c -> C`, "cat" -> "cAt" (g) -> "Cat" (f).
#[test]
fn test_phon_call_nested_innermost_first() {
    let phon_hu = r#"
phonrule g {
  "a" -> "A"
}
phonrule f {
  "c" -> "C"
}
"#;
    let hut_src = r#"@use f, g from "phon.hu"
f(g("cat"))
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // After g: "cAt". After f: "CAt".
    assert_eq!(out, "CAt");
}

/// A phon_call sitting next to a plain token: the call is independent of the
/// neighbour. Plain "dog" stays as "dog"; `f("cat")` becomes "cad".
#[test]
fn test_phon_call_does_not_affect_surrounding_tokens() {
    let phon_hu = r#"
phonrule f {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use f from "phon.hu"
"dog" f("cat")
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "dog cad");
}

// =========================================================================
// @apply block: pushes onto the active apply stack for its scope.
// =========================================================================

/// `@apply Y { "cat" ~ "ate" }` with no file-level chain applies only `Y`.
#[test]
fn test_apply_block_basic() {
    let phon_hu = r#"
phonrule y {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use y from "phon.hu"
@apply y { "cat" ~ "ate" }
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cadade");
}

/// File-level `@apply X` + block `@apply Y { ... }` chains `X -> Y` inside
/// the block. `X: t -> d`, `Y: d -> z` =>  inside the block, "cat~ate"
/// goes through X then Y, giving "cazaze".
#[test]
fn test_apply_block_chains_after_file_level() {
    let phon_hu = r#"
phonrule x {
  "t" -> "d"
}
phonrule y {
  "d" -> "z"
}
"#;
    let hut_src = r#"@use x, y from "phon.hu"
@apply x
@apply y { "cat" ~ "ate" }
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cazaze");
}

/// After the block closes, the active apply stack pops back to the
/// file-level chain. A token outside the block sees only `X`, not `Y`.
#[test]
fn test_apply_block_pops_at_close() {
    let phon_hu = r#"
phonrule x {
  "t" -> "d"
}
phonrule y {
  "a" -> "A"
}
"#;
    let hut_src = r#"@use x, y from "phon.hu"
@apply x
@apply y { "cat" }
"bat"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // Inside block: x then y => "cat" -> "cad" -> "cAd".
    // After block, only x applies => "bat" -> "bad".
    // (A bare line break between tokens renders as the default separator.)
    assert_eq!(out, "cAd bad");
}

/// Nested blocks chain in declaration order:
/// `@apply X { @apply Y { ... } }` runs X then Y inside the inner block.
#[test]
fn test_apply_block_nested() {
    let phon_hu = r#"
phonrule x {
  "t" -> "d"
}
phonrule y {
  "d" -> "z"
}
"#;
    let hut_src = r#"@use x, y from "phon.hu"
@apply x {
  @apply y { "cat" ~ "ate" }
}
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cazaze");
}

/// An inline phon_call inside an `@apply` block still bypasses everything.
/// Block `Y` is active, but `f(...)` overrides it, applying only `f`.
#[test]
fn test_phon_call_inside_apply_block_still_overrides() {
    let phon_hu = r#"
phonrule f {
  "t" -> "d"
}
phonrule y {
  "a" -> "A"
}
"#;
    let hut_src = r#"@use f, y from "phon.hu"
@apply y {
  f("cat")
}
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // If y had applied, we'd see "cAd". phon_call overrides => "cad".
    assert_eq!(out, "cad");
}

// =========================================================================
// Errors: undefined phonrule name.
// =========================================================================

#[test]
fn test_phon_call_undefined_rule_is_error() {
    let phon_hu = r#"
phonrule defined {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use defined from "phon.hu"
ghost("cat")
"#;
    let err = render_hut_with_phon(hut_src, phon_hu)
        .expect_err("expected error for undefined phon_call target");
    assert!(
        err.contains("undefined phonrule") && err.contains("ghost"),
        "unexpected error message: {err}"
    );
}

#[test]
fn test_apply_block_undefined_rule_is_error() {
    let phon_hu = r#"
phonrule defined {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use defined from "phon.hu"
@apply nowhere { "cat" }
"#;
    let err = render_hut_with_phon(hut_src, phon_hu)
        .expect_err("expected error for undefined @apply block target");
    assert!(
        err.contains("undefined phonrule") && err.contains("nowhere"),
        "unexpected error message: {err}"
    );
}
