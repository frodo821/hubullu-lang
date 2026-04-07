//! Merkle-tree AST hashing for incremental compilation.
//!
//! Each AST item (phonrule, tagaxis, extend, inflection, entry) gets a hash
//! derived from its own AST content (via `std::hash::Hash`, which ignores
//! source spans) combined with the hashes of everything it references.
//!
//! Hash computation proceeds bottom-up:
//!   phonrule (leaf) → tagaxis (leaf) → extend → inflection → entry
//!
//! When a phonrule changes, the inflections that reference it get a different
//! hash, which in turn changes the hashes of entries using those inflections.
//! Only entries whose Merkle hash actually changed need re-expansion.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::ast::*;
use crate::dag;
use crate::phase1::Phase1Result;
use crate::span::FileId;
use crate::symbol_table::SymbolKind;

/// Merkle hashes for all items in the project, keyed by name.
pub struct MerkleHashes {
    pub phonrules: HashMap<String, [u8; 32]>,
    pub tagaxes: HashMap<String, [u8; 32]>,
    /// Keyed by the *target axis* name (not the extend block name), because
    /// entry expansion depends on the axis values which are contributed by all
    /// extends targeting that axis.
    pub extends_by_axis: HashMap<String, [u8; 32]>,
    pub inflections: HashMap<String, [u8; 32]>,
    /// `(source_path, entry_name) → hash`
    pub entries: HashMap<(PathBuf, String), [u8; 32]>,
}

