//! Phonological rule evaluation engine.
//!
//! Applies phonological rewrite rules (defined in `phonrule` blocks) to
//! morpheme-boundary–annotated strings. Morpheme boundaries are marked with
//! `\0` characters; the engine scans characters near boundaries and applies
//! context-sensitive rewrite rules.

use crate::ast::*;

/// Boundary marker character used internally between morphemes.
pub const BOUNDARY: char = '\0';

/// Apply a phonrule to an input string containing `\0` boundary markers.
pub fn apply_phonrule(input: &str, phonrule: &PhonRule) -> String {
    let mut result = input.to_string();
    for rule in &phonrule.rules {
        // Apply iteratively until convergence (for cascading harmony)
        loop {
            let next = apply_rewrite_rule(&result, rule, phonrule);
            if next == result {
                break;
            }
            result = next;
        }
    }
    result
}

/// Apply a single rewrite rule to the input.
/// All matches are found first, then applied simultaneously.
fn apply_rewrite_rule(input: &str, rule: &PhonRewriteRule, phonrule: &PhonRule) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();

    // Find all character positions that match the FROM pattern
    for i in 0..chars.len() {
        if chars[i] == BOUNDARY {
            continue;
        }

        // Check if the character matches the FROM pattern
        let ch_str = chars[i].to_string();
        let matched = match &rule.from {
            PhonPattern::Class(class_name) => {
                char_in_class(&ch_str, &class_name.node, phonrule)
            }
            PhonPattern::Literal(lit) => {
                // Multi-char literal match
                let lit_chars: Vec<char> = lit.node.chars().collect();
                if i + lit_chars.len() <= chars.len() {
                    let mut ok = true;
                    for (j, lc) in lit_chars.iter().enumerate() {
                        if chars[i + j] != *lc {
                            ok = false;
                            break;
                        }
                    }
                    ok
                } else {
                    false
                }
            }
        };

        if !matched {
            continue;
        }

        // Determine match length
        let match_len = match &rule.from {
            PhonPattern::Literal(lit) => lit.node.chars().count(),
            PhonPattern::Class(_) => 1,
        };

        // Check context
        if let Some(ctx) = &rule.context {
            if !check_context(&chars, i, match_len, ctx, phonrule) {
                continue;
            }
        }

        // Compute replacement
        let replacement = match &rule.to {
            PhonReplacement::Map(map_name) => {
                apply_map(&ch_str, &map_name.node, phonrule)
            }
            PhonReplacement::Literal(lit) => lit.node.clone(),
            PhonReplacement::Null => String::new(),
        };

        replacements.push((i, match_len, replacement));
    }

    // Apply replacements in reverse order to preserve indices
    if replacements.is_empty() {
        return input.to_string();
    }

    // Build result by applying all replacements (non-overlapping, simultaneous)
    // Since we want simultaneous application, use a marker approach
    let mut result = String::new();
    let mut skip_until = 0;
    for (ci, ch) in chars.iter().enumerate() {
        if ci < skip_until {
            continue;
        }
        if let Some((_, match_len, replacement)) = replacements.iter().find(|(pos, _, _)| *pos == ci) {
            result.push_str(replacement);
            skip_until = ci + match_len;
        } else {
            result.push(*ch);
        }
    }

    result
}

/// Check if a character (as string) belongs to a named character class.
fn char_in_class(ch: &str, class_name: &str, phonrule: &PhonRule) -> bool {
    for cls in &phonrule.classes {
        if cls.name.node == class_name {
            return match &cls.body {
                CharClassBody::List(members) => {
                    members.iter().any(|m| m.node == ch)
                }
                CharClassBody::Union(refs) => {
                    refs.iter().any(|r| char_in_class(ch, &r.node, phonrule))
                }
            };
        }
    }
    false
}

