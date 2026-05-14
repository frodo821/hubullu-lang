//! Syllable inventory resolution and greedy syllabification.
//!
//! A `syllable NAME { ... }` declaration (see [`crate::ast::Syllable`]) names
//! a syllabification strategy built on top of an F2a [`PhonemeInventory`]. At
//! a high level the algorithm is:
//!
//! 1. Strip internal markers (`\0`, `+`) from the input. These are bookkeeping
//!    tokens used by phonrule / compose evaluation and never appear in surface
//!    forms.
//! 2. Apply per-grapheme `unknown_overrides` first so users can carve out
//!    explicit boundaries (e.g. `" ": skip`) without polluting the inventory.
//! 3. Walk the remaining input with [`PhonemeInventory::longest_match_tokenize`].
//! 4. For each maximal run of *known* tokens, identify nucleus positions
//!    (tokens whose surface belongs to the declared nucleus phoneme) and
//!    distribute consonant runs into onsets/codas according to
//!    [`OnsetPriority`], `onset_max`, and `coda_max`.
//! 5. Treat each *unknown* token according to the effective [`UnknownMode`]
//!    (either an override or the syllable's default).
//!
//! The result is a list of [`SyllableSpan`]s covering the input (excluding
//! stripped markers); each span carries the character-index range it occupies
//! in the *stripped* input plus the concatenated surface form.

use crate::ast::{OnsetPriority, Syllable, UnknownMode};
use crate::phoneme::{longest_match_tokenize, PhonemeInventory, PhonemeToken};

/// One syllable produced by [`syllabify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllableSpan {
    /// Concatenated surface form (no internal separators).
    pub surface: String,
    /// Character indices in the *stripped* input that this syllable covers.
    /// `start` is inclusive, `end` exclusive (Python-like slicing).
    pub start: usize,
    pub end: usize,
}

/// Diagnostic raised while syllabifying. We surface `unknown: error` failures
/// and `unknown: warn` warnings here so that callers (phonrule evaluators,
/// CLI render passes, tests) can decide how to log them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllabifyEvent {
    pub kind: SyllabifyEventKind,
    pub surface: String,
    /// Character offset in the stripped input where the issue occurred.
    pub at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyllabifyEventKind {
    UnknownWarn,
    UnknownError,
}

/// Result of a syllabification pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyllabifyResult {
    pub syllables: Vec<SyllableSpan>,
    pub events: Vec<SyllabifyEvent>,
}

/// Internal markers that phonrule / compose evaluation may sprinkle into
/// strings. They never participate in syllabification.
const INTERNAL_MARKERS: &[char] = &['\0', '+'];

/// Strip every [`INTERNAL_MARKERS`] character from `s`.
fn strip_internal_markers(s: &str) -> String {
    if !s.contains(|c: char| INTERNAL_MARKERS.contains(&c)) {
        return s.to_string();
    }
    s.chars()
        .filter(|c| !INTERNAL_MARKERS.contains(c))
        .collect()
}

