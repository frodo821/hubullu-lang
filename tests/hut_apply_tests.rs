#![cfg(feature = "sqlite")]
//! Integration tests for F1b: `.hut` file-level `@apply` directive and `~`
//! reinterpreted as an agglutination marker.
//!
//! These tests cover the full pipeline:
//!   parse_hut -> apply_phonrule_chain (via HutPhonContext) -> smart_join

use hubullu::render::{
    apply_phonrule_chain, parse_hut, read_render_config, resolve, smart_join, HutPhonContext,
    ResolveContext,
};

/// Render a `.hut` source against a working directory containing the listed
/// `.hu` files. `phon_hu` is written to `phon.hu`. Returns the final string.
fn render_hut_with_phon(hut_src: &str, phon_hu: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("phon.hu"), phon_hu).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let parts = resolve(&hut_file.tokens, &ctx, &source_map)?;

    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };

    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

// =========================================================================
// Baseline: no @apply means no behaviour change.
// =========================================================================

/// Without any `@apply`, `.hut` rendering is exactly the legacy path:
/// `~` suppresses the separator (renders adjacent) but does NOT trigger any
/// phonological rule.
#[test]
fn test_no_apply_chain_is_unchanged() {
    let phon_hu = r#"
phonrule no_op {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use no_op from "phon.hu"
"cat" ~ "ate"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "catate");
}

// =========================================================================
// Single @apply: behaves like applying the rule to the phonological word.
// =========================================================================

/// One `@apply X` rewrites `t -> d` in the phonological word "cat~ate".
/// The `~`-separated tokens form ONE phonological word so the rule sees
/// "cat\0ate" and produces "cadade".
#[test]
fn test_single_apply_rewrites_phon_word() {
    let phon_hu = r#"
phonrule lenition {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use lenition from "phon.hu"
@apply lenition
"cat" ~ "ate"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cadade");
}

// =========================================================================
// @apply chain: two rules applied left-to-right.
// =========================================================================

/// `@apply A` followed by `@apply B` chains the two rules in declaration
/// order. A rewrites `t -> d`, B rewrites `d -> z`; both fire so we end up
/// with `z` everywhere `t` started.
#[test]
fn test_apply_chain_left_to_right() {
    let phon_hu = r#"
phonrule a {
  "t" -> "d"
}
phonrule b {
  "d" -> "z"
}
"#;
    let hut_src = r#"@use a, b from "phon.hu"
@apply a
@apply b
"cat" ~ "ate"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cazaze");
}

// =========================================================================
// Independence of phonological words separated by whitespace.
// =========================================================================

/// Two tokens separated only by a space (no `~`) are two separate
/// phonological words; each is rewritten independently. The rule's word-end
/// anchor `$` distinguishes them: only word-final `t` is dropped, so "cat"
/// becomes "ca" but the middle `t` of "patate" stays.
#[test]
fn test_whitespace_separated_words_are_independent() {
    let phon_hu = r#"
phonrule final_t_drop {
  "t" -> "" / _ $
}
"#;
    let hut_src = r#"@use final_t_drop from "phon.hu"
@apply final_t_drop
"cat" "patate"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "ca patate");
}

/// With `~`, the two tokens form one phonological word; the word-final `t`
/// is now the inner `t` of "ca~at" (joined as "ca\0at" -> word-final `t` is
/// at the end), so only the trailing `t` is dropped.
#[test]
fn test_tilde_joins_words_for_word_final_anchor() {
    let phon_hu = r#"
phonrule final_t_drop {
  "t" -> "" / _ $
}
"#;
    let hut_src = r#"@use final_t_drop from "phon.hu"
@apply final_t_drop
"cat" ~ "at"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // BOUNDARY between cat and at means '$' (word-end) sees only the trailing
    // 't' as word-final, so we get "cat\0a" -> strip -> "cata".
    assert_eq!(out, "cata");
}

// =========================================================================
// Three-token `~` chain forms one phonological word.
// =========================================================================

/// `a ~ b ~ c` is a single phonological word; the renderer collapses the
/// three tokens into one Text and a word-final substitution fires once at
/// the end of the combined string.
#[test]
fn test_three_token_tilde_chain_is_one_phon_word() {
    // Substitute final `c` with `Z` only at word end.
    let phon_hu = r#"
phonrule final_c_to_z {
  "c" -> "Z" / _ $
}
"#;
    let hut_src = r#"@use final_c_to_z from "phon.hu"
@apply final_c_to_z
"a" ~ "b" ~ "c"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    // The three tokens form one word -> "a\0b\0c" -> "a\0b\0Z" -> strip
    // boundaries -> "abZ". The single-token case "c" alone (separate word)
    // would also map to "Z" if it occurred, but here the whole chain is one.
    assert_eq!(out, "abZ");
}

// =========================================================================
// Error: @apply refers to an undefined phonrule.
// =========================================================================

#[test]
fn test_apply_undefined_phonrule_is_error() {
    let phon_hu = r#"
phonrule defined {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use defined from "phon.hu"
@apply undefined_rule
"cat"
"#;
    let err = render_hut_with_phon(hut_src, phon_hu)
        .expect_err("expected error for undefined @apply target");
    assert!(
        err.contains("undefined phonrule")
            && err.contains("undefined_rule"),
        "unexpected error message: {err}"
    );
}

// =========================================================================
// Free-order parsing: @apply mixed with @reference / @use.
// =========================================================================

#[test]
fn test_apply_free_order_with_use() {
    let phon_hu = r#"
phonrule a {
  "t" -> "d"
}
"#;
    // Interleave @apply with @use freely.
    let hut_src = r#"@apply a
@use a from "phon.hu"
"cat"
"#;
    let out = render_hut_with_phon(hut_src, phon_hu).expect("render");
    assert_eq!(out, "cad");
}

// =========================================================================
// AST shape: apply_chain captured in declaration order.
// =========================================================================

#[test]
fn test_apply_chain_ast_order() {
    let src = r#"@apply first
@apply second
@apply third
"x"
"#;
    let (hut_file, _) = parse_hut(src, "test.hut").expect("parse");
    assert_eq!(hut_file.apply_chain.len(), 3);
    assert_eq!(hut_file.apply_chain[0].node, "first");
    assert_eq!(hut_file.apply_chain[1].node, "second");
    assert_eq!(hut_file.apply_chain[2].node, "third");
}