/// Compute Merkle hashes for all items in the Phase1Result.
pub fn compute(p1: &Phase1Result) -> MerkleHashes {
    let mut hashes = MerkleHashes {
        phonrules: HashMap::new(),
        tagaxes: HashMap::new(),
        extends_by_axis: HashMap::new(),
        inflections: HashMap::new(),
        entries: HashMap::new(),
    };

    // Phase 1: hash all phonrules (leaf nodes)
    for_each_item(p1, |_, item| {
        if let Item::PhonRule(pr) = item {
            let h = merkle_leaf(pr);
            hashes.phonrules.insert(pr.name.node.clone(), h);
        }
    });

    // Phase 2: hash all tagaxes (leaf nodes)
    for_each_item(p1, |_, item| {
        if let Item::TagAxis(ta) = item {
            let h = merkle_leaf(ta);
            hashes.tagaxes.insert(ta.name.node.clone(), h);
        }
    });

    // Phase 3: hash extends, grouped by target axis.
    // Multiple @extend blocks can target the same axis; combine them all.
    let mut extends_by_axis: HashMap<String, Vec<&Extend>> = HashMap::new();
    for_each_item(p1, |_, item| {
        if let Item::Extend(ext) = item {
            extends_by_axis
                .entry(ext.target_axis.node.clone())
                .or_default()
                .push(ext);
        }
    });
    for (axis_name, exts) in &extends_by_axis {
        let mut sha = Sha256::new();
        // Include tagaxis hash
        if let Some(ta_hash) = hashes.tagaxes.get(axis_name) {
            sha.update(ta_hash);
        }
        // Include each extend block's AST hash (sorted by extend name for determinism)
        let mut ext_hashes: Vec<(String, u64)> = exts
            .iter()
            .map(|e| (e.name.node.clone(), ast_hash(e)))
            .collect();
        ext_hashes.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, h) in &ext_hashes {
            sha.update(h.to_le_bytes());
        }
        hashes
            .extends_by_axis
            .insert(axis_name.clone(), sha.finalize().into());
    }

    // Phase 4: hash inflections (depend on phonrules and other inflections via delegate).
    // collect_inflections now returns (canonical_name → (file_id, &Inflection)).
    let infl_items = collect_inflections(p1);

    // Build delegate edges using resolved (canonical) names for topological sort.
    let mut delegate_edges: Vec<(String, String)> = Vec::new();
    for (name, (file_id, infl)) in &infl_items {
        for local_dep in delegate_refs(&infl.body) {
            if let Some(canonical) = resolve_inflection_name(p1, *file_id, &local_dep) {
                delegate_edges.push((canonical, name.clone()));
            }
        }
    }
    // Kahn's algorithm gives us a valid processing order
    let topo_order = match dag::check_dag(&delegate_edges) {
        Ok(sorted) => sorted,
        Err(_) => {
            // Cycle — phase2 will report the error. Process in arbitrary order;
            // cyclic references simply won't have their delegate hash mixed in.
            infl_items.keys().cloned().collect()
        }
    };
    // Also include inflections with no delegate edges (not in topo_order)
    let mut processed: HashSet<&str> = HashSet::new();
    let ordered: Vec<String> = {
        let mut v: Vec<String> = topo_order;
        for name in infl_items.keys() {
            if !v.contains(name) {
                v.push(name.clone());
            }
        }
        v
    };
    for name in &ordered {
        if processed.contains(name.as_str()) {
            continue;
        }
        processed.insert(name);
        if let Some((file_id, infl)) = infl_items.get(name) {
            let self_hash = ast_hash(infl);
            let mut sha = Sha256::new();
            sha.update(self_hash.to_le_bytes());

            // Mix in phonrule dependency hashes (resolved to canonical names, sorted)
            let mut pr_deps: Vec<String> = phonrule_refs(&infl.body)
                .into_iter()
                .filter_map(|local| resolve_phonrule_name(p1, *file_id, &local))
                .collect();
            pr_deps.sort();
            pr_deps.dedup();
            for pr_name in &pr_deps {
                if let Some(h) = hashes.phonrules.get(pr_name) {
                    sha.update(h);
                }
            }

            // Mix in delegate target hashes (resolved to canonical names, sorted)
            let mut del_deps: Vec<String> = delegate_refs(&infl.body)
                .into_iter()
                .filter_map(|local| resolve_inflection_name(p1, *file_id, &local))
                .collect();
            del_deps.sort();
            del_deps.dedup();
            for del_name in &del_deps {
                if let Some(h) = hashes.inflections.get(del_name) {
                    sha.update(h);
                }
            }

            hashes.inflections.insert(name.clone(), sha.finalize().into());
        }
    }

    // Phase 5: hash entries
    for_each_item_with_file(p1, |file_id, item| {
        if let Item::Entry(entry) = item {
            let path = p1.source_map.path(file_id).to_path_buf();
            let self_hash = ast_hash(entry);
            let mut sha = Sha256::new();
            sha.update(self_hash.to_le_bytes());

            // Mix in inflection class hash
            match &entry.inflection {
                Some(EntryInflection::Class(class_name)) => {
                    // Resolve the inflection name via symbol table
                    let resolved_name =
                        resolve_inflection_name(p1, file_id, &class_name.node);
                    if let Some(h) = resolved_name
                        .as_ref()
                        .and_then(|n| hashes.inflections.get(n))
                    {
                        sha.update(h);
                    }

                    // Mix in axis extend hashes for the inflection's axes
                    if let Some(ref_name) = &resolved_name {
                        if let Some((_, infl)) = infl_items.get(ref_name) {
                            let mut axes: Vec<&str> =
                                infl.axes.iter().map(|a| a.node.as_str()).collect();
                            axes.sort();
                            for axis in axes {
                                if let Some(h) = hashes.extends_by_axis.get(axis) {
                                    sha.update(h);
                                }
                            }
                        }
                    }
                }
                Some(EntryInflection::Inline(inline)) => {
                    // Inline inflection: mix in phonrule hashes directly
                    let mut pr_deps: Vec<String> =
                        phonrule_refs(&inline.body).into_iter().collect();
                    pr_deps.sort();
                    for pr_name in &pr_deps {
                        // Resolve phonrule name via symbol table
                        let resolved = resolve_phonrule_name(p1, file_id, &pr_name);
                        if let Some(h) =
                            resolved.as_ref().and_then(|n| hashes.phonrules.get(n))
                        {
                            sha.update(h);
                        }
                    }
                    // Mix in delegate hashes
                    let mut del_deps: Vec<String> =
                        delegate_refs(&inline.body).into_iter().collect();
                    del_deps.sort();
                    for del_name in &del_deps {
                        let resolved =
                            resolve_inflection_name(p1, file_id, &del_name);
                        if let Some(h) =
                            resolved.as_ref().and_then(|n| hashes.inflections.get(n))
                        {
                            sha.update(h);
                        }
                    }
                    // Mix in axis extend hashes
                    let mut axes: Vec<&str> =
                        inline.axes.iter().map(|a| a.node.as_str()).collect();
                    axes.sort();
                    for axis in axes {
                        if let Some(h) = hashes.extends_by_axis.get(axis) {
                            sha.update(h);
                        }
                    }
                }
                None => {}
            }

            // Mix in extend hashes for axes referenced in entry tags
            // (tag axis values affect resolved tags, though they're static in the entry AST).
            // Not strictly necessary since tag values are in the entry AST itself,
            // but axis role/display changes matter for emit.
            let mut tag_axes: Vec<&str> = entry
                .tags
                .iter()
                .map(|tc| tc.axis.node.as_str())
                .collect();
            tag_axes.sort();
            tag_axes.dedup();
            for axis in tag_axes {
                if let Some(h) = hashes.extends_by_axis.get(axis) {
                    sha.update(h);
                }
            }

            hashes
                .entries
                .insert((path, entry.name.node.clone()), sha.finalize().into());
        }
    });

    hashes
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute a u64 hash of an AST node via `std::hash::Hash`.
/// Span fields are excluded because `Span::hash` is a no-op.
fn ast_hash<T: Hash>(val: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    val.hash(&mut hasher);
    hasher.finish()
}

/// Compute a Merkle leaf hash (no dependencies) from a single AST node.
fn merkle_leaf<T: Hash>(val: &T) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(ast_hash(val).to_le_bytes());
    sha.finalize().into()
}

/// Iterate over all items in all files.
fn for_each_item<'a>(p1: &'a Phase1Result, mut f: impl FnMut(FileId, &'a Item)) {
    for (&file_id, file) in &p1.files {
        let path = p1.source_map.path(file_id);
        if crate::stdlib::is_std_path(path) {
            continue;
        }
        for item in &file.items {
            f(file_id, &item.node);
        }
    }
}

