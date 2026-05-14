#![cfg(feature = "sqlite")]
//! Integration tests for F4: CLI `-e <code>` eval flag and the underlying
//! [`parse_hut_with_eval`] API.
//!
//! These tests cover:
//!   * `-e` empty -> identical to legacy `parse_hut`
//!   * Single `-e` adding a file-level `@apply` (chain extension)
//!   * Multiple `-e A -e B` are equivalent to `-e "A; B"` (separator merge)
//!   * Inline `phonrule q { ... }` definitions inside `-e` strings
//!   * `@file:<path>` sugar reads code from a file
//!   * Eval tokens are appended to the existing token list
//!   * File-level `@apply` from the primary `.hut` plus an extra `-e @apply`
//!     produces a chain whose extra entry is appended (not replaced).

use hubullu::render::{
    apply_phonrule_chain, parse_hut_with_eval, read_render_config, resolve, smart_join,
    HutPhonContext, ResolveContext, ResolvedPart,
};

/// Render a `.hut` source plus optional `-e` eval strings against a working
/// directory containing the listed `.hu` files. `phon_hu` is written to
/// `phon.hu`. Returns the final string.
fn render_hut_eval(
    hut_src: &str,
    phon_hu: &str,
    eval: &[&str],
) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("phon.hu"), phon_hu).unwrap();

    let eval_owned: Vec<String> = eval.iter().map(|s| s.to_string()).collect();
    let (hut_file, source_map) = parse_hut_with_eval(hut_src, "test.hut", &eval_owned)?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let parts = resolve(&hut_file.tokens, &ctx, &source_map)?;

    // Mirror main.rs's gate: dispatch to the phonrule pipeline if we have a
    // file-level @apply chain *or* any inline phon_call / @apply block in the
    // resolved parts.
    let needs_phonrules = !hut_file.apply_chain.is_empty()
        || parts.iter().any(|p| {
            matches!(
                p,
                ResolvedPart::PhonCallStart(_) | ResolvedPart::ApplyBlockStart(_)
            )
        });
    let parts = if !needs_phonrules {
        parts
    } else {
        let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };

    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

// =========================================================================
// Baseline: `-e` with no eval strings is identical to legacy parse_hut.
// =========================================================================

#[test]
fn test_no_eval_matches_legacy() {
    let phon_hu = r#"
phonrule lenition {
  "t" -> "d"
}
"#;
    let hut_src = r#"@use lenition from "phon.hu"
@apply lenition
"cat" ~ "ate"
"#;
    let out = render_hut_eval(hut_src, phon_hu, &[]).expect("render");
    assert_eq!(out, "cadade");
}

// =========================================================================
// `-e "@apply X"` extends the apply chain (the rule is already @use'd).
// =========================================================================

#[test]
fn test_eval_adds_apply_directive() {
    let phon_hu = r#"
phonrule lenition {
  "t" -> "d"
}
"#;
    // Primary `.hut` only @use's the rule but does not @apply it. Adding
    // `@apply lenition` via -e should turn lenition on.
    let hut_src = r#"@use lenition from "phon.hu"
"cat" ~ "ate"
"#;
    let out = render_hut_eval(hut_src, phon_hu, &["@apply lenition"]).expect("render");
    assert_eq!(out, "cadade");
}

// =========================================================================
// `-e A -e B` is equivalent to `-e "A; B"` (joined with the F5 separator).
// =========================================================================

#[test]
fn test_eval_multiple_strings_equivalent_to_semicolon_join() {
    let phon_hu = r#"
phonrule a {
  "t" -> "d"
}
phonrule b {
  "d" -> "z"
}
"#;
    let hut_src = r#"@use a, b from "phon.hu"
"cat" ~ "ate"
"#;
    let split = render_hut_eval(hut_src, phon_hu, &["@apply a", "@apply b"])
        .expect("render split");
    let joined = render_hut_eval(hut_src, phon_hu, &["@apply a; @apply b"])
        .expect("render joined");
    assert_eq!(split, joined);
    assert_eq!(split, "cazaze");
}

// =========================================================================
// Inline `phonrule q { ... }` definition + `@apply q` inside `-e`.
// =========================================================================

#[test]
fn test_eval_defines_inline_phonrule_and_applies_it() {
    let phon_hu = "";
    let hut_src = r#""hello"
"#;
    // Define a phonrule entirely inside the -e string, then apply it.
    let eval = r#"phonrule q { "l" -> "L" } @apply q"#;
    let out = render_hut_eval(hut_src, phon_hu, &[eval]).expect("render");
    assert_eq!(out, "heLLo");
}

// =========================================================================
// `@file:<path>` sugar reads code from a file.
// =========================================================================

#[test]
fn test_eval_file_sugar_loads_external_hut_snippet() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("phon.hu"),
        r#"
phonrule lenition {
  "t" -> "d"
}
"#,
    )
    .unwrap();
    let extra_path = dir.path().join("extra.hut");
    std::fs::write(&extra_path, "@apply lenition\n").unwrap();

    let hut_src = r#"@use lenition from "phon.hu"
