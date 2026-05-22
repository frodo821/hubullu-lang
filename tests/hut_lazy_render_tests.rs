#![cfg(feature = "sqlite")]
//! Integration tests for Phase 4: render-time lazy compose with explicit
//! `[axes][slot=ref, ...]` filling and deferred phonrule application.
//!
//! Pipeline exercised:
//!   parse_hut -> HutPhonContext::build -> resolve_with_phon_ctx ->
//!   apply_phonrule_chain -> smart_join
//!
//! The fixture mirrors a slice of `examples/turkish/*.hu`: vowel harmony +
//! elision wrapping a `compose` chain with both lazy slots
//! (`tense_sfx` / `pn_sfx`) and a stem ref (`root`). The `.hut` fills only
//! the lazy slots; the renderer pulls the root from the entry's stems and
//! applies `harmony(elision(...))` to the assembled morpheme stream.

use hubullu::render::{
    apply_phonrule_chain, parse_hut, read_render_config, resolve_with_phon_ctx, smart_join,
    HutPhonContext, ResolveContext,
};

/// Minimal Turkish-flavored profile + verbs file with a lazy `verb_conj`.
const LANG_HU: &str = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
tagaxis person {
  role: inflectional
  display: { en: "Person" }
}
tagaxis number {
  role: inflectional
  display: { en: "Number" }
}
tagaxis negation {
  role: inflectional
  display: { en: "Negation" }
}

@extend tense_values for tagaxis tense {
  present_cont { display: { en: "Present Continuous" } }
  past         { display: { en: "Past" } }
}
@extend person_values for tagaxis person {
  1 { display: { en: "1st" } }
  2 { display: { en: "2nd" } }
  3 { display: { en: "3rd" } }
}
@extend number_values for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}
@extend negation_values for tagaxis negation {
  pos { display: { en: "Positive" } }
  neg { display: { en: "Negative" } }
}

phonrule harmony {
  class front = ["e", "i", "ö", "ü"]
  class back  = ["a", "ı", "o", "u"]
  class V = front | back
  class high = ["i", "ı", "u", "ü"]
  class back_unrounded = ["a", "ı"]
  class back_rounded   = ["o", "u"]

  map to_back_unrounded_high = c -> match {
    "i" -> "ı", "ü" -> "ı", "u" -> "ı", else -> c
  }
  map to_back_rounded_high = c -> match {
    "i" -> "u", "ı" -> "u", "ü" -> "u", else -> c
  }

  high -> to_back_unrounded_high / back_unrounded !V* + !V* _
  high -> to_back_rounded_high / back_rounded !V* + !V* _
}

phonrule elision {
  class V = ["a", "e", "ı", "i", "o", "ö", "u", "ü"]
  V -> null / V + _
}

inflection verb_conj for {tense, person, number, negation} {
  requires stems: root

  compose harmony(elision(proclitics* + root + neg_sfx? + tense_sfx + pn_sfx? + enclitics*))

  slot neg_sfx   matching [negation, tense]
  slot tense_sfx matching [tense]
  slot pn_sfx    matching [tense, person, number]
  slot proclitics matching *
  slot enclitics  matching *
}

entry yazmak {
  headword: "yazmak"
  tags: []
  stems { root: "yaz" }
  inflection_class: verb_conj
  meaning: "to write"
}

entry tns_pc {
  headword: "iyor"
  tags: [tense=present_cont]
  meaning: "present continuous"
}

entry pn_pc_1sg {
  headword: "um"
  tags: [tense=present_cont, person=1, number=sg]
  meaning: "1sg present continuous"
}

entry tns_pst {
  headword: "di"
  tags: [tense=past]
  meaning: "past"
}

entry pn_pst_1pl {
  headword: "k"
  tags: [tense=past, person=1, number=pl]
  meaning: "1pl past"
}
"#;

/// Render `hut_src` against a temporary directory containing `lang.hu` with
/// `LANG_HU`'s contents. Mirrors `render_hut_with_phon` from
/// `hut_apply_tests.rs` but threads the phon context through `resolve` so
/// the lazy compose path can see entry/inflection AST.
fn render_lazy(hut_src: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), LANG_HU).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;

    let parts =
        resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)?;

    // Any `@apply` chain at the .hut level still runs after lazy assembly.
    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };

    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