/// Like `for_each_item` but also passes file_id through for entry resolution.
fn for_each_item_with_file<'a>(
    p1: &'a Phase1Result,
    mut f: impl FnMut(FileId, &'a Item),
) {
    for_each_item(p1, |fid, item| f(fid, item));
}

/// Collect all named inflections keyed by canonical name, with their file_id.
fn collect_inflections(p1: &Phase1Result) -> HashMap<String, (FileId, &Inflection)> {
    let mut result = HashMap::new();
    for_each_item(p1, |file_id, item| {
        if let Item::Inflection(infl) = item {
            result.insert(infl.name.node.clone(), (file_id, infl));
        }
    });
    result
}

/// Extract phonrule names referenced in an InflectionBody.
fn phonrule_refs(body: &InflectionBody) -> HashSet<String> {
    let mut refs = HashSet::new();
    match body {
        InflectionBody::Rules(rules_body) => {
            if let Some(apply) = &rules_body.apply {
                collect_phonrules_from_apply(apply, &mut refs);
            }
            for rule in &rules_body.rules {
                collect_phonrules_from_rhs(&rule.rhs.node, &mut refs);
            }
        }
        InflectionBody::Compose(comp) => {
            collect_phonrules_from_compose(&comp.chain, &mut refs);
            for slot in &comp.slots {
                for rule in &slot.rules {
                    collect_phonrules_from_rhs(&rule.rhs.node, &mut refs);
                }
            }
            for rule in &comp.overrides {
                collect_phonrules_from_rhs(&rule.rhs.node, &mut refs);
            }
        }
    }
    refs
}

fn collect_phonrules_from_apply(expr: &ApplyExpr, refs: &mut HashSet<String>) {
    match expr {
        ApplyExpr::Cell => {}
        ApplyExpr::PhonApply { rule, inner } => {
            refs.insert(rule.node.clone());
            collect_phonrules_from_apply(inner, refs);
        }
    }
}

fn collect_phonrules_from_rhs(rhs: &RuleRhs, refs: &mut HashSet<String>) {
    match rhs {
        RuleRhs::PhonApply { rule, inner } => {
            refs.insert(rule.node.clone());
            collect_phonrules_from_rhs(&inner.node, refs);
        }
        RuleRhs::Template(_) | RuleRhs::Null | RuleRhs::Delegate(_) => {}
    }
}

fn collect_phonrules_from_compose(expr: &ComposeExpr, refs: &mut HashSet<String>) {
    match expr {
        ComposeExpr::Slot(_) => {}
        ComposeExpr::Concat(parts) => {
            for part in parts {
                collect_phonrules_from_compose(part, refs);
            }
        }
        ComposeExpr::PhonApply { rule, inner } => {
            refs.insert(rule.node.clone());
            collect_phonrules_from_compose(inner, refs);
        }
    }
}

/// Extract delegate target inflection names from an InflectionBody.
fn delegate_refs(body: &InflectionBody) -> HashSet<String> {
    let mut refs = HashSet::new();
    match body {
        InflectionBody::Rules(rules_body) => {
            for rule in &rules_body.rules {
                collect_delegates_from_rhs(&rule.rhs.node, &mut refs);
            }
        }
        InflectionBody::Compose(comp) => {
            for slot in &comp.slots {
                for rule in &slot.rules {
                    collect_delegates_from_rhs(&rule.rhs.node, &mut refs);
                }
            }
            for rule in &comp.overrides {
                collect_delegates_from_rhs(&rule.rhs.node, &mut refs);
            }
        }
    }
    refs
}

fn collect_delegates_from_rhs(rhs: &RuleRhs, refs: &mut HashSet<String>) {
    match rhs {
        RuleRhs::Delegate(d) => {
            refs.insert(d.target.node.clone());
        }
        RuleRhs::PhonApply { inner, .. } => {
            collect_delegates_from_rhs(&inner.node, refs);
        }
        RuleRhs::Template(_) | RuleRhs::Null => {}
    }
}

/// Resolve an inflection name through the symbol table to get the canonical name.
/// Returns `None` if the name cannot be resolved (phase2 will report the error).
fn resolve_inflection_name(
    p1: &Phase1Result,
    file_id: FileId,
    name: &str,
) -> Option<String> {
    resolve_symbol_name(p1, file_id, name, SymbolKind::Inflection)
}

/// Resolve a phonrule name through the symbol table to get the canonical name.
fn resolve_phonrule_name(
    p1: &Phase1Result,
    file_id: FileId,
    name: &str,
) -> Option<String> {
    resolve_symbol_name(p1, file_id, name, SymbolKind::PhonRule)
}

fn resolve_symbol_name(
    p1: &Phase1Result,
    file_id: FileId,
    name: &str,
    kind: SymbolKind,
) -> Option<String> {
    let scope = p1.symbol_table.scope(file_id)?;
    for sym in scope.resolve(name) {
        if sym.kind == kind {
            // Return the original (canonical) name, not the local alias
            return Some(sym.name.clone());
        }
    }
    None
}