/// Apply a named map to a character string.
fn apply_map(ch: &str, map_name: &str, phonrule: &PhonRule) -> String {
    for map_def in &phonrule.maps {
        if map_def.name.node != map_name {
            continue;
        }
        let PhonMapBody::Match { arms, else_arm } = &map_def.body;
        for arm in arms {
            if arm.from.node == ch {
                return match &arm.to {
                    PhonMapResult::Literal(lit) => lit.node.clone(),
                    PhonMapResult::Var(_) => ch.to_string(),
                };
            }
        }
        if let Some(else_arm) = else_arm {
            return match else_arm {
                PhonMapElse::Literal(lit) => lit.node.clone(),
                PhonMapElse::Var(_) => ch.to_string(),
            };
        }
        break;
    }
    ch.to_string()
}

/// Check if the context condition matches at position `pos` in the character array.
fn check_context(
    chars: &[char],
    pos: usize,
    match_len: usize,
    ctx: &PhonContext,
    phonrule: &PhonRule,
) -> bool {
    // Check left context (reading backwards from pos)
    if !match_left_context(chars, pos, &ctx.left, phonrule) {
        return false;
    }
    // Check right context (reading forwards from pos + match_len)
    if !match_right_context(chars, pos + match_len, &ctx.right, phonrule) {
        return false;
    }
    true
}

/// Match left context elements going backwards from `pos`.
fn match_left_context(
    chars: &[char],
    pos: usize,
    elements: &[PhonContextElem],
    phonrule: &PhonRule,
) -> bool {
    // We need to match the elements right-to-left against chars left of pos
    let mut cursor = pos;
    // Process elements in reverse (rightmost element is closest to match position)
    for elem in elements.iter().rev() {
        if !match_left_elem(chars, &mut cursor, elem, phonrule) {
            return false;
        }
    }
    true
}

fn match_left_elem(
    chars: &[char],
    cursor: &mut usize,
    elem: &PhonContextElem,
    phonrule: &PhonRule,
) -> bool {
    match elem {
        PhonContextElem::Boundary => {
            if *cursor > 0 && chars[*cursor - 1] == BOUNDARY {
                *cursor -= 1;
                true
            } else if *cursor == 0 {
                true
            } else {
                false
            }
        }
        PhonContextElem::WordStart => {
            *cursor == 0
        }
        PhonContextElem::WordEnd => {
            // WordEnd in left context: not meaningful (word end is to the right)
            false
        }
        PhonContextElem::Class(name) => {
            if *cursor == 0 {
                return false;
            }
            if chars[*cursor - 1] == BOUNDARY {
                return false;
            }
            let ch_str = chars[*cursor - 1].to_string();
            if char_in_class(&ch_str, &name.node, phonrule) {
                *cursor -= 1;
                true
            } else {
                false
            }
        }
        PhonContextElem::NegClass(name) => {
            if *cursor == 0 {
                return false;
            }
            if chars[*cursor - 1] == BOUNDARY {
                return false;
            }
            let ch_str = chars[*cursor - 1].to_string();
            if !char_in_class(&ch_str, &name.node, phonrule) {
                *cursor -= 1;
                true
            } else {
                false
            }
        }
        PhonContextElem::Repeat(inner) => {
            loop {
                let saved = *cursor;
                if !match_left_elem(chars, cursor, inner, phonrule) {
                    *cursor = saved;
                    break;
                }
            }
            true
        }
        PhonContextElem::Literal(lit) => {
            let lit_chars: Vec<char> = lit.node.chars().collect();
            let mut c = *cursor;
            for lch in lit_chars.iter().rev() {
                if c == 0 || chars[c - 1] == BOUNDARY || chars[c - 1] != *lch {
                    return false;
                }
                c -= 1;
            }
            *cursor = c;
            true
        }
    }
}

/// Match right context elements going forwards from `pos`.
fn match_right_context(
    chars: &[char],
    pos: usize,
    elements: &[PhonContextElem],
    phonrule: &PhonRule,
) -> bool {
    let mut cursor = pos;
    for elem in elements {
        if !match_right_elem(chars, &mut cursor, elem, phonrule) {
            return false;
        }
    }
    true
}

