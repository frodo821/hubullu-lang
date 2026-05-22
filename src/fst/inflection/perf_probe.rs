//! F4 perf probe — temporary diagnostic. Delete once F4 is green.

#[cfg(test)]
mod tests {
    use crate::ast::{
        AxisConstraint, AxisFilter, ComposeExpr, Entry, Headword, LazyMatching, MeaningDef,
        PhonRule, SlotBody, SlotDef, SlotKind, SlotQuantifier, Span, Spanned, TagCondition,
    };
    use crate::span::FileId;
    use crate::fst::alphabet::PhonruleAlphabet;
    use crate::fst::inflection::{build_chain_fst, slot};
    use crate::fst::phonrule::compile_phonrule;
    use crate::fst::rustfst_backend::{RustFstBackend, RustFstWrapper};
    use crate::fst::FstBackend;
    use std::collections::HashMap;

    fn sp() -> Span {
        Span {
            file_id: FileId(0),
            start: 0,
            end: 0,
        }
    }

    fn ident(name: &str) -> crate::ast::Ident {
        Spanned::new(name.to_string(), sp())
    }

    fn slit(s: &str) -> crate::ast::StringLit {
        Spanned::new(s.to_string(), sp())
    }

    fn tag(axis: &str, value: &str) -> TagCondition {
        TagCondition {
            axis: ident(axis),
            value: ident(value),
        }
    }

    fn morph_entry(name: &str, headword: &str, tags: Vec<TagCondition>) -> Entry {
        Entry {
            name: ident(name),
            headword: Headword::Simple(slit(headword)),
            tags,
            stems: Vec::new(),
            inflection: None,
            meaning: MeaningDef::Single(slit("")),
            forms_override: Vec::new(),
            etymology: None,
            examples: Vec::new(),
            is_peripheral: false,
        }
    }

    fn slot_lazy(name: &str, matching: LazyMatching) -> SlotDef {
        SlotDef {
            name: ident(name),
            body: SlotBody::Lazy(matching),
            kind: SlotKind::Normal,
            span: sp(),
        }
    }

    #[test]
    #[ignore]
    fn perf_probe_phonrule_compose() {
        let t_parse = std::time::Instant::now();
        let src = include_str!("../../../examples/turkish/profile.hu");
        let parsed = crate::parse_source(src, "profile.hu");
        let mut resolver: HashMap<String, PhonRule> = HashMap::new();
        for item in &parsed.file.items {
            if let crate::ast::Item::PhonRule(pr) = &item.node {
                resolver.insert(pr.name.node.clone(), pr.clone());
            }
        }
        eprintln!("PARSE: {:?}", t_parse.elapsed());

        let mut alpha = PhonruleAlphabet::empty();
        // Mirror val_turkish_elision_real_rule's alpha_for corpus.
        let corpus_chars: Vec<char> = "aeivkpırbsulm".chars().collect();
        for ch in corpus_chars {
            alpha.intern(&ch.to_string());
        }
        let t0 = std::time::Instant::now();
        let elision_fst = compile_phonrule(resolver.get("elision").unwrap(), &resolver, &mut alpha)
            .expect("compile elision");
        eprintln!(
            "elision compile: {:?}, states={}",
            t0.elapsed(),
            RustFstBackend::num_states(&elision_fst)
        );

        let t0 = std::time::Instant::now();
        let harmony_fst = compile_phonrule(resolver.get("harmony").unwrap(), &resolver, &mut alpha)
            .expect("compile harmony");
        eprintln!(
            "harmony compile: {:?}, states={}",
            t0.elapsed(),
            RustFstBackend::num_states(&harmony_fst)
        );

        // Tiny slot FST.
        let entries = vec![
            morph_entry("tns_pc", "iyor", vec![tag("tense", "present_cont")]),
            morph_entry("tns_pst", "di", vec![tag("tense", "past")]),
        ];
        let refs: Vec<&Entry> = entries.iter().collect();
        let morpheme_fsts =
            slot::build_morpheme_fst_cache(&refs, &mut alpha).expect("morpheme cache");
        let tense_sfx = slot_lazy(
            "tense_sfx",
            LazyMatching::Filter(vec![AxisFilter {
                axis: ident("tense"),
                constraint: AxisConstraint::Any,
            }]),
        );
        let slot_fst = match &tense_sfx.body {
            SlotBody::Lazy(m) => {
                slot::build_lazy_slot_fst(&tense_sfx, m, &refs, &morpheme_fsts).expect("slot")
            }
            _ => unreachable!(),
        };
        eprintln!("slot states: {}", RustFstBackend::num_states(&slot_fst));

        let mut slot_fsts: HashMap<String, RustFstWrapper> = HashMap::new();
        slot_fsts.insert("tense_sfx".to_string(), slot_fst);
        let stem_fsts: HashMap<String, RustFstWrapper> = HashMap::new();
        let mut phonrule_fsts: HashMap<String, RustFstWrapper> = HashMap::new();
        phonrule_fsts.insert("elision".to_string(), elision_fst);
        phonrule_fsts.insert("harmony".to_string(), harmony_fst);

        let chain = ComposeExpr::PhonApply {
            rule: ident("elision"),
            inner: Box::new(ComposeExpr::Slot {
                name: ident("tense_sfx"),
                quantifier: SlotQuantifier::One,
            }),
        };
        let t0 = std::time::Instant::now();
        let chain_fst = build_chain_fst(&chain, &slot_fsts, &stem_fsts, &phonrule_fsts)
            .expect("chain elision");
        eprintln!(
            "chain[elision(tense_sfx)] compile: {:?}, states={}",
            t0.elapsed(),
            RustFstBackend::num_states(&chain_fst)
        );

        let chain = ComposeExpr::PhonApply {
            rule: ident("harmony"),
            inner: Box::new(ComposeExpr::Slot {
                name: ident("tense_sfx"),
                quantifier: SlotQuantifier::One,
            }),
        };
        let t0 = std::time::Instant::now();
        let chain_fst = build_chain_fst(&chain, &slot_fsts, &stem_fsts, &phonrule_fsts)
            .expect("chain harmony");
        eprintln!(
            "chain[harmony(tense_sfx)] compile: {:?}, states={}",
            t0.elapsed(),
            RustFstBackend::num_states(&chain_fst)
        );
    }

