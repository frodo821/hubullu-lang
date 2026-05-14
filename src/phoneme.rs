//! Phoneme inventory resolution and longest-match tokenization.
//!
//! A `phoneme NAME { ... }` declaration names a set of surface forms. Members
//! may be literal strings (`"a"`, `"ng"`) or references to other phonemes.
//! Resolution expands references transitively into a terminal set of literal
//! forms. Cycles among references are an error.
//!
//! Longest-match-first tokenization handles multigraphs: given an inventory
//! containing both `"n"` and `"ng"`, the string `"nga"` tokenizes as
//! `["ng", "a"]` (not `["n", "g", "a"]`).

use std::collections::{HashMap, HashSet};

use crate::ast::{Phoneme, PhonemeMember};
use crate::error::Diagnostic;

/// Resolved phoneme inventory.
///
/// Each phoneme name maps to its terminal set of surface forms (literal
/// strings reachable via any chain of `Ref` members). The `all_terminals`
/// field aggregates every terminal observed across the whole inventory and is
/// used as the tokenization alphabet.
#[derive(Debug, Default, Clone)]
pub struct PhonemeInventory {
    /// Per-phoneme terminal set, by phoneme name.
    pub terminals: HashMap<String, HashSet<String>>,
    /// Union of every terminal across all declared phonemes.
    pub all_terminals: HashSet<String>,
}

impl PhonemeInventory {
    /// Returns true if the given literal surface form belongs to the named
    /// phoneme's terminal set.
    pub fn contains(&self, phoneme: &str, surface: &str) -> bool {
        self.terminals
            .get(phoneme)
            .is_some_and(|s| s.contains(surface))
    }

    /// Returns true if a phoneme with this name has been declared.
    pub fn has_phoneme(&self, name: &str) -> bool {
        self.terminals.contains_key(name)
    }
}

/// A single token produced by longest-match tokenization against an inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonemeToken {
    /// Surface form (verbatim slice from the input).
    pub surface: String,
    /// `true` if `surface` appears as a literal in some phoneme's terminal
    /// set, `false` if it is a single character that fell through unmatched.
    pub known: bool,
}

/// Resolve the inventory of a slice of phoneme declarations.
///
/// Detects:
///   * references to undefined phonemes
///   * reference cycles (DFS)
///   * duplicate declarations (skipped here — assumed already caught at the
///     symbol-table level; subsequent entries silently overwrite).
///
/// Returns either a resolved [`PhonemeInventory`] or a list of diagnostics
/// (file/span tagged via the AST nodes' inherent spans).
pub fn resolve_inventory(
    phonemes: &[&Phoneme],
) -> Result<PhonemeInventory, Vec<Diagnostic>> {
    // Build name → phoneme lookup. The last declaration wins on duplicates
    // (those should be caught elsewhere as "duplicate definition" errors).
    let mut by_name: HashMap<&str, &Phoneme> = HashMap::new();
    for ph in phonemes {
        by_name.insert(ph.name.node.as_str(), ph);
    }

    let mut diags = Vec::new();
    let mut terminals: HashMap<String, HashSet<String>> = HashMap::new();

    // DFS with WHITE/GRAY/BLACK coloring. GRAY = on the recursion stack.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<&str, Color> = HashMap::new();
    for ph in phonemes {
        color.insert(ph.name.node.as_str(), Color::White);
    }

    // Iterative DFS root loop, recursive helper. The stack/color machinery is
    // straightforward enough that recursion is clearer than an explicit stack
    // (inventories are small in practice).
    fn dfs<'a>(
        node: &'a Phoneme,
        by_name: &HashMap<&'a str, &'a Phoneme>,
        color: &mut HashMap<&'a str, Color>,
        terminals: &mut HashMap<String, HashSet<String>>,
        diags: &mut Vec<Diagnostic>,
        path: &mut Vec<&'a str>,
    ) {
        let name: &'a str = node.name.node.as_str();
        match color.get(name).copied().unwrap_or(Color::White) {
            Color::Black => return,
            Color::Gray => {
                // Cycle detected. Build a readable trace from the path tail.
                let cycle_start = path
                    .iter()
                    .position(|n| *n == name)
                    .unwrap_or(0);
                let cycle: Vec<&str> = path[cycle_start..]
                    .iter()
                    .copied()
                    .chain(std::iter::once(name))
                    .collect();
                diags.push(
                    Diagnostic::error(format!(
                        "phoneme cycle detected: {}",
                        cycle.join(" -> ")
                    ))
                    .with_label(node.name.span, "cycle closes here"),
                );
                return;
            }
            Color::White => {}
        }

        color.insert(name, Color::Gray);
        path.push(name);

        let mut my_terminals: HashSet<String> = HashSet::new();
        for member in &node.members {
            match member {
                PhonemeMember::Lit(s) => {
                    if s.node.is_empty() {
                        diags.push(
                            Diagnostic::error(format!(
                                "phoneme '{}': empty literal is not allowed",
                                name
                            ))
                            .with_label(s.span, "empty string"),
                        );
                        continue;
                    }
                    my_terminals.insert(s.node.clone());
                }
                PhonemeMember::Ref(ident) => {
                    let Some(&target) = by_name.get(ident.node.as_str()) else {
                        diags.push(
                            Diagnostic::error(format!(
                                "phoneme '{}': reference to undefined phoneme '{}'",
                                name, ident.node
                            ))
                            .with_label(ident.span, "undefined phoneme"),
                        );
                        continue;
                    };
                    dfs(target, by_name, color, terminals, diags, path);
                    if let Some(child_set) = terminals.get(ident.node.as_str()) {
                        for t in child_set {
                            my_terminals.insert(t.clone());
                        }
                    }
                }
            }
        }

        terminals.insert(name.to_string(), my_terminals);
        path.pop();
        color.insert(name, Color::Black);
    }

    let mut path: Vec<&str> = Vec::new();
    for ph in phonemes {
        dfs(ph, &by_name, &mut color, &mut terminals, &mut diags, &mut path);
    }

    if !diags.is_empty() {
        return Err(diags);
    }

    let mut all_terminals: HashSet<String> = HashSet::new();
    for set in terminals.values() {
        for t in set {
            all_terminals.insert(t.clone());
        }
    }

    Ok(PhonemeInventory {
        terminals,
        all_terminals,
    })
}