/// Syllabify `text` according to `syl` and `inventory`.
///
/// Internal markers (`\0`, `+`) are stripped before syllabification.
/// Surface output never contains them.
pub fn syllabify(
    text: &str,
    syl: &Syllable,
    inventory: &PhonemeInventory,
) -> SyllabifyResult {
    let stripped = strip_internal_markers(text);
    if stripped.is_empty() {
        return SyllabifyResult::default();
    }

    // Per-grapheme override lookup. Strings are small enough that a linear
    // scan is fine; we keep them sorted by the parser for determinism.
    let override_for = |surface: &str| -> Option<UnknownMode> {
        syl.unknown_overrides
            .iter()
            .find(|(k, _)| k == surface)
            .map(|(_, m)| *m)
    };

    // First-pass tokenization. We then walk the token list and group known
    // tokens into syllable-sized chunks, while unknown tokens (after override
    // application) act according to their effective UnknownMode.
    let tokens = longest_match_tokenize(&stripped, inventory);

    let nucleus_name = syl.nucleus.node.as_str();
    let is_nucleus = |tok: &PhonemeToken| -> bool {
        tok.known && inventory.contains(nucleus_name, &tok.surface)
    };

    let mut syllables: Vec<SyllableSpan> = Vec::new();
    let mut events: Vec<SyllabifyEvent> = Vec::new();

    // Char-index offsets per token.
    let mut starts: Vec<usize> = Vec::with_capacity(tokens.len() + 1);
    let mut acc = 0usize;
    for t in &tokens {
        starts.push(acc);
        acc += t.surface.chars().count();
    }
    starts.push(acc);

    // Walk the token list. We accumulate a "current run" of consecutive known
    // tokens; when we hit an unknown token (or end of input) we close out the
    // run by syllabifying it, then handle the unknown token per its mode.
    let mut run_start_tok = 0usize;
    let mut i = 0usize;
    while i < tokens.len() {
        if !tokens[i].known {
            // Effective mode (per-grapheme override beats default).
            let mode = override_for(&tokens[i].surface).unwrap_or(syl.unknown);

            // For Ignore: include this token in the *current* run by simply
            // skipping the close-out step; it will end up in whichever syllable
            // is currently being built. We accomplish this by leaving the
            // run_start_tok where it is and advancing `i`.
            match mode {
                UnknownMode::Ignore => {
                    i += 1;
                    continue;
                }
                UnknownMode::Skip => {
                    // Close out the run before this unknown token, drop the
                    // unknown token entirely, start a new run after it.
                    syllabify_run(
                        &tokens[run_start_tok..i],
                        &starts[run_start_tok..=i],
                        syl,
                        &is_nucleus,
                        &mut syllables,
                    );
                    run_start_tok = i + 1;
                    i += 1;
                    continue;
                }
                UnknownMode::Warn => {
                    events.push(SyllabifyEvent {
                        kind: SyllabifyEventKind::UnknownWarn,
                        surface: tokens[i].surface.clone(),
                        at: starts[i],
                    });
                    syllabify_run(
                        &tokens[run_start_tok..i],
                        &starts[run_start_tok..=i],
                        syl,
                        &is_nucleus,
                        &mut syllables,
                    );
                    run_start_tok = i + 1;
                    i += 1;
                    continue;
                }
                UnknownMode::Error => {
                    events.push(SyllabifyEvent {
                        kind: SyllabifyEventKind::UnknownError,
                        surface: tokens[i].surface.clone(),
                        at: starts[i],
                    });
                    syllabify_run(
                        &tokens[run_start_tok..i],
                        &starts[run_start_tok..=i],
                        syl,
                        &is_nucleus,
                        &mut syllables,
                    );
                    run_start_tok = i + 1;
                    i += 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Final run (tail of input, possibly containing Ignored unknown tokens).
    if run_start_tok < tokens.len() {
        syllabify_run(
            &tokens[run_start_tok..],
            &starts[run_start_tok..=tokens.len()],
            syl,
            &is_nucleus,
            &mut syllables,
        );
    }

    SyllabifyResult { syllables, events }
}

/// Carve a sequence of tokens (a single "known run", possibly with Ignored
/// unknowns embedded) into syllables.
///
/// `starts` is the char-offset array for the slice plus a final sentinel —
/// i.e. `starts.len() == tokens.len() + 1`, with `starts[k]` giving the
/// stripped-input char-offset where `tokens[k]` begins and `starts[tokens.len()]`
/// being the offset immediately after the last token.
fn syllabify_run(
    tokens: &[PhonemeToken],
    starts: &[usize],
    syl: &Syllable,
    is_nucleus: &dyn Fn(&PhonemeToken) -> bool,
    out: &mut Vec<SyllableSpan>,
) {
    if tokens.is_empty() {
        return;
    }
    debug_assert_eq!(starts.len(), tokens.len() + 1);

    // Find all nucleus positions. If there are none, the whole run is one
    // syllable-less chunk — emit it as a single "syllable" anyway so callers
    // get a continuous span (matches the existing behaviour of treating any
    // contiguous known stretch as at-least-one orthographic unit).
    let nuclei: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter_map(|(idx, tok)| if is_nucleus(tok) { Some(idx) } else { None })
        .collect();

    if nuclei.is_empty() {
        let surface: String = tokens.iter().map(|t| t.surface.as_str()).collect();
        out.push(SyllableSpan {
            surface,
            start: starts[0],
            end: starts[tokens.len()],
        });
        return;
    }

    // Compute boundary token-indices between consecutive nuclei.
    //
    // For each pair (n_k, n_{k+1}) we split the consonant chain
    // [n_k + 1 .. n_{k+1}] into a coda (left half) and an onset (right half).
    // - `Max` priority: maximize onset (push consonants to next syllable),
    //   but cap by `onset_max`. Remaining consonants stay as coda (also
    //   capped by `coda_max`; anything past both caps stays attached to the
    //   left, but in practice the parser-validated caps usually accommodate
    //   the actual run length).
    // - `Min` priority: maximize coda first (cap by `coda_max`), the rest go
    //   to onset (cap by `onset_max`).
    //
    // We tolerate "leftover" consonants by absorbing them into the coda of
    // the left syllable; that keeps every input character accounted for.
    let onset_max = syl.onset_max.unwrap_or(u32::MAX) as usize;
    let coda_max = syl.coda_max.unwrap_or(u32::MAX) as usize;

    // The first syllable starts at token 0.
    let mut syl_start = 0usize;

    for window in nuclei.windows(2) {
        let n_left = window[0];
        let n_right = window[1];
        let gap = n_right - n_left - 1; // consonants between the two nuclei
        let (coda_take, onset_take) = match syl.onset_priority {
            OnsetPriority::Max => {
                let onset = gap.min(onset_max);
                let coda = (gap - onset).min(coda_max);
                let leftover = gap - onset - coda;
                // Anything left over piles into the coda (overrunning the cap
                // is preferable to losing characters).
                (coda + leftover, onset)
            }
            OnsetPriority::Min => {
                let coda = gap.min(coda_max);
                let onset = (gap - coda).min(onset_max);
                let leftover = gap - coda - onset;
                (coda + leftover, onset)
            }
        };

        let left_end = n_left + 1 + coda_take; // exclusive
        let right_start = n_right - onset_take;
        debug_assert!(left_end == right_start);

        // Emit the left syllable spanning [syl_start .. left_end).
        let surface: String = tokens[syl_start..left_end]
            .iter()
            .map(|t| t.surface.as_str())
            .collect();
        out.push(SyllableSpan {
            surface,
            start: starts[syl_start],
            end: starts[left_end],
        });

        syl_start = right_start;
    }

    // Last syllable: from `syl_start` to end of run.
    let surface: String = tokens[syl_start..]
        .iter()
        .map(|t| t.surface.as_str())
        .collect();
    out.push(SyllableSpan {
        surface,
        start: starts[syl_start],
        end: starts[tokens.len()],
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        OnsetPriority, Phoneme, PhonemeMember, Span, Spanned, Syllable, SyllableTemplate,
        SyllableTemplateSlot, UnknownMode,
    };
    use crate::phoneme::resolve_inventory;
    use crate::span::FileId;

    fn sp() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn lit(s: &str) -> PhonemeMember {
        PhonemeMember::Lit(Spanned::new(s.to_string(), sp()))
    }

    fn ph(name: &str, members: Vec<PhonemeMember>) -> Phoneme {
        Phoneme { name: Spanned::new(name.to_string(), sp()), members, span: sp() }
    }

    fn ident(s: &str) -> Spanned<String> {
        Spanned::new(s.to_string(), sp())
    }

    fn cv_inventory() -> PhonemeInventory {
        let c = ph("C", vec![lit("b"), lit("k"), lit("s"), lit("t"), lit("n"), lit("p")]);
        let v = ph("V", vec![lit("a"), lit("i"), lit("u"), lit("e"), lit("o")]);
        resolve_inventory(&[&c, &v]).unwrap()
    }

    fn cv_syllable(unknown: UnknownMode) -> Syllable {
        Syllable {
            name: ident("lang"),
            template: SyllableTemplate {
                slots: vec![
                    SyllableTemplateSlot { class: ident("C"), optional: true },
                    SyllableTemplateSlot { class: ident("V"), optional: false },
                ],
                span: sp(),
            },
            nucleus: ident("V"),
            onset_max: Some(1),
            coda_max: Some(0),
            onset_priority: OnsetPriority::Max,
            unknown,
            unknown_overrides: vec![],
            span: sp(),
        }
    }

    fn cvc_syllable(unknown: UnknownMode) -> Syllable {
        Syllable {
            name: ident("lang"),
            template: SyllableTemplate {
                slots: vec![
                    SyllableTemplateSlot { class: ident("C"), optional: true },
                    SyllableTemplateSlot { class: ident("V"), optional: false },
                    SyllableTemplateSlot { class: ident("C"), optional: true },
                ],
                span: sp(),
            },
            nucleus: ident("V"),
            onset_max: Some(1),
            coda_max: Some(1),
            onset_priority: OnsetPriority::Max,
            unknown,
            unknown_overrides: vec![],
            span: sp(),
        }
    }

    #[test]
    fn cv_baka_splits_into_ba_and_ka() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Warn);
        let r = syllabify("baka", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["ba", "ka"]);
        assert!(r.events.is_empty());
    }

    #[test]
    fn cvc_max_onset_takes_intervocalic_consonant() {
        // Single intervocalic consonant between two nuclei: with Max-onset
        // priority the consonant becomes the onset of the next syllable, so
        // "VCV" splits as V.CV. "ata" → a.ta.
        let inv = cv_inventory();
        let syl = cvc_syllable(UnknownMode::Warn);
        let r = syllabify("ata", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["a", "ta"]);
    }

    #[test]
    fn cvc_min_onset_keeps_consonant_as_coda() {
        // Same single intervocalic consonant under Min-onset priority stays
        // in the previous syllable's coda. "ata" → at.a.
        let inv = cv_inventory();
        let mut syl = cvc_syllable(UnknownMode::Warn);
        syl.onset_priority = OnsetPriority::Min;
        let r = syllabify("ata", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["at", "a"]);
    }

    #[test]
    fn cvc_two_consonant_cluster_splits_evenly() {
        // "taksa": gap=2 between nuclei, onset_max=1, coda_max=1. Either
        // priority is forced to put 1 in coda and 1 in next onset → tak.sa.
        let inv = cv_inventory();
        let syl = cvc_syllable(UnknownMode::Warn);
        let r = syllabify("taksa", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["tak", "sa"]);
    }

    #[test]
    fn unknown_ignore_keeps_run_together() {
        // CV inventory: 'X' is unknown. With Ignore, "baXka" stays one chain
        // and the syllabifier still finds ba.ka (ignoring X — but X stays in
        // *some* syllable: it falls inside the first one because it's adjacent
        // to it during run construction).
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Ignore);
        let r = syllabify("baXka", &syl, &inv);
        // Two nuclei → two syllables; unknown 'X' rides along.
        assert_eq!(r.syllables.len(), 2);
        assert!(r.events.is_empty());
    }

    #[test]
    fn unknown_skip_breaks_syllable() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Skip);
        let r = syllabify("ba ka", &syl, &inv);
        // Space splits into two independent runs → two syllables.
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["ba", "ka"]);
        assert!(r.events.is_empty(), "Skip is silent");
    }

    #[test]
    fn unknown_warn_emits_event() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Warn);
        let r = syllabify("ba ka", &syl, &inv);
        assert_eq!(r.syllables.len(), 2);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].kind, SyllabifyEventKind::UnknownWarn);
        assert_eq!(r.events[0].surface, " ");
    }

    #[test]
    fn unknown_error_emits_event() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Error);
        let r = syllabify("ba?ka", &syl, &inv);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].kind, SyllabifyEventKind::UnknownError);
    }

    #[test]
    fn unknown_overrides_apply() {
        // Default is Error, but space is Skip via override → silent split.
        let inv = cv_inventory();
        let mut syl = cv_syllable(UnknownMode::Error);
        syl.unknown_overrides = vec![(" ".to_string(), UnknownMode::Skip)];
        let r = syllabify("ba ka", &syl, &inv);
        assert_eq!(r.syllables.len(), 2);
        assert!(r.events.is_empty());
    }

    #[test]
    fn internal_markers_stripped_before_syllabify() {
        // \0 and + must vanish before tokenisation, so "ba\0ka" syllabifies
        // the same as "baka".
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Error);
        let r = syllabify("ba\0ka", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["ba", "ka"]);
        assert!(r.events.is_empty(), "no unknowns after stripping");

        let r2 = syllabify("ba+ka", &syl, &inv);
        let surfaces2: Vec<&str> = r2.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces2, vec!["ba", "ka"]);
        assert!(r2.events.is_empty());
    }

    #[test]
    fn empty_input_yields_no_syllables() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Warn);
        let r = syllabify("", &syl, &inv);
        assert!(r.syllables.is_empty());
    }

    #[test]
    fn three_syllable_word() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Warn);
        let r = syllabify("banana", &syl, &inv);
        let surfaces: Vec<&str> = r.syllables.iter().map(|s| s.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["ba", "na", "na"]);
    }

    #[test]
    fn span_offsets_are_consistent() {
        let inv = cv_inventory();
        let syl = cv_syllable(UnknownMode::Warn);
        let r = syllabify("baka", &syl, &inv);
        assert_eq!(r.syllables[0].start, 0);
        assert_eq!(r.syllables[0].end, 2);
        assert_eq!(r.syllables[1].start, 2);
        assert_eq!(r.syllables[1].end, 4);
    }
}