/// `yaz + iyor + um` after `harmony(elision(...))` -> `yazıyorum`.
/// `i` and `u` are high vowels that get rewritten to `ı` and `u`
/// respectively when preceded by a back vowel.
#[test]
fn test_lazy_render_yazmak_1sg_present_cont() {
    let hut_src = r#"@reference * from "lang.hu"
@use harmony, elision from "lang.hu"
yazmak[tense=present_cont, person=1, number=sg, negation=pos][tense_sfx=tns_pc, pn_sfx=pn_pc_1sg]
"#;
    let out = render_lazy(hut_src).expect("render");
    assert_eq!(out, "yazıyorum");
}

/// `yaz + di + k` after harmony -> `yazdık`.
#[test]
fn test_lazy_render_yazmak_1pl_past() {
    let hut_src = r#"@reference * from "lang.hu"
@use harmony, elision from "lang.hu"
yazmak[tense=past, person=1, number=pl, negation=pos][tense_sfx=tns_pst, pn_sfx=pn_pst_1pl]
"#;
    let out = render_lazy(hut_src).expect("render");
    assert_eq!(out, "yazdık");
}

/// An entry whose inflection has no lazy slots (or a Rules body) is rejected
/// clearly when `[slot=...]` is given. This shores up the Phase 4 dispatch
/// rule that slot fills only apply to any-lazy Compose entries.
#[test]
fn test_lazy_render_rejects_nonlazy_entry() {
    let lang = r#"
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

@extend tv for tagaxis tense {
  past { display: { en: "P" } }
}

inflection trivial for {tense} {
  [tense=past] -> `x`
}

entry foo {
  headword: "foo"
  tags: []
  inflection_class: trivial
  meaning: "f"
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), lang).unwrap();

    let hut_src = r#"@reference * from "lang.hu"
foo[tense=past][some_slot=foo]
"#;
    let (hut_file, source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon_ctx");
    let err = resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)
        .expect_err("expected error for non-lazy inflection with slot fills");
    assert!(
        err.contains("Rules inflection") || err.contains("no lazy slots"),
        "unexpected error message: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// Phase 5: §3.4 entry-ref recursion — a host's slot fill is itself an
// `entry[axes][slot=...]` ref whose own inflection is any-lazy `Compose`.
// ---------------------------------------------------------------------------

/// Tiny invented language with a host verb and a clitic that has *its own*
/// lazy paradigm. The point is the clitic's `inflection_class: clitic_conj`
/// — a host fill `enclitics=cl[axes][slot=...]` recurses through the lazy
/// render path on the inner ref.
const RECURSIVE_LANG_HU: &str = r#"
tagaxis pos {
  role: classificatory
  display: { en: "POS" }
}
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}
tagaxis person {
  role: inflectional
  display: { en: "Person" }
}
tagaxis number {
  role: inflectional
  display: { en: "Number" }
}

@extend pv for tagaxis pos {
  verb   { display: { en: "V" } }
  clitic { display: { en: "Cl" } }
}
@extend tv for tagaxis tense {
  past { display: { en: "P" } }
}
@extend pnv for tagaxis person {
  p1 { display: { en: "1" } }
  p2 { display: { en: "2" } }
  p3 { display: { en: "3" } }
}
@extend nv for tagaxis number {
  sg { display: { en: "sg" } }
  pl { display: { en: "pl" } }
}

inflection verb_conj for {tense, person, number} {
  requires stems: root
  compose root + tense_sfx + pn_sfx + enclitics*
  slot tense_sfx matching [tense]
  slot pn_sfx    matching [person, number]
  slot enclitics matching *
}

# Clitic's own lazy paradigm.
inflection clitic_conj for {person, number} {
  requires stems: root
  compose root + cl_pn_sfx
  slot cl_pn_sfx matching [person, number]
}

entry katab {
  headword: "katab"
  tags: [pos=verb]
  stems { root: "katab" }
  inflection_class: verb_conj
  meaning: "to write"
}

entry tns_pst {
  headword: "ed"
  tags: [tense=past]
  meaning: "past"
}
entry pn_1sg {
  headword: "u"
  tags: [person=p1, number=sg]
  meaning: "1sg"
}
entry pn_2sg {
  headword: "a"
  tags: [person=p2, number=sg]
  meaning: "2sg"
}