/// Longest-match-first tokenize a string against an inventory's alphabet.
///
/// At each position, the longest surface form in `inventory.all_terminals`
/// that matches is consumed as a single token. Characters with no match
/// produce a single-character token marked `known = false` and advance by one
/// `char`.
pub fn longest_match_tokenize(text: &str, inventory: &PhonemeInventory) -> Vec<PhonemeToken> {
    // Cache the maximum terminal length in chars for an early upper bound.
    let max_len = inventory
        .all_terminals
        .iter()
        .map(|t| t.chars().count())
        .max()
        .unwrap_or(0);

    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let upper = (i + max_len).min(chars.len());
        let mut matched: Option<usize> = None;
        // Try lengths from longest to shortest.
        for len in (1..=(upper - i)).rev() {
            let candidate: String = chars[i..i + len].iter().collect();
            if inventory.all_terminals.contains(&candidate) {
                matched = Some(len);
                break;
            }
        }
        match matched {
            Some(len) => {
                let surface: String = chars[i..i + len].iter().collect();
                out.push(PhonemeToken { surface, known: true });
                i += len;
            }
            None => {
                out.push(PhonemeToken {
                    surface: chars[i].to_string(),
                    known: false,
                });
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Span, Spanned};
    use crate::span::FileId;

    fn sp() -> Span {
        Span { file_id: FileId(0), start: 0, end: 0 }
    }

    fn lit(s: &str) -> PhonemeMember {
        PhonemeMember::Lit(Spanned::new(s.to_string(), sp()))
    }

    fn r#ref(s: &str) -> PhonemeMember {
        PhonemeMember::Ref(Spanned::new(s.to_string(), sp()))
    }

    fn ph(name: &str, members: Vec<PhonemeMember>) -> Phoneme {
        Phoneme {
            name: Spanned::new(name.to_string(), sp()),
            members,
            span: sp(),
        }
    }

    #[test]
    fn resolves_simple_literals() {
        let p = ph("V", vec![lit("a"), lit("œ")]);
        let inv = resolve_inventory(&[&p]).unwrap();
        assert!(inv.contains("V", "a"));
        assert!(inv.contains("V", "œ"));
        assert!(!inv.contains("V", "b"));
    }

    #[test]
    fn resolves_union_of_other_phonemes() {
        let front = ph("front", vec![lit("e"), lit("i")]);
        let back = ph("back", vec![lit("a"), lit("o")]);
        let v = ph("V", vec![r#ref("front"), r#ref("back")]);
        let inv = resolve_inventory(&[&front, &back, &v]).unwrap();
        assert!(inv.contains("V", "e"));
        assert!(inv.contains("V", "a"));
        assert!(inv.contains("V", "o"));
        assert_eq!(inv.terminals["V"].len(), 4);
    }

    #[test]
    fn detects_simple_cycle() {
        let a = ph("A", vec![r#ref("B")]);
        let b = ph("B", vec![r#ref("A")]);
        let err = resolve_inventory(&[&a, &b]).unwrap_err();
        assert!(err.iter().any(|d| d.message.contains("cycle")));
    }

    #[test]
    fn detects_self_cycle() {
        let a = ph("A", vec![r#ref("A")]);
        let err = resolve_inventory(&[&a]).unwrap_err();
        assert!(err.iter().any(|d| d.message.contains("cycle")));
    }

    #[test]
    fn undefined_reference_errors() {
        let a = ph("A", vec![r#ref("nope")]);
        let err = resolve_inventory(&[&a]).unwrap_err();
        assert!(err.iter().any(|d| d.message.contains("undefined")));
    }

    #[test]
    fn longest_match_multigraph() {
        let c = ph("C", vec![lit("n"), lit("ng")]);
        let inv = resolve_inventory(&[&c]).unwrap();
        let tokens = longest_match_tokenize("nga", &inv);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].surface, "ng");
        assert!(tokens[0].known);
        assert_eq!(tokens[1].surface, "a");
        assert!(!tokens[1].known);
    }

    #[test]
    fn empty_literal_errors() {
        let a = ph("A", vec![lit("")]);
        let err = resolve_inventory(&[&a]).unwrap_err();
        assert!(err.iter().any(|d| d.message.contains("empty")));
    }
}