"cat" ~ "ate"
"#;
    let eval = format!("@file:{}", extra_path.to_string_lossy());

    let eval_owned = vec![eval];
    let (hut_file, source_map) =
        parse_hut_with_eval(hut_src, "test.hut", &eval_owned).expect("parse");
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())
        .expect("ctx");
    let parts = resolve(&hut_file.tokens, &ctx, &source_map).expect("resolve");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon ctx");
    let parts = apply_phonrule_chain(
        parts,
        &hut_file.apply_chain,
        &phon_ctx.resolver(),
        &source_map,
    )
    .expect("apply");
    let (sep, no_sep) = read_render_config(&ctx);
    let out = smart_join(&parts, &sep, &no_sep);
    assert_eq!(out, "cadade");
}

// =========================================================================
// Eval tokens are appended after the primary file's tokens.
// =========================================================================

#[test]
fn test_eval_tokens_are_appended() {
    let phon_hu = "";
    let hut_src = r#""alpha"
"#;
    // No phonrules involved — just check that token sequence concatenates.
    let out = render_hut_eval(hut_src, phon_hu, &[r#""beta" "gamma""#]).expect("render");
    assert_eq!(out, "alpha beta gamma");
}

// =========================================================================
// File-level @apply in primary + extra @apply in -e: chain is appended.
// =========================================================================

#[test]
fn test_eval_apply_appends_to_primary_chain() {
    let phon_hu = r#"
phonrule a {
  "t" -> "d"
}
phonrule b {
  "d" -> "z"
}
"#;
    // Primary has @apply a (t -> d). Eval adds @apply b (d -> z).
    let hut_src = r#"@use a, b from "phon.hu"
@apply a
"cat" ~ "ate"
"#;
    let out = render_hut_eval(hut_src, phon_hu, &["@apply b"]).expect("render");
    assert_eq!(out, "cazaze");

    // And verify the AST chain order is primary-then-eval:
    let eval_owned = vec!["@apply b".to_string()];
    let (hut_file, _) =
        parse_hut_with_eval(hut_src, "test.hut", &eval_owned).expect("parse");
    let names: Vec<&str> = hut_file.apply_chain.iter().map(|i| i.node.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
}

// =========================================================================
// `@file:` failure path: missing file is reported with a clear error.
// =========================================================================

#[test]
fn test_eval_file_sugar_missing_path_is_error() {
    let hut_src = "";
    let eval = "@file:/definitely/does/not/exist/extra.hut".to_string();
    let err = parse_hut_with_eval(hut_src, "test.hut", &[eval])
        .expect_err("missing file should error");
    assert!(
        err.contains("cannot read") && err.contains("/definitely/does/not/exist/extra.hut"),
        "unexpected error message: {err}"
    );
}

// =========================================================================
// CLI end-to-end: invoke the built `hubullu render` binary with `-e`.
// =========================================================================

/// Locate the `hubullu` binary produced by cargo for this test crate.
fn hubullu_bin() -> std::path::PathBuf {
    // `env!("CARGO_BIN_EXE_hubullu")` is set when the test crate depends on
    // the binary; available because the integration test runs in the same
    // package as the `hubullu` bin target.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_hubullu"))
}

#[test]
fn test_cli_render_eval_flag_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let phon_path = dir.path().join("phon.hu");
    std::fs::write(
        &phon_path,
        r#"
phonrule lenition {
  "t" -> "d"
}
"#,
    )
    .unwrap();
    let hut_path = dir.path().join("input.hut");
    std::fs::write(
        &hut_path,
        r#"@use lenition from "phon.hu"
"cat" ~ "ate"
"#,
    )
    .unwrap();

    let out = std::process::Command::new(hubullu_bin())
        .arg("render")
        .arg(&hut_path)
        .arg("-e")
        .arg("@apply lenition")
        .output()
        .expect("spawn hubullu");
    assert!(
        out.status.success(),
        "hubullu render failed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert_eq!(stdout.trim_end(), "cadade");
}

#[test]
fn test_cli_render_eval_flag_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let phon_path = dir.path().join("phon.hu");
    std::fs::write(
        &phon_path,
        r#"
phonrule a {
  "t" -> "d"
}
phonrule b {
  "d" -> "z"
}
"#,
    )
    .unwrap();
    let hut_path = dir.path().join("input.hut");
    std::fs::write(
        &hut_path,
        r#"@use a, b from "phon.hu"
"cat" ~ "ate"
"#,
    )
    .unwrap();

    let out = std::process::Command::new(hubullu_bin())
        .arg("render")
        .arg(&hut_path)
        .arg("-e")
        .arg("@apply a")
        .arg("-e")
        .arg("@apply b")
        .output()
        .expect("spawn hubullu");
    assert!(
        out.status.success(),
        "hubullu render failed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert_eq!(stdout.trim_end(), "cazaze");
}