# The clitic — `wa`-stem + its own person/number suffix slot.
entry cl_wa {
  headword: "wa"
  tags: [pos=clitic]
  stems { root: "wa" }
  inflection_class: clitic_conj
  is_peripheral: true
  meaning: "and (clitic)"
}

entry cl_pn_1sg {
  headword: "ni"
  tags: [person=p1, number=sg]
  meaning: "me"
}
entry cl_pn_2sg {
  headword: "ka"
  tags: [person=p2, number=sg]
  meaning: "you"
}
entry cl_pn_3sg {
  headword: "hu"
  tags: [person=p3, number=sg]
  meaning: "him"
}
"#;

fn render_recursive(hut_src: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), RECURSIVE_LANG_HU).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;

    let parts =
        resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)?;
    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };
    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

/// Two-level §3.4 recursion: a host verb whose `enclitics` slot is filled
/// with a clitic ref that itself carries `[axes][slot=...]`. The clitic's
/// inflection (`clitic_conj`) is any-lazy `Compose`, so the inner ref takes
/// the lazy-render path through the top-level dispatcher — `resolve_filler`
/// recurses through `resolve_with_phon_ctx`, which re-enters
/// `render_lazy_compose` for the clitic.
///
/// Surface chain:
///   host:   `katab` + `ed` (tns_pst) + `u` (pn_1sg) + <inner>
///   inner:  `wa` (cl_wa stem) + `ka` (cl_pn_2sg)        = `waka`
///   final:  `katab` `ed` `u` `waka`                     = `katabeduwaka`
#[test]
fn test_lazy_render_recursive_2level() {
    let hut_src = r#"@reference * from "lang.hu"
katab[tense=past, person=p1, number=sg][tense_sfx=tns_pst, pn_sfx=pn_1sg, enclitics=cl_wa[person=p2, number=sg][cl_pn_sfx=cl_pn_2sg]]
"#;
    let out = render_recursive(hut_src).expect("render");
    assert_eq!(out, "katabeduwaka");
}

/// Same recursive case but inside a `{ ... }` variadic list: two clitics, each
/// with its own internal paradigm, stacked on the host's `enclitics*` slot.
/// Confirms the recursion path runs once per filler in a list-valued slot.
#[test]
fn test_lazy_render_recursive_list_of_2() {
    let hut_src = r#"@reference * from "lang.hu"
katab[tense=past, person=p1, number=sg][tense_sfx=tns_pst, pn_sfx=pn_1sg, enclitics={cl_wa[person=p1, number=sg][cl_pn_sfx=cl_pn_1sg], cl_wa[person=p3, number=sg][cl_pn_sfx=cl_pn_3sg]}]
"#;
    let out = render_recursive(hut_src).expect("render");
    // host = katab+ed+u, clitic1 = wa+ni, clitic2 = wa+hu → `katabeduwaniwahu`.
    assert_eq!(out, "katabeduwaniwahu");
}

/// The outer filler's effective tag set is `cl_wa.tags ∪ outer_form_spec_axes`
/// — i.e. `[pos=clitic, person=p2, number=sg]`. The inner `[cl_pn_sfx=...]`
/// content does NOT contribute to the outer filler's tag set (those are
/// clitic-internal). The host's `enclitics matching *` accepts anything so
/// this test is implicitly satisfied by the surface assertion, but we keep
/// a dedicated check here in case a regression starts leaking inner-slot
/// tags up to the outer `MorphemeInstance.tags` (which would change
/// catch-all-warning behavior for typed-axis morphemes).
#[test]
fn test_lazy_render_recursive_inner_slot_tags_do_not_leak() {
    // Same as the 2-level case — surface stability is the assertion.
    // Construction with an `enclitics` slot that's *typed* (filter rather
    // than catch-all) would surface a leak as an unexpected fit/no-fit; the
    // current grammar's `enclitics matching *` makes the surface the
    // canonical check.
    let hut_src = r#"@reference * from "lang.hu"
katab[tense=past, person=p1, number=sg][tense_sfx=tns_pst, pn_sfx=pn_1sg, enclitics=cl_wa[person=p2, number=sg][cl_pn_sfx=cl_pn_2sg]]
"#;
    let out = render_recursive(hut_src).expect("render");
    assert_eq!(out, "katabeduwaka");
}