fn match_right_elem(
    chars: &[char],
    cursor: &mut usize,
    elem: &PhonContextElem,
    phonrule: &PhonRule,
) -> bool {
    match elem {
        PhonContextElem::Boundary => {
            if *cursor < chars.len() && chars[*cursor] == BOUNDARY {
                *cursor += 1;
                true
            } else if *cursor >= chars.len() {
                true
            } else {
                false
            }
        }
        PhonContextElem::WordStart => {
            // WordStart in right context: not meaningful (word start is to the left)
            false
        }
        PhonContextElem::WordEnd => {
            *cursor >= chars.len()
        }
        PhonContextElem::Class(name) => {
            if *cursor >= chars.len() || chars[*cursor] == BOUNDARY {
                return false;
            }
            let ch_str = chars[*cursor].to_string();
            if char_in_class(&ch_str, &name.node, phonrule) {
                *cursor += 1;
                true
            } else {
                false
            }
        }
        PhonContextElem::NegClass(name) => {
            if *cursor >= chars.len() || chars[*cursor] == BOUNDARY {
                return false;
            }
            let ch_str = chars[*cursor].to_string();
            if !char_in_class(&ch_str, &name.node, phonrule) {
                *cursor += 1;
                true
            } else {
                false
            }
        }
        PhonContextElem::Repeat(inner) => {
            loop {
                let saved = *cursor;
                if !match_right_elem(chars, cursor, inner, phonrule) {
                    *cursor = saved;
                    break;
                }
            }
            true
        }
        PhonContextElem::Literal(lit) => {
            let lit_chars: Vec<char> = lit.node.chars().collect();
            let mut c = *cursor;
            for lch in &lit_chars {
                if c >= chars.len() || chars[c] == BOUNDARY || chars[c] != *lch {
                    return false;
                }
                c += 1;
            }
            *cursor = c;
            true
        }
    }
}