    /// F2c4-#5 production cutover: the real Turkish vowel-harmony phonrule
    /// (`low -> to_back_low / back !V* + !V* _`, plus the three `high -> ...`
    /// rules — all `!V*`-bearing) must now compile through the PRODUCTION
    /// `compile_phonrule` path.
    ///
    /// This was the rule shape Strategy B could not compile (>10 min /
    /// effectively uncompilable — its `constraint.rs` complement explodes on
    /// `!V*`). With the Strategy A directed-replacement engine wired in as the
    /// default (Path B, no exponential constraint stage):
    ///
    ///   * each of the 4 `!V*` rewrite rules compiles **sub-second** (the
    ///     hard per-rule budget — measured directly here), and
    ///   * the whole `harmony` phonrule (4 rules + their composition) compiles
    ///     in a couple of seconds, versus Strategy B's >10-minute hang.
    ///
    /// Un-ignored successor to `perf_probe_phonrule_compose`'s harmony section.
    #[test]
    fn turkish_harmony_production_path_subsecond() {
        use crate::ast::{CharClassBody, PhonBodyItem};
        use crate::fst::phonrule::{build_directed_replacement, compile_map};

        let src = include_str!("../../../examples/turkish/profile.hu");
        let parsed = crate::parse_source(src, "profile.hu");
        let mut resolver: HashMap<String, PhonRule> = HashMap::new();
        for item in &parsed.file.items {
            if let crate::ast::Item::PhonRule(pr) = &item.node {
                resolver.insert(pr.name.node.clone(), pr.clone());
            }
        }

        // Closed Σ: full Turkish vowel set + a representative consonant set.
        let close_alpha = || {
            let mut a = PhonruleAlphabet::empty();
            for ch in "aeıiouöüklmnrstvyz".chars() {
                a.intern(&ch.to_string());
            }
            a
        };

        let harmony = resolver
            .get("harmony")
            .expect("examples/turkish/profile.hu must define `harmony`")
            .clone();

        // (a) Per-rule budget — the HARD sub-second constraint. Build each
        //     `!V*` rewrite rule directly through the Strategy A engine the
        //     production dispatch selects, and assert each is sub-second.
        let mut alpha = close_alpha();
        let mut class_members: HashMap<String, Vec<String>> = HashMap::new();
        for c in &harmony.classes {
            let members: Vec<String> = match &c.body {
                CharClassBody::List(l) => l.iter().map(|x| x.node.clone()).collect(),
                CharClassBody::Union(u) => {
                    let mut v = Vec::new();
                    for n in u {
                        if let Some(p) = class_members.get(&n.node) {
                            v.extend(p.iter().cloned());
                        }
                    }
                    v
                }
            };
            class_members.insert(c.name.node.clone(), members);
        }
        let mut map_table: HashMap<String, RustFstWrapper> = HashMap::new();
        for m in &harmony.maps {
            map_table.insert(m.name.node.clone(), compile_map(m, &mut alpha));
        }
        for (i, item) in harmony.body.iter().enumerate() {
            if let PhonBodyItem::Rewrite(r) = item {
                let t = std::time::Instant::now();
                let f = build_directed_replacement(r, &mut alpha, &class_members, &map_table)
                    .expect("Strategy A must compile the !V* harmony rule");
                let dt = t.elapsed();
                eprintln!(
                    "harmony rule {} Strategy A compile: {:?}, states={}",
                    i,
                    dt,
                    RustFstBackend::num_states(&f)
                );
                assert!(
                    dt.as_millis() < 1000,
                    "harmony rule {} per-rule compile not sub-second: {:?} \
                     (Strategy B could not compile this in 10 min)",
                    i,
                    dt
                );
            }
        }

        // (b) Whole-phonrule production path — must complete fast (a couple of
        //     seconds for the 4 rules + composition), proving the cutover. Far
        //     below the Strategy B >10-min hang.
        let mut alpha2 = close_alpha();
        let t0 = std::time::Instant::now();
        let harmony_fst = compile_phonrule(&harmony, &resolver, &mut alpha2)
            .expect("compile harmony via production path");
        let elapsed = t0.elapsed();
        eprintln!(
            "turkish harmony FULL production-path compile: {:?}, states={}",
            elapsed,
            RustFstBackend::num_states(&harmony_fst)
        );
        assert!(
            elapsed.as_secs() < 10,
            "harmony full production compile regressed: {:?} \
             (was uncompilable / >10 min under Strategy B)",
            elapsed
        );
    }
}