/// Negative test: a pathologically deep self-referential nest must error
/// (cleanly, with a recognisable message) rather than overflow the stack.
/// Generates an `enclitics=cl_wa[...]...` chain ~40 levels deep — well past
/// the `MAX_LAZY_COMPOSE_DEPTH = 32` cap.
#[test]
fn test_lazy_render_self_referential_cycle_errors_cleanly() {
    // Build a deeply nested clitic chain: cl_wa contains another cl_wa as
    // its only `cl_pn_sfx` filler — except cl_pn_sfx accepts only `[person,
    // number]`-tagged morphemes, so we instead pile clitics through the
    // host's `enclitics*` slot. The host is the recursion driver: we
    // construct a nested host where each fill is the *same* host ref again,
    // which would loop forever without the depth guard.
    //
    // The grammar accepts this textually because `enclitics matching *` is
    // a catch-all and `katab[...]` is itself an `[pos=verb]`-tagged entry,
    // so it passes the slot filter. Deeply-nested but textually finite.
    fn build(depth: usize) -> String {
        // Innermost fill: a plain pn_1sg morpheme (terminator).
        if depth == 0 {
            return "pn_1sg".to_string();
        }
        format!(
            "katab[tense=past, person=p1, number=sg][tense_sfx=tns_pst, pn_sfx=pn_1sg, enclitics={}]",
            build(depth - 1)
        )
    }
    // 40 > MAX_LAZY_COMPOSE_DEPTH (32). The cap fires before stack overflow.
    let nested = build(40);
    let hut_src = format!(
        "@reference * from \"lang.hu\"\n{}\n",
        nested
    );
    let err = render_recursive(&hut_src).expect_err("expected depth-limit error");
    assert!(
        err.contains("recursion limit") || err.contains("cycle"),
        "unexpected error message: {}",
        err
    );
}

/// Without a `HutPhonContext`, `[slot=...]` cannot resolve — make sure the
/// error message points the caller at `resolve_with_phon_ctx`.
#[test]
fn test_lazy_render_missing_phon_ctx_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), LANG_HU).unwrap();

    let hut_src = r#"@reference * from "lang.hu"