/// Strip all boundary markers from a string.
pub fn strip_boundaries(s: &str) -> String {
    s.chars().filter(|c| *c != BOUNDARY).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    fn make_span() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn make_ident(s: &str) -> Ident {
        Spanned::new(s.to_string(), make_span())
    }

    fn make_string_lit(s: &str) -> StringLit {
        Spanned::new(s.to_string(), make_span())
    }

    fn make_test_harmony() -> PhonRule {
        // A simplified Turkish vowel harmony phonrule
        PhonRule {
            name: make_ident("harmony"),
            classes: vec![
                CharClassDef {
                    name: make_ident("front"),
                    body: CharClassBody::List(vec![
                        make_string_lit("e"), make_string_lit("i"),
                        make_string_lit("ö"), make_string_lit("ü"),
                    ]),
                },
                CharClassDef {
                    name: make_ident("back"),
                    body: CharClassBody::List(vec![
                        make_string_lit("a"), make_string_lit("ı"),
                        make_string_lit("o"), make_string_lit("u"),
                    ]),
                },
                CharClassDef {
                    name: make_ident("V"),
                    body: CharClassBody::Union(vec![
                        make_ident("front"), make_ident("back"),
                    ]),
                },
            ],
            maps: vec![
                PhonMapDef {
                    name: make_ident("to_back"),
                    param: make_ident("c"),
                    body: PhonMapBody::Match {
                        arms: vec![
                            PhonMapArm { from: make_string_lit("e"), to: PhonMapResult::Literal(make_string_lit("a")) },
                            PhonMapArm { from: make_string_lit("i"), to: PhonMapResult::Literal(make_string_lit("ı")) },
                        ],
                        else_arm: Some(PhonMapElse::Var(make_ident("c"))),
                    },
                },
            ],
            rules: vec![
                // V -> to_back / back !back* + !back* _
                PhonRewriteRule {
                    from: PhonPattern::Class(make_ident("V")),
                    to: PhonReplacement::Map(make_ident("to_back")),
                    context: Some(PhonContext {
                        left: vec![
                            PhonContextElem::Class(make_ident("back")),
                            PhonContextElem::Repeat(Box::new(PhonContextElem::NegClass(make_ident("back")))),
                            PhonContextElem::Boundary,
                            PhonContextElem::Repeat(Box::new(PhonContextElem::NegClass(make_ident("back")))),
                        ],
                        right: vec![],
                    }),
                    span: make_span(),
                },
            ],
            span: make_span(),
        }
    }

    #[test]
    fn test_harmony_back_vowel() {
        let harmony = make_test_harmony();
        // "yol" + "ler" → boundary-marked: "yol\0ler"
        // The 'e' in "ler" should become 'a' (back harmony from 'o')
        let input = format!("yol{}ler", BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "yollar");
    }

    #[test]
    fn test_harmony_front_vowel_unchanged() {
        let harmony = make_test_harmony();
        // "ev" + "ler" → "ev\0ler"
        // 'e' is front, last vowel in "ev" is 'e' (front), so no change
        let input = format!("ev{}ler", BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "evler");
    }

    #[test]
    fn test_harmony_cascade() {
        let harmony = make_test_harmony();
        // "yol" + "ler" + "in" → "yol\0ler\0in"
        // First 'e' → 'a' (from 'o'), then 'i' → 'ı' (from 'a')
        let input = format!("yol{}ler{}in", BOUNDARY, BOUNDARY);
        let result = apply_phonrule(&input, &harmony);
        assert_eq!(strip_boundaries(&result), "yolların");
    }

    #[test]
    fn test_strip_boundaries() {
        assert_eq!(strip_boundaries("yol\0lar"), "yollar");
        assert_eq!(strip_boundaries("abc"), "abc");
        assert_eq!(strip_boundaries("\0a\0b\0"), "ab");
    }

    /// Helper: build a phonrule with a single rewrite rule for word-boundary tests.
    fn make_word_boundary_rule(
        from: PhonPattern,
        to: PhonReplacement,
        context: PhonContext,
    ) -> PhonRule {
        PhonRule {
            name: make_ident("wb_test"),
            classes: vec![
                CharClassDef {
                    name: make_ident("C"),
                    body: CharClassBody::List(vec![
                        make_string_lit("p"), make_string_lit("t"), make_string_lit("k"),
                        make_string_lit("b"), make_string_lit("d"), make_string_lit("g"),
                    ]),
                },
            ],
            maps: vec![],
            rules: vec![
                PhonRewriteRule {
                    from,
                    to,
                    context: Some(context),
                    span: make_span(),
                },
            ],
            span: make_span(),
        }
    }

    #[test]
    fn test_word_start_matches_beginning() {
        // "k" -> "g" / ^ _   (voice word-initial k)
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("k")),
            PhonReplacement::Literal(make_string_lit("g")),
            PhonContext {
                left: vec![PhonContextElem::WordStart],
                right: vec![],
            },
        );
        // Word-initial k should become g
        assert_eq!(strip_boundaries(&apply_phonrule("kale", &rule)), "gale");
        // k after morpheme boundary should NOT change (^ ≠ +)
        let input = format!("a{}kale", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "akale");
    }

    #[test]
    fn test_word_end_matches_end() {
        // "b" -> "p" / _ $   (devoice word-final b)
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("b")),
            PhonReplacement::Literal(make_string_lit("p")),
            PhonContext {
                left: vec![],
                right: vec![PhonContextElem::WordEnd],
            },
        );
        // Word-final b should become p
        assert_eq!(strip_boundaries(&apply_phonrule("kitab", &rule)), "kitap");
        // b before morpheme boundary should NOT change ($ ≠ +)
        let input = format!("kitab{}a", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "kitaba");
    }

    #[test]
    fn test_word_end_with_trailing_boundary() {
        // "b" -> "p" / _ $   — input has trailing boundary marker
        let rule = make_word_boundary_rule(
            PhonPattern::Literal(make_string_lit("b")),
            PhonReplacement::Literal(make_string_lit("p")),
            PhonContext {
                left: vec![],
                right: vec![PhonContextElem::WordEnd],
            },
        );
        // Even with a trailing \0, the string ends after it, so b\0 → b is NOT word-final
        let input = format!("kitab{}", BOUNDARY);
        assert_eq!(strip_boundaries(&apply_phonrule(&input, &rule)), "kitab");
    }

    #[test]
    fn test_boundary_still_matches_word_edges() {
        // Verify + still matches word start/end (existing behavior preserved)
        let harmony = make_test_harmony();
        // Single morpheme, no \0 — the + in context should still match at string start
        let result = apply_phonrule("yollar", &harmony);
        assert_eq!(strip_boundaries(&result), "yollar");
    }
}