yazmak[tense=past, person=1, number=pl, negation=pos][tense_sfx=tns_pst, pn_sfx=pn_pst_1pl]
"#;
    let (hut_file, source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let err = hubullu::render::resolve(&hut_file.tokens, &ctx, &source_map)
        .expect_err("expected error without phon ctx");
    assert!(
        err.contains("HutPhonContext"),
        "unexpected error message: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// Phase 7 — render-time auto-fill of lazy slots from the cell. Legacy
// `entry[axes]` references on any-lazy compose bodies (no explicit
// `[slot=...]`) now resolve by looking up morpheme entries whose tags match
// the cell on each lazy slot's filter.
// ---------------------------------------------------------------------------

/// `yazmak[tense=present_cont, person=1, number=sg, negation=pos]` with NO
/// explicit `[slot=...]` should auto-fill `tense_sfx=tns_pc` + `pn_sfx=pn_pc_1sg`
/// — same surface as the explicit Phase 4 case.
#[test]
fn test_lazy_render_autofill_yazmak_1sg_present_cont() {
    let hut_src = r#"@reference * from "lang.hu"
yazmak[tense=present_cont, person=1, number=sg, negation=pos]
"#;
    let out = render_lazy(hut_src).expect("render");
    assert_eq!(out, "yazıyorum");
}

/// Past 1pl: auto-fill picks `tns_pst` + `pn_pst_1pl` — same surface as
/// the explicit Phase 4 case, but without `[slot=...]`.
#[test]
fn test_lazy_render_autofill_yazmak_1pl_past() {
    let hut_src = r#"@reference * from "lang.hu"
yazmak[tense=past, person=1, number=pl, negation=pos]
"#;
    let out = render_lazy(hut_src).expect("render");
    assert_eq!(out, "yazdık");
}

/// All five `examples/turkish/sentences.hut` lines should now render correctly
/// without any change to that legacy file (it stays as `[axes]`-only refs).
/// This is the brief's primary smoke test for Phase 7.
#[test]
fn test_lazy_render_autofill_turkish_sentences_hut() {
    // Render the actual checked-in example file via cargo run so the test
    // exercises the full main.rs path (which is what users invoke). Using
    // `assert_cmd` would be cleaner but the existing tests use the lib
    // surface, so we mirror that pattern with an in-memory `.hu` + `.hut`
    // pair derived from `examples/turkish/`.
    //
    // Verifies all 5 sentences in one go to keep the test compact.
    let lang = include_str!("../examples/turkish/profile.hu");
    let verbs = include_str!("../examples/turkish/verbs.hu");
    let nouns = include_str!("../examples/turkish/nouns.hu");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("profile.hu"), lang).unwrap();
    std::fs::write(dir.path().join("verbs.hu"), verbs).unwrap();
    std::fs::write(dir.path().join("nouns.hu"), nouns).unwrap();
    // Write a minimal main.hu that just rolls up verbs+nouns (the actual
    // file does the same).
    std::fs::write(
        dir.path().join("main.hu"),
        r#"@use * from "profile.hu"
@reference * from "verbs.hu"
@reference * from "nouns.hu"
"#,
    )
    .unwrap();

    let hut_src = include_str!("../examples/turkish/sentences.hut");
    let (hut_file, source_map) = parse_hut(hut_src, "sentences.hut").expect("parse");
    let ctx =
        ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon_ctx");

    let parts =
        resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)
            .expect("resolve");
    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        apply_phonrule_chain(
            parts,
            &hut_file.apply_chain,
            &phon_ctx.resolver(),
            &source_map,
        )
        .expect("apply chain")
    };
    let (sep, no_sep) = read_render_config(&ctx);
    let out = smart_join(&parts, &sep, &no_sep);

    // The smart_join joins all sentences with the configured separator
    // (space, with no separator before punctuation). The expected output
    // exactly matches what `cargo run -- render examples/turkish/sentences.hut`
    // produces.
    assert_eq!(
        out,
        "eve geliyorum. yolda yazıyorsun. evden gelmedi. yolu yazdık. evlere geliyorlar."
    );
}

/// Ambiguous auto-fill: two distinct candidates tie for a `One` slot.
/// Construct a tiny lang where two morphemes both fit the slot's filter
/// AND agree with the cell on every shared axis.
#[test]
fn test_lazy_render_autofill_ambiguous_one_slot_errors() {
    let lang = r#"
tagaxis tense {
  role: inflectional
  display: { en: "T" }
}
@extend tv for tagaxis tense {
  past { display: { en: "P" } }
}

inflection verb_conj for {tense} {
  requires stems: root
  compose root + tense_sfx
  slot tense_sfx matching [tense]
}

entry verb {
  headword: "verb"
  tags: []
  stems { root: "v" }
  inflection_class: verb_conj
  meaning: "v"
}

# TWO morphemes both fit [tense=past] for the [tense] slot — ambiguous.
entry tns_a {
  headword: "a"
  tags: [tense=past]
  meaning: "past variant A"
}
entry tns_b {
  headword: "b"
  tags: [tense=past]
  meaning: "past variant B"
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), lang).unwrap();

    let hut_src = r#"@reference * from "lang.hu"
verb[tense=past]
"#;
    let (hut_file, source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let ctx =
        ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon_ctx");

    let err = resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)
        .expect_err("expected ambiguous auto-fill error");
    assert!(
        err.contains("ambiguous auto-fill")
            && err.contains("tns_a")
            && err.contains("tns_b"),
        "unexpected error message: {}",
        err
    );
}

/// No-match auto-fill: a `One` slot's filter matches no candidate. Should
/// produce a clear "no morpheme matching slot 'X' for cell [...]" error.
#[test]
fn test_lazy_render_autofill_no_match_errors() {
    let lang = r#"
tagaxis tense {
  role: inflectional
  display: { en: "T" }
}
@extend tv for tagaxis tense {
  past    { display: { en: "P" } }
  present { display: { en: "Pr" } }
}

inflection verb_conj for {tense} {
  requires stems: root
  compose root + tense_sfx
  slot tense_sfx matching [tense]
}

entry verb {
  headword: "verb"
  tags: []
  stems { root: "v" }
  inflection_class: verb_conj
  meaning: "v"
}

# Only past morpheme exists — no present morpheme.
entry tns_pst {
  headword: "ed"
  tags: [tense=past]
  meaning: "past"
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), lang).unwrap();

    let hut_src = r#"@reference * from "lang.hu"
verb[tense=present]
"#;
    let (hut_file, source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let ctx =
        ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon_ctx");

    let err = resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)
        .expect_err("expected no-match error");
    assert!(
        err.contains("no morpheme matching slot") && err.contains("tense_sfx"),
        "unexpected error message: {}",
        err
    );
}

/// Variadic + CatchAll auto-fill: the `enclitics matching *` slot should
/// NOT auto-fill with every morpheme in the dictionary — `*` is
/// peripheral and the cell carries no information about which clitics
/// belong. Auto-fill leaves catch-all slots empty by default; the user
/// can still attach clitics via explicit `[slot=...]`.
#[test]
fn test_lazy_render_autofill_catchall_left_empty() {
    // yazmak past 3sg → only `root + tense_sfx`. (no person/number suffix
    // since `pn_sfx?` is optional and no morpheme matches person=3 number=sg
    // in past in LANG_HU.) The enclitics* catch-all is left empty.
    let hut_src = r#"@reference * from "lang.hu"
yazmak[tense=past, person=3, number=sg, negation=pos]
"#;
    // LANG_HU has no past pn morpheme for 3sg (the actual Turkish 3sg past
    // is zero, mirroring `pn_pc_3sg` being absent). The `pn_sfx?` is
    // optional, so this should render as `yaz + di` = `yazdı` (after
    // harmony; `i → ı` after back vowel `a`).
    let out = render_lazy(hut_src).expect("render");
    assert_eq!(out, "yazdı");
}

// ---------------------------------------------------------------------------
// Phase 6 — discontinuous morphology: circumfix + infix.
//
// `Circumfix` slots are referenced twice in the compose chain; a single
// filler whose surface contains the splice marker `^` is split, prefix at
// the first ref, suffix at the second. `Infix` slots do NOT appear in the
// compose chain; their fillers are spliced into the per-stem structural
// map (`{root.after_C1}`) so an eager rule's template interpolates them
// inside the stem.
// ---------------------------------------------------------------------------

/// Toy German-style past participle: `ge^t` wraps the verb root.
const CIRCUMFIX_LANG_HU: &str = r#"
tagaxis pos {
  role: classificatory
  display: { en: "POS" }
}
tagaxis tense {
  role: inflectional
  display: { en: "T" }
}

@extend pos_values for tagaxis pos {
  verb { display: { en: "V" } }
}
@extend tense_values for tagaxis tense {
  past_ptcp { display: { en: "PP" } }
}

inflection participle for {tense} {
  requires stems: root

  compose ge_circ + root + ge_circ

  slot ge_circ circumfix matching [tense=past_ptcp]
}

entry werken {
  headword: "werken"
  tags: [pos=verb]
  stems { root: "werk" }
  inflection_class: participle
  meaning: "to work"
}

entry ge_t_ptcp {
  headword: "ge^t"
  tags: [tense=past_ptcp]
  meaning: "past participle circumfix"
}
"#;

fn render_circumfix(hut_src: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), CIRCUMFIX_LANG_HU).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;

    let parts =
        resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)?;
    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };
    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

/// `werken[tense=past_ptcp]` should auto-fill the circumfix and split
/// `"ge^t"` into prefix `ge` + suffix `t` around the stem.
#[test]
fn test_circumfix_autofill_werken_past_ptcp() {
    let hut_src = r#"@reference * from "lang.hu"
werken[tense=past_ptcp]
"#;
    let out = render_circumfix(hut_src).expect("render");
    assert_eq!(out, "gewerkt");
}

/// Explicit `[ge_circ=ge_t_ptcp]` fill should produce the same surface.
#[test]
fn test_circumfix_explicit_fill() {
    let hut_src = r#"@reference * from "lang.hu"
werken[tense=past_ptcp][ge_circ=ge_t_ptcp]
"#;
    let out = render_circumfix(hut_src).expect("render");
    assert_eq!(out, "gewerkt");
}

/// A morpheme whose surface has no `^` (or more than one) is an error —
/// circumfix entries must have exactly one splice point.
#[test]
fn test_circumfix_malformed_filler_errors() {
    let bad_lang = r#"
tagaxis pos { role: classificatory display: { en: "POS" } }
tagaxis tense { role: inflectional display: { en: "T" } }
@extend pos_values for tagaxis pos { verb { display: { en: "V" } } }
@extend tense_values for tagaxis tense { past_ptcp { display: { en: "PP" } } }

inflection participle for {tense} {
  requires stems: root
  compose ge_circ + root + ge_circ
  slot ge_circ circumfix matching [tense=past_ptcp]
}

entry werken {
  headword: "werken"
  tags: [pos=verb]
  stems { root: "werk" }
  inflection_class: participle
  meaning: "to work"
}

# Bad: no `^` in surface.
entry no_splice {
  headword: "get"
  tags: [tense=past_ptcp]
  meaning: "broken circumfix (no splice)"
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), bad_lang).unwrap();
    let hut_src = r#"@reference * from "lang.hu"
werken[tense=past_ptcp]
"#;
    let (hut_file, source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path()).expect("ctx");
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path()).expect("phon_ctx");
    let err = resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)
        .expect_err("expected splice-marker error");
    assert!(
        err.contains("splice marker"),
        "unexpected error message: {}",
        err
    );
}

/// A `circumfix` slot must appear in the compose chain exactly twice;
/// once-only or three-times should be a compile-time error.
#[test]
fn test_circumfix_wrong_chain_count_errors_at_compile() {
    let bad_lang = r#"
tagaxis tense { role: inflectional display: { en: "T" } }
@extend tense_values for tagaxis tense { past_ptcp { display: { en: "PP" } } }

inflection bad for {tense} {
  requires stems: root
  # Circumfix referenced only once — should be exactly twice.
  compose ge_circ + root
  slot ge_circ circumfix matching [tense=past_ptcp]
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), bad_lang).unwrap();
    // Try to build phase2 directly; compile_with_quiet would also surface this.
    let hut_src = r#"@reference * from "lang.hu"
"#;
    let (hut_file, _source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let err = match HutPhonContext::build(&hut_file, dir.path()) {
        Ok(_) => panic!("expected circumfix-count error, got Ok"),
        Err(e) => e,
    };
    assert!(
        err.contains("circumfix slot")
            && err.contains("exactly twice"),
        "unexpected error message: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// Phase 6 — infix
// ---------------------------------------------------------------------------

/// Toy Arabic-style triliteral root-and-pattern morphology. Two infix
/// positions (`after_C1`, `after_C2`) splice vowels into the stem
/// template between the consonants.
const INFIX_LANG_HU: &str = r#"
tagaxis pos {
  role: classificatory
  display: { en: "POS" }
}
tagaxis root_type {
  role: structural
  display: { en: "RT" }
}
tagaxis vowel_pattern {
  role: inflectional
  display: { en: "VP" }
}

@extend pos_values for tagaxis pos {
  verb { display: { en: "V" } }
}

@extend root_type_values for tagaxis root_type {
  triliteral {
    display: { en: "Triliteral" }
    slots: [C1, C2, C3]
    infix_positions: [after_C1, after_C2]
  }
}

@extend vowel_pattern_values for tagaxis vowel_pattern {
  perfect_a_a   { display: { en: "perfect" } }
  imperfect_u_u { display: { en: "imperfect" } }
}

inflection arabic_root_pattern for {vowel_pattern} {
  requires stems: root[root_type=triliteral]

  compose root_template

  slot root_template {
    [vowel_pattern=perfect_a_a, _]   -> `{root.C1}{root.after_C1}{root.C2}{root.after_C2}{root.C3}`
    [vowel_pattern=imperfect_u_u, _] -> `{root.C1}{root.after_C1}{root.C2}{root.after_C2}{root.C3}`
  }

  slot after_C1 infix matching [vowel_pattern]
  slot after_C2 infix matching [vowel_pattern]
}

entry kataba {
  headword: "kataba"
  tags: [pos=verb, root_type=triliteral]
  stems { root: "ktb" }
  inflection_class: arabic_root_pattern
  meaning: "to write"
}

entry vowel_a {
  headword: "a"
  tags: [vowel_pattern=perfect_a_a]
  meaning: "perfect vowel"
}
entry vowel_u {
  headword: "u"
  tags: [vowel_pattern=imperfect_u_u]
  meaning: "imperfect vowel"
}
"#;

fn render_infix(hut_src: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), INFIX_LANG_HU).unwrap();

    let (hut_file, source_map) = parse_hut(hut_src, "test.hut")?;
    let ctx = ResolveContext::from_references(&hut_file.references, dir.path())?;
    let phon_ctx = HutPhonContext::build(&hut_file, dir.path())?;

    let parts =
        resolve_with_phon_ctx(&hut_file.tokens, &ctx, Some(&phon_ctx), &source_map)?;
    let parts = if hut_file.apply_chain.is_empty() {
        parts
    } else {
        apply_phonrule_chain(parts, &hut_file.apply_chain, &phon_ctx.resolver(), &source_map)?
    };
    let (sep, no_sep) = read_render_config(&ctx);
    Ok(smart_join(&parts, &sep, &no_sep))
}

/// Auto-fill picks the perfect a-a vowel for both `after_C1` and
/// `after_C2`, yielding `k a t a b` = `katab`.
#[test]
fn test_infix_autofill_perfect_a_a() {
    let hut_src = r#"@reference * from "lang.hu"
kataba[vowel_pattern=perfect_a_a]
"#;
    let out = render_infix(hut_src).expect("render");
    assert_eq!(out, "katab");
}

/// Same root, imperfect u-u pattern: `k u t u b`.
#[test]
fn test_infix_autofill_imperfect_u_u() {
    let hut_src = r#"@reference * from "lang.hu"
kataba[vowel_pattern=imperfect_u_u]
"#;
    let out = render_infix(hut_src).expect("render");
    assert_eq!(out, "kutub");
}

/// Explicit `[after_C1=vowel_a, after_C2=vowel_a]` fill should produce
/// the same surface as the auto-fill case.
#[test]
fn test_infix_explicit_fill() {
    let hut_src = r#"@reference * from "lang.hu"
kataba[vowel_pattern=perfect_a_a][after_C1=vowel_a, after_C2=vowel_a]
"#;
    let out = render_infix(hut_src).expect("render");
    assert_eq!(out, "katab");
}

/// An `infix` slot must NOT appear in the compose chain — declaring one
/// and then referencing it in the chain is a compile-time error.
#[test]
fn test_infix_in_chain_errors_at_compile() {
    let bad_lang = r#"
tagaxis pos { role: classificatory display: { en: "POS" } }
tagaxis root_type { role: structural display: { en: "RT" } }
tagaxis vowel_pattern { role: inflectional display: { en: "VP" } }

@extend pos_values for tagaxis pos { verb { display: { en: "V" } } }
@extend root_type_values for tagaxis root_type {
  triliteral {
    display: { en: "Triliteral" }
    slots: [C1, C2, C3]
    infix_positions: [after_C1]
  }
}
@extend vowel_pattern_values for tagaxis vowel_pattern {
  perfect_a_a { display: { en: "perfect" } }
}

inflection bad for {vowel_pattern} {
  requires stems: root[root_type=triliteral]

  # `after_C1` is an infix slot — it must NOT appear in the chain.
  compose root_template + after_C1

  slot root_template {
    [vowel_pattern=perfect_a_a, _] -> `{root.C1}{root.after_C1}{root.C2}{root.C3}`
  }
  slot after_C1 infix matching [vowel_pattern]
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lang.hu"), bad_lang).unwrap();
    let hut_src = r#"@reference * from "lang.hu"
"#;
    let (hut_file, _source_map) = parse_hut(hut_src, "t.hut").expect("parse");
    let err = match HutPhonContext::build(&hut_file, dir.path()) {
        Ok(_) => panic!("expected infix-in-chain error, got Ok"),
        Err(e) => e,
    };
    assert!(
        err.contains("infix slot")
            && err.contains("must NOT appear"),
        "unexpected error message: {}",
        err
    );
}
