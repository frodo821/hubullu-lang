//! `.hut` file rendering — resolves token lists against compiled `.huc` files.
//!
//! Each `.hut` file declares `@reference` directives pointing at `.hu` source
//! files. The renderer either compiles those sources on demand (with
//! mtime-based caching) or uses a pre-compiled `.huc` file supplied via
//! `--huc`, resolving entry references through namespace-aware lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::Connection;

use crate::ast;
use crate::ast::{HutFile, ImportTarget};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::span::SourceMap;

// ---------------------------------------------------------------------------
// Form-spec matching helpers
// ---------------------------------------------------------------------------

/// Search forms for an entry and find the unique form matching the given
/// tag conditions (subset match).
///
/// Multiple cells may match a partial form_spec (e.g. a `_`-wildcarded rule
/// expands to many cells with identical values). Such matches are deduplicated
/// by `form_str`: if every matching cell yields the same string, that string
/// is returned. Only when matching cells yield *distinct* strings is the spec
/// reported as ambiguous. Returns an error when zero forms match.
fn find_form_by_spec(
    conn: &Connection,
    db_name: &str,
    form_spec: &ast::TagConditionList,
    location: &str,
    local_name: &str,
) -> Result<String, String> {
    let requested: Vec<(String, String)> = form_spec
        .conditions
        .iter()
        .map(|c| (c.axis.node.clone(), c.value.node.clone()))
        .collect();

    let mut stmt = conn
        .prepare(
            "SELECT f.form_str, f.tags FROM forms f \
             JOIN entries e ON f.entry_id = e.id \
             WHERE e.name = ?1",
        )
        .map_err(|e| format!("query failed: {}", e))?;
    let mut rows = stmt
        .query([db_name])
        .map_err(|e| format!("query failed: {}", e))?;

    let mut found: Option<String> = None;
    while let Some(row) = rows.next().map_err(|e| format!("query failed: {}", e))? {
        let form_str: String = row.get(0).map_err(|e| format!("read failed: {}", e))?;
        let tags_str: String = row.get(1).map_err(|e| format!("read failed: {}", e))?;
        let stored: Vec<(String, String)> = tags_str
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                Some((parts.next()?.to_string(), parts.next()?.to_string()))
            })
            .collect();
        let all_match = requested.iter().all(|(rk, rv)| {
            stored.iter().any(|(sk, sv)| sk == rk && sv == rv)
        });
        if all_match {
            match &found {
                Some(existing) if existing != &form_str => {
                    let tags_display = format_tags_display(form_spec);
                    return Err(format!(
                        "{}: entry '{}' has ambiguous form spec [{}]",
                        location, local_name, tags_display
                    ));
                }
                Some(_) => {} // same value — silent dedupe
                None => found = Some(form_str),
            }
        }
    }

    let tags_display = format_tags_display(form_spec);
    found.ok_or_else(|| {
        format!(
            "{}: entry '{}' has no form matching [{}]",
            location, local_name, tags_display
        )
    })
}

fn format_tags_display(form_spec: &ast::TagConditionList) -> String {
    form_spec
        .conditions
        .iter()
        .map(|c| format!("{}={}", c.axis.node, c.value.node))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Lazy compose render (Phase 4)
//
// When an `entry[axes][slot=ref, ...]` reference is hit AND the entry's
// inflection has a `Compose` body that contains at least one lazy slot, the
// render dispatch (see `Token::Ref` arms) calls into this section to assemble
// the surface form on demand. The forms-table lookup (`find_form_by_spec`) is
// bypassed entirely on this path — explicit slot fills are the source of
// truth, with phonrule wrappers around the compose chain applied at render
// time over the assembled morpheme stream (BOUNDARY-marked).
//
// Inner refs (`slot=cl_lex[axes][slot=...]`) recurse naturally through the
// top-level `resolve_with_phon_ctx` dispatch — i.e. §3.4 recursion is "free"
// once Phase 4 wires the dispatch.
//
// Phase 5 added a depth guard (`LAZY_COMPOSE_DEPTH`) so a self-referential or
// pathological recursion (e.g. `entry[][slot=entry[][slot=...]]`) returns a
// clear error instead of overflowing the call stack. The depth is tracked via
// thread-local state and bumped/popped in `render_lazy_compose` through an
// RAII `DepthGuard`. The limit (`MAX_LAZY_COMPOSE_DEPTH = 32`) is generous —
// no natural language morphology nests that deep — but defends against
// malformed input.
// ---------------------------------------------------------------------------

/// Hard cap on nested `render_lazy_compose` calls. Natural-language clitic /
/// affix recursion in conlangs never approaches this; any input that does is
/// almost certainly a self-referential cycle in the `.hut` file.
const MAX_LAZY_COMPOSE_DEPTH: usize = 32;

thread_local! {
    /// Per-thread re-entrancy counter for [`render_lazy_compose`]. Incremented
    /// on entry by [`DepthGuard::enter`] and decremented automatically on the
    /// guard's drop, so even an early `?`-propagated error path resets the
    /// counter cleanly. Renders are single-threaded (one `.hut` per render
    /// call), so a thread-local is sufficient — no atomic needed.
    static LAZY_COMPOSE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII guard for the lazy-compose recursion counter. Constructed via
/// [`DepthGuard::enter`], which returns `Err` once the depth limit is hit.
struct DepthGuard;

impl DepthGuard {
    /// Increment the depth counter and return a guard. Returns `Err(msg)`
    /// when [`MAX_LAZY_COMPOSE_DEPTH`] would be exceeded — the caller propagates
    /// this with the offending entry-ref span attached.
    fn enter(entry_name: &str, at: &str) -> Result<Self, String> {
        let next = LAZY_COMPOSE_DEPTH.with(|d| {
            let v = d.get() + 1;
            d.set(v);
            v
        });
        if next > MAX_LAZY_COMPOSE_DEPTH {
            // Decrement immediately so a recoverable test environment can
            // continue. (The guard isn't returned, so Drop won't fire.)
            LAZY_COMPOSE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
            return Err(format!(
                "{}: lazy compose recursion limit ({}) exceeded while rendering \
                 entry '{}' — likely a self-referential `[slot=...]` cycle",
                at, MAX_LAZY_COMPOSE_DEPTH, entry_name
            ));
        }
        Ok(DepthGuard)
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        LAZY_COMPOSE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Compute the union of typed axes that the inflection's eager-core slots
/// cover. Used by [`slot_parse::validate_slot_fill`] to fire the catch-all
/// warning when a feature-bearing morpheme falls through to a `CatchAll` zone
/// instead of an eager-typed slot.
fn eager_core_axes(comp: &ast::ComposeBody) -> Vec<String> {
    let mut axes: Vec<String> = Vec::new();
    for slot in &comp.slots {
        if let ast::SlotBody::Eager(rules) = &slot.body {
            for rule in rules {
                for cond in &rule.condition.conditions {
                    if !axes.contains(&cond.axis.node) {
                        axes.push(cond.axis.node.clone());
                    }
                }
            }
        }
    }
    axes
}

/// Returns `true` when the chain contains at least one lazy `SlotDef`.
fn compose_has_lazy_slot(comp: &ast::ComposeBody) -> bool {
    comp.slots
        .iter()
        .any(|s| matches!(s.body, ast::SlotBody::Lazy(_)))
}

/// Render the surface form for an entry reference that carries an explicit
/// `[slot=ref, ...]` filling, using `phon_ctx` for entry/inflection AST + the
/// phonrule resolver. Returns the assembled surface string (post phonrule).
///
/// Bypasses `find_form_by_spec` entirely; the forms-table contract for
/// Rules / all-eager Compose entries is unaffected (that path is taken when
/// `entry_ref.slot_spec.is_none()`).
fn render_lazy_compose(
    entry_ref: &ast::EntryRef,
    ctx: &ResolveContext,
    phon_ctx: &HutPhonContext,
    source_map: &SourceMap,
) -> Result<String, String> {
    let local_name = &entry_ref.entry_id.node;
    let at = loc(source_map, &entry_ref.span);

    // Phase 5 cycle guard: bump the thread-local recursion depth and bail out
    // with a clear error if a self-referential `[slot=...]` chain would
    // otherwise blow the call stack. The guard auto-decrements on drop so any
    // `?` propagation below cleans up correctly.
    let _depth_guard = DepthGuard::enter(local_name, &at)?;

    let (entry, compose) = lookup_entry_compose(entry_ref, phon_ctx, &at)?;

    // Build the cell from the first `[axes]` bracket. An absent or empty
    // form_spec just yields an empty axis map — that's fine for an
    // inflection whose only meaningful axes are covered lazily by the
    // filler morphemes themselves.
    let cell = build_cell(entry_ref);

    // Resolve each `SlotAssignment` to one or more `MorphemeInstance`s.
    // `fillers` is keyed by slot name (the slot the assignment targets).
    let mut fillers: HashMap<String, Vec<crate::slot_parse::MorphemeInstance>> = HashMap::new();
    if let Some(spec) = &entry_ref.slot_spec {
        for assignment in &spec.assignments {
            let slot_name = assignment.slot.node.clone();
            let entry_list: Vec<&ast::EntryRef> = match &assignment.value {
                ast::SlotValue::Single(boxed) => vec![boxed.as_ref()],
                ast::SlotValue::List(refs) => refs.iter().collect(),
            };
            let mut row: Vec<crate::slot_parse::MorphemeInstance> = Vec::new();
            for inner in entry_list {
                row.push(resolve_filler(inner, ctx, phon_ctx, source_map)?);
            }
            fillers.entry(slot_name).or_default().extend(row);
        }
    }

    // Phase 4 catches assignments targeting unknown / non-lazy chain slots.
    // The auto-fill path (Phase 7) never produces such assignments, so this
    // check is only meaningful for the explicit-fill path. Done in-line here
    // (rather than inside the shared core) to keep the auto-fill path noise
    // free.
    if let Some(spec) = &entry_ref.slot_spec {
        let chain_slots = collect_chain_slot_refs(&compose.chain);
        for assignment in &spec.assignments {
            let name = &assignment.slot.node;
            // Phase 6 — `infix` slots are valid fill targets even though
            // they don't appear in the compose chain (their filler is
            // spliced into the stem template, not concatenated).
            let is_infix = compose.slots.iter().any(|s| {
                s.name.node == *name && matches!(s.kind, ast::SlotKind::Infix)
            });
            let chain_has = chain_slots.iter().any(|(n, _)| n == name);
            if !chain_has && !is_infix {
                return Err(format!(
                    "{}: entry '{}' slot fill '{}' does not name any slot in \
                     the compose chain",
                    at, local_name, name
                ));
            }
            let is_lazy = compose
                .slots
                .iter()
                .any(|s| s.name.node == *name && matches!(s.body, ast::SlotBody::Lazy(_)));
            if !is_lazy {
                return Err(format!(
                    "{}: entry '{}' slot fill '{}' targets a non-lazy slot — \
                     only lazy `matching` slots accept explicit fills",
                    at, local_name, name
                ));
            }
        }
    }

    render_lazy_compose_from_fillers(
        entry, compose, &cell, fillers, phon_ctx, source_map, &at, local_name,
    )
}

/// Phase 7 dispatch entry: render `entry[axes]` (no explicit `[slot=...]`)
/// against an any-lazy compose body by **auto-filling** every lazy slot from
/// morpheme entries whose tags match the cell on each slot's filter, then
/// delegating to the shared lazy-compose assembly path.
///
/// Returns `Err` (rather than falling through) when:
/// - a `One` slot has 0 candidates or >1 candidates given the cell
/// - a `ZeroOrOne` slot has >1 candidates
/// - any other auto-fill ambiguity that would be silent
///
/// Catch-all (`matching *`) variadic slots are auto-filled with **nothing**
/// — proclitics/enclitics are peripheral and not derivable from the cell's
/// inflectional axes. Users still get explicit-fill control over those via
/// the Phase 4 `[slot=...]` syntax.
fn render_lazy_compose_autofill(
    entry_ref: &ast::EntryRef,
    phon_ctx: &HutPhonContext,
    source_map: &SourceMap,
) -> Result<String, String> {
    let local_name = &entry_ref.entry_id.node;
    let at = loc(source_map, &entry_ref.span);

    // Phase 5 cycle guard: the auto-fill path doesn't recurse through entry
    // refs (morphemes are inflectionless), but we share the guard with
    // `render_lazy_compose` for symmetry and to defend against future
    // expansions where an auto-fill candidate might itself trigger lazy
    // assembly.
    let _depth_guard = DepthGuard::enter(local_name, &at)?;

    let (entry, compose) = lookup_entry_compose(entry_ref, phon_ctx, &at)?;
    let cell = build_cell(entry_ref);

    let fillers = auto_fill_lazy_slots(entry, compose, &cell, phon_ctx, &at, local_name)?;

    render_lazy_compose_from_fillers(
        entry, compose, &cell, fillers, phon_ctx, source_map, &at, local_name,
    )
}

/// Shared lazy-compose assembly: per-slot validation (fit + quantifier +
/// catch-all warning) and `ComposeExpr` walk with deferred phonrule.
/// Used by both the explicit `[slot=...]` path (`render_lazy_compose`) and
/// the Phase 7 auto-fill path (`render_lazy_compose_autofill`).
#[allow(clippy::too_many_arguments)]
fn render_lazy_compose_from_fillers(
    entry: &ast::Entry,
    compose: &ast::ComposeBody,
    cell: &crate::inflection_eval::Cell,
    fillers: HashMap<String, Vec<crate::slot_parse::MorphemeInstance>>,
    phon_ctx: &HutPhonContext,
    source_map: &SourceMap,
    at: &str,
    _local_name: &str,
) -> Result<String, String> {
    // The entry's stems map: { stem_name -> stem_value }.
    let mut stems: HashMap<String, String> = HashMap::new();
    for stem in &entry.stems {
        stems.insert(stem.name.node.clone(), stem.value.node.clone());
    }
    // Phase 6 — populate `struct_stems` from the inflection's
    // `required_stems` constraints (mirrors `phase2::build_struct_stems`).
    // This makes `{root.C1}` etc. resolvable in eager-rule templates
    // evaluated during lazy-compose assembly. Infix fillers are layered on
    // top below.
    let inflection = lookup_inflection_ast(entry, phon_ctx);
    let mut struct_stems: HashMap<String, HashMap<String, String>> =
        build_struct_stems_at_render(
            inflection.map(|i| &i.required_stems[..]).unwrap_or(&[]),
            &stems,
            phon_ctx.axes(),
        );

    // Per-slot validation walks the compose chain (slot kinds appear in
    // chain order with the chain's quantifier). Eager slots' quantifier is
    // always `One`; lazy slots take whatever the chain declares. Stem refs
    // and undeclared chain refs are skipped here — they are not slot fills.
    let eager_axes = eager_core_axes(compose);
    let chain_slots = collect_chain_slot_refs(&compose.chain);
    for (slot_ref_name, quantifier) in &chain_slots {
        let slot_def = compose.slots.iter().find(|s| s.name.node == *slot_ref_name);
        let lazy_filter = match slot_def {
            Some(ast::SlotDef { body: ast::SlotBody::Lazy(m), .. }) => m,
            _ => continue, // stem ref or eager slot — skip
        };
        let supplied: &[crate::slot_parse::MorphemeInstance] = fillers
            .get(slot_ref_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let outcome = crate::slot_parse::validate_slot_fill(
            slot_ref_name,
            lazy_filter,
            *quantifier,
            supplied,
            &eager_axes,
        );
        for w in &outcome.warnings {
            eprintln!("warning: {}: {}", at, w.message);
        }
        if !outcome.errors.is_empty() {
            let first = &outcome.errors[0];
            return Err(format!("{}: {}", at, first.message));
        }
    }

    // Phase 6 — infix slots: a `slot NAME infix matching [...]` does NOT
    // appear in the compose chain. Splice its filler's surface into
    // `struct_stems[<stem>][NAME]` so templates that interpolate
    // `{<stem>.NAME}` see it. The stem to splice into is the unique
    // structural stem (the one whose `struct_stems` entry contains a key
    // matching the infix slot's name — pre-populated as an empty string by
    // the per-required-stem walk above).
    apply_infix_fills(compose, &fillers, &mut struct_stems, at)?;

    // Phase 6 — circumfix slots: track each slot ref's occurrence index so
    // `assemble_compose_expr` can emit prefix/suffix at the right position.
    let circumfix_names: std::collections::HashSet<String> = compose
        .slots
        .iter()
        .filter(|s| matches!(s.kind, ast::SlotKind::Circumfix))
        .map(|s| s.name.node.clone())
        .collect();
    let mut slot_occurrences: HashMap<String, u32> = HashMap::new();

    // Walk the ComposeExpr to assemble the buffer (with BOUNDARY markers
    // between adjacent parts so phonrules see the morpheme joints).
    let phon_resolver = phon_ctx.resolver();
    let buf = assemble_compose_expr(
        &compose.chain,
        compose,
        cell,
        &stems,
        &struct_stems,
        &fillers,
        &circumfix_names,
        &mut slot_occurrences,
        &phon_resolver,
        source_map,
    )?;

    Ok(crate::phonrule_eval::strip_boundaries(&buf))
}

/// Phase 6 — find the inflection AST a host entry uses, returning `None`
/// for the inline / class-not-found cases (the caller already validated
/// these in `lookup_entry_compose` for the explicit path; the auto-fill
/// path runs the same lookup before calling here).
fn lookup_inflection_ast<'a>(
    entry: &ast::Entry,
    phon_ctx: &'a HutPhonContext,
) -> Option<&'a ast::Inflection> {
    let class = match &entry.inflection {
        Some(ast::EntryInflection::Class(c)) => c,
        _ => return None,
    };
    let (_, file_id) =
        phon_ctx.find_entry_ast(&[], &entry.name.node)?;
    phon_ctx.find_inflection_ast(&class.node, file_id)
}

/// Phase 6 — render-time analog of `phase2::ContextPhase2::build_struct_stems`.
///
/// Walks the inflection's `required_stems`; for each stem whose constraint
/// references a structural axis value with `slots: [...]`, splits the stem
/// string into per-slot characters AND pre-populates every
/// `infix_positions` key with an empty string. Mirrors the compile-time
/// builder exactly so eager-rule templates work the same way through both
/// paths.
fn build_struct_stems_at_render(
    stem_reqs: &[ast::StemReq],
    stems: &HashMap<String, String>,
    axes: &std::collections::HashMap<String, crate::phase2::ResolvedAxis>,
) -> HashMap<String, HashMap<String, String>> {
    let mut struct_stems: HashMap<String, HashMap<String, String>> = HashMap::new();
    for req in stem_reqs {
        if req.constraint.is_empty() {
            continue;
        }
        let stem_val = match stems.get(&req.name.node) {
            Some(v) => v,
            None => continue,
        };
        for cond in &req.constraint {
            let axis = match axes.get(&cond.axis.node) {
                Some(a) => a,
                None => continue,
            };
            let Some(slot_names) = axis.slots.get(&cond.value.node) else {
                continue;
            };
            if slot_names.is_empty() {
                continue;
            }
            let chars: Vec<String> =
                stem_val.chars().map(|c| c.to_string()).collect();
            if chars.len() != slot_names.len() {
                // Phase 6 — should have been caught at compile time
                // (`build_struct_stems` emits the same diagnostic). Skip
                // here to avoid masking the compile-time error.
                continue;
            }
            let mut slot_map: HashMap<String, String> = slot_names
                .iter()
                .zip(chars.iter())
                .map(|(name, ch)| (name.clone(), ch.clone()))
                .collect();
            if let Some(pos_names) = axis.infix_positions.get(&cond.value.node) {
                for pos in pos_names {
                    slot_map.entry(pos.clone()).or_default();
                }
            }
            struct_stems.insert(req.name.node.clone(), slot_map);
        }
    }
    struct_stems
}

/// Phase 6 — splice each `infix` slot's filler into the appropriate
/// `struct_stems` entry. An infix slot's `name` (e.g. `after_C1`) must
/// match one of the `infix_positions` keys we pre-populated. The unique
/// stem whose map contains that key is the splice target.
///
/// Errors out if no struct_stems entry has the key, or if multiple do —
/// neither case is hit by the current single-structural-stem fixtures, but
/// the explicit message saves debug time when a future inflection wires
/// two structural stems with overlapping infix names.
fn apply_infix_fills(
    compose: &ast::ComposeBody,
    fillers: &HashMap<String, Vec<crate::slot_parse::MorphemeInstance>>,
    struct_stems: &mut HashMap<String, HashMap<String, String>>,
    at: &str,
) -> Result<(), String> {
    for slot in &compose.slots {
        if !matches!(slot.kind, ast::SlotKind::Infix) {
            continue;
        }
        let slot_name = &slot.name.node;
        let supplied = match fillers.get(slot_name) {
            Some(v) if !v.is_empty() => v,
            _ => continue, // optional infix, no filler → leave empty
        };

        // Identify which structural stem owns this infix position. The
        // pre-population step ensures the key exists in exactly one
        // stem's map (one structural axis per `required_stems` constraint).
        let owners: Vec<String> = struct_stems
            .iter()
            .filter_map(|(stem_name, slots)| {
                if slots.contains_key(slot_name) {
                    Some(stem_name.clone())
                } else {
                    None
                }
            })
            .collect();
        if owners.is_empty() {
            return Err(format!(
                "{}: infix slot '{}' has no matching `infix_positions` \
                 entry on any of this entry's structural stems",
                at, slot_name
            ));
        }
        if owners.len() > 1 {
            return Err(format!(
                "{}: infix slot '{}' matches `infix_positions` on multiple \
                 structural stems ({:?}) — names must be unique across the \
                 entry's structural stems",
                at, slot_name, owners
            ));
        }

        // Concatenate every supplied filler's surface (variadic infixes are
        // unusual but harmless — same join policy as a normal lazy slot).
        let joined: String = supplied
            .iter()
            .map(|m| m.surface.as_str())
            .collect::<Vec<_>>()
            .join("");
        if let Some(slots) = struct_stems.get_mut(&owners[0]) {
            slots.insert(slot_name.clone(), joined);
        }
    }
    Ok(())
}

/// Shared entry+inflection lookup for the two lazy-compose dispatch entry
/// points. Returns the entry and the `ComposeBody` it inflects through,
/// erroring (uniformly) when the entry is missing, has no inflection, uses
/// an inline inflection (unsupported), uses a `Rules` body, or whose Compose
/// body has no lazy slots.
fn lookup_entry_compose<'a>(
    entry_ref: &ast::EntryRef,
    phon_ctx: &'a HutPhonContext,
    at: &str,
) -> Result<(&'a ast::Entry, &'a ast::ComposeBody), String> {
    let local_name = &entry_ref.entry_id.node;
    let (entry, entry_file_id) = phon_ctx
        .find_entry_ast(&entry_ref.namespace, local_name)
        .ok_or_else(|| format!("{}: entry '{}' is not defined", at, local_name))?;

    let inflection = match &entry.inflection {
        Some(ast::EntryInflection::Class(class_ident)) => phon_ctx
            .find_inflection_ast(&class_ident.node, entry_file_id)
            .ok_or_else(|| {
                format!(
                    "{}: entry '{}' references unknown inflection class '{}'",
                    at, local_name, class_ident.node
                )
            })?,
        Some(ast::EntryInflection::Inline(_)) => {
            return Err(format!(
                "{}: inline inflections are not supported with explicit slot fills",
                at
            ));
        }
        None => {
            return Err(format!(
                "{}: entry '{}' has no inflection — cannot fill slots",
                at, local_name
            ));
        }
    };

    let compose = match &inflection.body {
        ast::InflectionBody::Compose(c) => c,
        ast::InflectionBody::Rules(_) => {
            return Err(format!(
                "{}: entry '{}' uses a Rules inflection — no slots to fill",
                at, local_name
            ));
        }
    };

    if !compose_has_lazy_slot(compose) {
        return Err(format!(
            "{}: entry '{}' has no lazy slots to fill (use [axes] instead of \
             [axes][slot=...])",
            at, local_name
        ));
    }

    Ok((entry, compose))
}

/// Build the rendering cell from an entry ref's `form_spec` (the first
/// `[axes]` bracket). Absent / empty form_spec ⇒ empty cell.
fn build_cell(entry_ref: &ast::EntryRef) -> crate::inflection_eval::Cell {
    let mut cell_tags: HashMap<String, String> = HashMap::new();
    if let Some(spec) = &entry_ref.form_spec {
        for cond in &spec.conditions {
            cell_tags.insert(cond.axis.node.clone(), cond.value.node.clone());
        }
    }
    crate::inflection_eval::Cell { tags: cell_tags }
}

// ---------------------------------------------------------------------------
// Phase 7 — render-time auto-fill of lazy slots from the cell
//
// When an `entry[axes]` reference (no explicit `[slot=...]`) lands on an
// any-lazy compose body, we auto-fill every lazy slot in the chain by
// searching the loaded `.hu` sources for inflectionless morpheme entries
// whose tags both satisfy the slot's `matching [...]` filter AND agree with
// the cell on every shared (axis, value) pair.
//
// Resolution per slot quantifier:
//   - `One`       : exactly 1 candidate required (0 ⇒ error, >1 ⇒ ambiguous error)
//   - `ZeroOrOne` : 0 or 1 candidates (0 ⇒ leave empty, 1 ⇒ use it, >1 ⇒ ambiguous error)
//   - `*`/`+`/`{n,m}`:
//       * `CatchAll` filter (no axis constraint) ⇒ auto-fill with NOTHING
//         (proclitics/enclitics are peripheral; users may still attach them
//          explicitly via the Phase 4 `[slot=...]` syntax)
//       * non-trivial filter ⇒ collect every matching candidate (in load
//         order — see `auto_fill_lazy_slots` for the iteration policy)
//
// The plan's wording about compile-time "partial forms" is one approach;
// this Phase 7 implementation takes the simpler all-render-time route. No
// compile-time changes; no new SQLite emission. The forms-table contract
// for all-eager Compose / Rules entries is unchanged.
// ---------------------------------------------------------------------------

/// For each lazy slot referenced by the compose chain, pick the morpheme
/// entries that fit the slot's filter and agree with the cell on every
/// shared axis. See the module comment above for the per-quantifier policy.
///
/// `entry` is unused for the lookup itself (we iterate every loaded entry)
/// but kept in the signature for parity with `render_lazy_compose_from_fillers`
/// and to surface a future "self-fill" / "host's own tags" extension.
fn auto_fill_lazy_slots(
    _entry: &ast::Entry,
    compose: &ast::ComposeBody,
    cell: &crate::inflection_eval::Cell,
    phon_ctx: &HutPhonContext,
    at: &str,
    host_name: &str,
) -> Result<HashMap<String, Vec<crate::slot_parse::MorphemeInstance>>, String> {
    // Snapshot of every inflectionless entry: these are the morpheme
    // candidates per the §3 "morpheme = inflectionless tagged entry" rule.
    // For Turkish-scale dictionaries this is small enough that we eat the
    // per-slot linear scan; a future indexing pass could bucket by axis.
    let candidates: Vec<&ast::Entry> = phon_ctx
        .iter_morpheme_entries()
        .collect();

    // Phase 6 — auto-fill targets:
    //   * Every lazy slot that appears in the compose chain (deduped by
    //     name so a circumfix slot, which is referenced twice, only gets
    //     looked up once).
    //   * Every `infix` slot (which by construction does NOT appear in
    //     the chain — its filler is spliced into the stem template).
    let chain_slots = collect_chain_slot_refs(&compose.chain);
    let mut targets: Vec<(String, ast::SlotQuantifier)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, q) in &chain_slots {
        if seen.insert(name.clone()) {
            targets.push((name.clone(), *q));
        }
    }
    for slot in &compose.slots {
        if matches!(slot.kind, ast::SlotKind::Infix)
            && !seen.contains(&slot.name.node)
        {
            // Phase 6 — infixes don't carry a chain quantifier; treat
            // them as `ZeroOrOne` for auto-fill so a cell with no
            // matching morpheme leaves the splice empty.
            targets.push((
                slot.name.node.clone(),
                ast::SlotQuantifier::ZeroOrOne,
            ));
            seen.insert(slot.name.node.clone());
        }
    }

    let mut fillers: HashMap<String, Vec<crate::slot_parse::MorphemeInstance>> = HashMap::new();

    for (slot_name, quantifier) in &targets {
        let slot_def = match compose.slots.iter().find(|s| s.name.node == *slot_name) {
            Some(s) => s,
            None => continue, // stem ref (not declared as a SlotDef) — skip
        };
        let lazy_filter = match &slot_def.body {
            ast::SlotBody::Lazy(m) => m,
            ast::SlotBody::Eager(_) => continue, // eager slot evaluated by rules
        };

        // Variadic + CatchAll: don't auto-fill — the cell carries no
        // information about peripheral clitics. See the module comment.
        if quantifier.is_variadic() && matches!(lazy_filter, ast::LazyMatching::CatchAll) {
            // Leave empty; the chain quantifier (`*`/etc.) allows 0.
            continue;
        }

        let matches: Vec<&ast::Entry> = candidates
            .iter()
            .copied()
            .filter(|e| candidate_fits_cell(e, lazy_filter, cell))
            .collect();

        match (matches.len(), *quantifier) {
            (0, ast::SlotQuantifier::ZeroOrOne)
            | (0, ast::SlotQuantifier::ZeroOrMore) => {
                // Slot is optional and no candidate — leave empty.
            }
            (0, ast::SlotQuantifier::Bounded { min: 0, .. }) => {
                // Bounded with min=0 — leave empty.
            }
            (0, _) => {
                return Err(format!(
                    "{}: entry '{}' has no morpheme matching slot '{}' for cell [{}]",
                    at,
                    host_name,
                    slot_name,
                    format_cell(cell)
                ));
            }
            (_, ast::SlotQuantifier::One) | (_, ast::SlotQuantifier::ZeroOrOne)
                if matches.len() > 1 =>
            {
                let cands: Vec<String> = matches
                    .iter()
                    .map(|e| e.name.node.clone())
                    .collect();
                return Err(format!(
                    "{}: entry '{}' has ambiguous auto-fill for slot '{}' \
                     with cell [{}] — candidates: [{}]",
                    at,
                    host_name,
                    slot_name,
                    format_cell(cell),
                    cands.join(", ")
                ));
            }
            _ => {
                // For variadic non-catch-all quantifiers we collect every
                // candidate. `validate_slot_fill` will still flag a count
                // outside the bound (e.g. `+` with 0 — already handled above).
            }
        }

        let mut row: Vec<crate::slot_parse::MorphemeInstance> = Vec::new();
        for m in matches {
            row.push(morpheme_instance_from_entry(m));
        }
        if !row.is_empty() {
            fillers.insert(slot_name.clone(), row);
        }
    }

    Ok(fillers)
}

/// Whether `entry` (treated as a morpheme candidate) is a good citizen of
/// the slot whose filter is `lazy_filter`, given the rendering `cell`.
///
/// Three predicates AND together:
/// 1. **Fit**: the morpheme satisfies the slot's `matching` filter (reuses
///    `slot_parse::fits`).
/// 2. **Cell agreement**: for every axis the morpheme carries that the cell
///    also sets, the values must match. The cell may carry axes the
///    morpheme is silent on (e.g. cell `negation=pos` for a tense-only
///    morpheme) — that's fine.
/// 3. **Slot domain containment**: every axis the morpheme carries must be
///    one of the slot's filter axes. This is what disambiguates
///    `tns_pc` (axes: {tense}) from `pn_pc_1sg` (axes: {tense, person,
///    number}) for a `tense_sfx matching [tense]` slot — the latter
///    "spills" into person/number, so it doesn't belong to `tense_sfx`.
///    For a `CatchAll` (`matching *`) slot the domain is universal, so
///    every entry passes this check.
///
/// Together, (1)+(3) implement the canonical "most specific slot owns the
/// morpheme" rule that the inflection author would expect when writing a
/// distinct slot per axis-group.
fn candidate_fits_cell(
    entry: &ast::Entry,
    lazy_filter: &ast::LazyMatching,
    cell: &crate::inflection_eval::Cell,
) -> bool {
    let synth = morpheme_instance_from_entry(entry);
    if !crate::slot_parse::fits(&synth, lazy_filter) {
        return false;
    }
    for cond in &entry.tags {
        let axis = &cond.axis.node;
        if let Some(cell_val) = cell.tags.get(axis) {
            if cell_val != &cond.value.node {
                return false;
            }
        }
    }
    // Domain containment: the morpheme's axes must all be in the slot's
    // filter axes. `CatchAll` has the universal domain (always passes).
    if let ast::LazyMatching::Filter(axis_filters) = lazy_filter {
        let domain: Vec<&str> = axis_filters.iter().map(|f| f.axis.node.as_str()).collect();
        for cond in &entry.tags {
            if !domain.contains(&cond.axis.node.as_str()) {
                return false;
            }
        }
    }
    true
}

/// Build a `MorphemeInstance` from an inflectionless entry. The surface form
/// is the entry's headword (matching the `forms` row contract for
/// inflectionless entries — see `phase2::expand_inflection_forms`); tags are
/// the entry's declared `tags`.
fn morpheme_instance_from_entry(entry: &ast::Entry) -> crate::slot_parse::MorphemeInstance {
    crate::slot_parse::MorphemeInstance {
        entry_id: entry.name.node.clone(),
        surface: headword_to_string(&entry.headword),
        tags: entry
            .tags
            .iter()
            .map(|c| (c.axis.node.clone(), c.value.node.clone()))
            .collect(),
        is_peripheral: entry.is_peripheral,
    }
}

/// Extract a single string from an [`ast::Headword`] for slot filler use.
/// `MultiScript` headwords fall back to the `default` entry (or the first
/// declared script) — slot fills are surface text, not glossary metadata.
fn headword_to_string(hw: &ast::Headword) -> String {
    match hw {
        ast::Headword::Simple(s) => s.node.clone(),
        ast::Headword::MultiScript(scripts) => {
            for (name, value) in scripts {
                if name.node == "default" {
                    return value.node.clone();
                }
            }
            scripts
                .first()
                .map(|(_, v)| v.node.clone())
                .unwrap_or_default()
        }
    }
}

/// Format a cell as `axis=value, ...` for error messages.
fn format_cell(cell: &crate::inflection_eval::Cell) -> String {
    let mut pairs: Vec<(String, String)> = cell
        .tags
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve a single inner [`ast::EntryRef`] used as a slot filler into a
/// [`MorphemeInstance`] suitable for `slot_parse::validate_slot_fill`.
///
/// - `surface` is computed by recursively rendering the inner ref through
///   the top-level dispatch (`resolve_with_phon_ctx`), so an inner ref with
///   its own `slot_spec` recurses naturally — §3.4 falls out for free.
/// - `tags` is the **effective tag set**: the morpheme entry's `tags` UNION
///   the inner ref's `form_spec` axes. This is what makes `e2[case=acc]`
///   count as carrying `case=acc` when fitting an `[case]` lazy filter,
///   matching the §3.1 "tags drive slot fit" rule extended to inflected
///   forms.
fn resolve_filler(
    inner: &ast::EntryRef,
    ctx: &ResolveContext,
    phon_ctx: &HutPhonContext,
    source_map: &SourceMap,
) -> Result<crate::slot_parse::MorphemeInstance, String> {
    // Recursively render the inner ref. We wrap it in a single-token slice
    // and feed it through the top-level dispatch. Any Text parts get joined
    // (typically there's exactly one); Glue or non-text parts are not
    // expected here (a slot filler is a single morpheme reference) and we
    // surface a clear error if they appear.
    let inner_token = ast::Token::Ref(inner.clone());
    let parts = resolve_with_phon_ctx(&[inner_token], ctx, Some(phon_ctx), source_map)?;
    let mut surface = String::new();
    for p in &parts {
        match p {
            ResolvedPart::Text(s) => surface.push_str(s),
            ResolvedPart::Glue
            | ResolvedPart::Newline
            | ResolvedPart::TagOpen(..)
            | ResolvedPart::TagClose(_)
            | ResolvedPart::SelfClosingTag(..)
            | ResolvedPart::PhonCallStart(_)
            | ResolvedPart::PhonCallEnd
            | ResolvedPart::ApplyBlockStart(_)
            | ResolvedPart::ApplyBlockEnd => {
                let at = loc(source_map, &inner.span);
                return Err(format!(
                    "{}: slot filler '{}' produced non-text content",
                    at, inner.entry_id.node
                ));
            }
        }
    }

    // Pull the entry AST for `tags` + `is_peripheral`. Falls back to an
    // empty tag set if the entry was looked up through a forms-table only
    // path (e.g. an old `.huc` without the AST loaded) — but `phon_ctx`
    // is always built from the same `.hu` sources so this should always
    // hit.
    let (filler_entry, _file_id) = phon_ctx
        .find_entry_ast(&inner.namespace, &inner.entry_id.node)
        .ok_or_else(|| {
            let at = loc(source_map, &inner.span);
            format!("{}: filler entry '{}' is not defined", at, inner.entry_id.node)
        })?;
    let mut tags: Vec<(String, String)> = filler_entry
        .tags
        .iter()
        .map(|c| (c.axis.node.clone(), c.value.node.clone()))
        .collect();
    if let Some(spec) = &inner.form_spec {
        for cond in &spec.conditions {
            let pair = (cond.axis.node.clone(), cond.value.node.clone());
            if !tags.contains(&pair) {
                tags.push(pair);
            }
        }
    }

    Ok(crate::slot_parse::MorphemeInstance {
        entry_id: inner.entry_id.node.clone(),
        surface,
        tags,
        is_peripheral: filler_entry.is_peripheral,
    })
}

/// Walk a `ComposeExpr` and collect `(slot_name, quantifier)` for every
/// `Slot { ... }` leaf, in left-to-right chain order. Used by Phase 4 to
/// pair lazy slots with chain quantifiers when calling
/// `slot_parse::validate_slot_fill`.
fn collect_chain_slot_refs(expr: &ast::ComposeExpr) -> Vec<(String, ast::SlotQuantifier)> {
    let mut out = Vec::new();
    fn walk(e: &ast::ComposeExpr, out: &mut Vec<(String, ast::SlotQuantifier)>) {
        match e {
            ast::ComposeExpr::Slot { name, quantifier } => {
                out.push((name.node.clone(), *quantifier));
            }
            ast::ComposeExpr::Concat(parts) => {
                for p in parts {
                    walk(p, out);
                }
            }
            ast::ComposeExpr::PhonApply { inner, .. } => walk(inner, out),
        }
    }
    walk(expr, &mut out);
    out
}

/// Recursive ComposeExpr assembler used by [`render_lazy_compose`].
///
/// Inserts [`phonrule_eval::BOUNDARY`] markers between adjacent `Concat`
/// terms so the deferred phonrule wrappers see the morpheme joints (the
/// Turkish `harmony` rule's `back !V* + !V* _` context relies on these).
/// Empty terms still consume a slot but contribute no characters — the
/// surrounding boundary marker is still emitted because a `?`/`*` slot with
/// no filler is conceptually present.
///
/// `PhonApply { rule, inner }` assembles `inner` first, then applies the
/// named phonrule via `apply_phonrule_with_resolver` — the same helper the
/// rest of the renderer uses (`apply_phonrule_chain`, `evaluate_compose`).
///
/// Phase 6 — `circumfix_names` lists slots whose filler surface is split
/// at the splice marker `^`; `slot_occurrences` tracks per-slot occurrence
/// counts as the walker descends so the first ref emits the prefix half
/// and the second ref emits the suffix half.
#[allow(clippy::too_many_arguments)]
fn assemble_compose_expr(
    expr: &ast::ComposeExpr,
    comp: &ast::ComposeBody,
    cell: &crate::inflection_eval::Cell,
    stems: &HashMap<String, String>,
    struct_stems: &HashMap<String, HashMap<String, String>>,
    fillers: &HashMap<String, Vec<crate::slot_parse::MorphemeInstance>>,
    circumfix_names: &std::collections::HashSet<String>,
    slot_occurrences: &mut HashMap<String, u32>,
    phon_resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<String, String> {
    use crate::phonrule_eval::{apply_phonrule_with_resolver, BOUNDARY};

    match expr {
        ast::ComposeExpr::Slot { name, quantifier: _ } => {
            // Stem reference?
            if let Some(stem_val) = stems.get(&name.node) {
                return Ok(stem_val.clone());
            }
            // Slot lookup
            let slot_def = comp.slots.iter().find(|s| s.name.node == name.node);
            match slot_def {
                Some(ast::SlotDef { body: ast::SlotBody::Eager(rules), .. }) => {
                    // Evaluate the eager rules against the cell, reusing the
                    // best-match logic via a tiny inline wrapper. We
                    // intentionally re-implement here rather than call
                    // `eval_compose_expr`: that function returns Option<String>
                    // for null handling, but here null slots are unusual and
                    // we keep error formatting consistent.
                    let mut best: Option<&ast::InflectionRule> = None;
                    let mut best_specificity: i32 = -1;
                    for rule in rules {
                        let matches_all = rule.condition.conditions.iter().all(|c| {
                            cell.tags.get(&c.axis.node).map(|v| v == &c.value.node).unwrap_or(false)
                        });
                        if !matches_all {
                            continue;
                        }
                        let spec = rule.condition.conditions.len() as i32;
                        if spec > best_specificity {
                            best_specificity = spec;
                            best = Some(rule);
                        }
                    }
                    let rule = best.ok_or_else(|| {
                        let tag_desc = cell
                            .tags
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "no rule matches eager slot '{}' for cell [{}]",
                            name.node, tag_desc
                        )
                    })?;
                    match &rule.rhs.node {
                        ast::RuleRhs::Template(tmpl) => {
                            crate::inflection_eval::render_template(tmpl, stems, struct_stems)
                                .map_err(|d| d.render(source_map))
                        }
                        ast::RuleRhs::Null => Ok(String::new()),
                        _ => Err(format!(
                            "eager slot '{}' has unsupported RHS in render-time lazy assembly",
                            name.node
                        )),
                    }
                }
                Some(ast::SlotDef { body: ast::SlotBody::Lazy(_), .. }) => {
                    // Phase 6 — circumfix slot: bump the per-name occurrence
                    // counter, split the (single) filler's surface at the
                    // splice marker `^`, and emit the prefix at the first
                    // ref / the suffix at the second. Validation already
                    // ensured the slot appears exactly twice in the chain
                    // (`validate_compose_layout`) and the filler had `^`
                    // exactly once.
                    if circumfix_names.contains(&name.node) {
                        let occ = slot_occurrences
                            .entry(name.node.clone())
                            .and_modify(|n| *n += 1)
                            .or_insert(1);
                        let occ_n = *occ;
                        let supplied = fillers.get(&name.node);
                        let filler = match supplied.and_then(|v| v.first()) {
                            Some(m) => m,
                            None => {
                                // Optional circumfix with no fill — both
                                // halves are empty. (Quantifier validation
                                // already handled missing-required cases.)
                                return Ok(String::new());
                            }
                        };
                        let surface = &filler.surface;
                        let (prefix, suffix) = split_circumfix_surface(
                            surface,
                            &name.node,
                            &filler.entry_id,
                        )?;
                        return Ok(if occ_n == 1 { prefix } else { suffix });
                    }

                    // Lazy slot — concatenate the filler surfaces in the
                    // order the user supplied them. Quantifier validation
                    // already ran in `render_lazy_compose`, so an
                    // unsupplied `?`/`*` slot just yields an empty string.
                    //
                    // Phase 6 — `Infix` slots are excluded from the chain
                    // by `validate_compose_layout`, so we never hit one
                    // here; their fillers are spliced into `struct_stems`
                    // before this walk.
                    let supplied = fillers.get(&name.node);
                    let mut buf = String::new();
                    if let Some(list) = supplied {
                        for (i, m) in list.iter().enumerate() {
                            if i > 0 {
                                // Multi-filler list: still mark each joint
                                // so phonrules can see them.
                                buf.push(BOUNDARY);
                            }
                            buf.push_str(&m.surface);
                        }
                    }
                    Ok(buf)
                }
                None => Err(format!(
                    "compose chain references slot '{}' which is not declared",
                    name.node
                )),
            }
        }
        ast::ComposeExpr::Concat(parts) => {
            let mut composed = String::new();
            for (i, part) in parts.iter().enumerate() {
                let piece = assemble_compose_expr(
                    part, comp, cell, stems, struct_stems, fillers,
                    circumfix_names, slot_occurrences,
                    phon_resolver, source_map,
                )?;
                if i > 0 {
                    // Always insert BOUNDARY between chain parts so the
                    // phonrule context sees morpheme joints, even when one
                    // side is empty (`?`/`*` slot with no filler). The
                    // post-assembly `strip_boundaries` cleans up leftovers.
                    composed.push(BOUNDARY);
                }
                composed.push_str(&piece);
            }
            Ok(composed)
        }
        ast::ComposeExpr::PhonApply { rule, inner } => {
            let pr = phon_resolver.resolve(&rule.node).ok_or_else(|| {
                format!("phonrule '{}' not found", rule.node)
            })?;
            let inner_buf = assemble_compose_expr(
                inner, comp, cell, stems, struct_stems, fillers,
                circumfix_names, slot_occurrences,
                phon_resolver, source_map,
            )?;
            apply_phonrule_with_resolver(&inner_buf, pr, phon_resolver)
                .map_err(|d| d.render(source_map))
        }
    }
}

/// Phase 6 — split a circumfix filler's surface at the splice marker `^`
/// into `(prefix, suffix)`. Errors if zero or more than one `^` is present
/// (the convention is exactly one splice point per circumfix entry —
/// e.g. `"ge^t"`).
fn split_circumfix_surface(
    surface: &str,
    slot_name: &str,
    entry_id: &str,
) -> Result<(String, String), String> {
    let parts: Vec<&str> = surface.splitn(3, crate::CIRCUMFIX_SPLICE).collect();
    if parts.len() != 2 {
        return Err(format!(
            "circumfix slot '{}': filler entry '{}' surface '{}' must \
             contain exactly one splice marker '{}' (got {})",
            slot_name,
            entry_id,
            surface,
            crate::CIRCUMFIX_SPLICE,
            parts.len().saturating_sub(1)
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Phase 7 dispatch helper used by both `resolve_with_phon_ctx` and
/// `resolve_annotated_with_phon_ctx`. Returns:
/// - `Ok(Some(surface))` when the entry's inflection is an any-lazy Compose
///   body — i.e. auto-fill applies and produced a surface form.
/// - `Ok(None)` when the entry is *not* an any-lazy Compose target (no
///   inflection, Rules body, all-eager Compose, missing entry, etc.) — the
///   caller falls through to the legacy `find_form_by_spec` path.
/// - `Err(msg)` when auto-fill applies but produced an actionable failure
///   (no match for a `One` slot, ambiguous candidates, etc.).
///
/// Errors that simply mean "this isn't an autofill target" (e.g. inline
/// inflection, no inflection) are swallowed into `Ok(None)` so the
/// forms-table path keeps its existing diagnostics. Errors that mean
/// "autofill was the right path but failed" propagate so the user sees a
/// specific message instead of the generic "no form matching" fallback.
fn try_render_lazy_autofill(
    entry_ref: &ast::EntryRef,
    phon_ctx: Option<&HutPhonContext>,
    source_map: &SourceMap,
) -> Result<Option<String>, String> {
    // Need the phon context to walk the entry / inflection AST. Without it
    // we can't even tell whether the entry is an autofill target, so fall
    // through silently — the forms-table path is the legacy answer for all
    // such callers.
    let pc = match phon_ctx {
        Some(p) => p,
        None => return Ok(None),
    };

    let local_name = &entry_ref.entry_id.node;

    // Cheap pre-flight: look up the entry + inflection and check whether
    // the body is any-lazy compose. If not, return `Ok(None)` so the caller
    // takes the legacy `find_form_by_spec` path.
    let (entry, file_id) = match pc.find_entry_ast(&entry_ref.namespace, local_name) {
        Some(p) => p,
        None => return Ok(None),
    };
    let class_ident = match &entry.inflection {
        Some(ast::EntryInflection::Class(c)) => c,
        // Inline / missing inflection: not an autofill target.
        _ => return Ok(None),
    };
    let inflection = match pc.find_inflection_ast(&class_ident.node, file_id) {
        Some(i) => i,
        None => return Ok(None),
    };
    let compose = match &inflection.body {
        ast::InflectionBody::Compose(c) => c,
        ast::InflectionBody::Rules(_) => return Ok(None),
    };
    if !compose_has_lazy_slot(compose) {
        return Ok(None);
    }

    // Eligible — delegate. Any failure from here is a real user-facing
    // error (no candidates, ambiguity, etc.) and propagates up.
    let surface = render_lazy_compose_autofill(entry_ref, pc, source_map)?;
    Ok(Some(surface))
}

/// Returns `true` if any [`ast::Token::Ref`] in the token tree (including
/// inside `Tag` / `PhonCall` / `ApplyBlock` wrappers) carries an explicit
/// `slot_spec`. Main.rs uses this to decide whether to pre-build a
/// [`HutPhonContext`] before calling `resolve_with_phon_ctx`.
pub fn tokens_have_slot_spec(tokens: &[ast::Token]) -> bool {
    for tok in tokens {
        match tok {
            ast::Token::Ref(r) => {
                if r.slot_spec.is_some() {
                    return true;
                }
                // Nested slot_specs are reached only through the outer ref's
                // own `slot_spec`, so the check above is sufficient — no
                // need to recurse into `SlotValue::Single`/`List` here.
            }
            ast::Token::Tag { children, .. } => {
                if tokens_have_slot_spec(children) {
                    return true;
                }
            }
            ast::Token::PhonCall { inner, .. } | ast::Token::ApplyBlock { inner, .. } => {
                if tokens_have_slot_spec(inner) {
                    return true;
                }
            }
            ast::Token::Glue
            | ast::Token::Newline
            | ast::Token::Lit(_)
            | ast::Token::SelfClosingTag { .. } => {}
        }
    }
    false
}

/// Returns `true` when the renderer needs a [`HutPhonContext`] to handle any
/// token in the tree. Used by main / render_html to decide whether to
/// pre-build the context.
///
/// Returns true for both:
/// - Explicit `[slot=...]` fills (Phase 4 path — needs entry+inflection AST
///   and the phonrule resolver during render)
/// - `[axes]`-only refs that *might* target an any-lazy compose body
///   (Phase 7 auto-fill path). We can't cheaply distinguish any-lazy
///   compose entries from forms-table entries without a SQLite lookup,
///   and an unnecessary phon-ctx build is cheap relative to a render
///   failure, so any `form_spec` triggers a pre-build.
///
/// `[axes]`-less bare refs (`headword`) and stem refs stay on the legacy
/// fast path — neither needs the AST.
pub fn tokens_need_phon_ctx(tokens: &[ast::Token]) -> bool {
    for tok in tokens {
        match tok {
            ast::Token::Ref(r) => {
                if r.slot_spec.is_some() || r.form_spec.is_some() {
                    return true;
                }
            }
            ast::Token::Tag { children, .. } => {
                if tokens_need_phon_ctx(children) {
                    return true;
                }
            }
            ast::Token::PhonCall { inner, .. } | ast::Token::ApplyBlock { inner, .. } => {
                if tokens_need_phon_ctx(inner) {
                    return true;
                }
            }
            ast::Token::Glue
            | ast::Token::Newline
            | ast::Token::Lit(_)
            | ast::Token::SelfClosingTag { .. } => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a `.hut` source string into a [`HutFile`] and its [`SourceMap`].
pub fn parse_hut(source: &str, filename: &str) -> Result<(HutFile, SourceMap), String> {
    parse_hut_with_eval(source, filename, &[])
}

/// F4: Expand a single `-e <code>` value. If the value begins with `@file:`
/// the remainder is treated as a filesystem path whose contents are returned;
/// otherwise the value is returned verbatim.
///
/// Errors propagate up as `Err(String)` with a human-readable message.
pub fn expand_eval_source(value: &str) -> Result<String, String> {
    if let Some(path) = value.strip_prefix("@file:") {
        std::fs::read_to_string(path).map_err(|e| {
            format!("cannot read '{}' (referenced by `-e @file:`): {}", path, e)
        })
    } else {
        Ok(value.to_string())
    }
}

/// Parse a `.hut` source string plus zero or more F4 `-e <code>` eval strings.
///
/// Semantics:
/// - Each entry in `eval_sources` is first run through [`expand_eval_source`]
///   so `@file:<path>` sugar transparently loads file contents.
/// - All expanded eval strings are concatenated with `;` as a statement
///   separator (F5) and parsed as a single virtual `.hut` file with filename
///   `<eval>` (a distinct [`FileId`] from the primary source).
/// - The two `HutFile`s are then merged:
///   * `references`, `uses`, `tokens` are concatenated (primary then eval).
///   * `apply_chain` from the eval source is appended to the primary's chain
///     (F4: "重ねがけ", file-level `@apply` from `-e` extends the existing
///     chain rather than replacing it).
///   * `inline_items` are concatenated.
/// - When `eval_sources` is empty this is exactly equivalent to the legacy
///   [`parse_hut`] code path (no extra `FileId` is allocated).
pub fn parse_hut_with_eval(
    source: &str,
    filename: &str,
    eval_sources: &[String],
) -> Result<(HutFile, SourceMap), String> {
    let mut source_map = SourceMap::new();
    let file_id = source_map.add_file(filename.into(), source.to_string());

    let lexer = Lexer::new(source_map.source(file_id), file_id);
    let (tokens, lex_errors) = lexer.tokenize();
    if !lex_errors.is_empty() {
        let msgs: Vec<String> = lex_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    let parser = Parser::new(tokens, file_id);
    let (mut hut_file, parse_errors) = parser.parse_token_list_to_eof();
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| e.render(&source_map)).collect();
        return Err(msgs.join("\n"));
    }

    if eval_sources.is_empty() {
        return Ok((hut_file, source_map));
    }

    // F4: expand each `-e` value (handling `@file:` sugar) and join with `;`
    // so the resulting virtual file parses each as a statement-bordered chunk.
    let mut expanded: Vec<String> = Vec::with_capacity(eval_sources.len());
    for raw in eval_sources {
        expanded.push(expand_eval_source(raw)?);
    }
    let eval_source = expanded.join("\n;\n");
    let eval_file_id = source_map.add_file("<eval>".into(), eval_source.clone());

    let eval_lexer = Lexer::new(source_map.source(eval_file_id), eval_file_id);
    let (eval_tokens, eval_lex_errors) = eval_lexer.tokenize();
    if !eval_lex_errors.is_empty() {
        let msgs: Vec<String> = eval_lex_errors
            .iter()
            .map(|e| e.render(&source_map))
            .collect();
        return Err(msgs.join("\n"));
    }

    let eval_parser = Parser::new(eval_tokens, eval_file_id);
    let (eval_hut, eval_parse_errors) = eval_parser.parse_token_list_to_eof();
    if !eval_parse_errors.is_empty() {
        let msgs: Vec<String> = eval_parse_errors
            .iter()
            .map(|e| e.render(&source_map))
            .collect();
        return Err(msgs.join("\n"));
    }

    // Merge the eval HutFile into the primary one. Order matters: eval content
    // appends to the end of every list so it behaves "as if appended to the
    // .hut file's tail" (proposal F4).
    hut_file.references.extend(eval_hut.references);
    hut_file.uses.extend(eval_hut.uses);
    hut_file.apply_chain.extend(eval_hut.apply_chain);
    hut_file.inline_items.extend(eval_hut.inline_items);
    hut_file.tokens.extend(eval_hut.tokens);

    Ok((hut_file, source_map))
}

// ---------------------------------------------------------------------------
// Cached compilation
// ---------------------------------------------------------------------------

/// Compile a `.hu` file to a `.huc` file, returning the path.
///
/// Uses mtime-based caching: if a cached `.huc` already exists and is newer
/// than the source file, compilation is skipped.  The cache is stored next to
/// the source as `<name>.hu.cache.sqlite`.
///
/// **Limitation:** transitive dependencies (files loaded via `@use`) are not
/// tracked — only the root `.hu` file's mtime is compared.
/// Check that a cached .huc file has all required tables.
fn huc_schema_up_to_date(huc_path: &Path) -> bool {
    let conn = match rusqlite::Connection::open_with_flags(
        huc_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Check for the `stems` table and `etymology_proto` column (added after initial schema).
    let ok = conn.prepare("SELECT 1 FROM stems LIMIT 0").is_ok()
        && conn.prepare("SELECT etymology_proto FROM entries LIMIT 0").is_ok();
    ok
}

pub fn compile_cached(hu_path: &Path) -> Result<PathBuf, String> {
    let hu_path = hu_path
        .canonicalize()
        .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

    let cache_dir = hu_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".hubullu-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let cache_path = cache_dir.join(
        hu_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(".hu", ".huc"),
    );

    let needs_compile = if cache_path.exists() {
        if !huc_schema_up_to_date(&cache_path) {
            true
        } else {
            // Run phase1 to discover all transitive source files, then
            // check if any of them is newer than the cached .huc.
            let cache_mtime = std::fs::metadata(&cache_path)
                .and_then(|m| m.modified())
                .map_err(|e| format!("cannot stat '{}': {}", cache_path.display(), e))?;

            let p1 = crate::phase1::run_phase1(&hu_path, Default::default());
            let any_newer = p1.path_to_id.keys().any(|src_path| {
                std::fs::metadata(src_path)
                    .and_then(|m| m.modified())
                    .map(|t| t > cache_mtime)
                    .unwrap_or(true) // if we can't stat it, assume stale
            });
            any_newer
        }
    } else {
        true
    };

    if needs_compile {
        crate::compile(&hu_path, &cache_path)?;
    }

    Ok(cache_path)
}

// ---------------------------------------------------------------------------
// Entry source — one compiled .huc file with its import rules
// ---------------------------------------------------------------------------

pub struct EntrySource {
    pub conn: Rc<Connection>,
    /// `None` = glob (all entries visible); `Some(map)` = named imports
    /// where key = local name, value = name in the .huc file.
    pub name_map: Option<HashMap<String, String>>,
}

impl EntrySource {
    /// Look up the .huc-side entry name for a local reference name.
    /// Returns `Some(huc_name)` if the entry is visible through this source.
    fn resolve_name<'a>(&'a self, local_name: &'a str) -> Option<&'a str> {
        match &self.name_map {
            None => Some(local_name), // glob — everything visible
            Some(map) => map.get(local_name).map(|s| s.as_str()),
        }
    }

    /// Check whether an entry actually exists in the `.huc` database.
    fn entry_exists(&self, huc_name: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM entries WHERE name = ?1",
                [huc_name],
                |_| Ok(()),
            )
            .is_ok()
    }
}

// ---------------------------------------------------------------------------
// Resolve context — namespace-aware lookup against .huc files
// ---------------------------------------------------------------------------

/// Holds compiled `.huc` connections and namespace mappings built from
/// `@reference` directives.
pub struct ResolveContext {
    /// namespace name → entry source
    pub namespaced: HashMap<String, EntrySource>,
    /// un-namespaced sources, searched in declaration order
    pub default_sources: Vec<EntrySource>,
}

impl ResolveContext {
    /// Build a [`ResolveContext`] from the `@reference` directives in a `.hut`
    /// file.  `hut_dir` is the directory containing the `.hut` file, used to
    /// resolve relative paths.  Each referenced `.hu` file is compiled (with
    /// mtime-based caching) to produce a `.huc` file.
    pub fn from_references(
        references: &[ast::Import],
        hut_dir: &Path,
    ) -> Result<Self, String> {
        let mut namespaced: HashMap<String, EntrySource> = HashMap::new();
        let mut default_sources: Vec<EntrySource> = Vec::new();
        // avoid compiling the same file twice
        let mut compiled: HashMap<PathBuf, Rc<Connection>> = HashMap::new();

        for import in references {
            let hu_rel = &import.path.node;
            let hu_path = hut_dir.join(hu_rel);
            let hu_canon = hu_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;

            let conn = match compiled.get(&hu_canon) {
                Some(c) => Rc::clone(c),
                None => {
                    let huc_path = compile_cached(&hu_canon)?;
                    let c = Rc::new(
                        Connection::open_with_flags(
                            &huc_path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .map_err(|e| format!("cannot open '{}': {}", huc_path.display(), e))?,
                    );
                    compiled.insert(hu_canon.clone(), Rc::clone(&c));
                    c
                }
            };

            let (namespace, name_map) = match &import.target {
                ImportTarget::Glob { alias } => {
                    (alias.as_ref().map(|a| a.node.clone()), None)
                }
                ImportTarget::Named(entries) => {
                    let map: HashMap<String, String> = entries
                        .iter()
                        .map(|e| {
                            let local = e.alias.as_ref().unwrap_or(&e.name).node.clone();
                            let huc_name = e.name.node.clone();
                            (local, huc_name)
                        })
                        .collect();
                    (None, Some(map))
                }
            };

            let source = EntrySource { conn, name_map };
            match namespace {
                Some(ns) => {
                    namespaced.insert(ns, source);
                }
                None => {
                    default_sources.push(source);
                }
            }
        }

        Ok(ResolveContext {
            namespaced,
            default_sources,
        })
    }

    /// Build a [`ResolveContext`] from a pre-compiled `.huc` file.
    ///
    /// Uses the `name_resolution` table inside the `.huc` to scope entry
    /// lookups per `@reference` directive, without re-compiling `.hu` sources.
    pub fn from_huc(
        references: &[ast::Import],
        hut_dir: &Path,
        huc_path: &Path,
    ) -> Result<Self, String> {
        use sha2::{Digest, Sha256};

        let conn = Rc::new(
            Connection::open_with_flags(huc_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("cannot open '{}': {}", huc_path.display(), e))?,
        );

        // Read entry point directory from compile_meta.
        let entry_point_dir: String = conn
            .query_row(
                "SELECT value FROM compile_meta WHERE key = 'entry_point_dir'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot read entry_point_dir from .huc: {}", e))?;
        let entry_point_dir = PathBuf::from(entry_point_dir);

        let mut namespaced: HashMap<String, EntrySource> = HashMap::new();
        let mut default_sources: Vec<EntrySource> = Vec::new();

        for import in references {
            let hu_rel = &import.path.node;
            let hu_path = hut_dir.join(hu_rel);
            // Compute relative path from the entry point directory.
            let hu_canon = hu_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve '{}': {}", hu_path.display(), e))?;
            let rel_path = hu_canon
                .strip_prefix(&entry_point_dir)
                .unwrap_or(&hu_canon);
            let file_hash = {
                let mut hasher = Sha256::new();
                hasher.update(rel_path.to_string_lossy().as_bytes());
                format!("{:x}", hasher.finalize())
            };

            // Query name_resolution for all entries visible in this file's scope.
            let scope_names: HashMap<String, String> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT nr.name, e.name FROM name_resolution nr \
                         JOIN entries e ON nr.entry_id = e.id \
                         WHERE nr.file_hash = ?1",
                    )
                    .map_err(|e| format!("query name_resolution failed: {}", e))?;
                let rows = stmt
                    .query_map([&file_hash], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| format!("query name_resolution failed: {}", e))?;
                let mut map = HashMap::new();
                for row in rows {
                    let (local_name, entry_name) =
                        row.map_err(|e| format!("read name_resolution row: {}", e))?;
                    map.insert(local_name, entry_name);
                }
                map
            };

            let (namespace, name_map) = match &import.target {
                ImportTarget::Glob { alias } => {
                    // For glob imports, the scope_names IS the name map.
                    (alias.as_ref().map(|a| a.node.clone()), Some(scope_names))
                }
                ImportTarget::Named(entries) => {
                    // For named imports, filter scope_names to only requested names.
                    let mut map = HashMap::new();
                    for entry in entries {
                        let orig_name = &entry.name.node;
                        let local = entry
                            .alias
                            .as_ref()
                            .map(|a| a.node.clone())
                            .unwrap_or_else(|| orig_name.clone());
                        if let Some(huc_name) = scope_names.get(orig_name) {
                            map.insert(local, huc_name.clone());
                        }
                    }
                    (None, Some(map))
                }
            };

            let source = EntrySource {
                conn: Rc::clone(&conn),
                name_map,
            };
            match namespace {
                Some(ns) => {
                    namespaced.insert(ns, source);
                }
                None => {
                    default_sources.push(source);
                }
            }
        }

        Ok(ResolveContext {
            namespaced,
            default_sources,
        })
    }

    /// Find the entry source and .huc-side name for the given reference.
    ///
    /// For glob imports, verifies that the entry actually exists in the DB.
    /// If the entry is found in multiple sources, returns an ambiguity error.
    fn find_entry<'a>(
        &'a self,
        namespace: &[ast::Ident],
        local_name: &'a str,
    ) -> Result<(&'a EntrySource, &'a str), String> {
        if namespace.is_empty() {
            // Search un-namespaced sources; verify DB existence and check ambiguity
            let mut found: Option<(usize, &'a str)> = None;
            for (i, src) in self.default_sources.iter().enumerate() {
                if let Some(huc_name) = src.resolve_name(local_name) {
                    if src.entry_exists(huc_name) {
                        if let Some((prev_idx, _)) = found {
                            return Err(format!(
                                "entry '{}' is ambiguous (found in @reference #{} and #{})",
                                local_name, prev_idx + 1, i + 1
                            ));
                        }
                        found = Some((i, huc_name));
                    }
                }
            }
            match found {
                Some((idx, huc_name)) => Ok((&self.default_sources[idx], huc_name)),
                None => Err(format!("entry '{}' not found in any @reference", local_name)),
            }
        } else {
            // Qualified lookup: first namespace component
            let ns = &namespace[0].node;
            let src = self
                .namespaced
                .get(ns)
                .ok_or_else(|| format!("namespace '{}' not found", ns))?;
            // If there are deeper namespaces we just join them with the entry
            // name — currently only one level is supported.
            if namespace.len() > 1 {
                return Err(format!(
                    "nested namespaces not supported: {}.{}",
                    namespace.iter().map(|i| i.node.as_str()).collect::<Vec<_>>().join("."),
                    local_name
                ));
            }
            match src.resolve_name(local_name) {
                Some(huc_name) if src.entry_exists(huc_name) => Ok((src, huc_name)),
                Some(_) => Err(format!("entry '{}' not found in namespace '{}'", local_name, ns)),
                None => Err(format!("entry '{}' not found in namespace '{}'", local_name, ns)),
            }
        }
    }

    /// Query display texts for tag axes and values.
    ///
    /// Returns a map from `(axis_name, value_name)` to `display_text`,
    /// plus a map from `axis_name` to its own display text (first row's lang).
    /// Searches all sources and merges results.
    pub fn query_tag_display(&self) -> (HashMap<String, String>, HashMap<(String, String), String>) {
        let mut axis_display: HashMap<String, String> = HashMap::new();
        let mut value_display: HashMap<(String, String), String> = HashMap::new();
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT axis_name, value_name, display_text FROM tagaxis_meta",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row in rows.flatten() {
                let (axis, value, display) = row;
                // Use the value display text for axis display if not yet set
                // (axis_name itself doesn't have a separate display row,
                // but we capitalize the axis name as fallback).
                axis_display.entry(axis.clone()).or_insert_with(|| {
                    let mut c = axis.chars();
                    match c.next() {
                        Some(first) => first.to_uppercase().to_string() + c.as_str(),
                        None => axis.clone(),
                    }
                });
                value_display.entry((axis, value)).or_insert(display);
            }
        }
        (axis_display, value_display)
    }

    /// Query all forms for a given entry name.
    ///
    /// Returns a list of `(form_string, tags_string)` pairs, where `tags_string`
    /// is comma-separated `axis=value` pairs (e.g. `"case=nom,number=sg"`).
    /// Searches default sources first, then namespaced sources.
    pub fn query_forms(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT f.form_str, f.tags FROM forms f \
                 JOIN entries e ON f.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY f.tags",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let forms: Vec<(String, String)> = rows.flatten().collect();
            if !forms.is_empty() {
                return forms;
            }
        }
        Vec::new()
    }

    /// Query all meanings for a given entry name.
    ///
    /// Returns a list of `(meaning_id, meaning_text)` pairs from `entry_meanings`.
    /// If the entry uses a single meaning (no `entry_meanings` rows), returns empty.
    pub fn query_meanings(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT m.meaning_id, m.meaning_text FROM entry_meanings m \
                 JOIN entries e ON m.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY m.rowid",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let meanings: Vec<(String, String)> = rows.flatten().collect();
            if !meanings.is_empty() {
                return meanings;
            }
        }
        Vec::new()
    }

    /// Query classificatory tags for a given entry name.
    ///
    /// Returns a list of `(axis, value)` pairs from `entry_tags`.
    pub fn query_entry_tags(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT t.axis, t.value FROM entry_tags t \
                 JOIN entries e ON t.entry_id = e.id \
                 WHERE e.name = ?1 ORDER BY t.axis, t.value",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let tags: Vec<(String, String)> = rows.flatten().collect();
            if !tags.is_empty() {
                return tags;
            }
        }
        Vec::new()
    }

    /// Query etymology information for a given entry name.
    ///
    /// Returns `(etymology_proto, etymology_note)` — both optional.
    pub fn query_etymology(&self, entry_name: &str) -> (Option<String>, Option<String>) {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let result = src.conn.query_row(
                "SELECT etymology_proto, etymology_note FROM entries WHERE name = ?1",
                [entry_name],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
            );
            if let Ok((proto, note)) = result {
                if proto.is_some() || note.is_some() {
                    return (proto, note);
                }
            }
        }
        (None, None)
    }

    /// Query the definition order of tag axis values.
    ///
    /// Returns a map from `axis_name` to an ordered list of `value_name`s,
    /// preserving the order they appear in `tagaxis_meta` (by rowid).
    pub fn query_axis_value_order(&self) -> HashMap<String, Vec<String>> {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT DISTINCT axis_name, value_name FROM tagaxis_meta ORDER BY id",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row in rows.flatten() {
                let (axis, value) = row;
                let vals = result.entry(axis).or_default();
                if !vals.contains(&value) {
                    vals.push(value);
                }
            }
        }
        result
    }

    /// Query linked entry names by link type for a given entry name.
    ///
    /// Returns a list of `(dst_entry_name, link_type)` pairs.
    pub fn query_links(&self, entry_name: &str) -> Vec<(String, String)> {
        let all_sources = self.default_sources.iter()
            .chain(self.namespaced.values());
        for src in all_sources {
            let mut stmt = match src.conn.prepare(
                "SELECT e2.name, l.link_type FROM links l \
                 JOIN entries e1 ON l.src_entry_id = e1.id \
                 JOIN entries e2 ON l.dst_entry_id = e2.id \
                 WHERE e1.name = ?1 ORDER BY l.link_type, e2.name",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([entry_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let links: Vec<(String, String)> = rows.flatten().collect();
            if !links.is_empty() {
                return links;
            }
        }
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// A resolved piece: a string part, a glue marker, or a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPart {
    Text(String),
    Glue,
    Newline,
    TagOpen(String, Vec<(String, String)>),
    TagClose(String),
    SelfClosingTag(String, Vec<(String, String)>),
    /// F1c marker: `f(` — opens an inline phon_call scope. The outer apply
    /// stack is ignored inside; only `rule` is applied to the inner phon-word(s).
    PhonCallStart(ast::Ident),
    /// F1c marker: `)` — closes the matching [`PhonCallStart`].
    PhonCallEnd,
    /// F1c marker: `@apply IDENT {` — pushes `rule` onto the active apply stack.
    ApplyBlockStart(ast::Ident),
    /// F1c marker: `}` — pops the matching [`ApplyBlockStart`].
    ApplyBlockEnd,
}

/// Metadata about a resolved entry reference (for annotated rendering).
#[derive(Debug, Clone)]
pub struct EntryAnnotation {
    /// Entry name (identifier in the .hu file).
    pub entry_name: String,
    /// The headword of the entry.
    pub headword: String,
    /// The meaning field of the entry.
    pub meaning: String,
    /// Form tags if a specific form was requested, e.g. "tense=present, number=sg".
    pub form_tags: Option<String>,
}

/// A resolved piece with optional entry annotation.
#[derive(Debug, Clone)]
pub enum AnnotatedPart {
    /// Literal text (no entry reference).
    Lit(String),
    /// Text resolved from a dictionary entry, with metadata.
    Entry { text: String, annotation: EntryAnnotation },
    Glue,
    Newline,
    /// Opening XML-like tag: `<em>` → `TagOpen("em", [])`
    TagOpen(String, Vec<(String, String)>),
    /// Closing XML-like tag: `</em>` → `TagClose("em")`
    TagClose(String),
    /// Self-closing XML-like tag: `<br/>` → `SelfClosingTag("br", [])`
    SelfClosingTag(String, Vec<(String, String)>),
    /// F1c marker: see [`ResolvedPart::PhonCallStart`].
    PhonCallStart(ast::Ident),
    /// F1c marker: see [`ResolvedPart::PhonCallEnd`].
    PhonCallEnd,
    /// F1c marker: see [`ResolvedPart::ApplyBlockStart`].
    ApplyBlockStart(ast::Ident),
    /// F1c marker: see [`ResolvedPart::ApplyBlockEnd`].
    ApplyBlockEnd,
}

/// Format an error message with source location (line:col) from a span.
fn loc(source_map: &SourceMap, span: &ast::Span) -> String {
    let path = source_map.path(span.file_id);
    let (line, col) = source_map.line_col(span.file_id, span.start);
    format!("{}:{}:{}", path.display(), line, col)
}

/// Resolve a list of AST tokens using the [`ResolveContext`].
///
/// Equivalent to [`resolve_with_phon_ctx`] called with `phon_ctx = None`.
/// References that carry an explicit `[slot=...]` slot spec require a
/// [`HutPhonContext`] (the lazy render path needs the entry/inflection AST
/// and the phonrule resolver) — without one those references fail with a
/// clear error.
pub fn resolve(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
    source_map: &SourceMap,
) -> Result<Vec<ResolvedPart>, String> {
    resolve_with_phon_ctx(tokens, ctx, None, source_map)
}

/// Like [`resolve`], but additionally accepts a [`HutPhonContext`] that
/// gives the lazy-render path access to entry/inflection AST and the
/// phonrule resolver. Pass `None` for legacy behavior.
pub fn resolve_with_phon_ctx(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
    phon_ctx: Option<&HutPhonContext>,
    source_map: &SourceMap,
) -> Result<Vec<ResolvedPart>, String> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            ast::Token::Glue => {
                parts.push(ResolvedPart::Glue);
            }
            ast::Token::Newline => {
                parts.push(ResolvedPart::Newline);
            }
            ast::Token::Lit(s) => {
                parts.push(ResolvedPart::Text(s.node.clone()));
            }
            ast::Token::Ref(entry_ref) => {
                let local_name = &entry_ref.entry_id.node;
                let at = loc(source_map, &entry_ref.span);
                let (src, db_name) = ctx.find_entry(&entry_ref.namespace, local_name)
                    .map_err(|e| format!("{}: {}", at, e))?;

                // Get headword (find_entry already verified existence).
                let headword: String = src
                    .conn
                    .query_row(
                        "SELECT headword FROM entries WHERE name = ?1",
                        [db_name],
                        |row| row.get(0),
                    )
                    .map_err(|_| format!("{}: entry '{}' is not defined", at, local_name))?;

                if let Some(stem_name) = &entry_ref.stem_spec {
                    let stem_value: String = src
                        .conn
                        .query_row(
                            "SELECT s.stem_value FROM stems s \
                             JOIN entries e ON s.entry_id = e.id \
                             WHERE e.name = ?1 AND s.stem_name = ?2",
                            rusqlite::params![db_name, stem_name.node],
                            |row| row.get(0),
                        )
                        .map_err(|_| {
                            let available = list_stems(src, db_name);
                            if available.is_empty() {
                                format!(
                                    "{}: entry '{}' has no stems defined (requested [$={}])",
                                    at, local_name, stem_name.node
                                )
                            } else {
                                format!(
                                    "{}: entry '{}' has no stem '{}' (available: {})",
                                    at, local_name, stem_name.node, available.join(", ")
                                )
                            }
                        })?;
                    parts.push(ResolvedPart::Text(stem_value));
                } else if entry_ref.slot_spec.is_some() {
                    // Phase 4: explicit [slot=...] filling. Bypasses the
                    // forms-table lookup and assembles via the compose
                    // chain + deferred phonrule.
                    let pc = phon_ctx.ok_or_else(|| {
                        format!(
                            "{}: entry '{}' uses explicit `[slot=...]` filling — \
                             the renderer needs a HutPhonContext (call \
                             `resolve_with_phon_ctx` with `Some(ctx)`)",
                            at, local_name
                        )
                    })?;
                    let surface = render_lazy_compose(entry_ref, ctx, pc, source_map)?;
                    parts.push(ResolvedPart::Text(surface));
                } else if let Some(surface) =
                    try_render_lazy_autofill(entry_ref, phon_ctx, source_map)?
                {
                    // Phase 7: legacy `[axes]` ref on an any-lazy compose
                    // entry — auto-fill every lazy slot from the cell and
                    // assemble through the shared lazy-compose path.
                    parts.push(ResolvedPart::Text(surface));
                } else { match &entry_ref.form_spec {
                    None => {
                        parts.push(ResolvedPart::Text(headword));
                    }
                    Some(form_spec) => {
                        let form_str = find_form_by_spec(&src.conn, db_name, form_spec, &at, local_name)?;
                        parts.push(ResolvedPart::Text(form_str));
                    }
                } }
            }
            ast::Token::Tag { name, attrs, children, .. } => {
                parts.push(ResolvedPart::TagOpen(name.clone(), attrs.clone()));
                parts.extend(resolve_with_phon_ctx(children, ctx, phon_ctx, source_map)?);
                parts.push(ResolvedPart::TagClose(name.clone()));
            }
            ast::Token::SelfClosingTag { name, attrs, .. } => {
                parts.push(ResolvedPart::SelfClosingTag(name.clone(), attrs.clone()));
            }
            ast::Token::PhonCall { rule, inner, .. } => {
                parts.push(ResolvedPart::PhonCallStart(rule.clone()));
                parts.extend(resolve_with_phon_ctx(inner, ctx, phon_ctx, source_map)?);
                parts.push(ResolvedPart::PhonCallEnd);
            }
            ast::Token::ApplyBlock { rule, inner, .. } => {
                parts.push(ResolvedPart::ApplyBlockStart(rule.clone()));
                parts.extend(resolve_with_phon_ctx(inner, ctx, phon_ctx, source_map)?);
                parts.push(ResolvedPart::ApplyBlockEnd);
            }
        }
    }
    Ok(parts)
}

/// Resolve a list of AST tokens with entry annotations (for HTML rendering).
///
/// Equivalent to [`resolve_annotated_with_phon_ctx`] called with
/// `phon_ctx = None`. See that function for behavior on `[slot=...]`
/// references.
pub fn resolve_annotated(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
    source_map: &SourceMap,
) -> Result<Vec<AnnotatedPart>, String> {
    resolve_annotated_with_phon_ctx(tokens, ctx, None, source_map)
}

/// Like [`resolve_annotated`], but with optional [`HutPhonContext`] for the
/// lazy compose render path.
pub fn resolve_annotated_with_phon_ctx(
    tokens: &[ast::Token],
    ctx: &ResolveContext,
    phon_ctx: Option<&HutPhonContext>,
    source_map: &SourceMap,
) -> Result<Vec<AnnotatedPart>, String> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            ast::Token::Glue => {
                parts.push(AnnotatedPart::Glue);
            }
            ast::Token::Newline => {
                parts.push(AnnotatedPart::Newline);
            }
            ast::Token::Lit(s) => {
                parts.push(AnnotatedPart::Lit(s.node.clone()));
            }
            ast::Token::Ref(entry_ref) => {
                let local_name = &entry_ref.entry_id.node;
                let at = loc(source_map, &entry_ref.span);
                let (src, db_name) = ctx.find_entry(&entry_ref.namespace, local_name)
                    .map_err(|e| format!("{}: {}", at, e))?;

                let (headword, meaning): (String, String) = src
                    .conn
                    .query_row(
                        "SELECT headword, meaning FROM entries WHERE name = ?1",
                        [db_name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|_| format!("{}: entry '{}' is not defined", at, local_name))?;

                if let Some(stem_name) = &entry_ref.stem_spec {
                    let stem_value: String = src
                        .conn
                        .query_row(
                            "SELECT s.stem_value FROM stems s \
                             JOIN entries e ON s.entry_id = e.id \
                             WHERE e.name = ?1 AND s.stem_name = ?2",
                            rusqlite::params![db_name, stem_name.node],
                            |row| row.get(0),
                        )
                        .map_err(|_| {
                            let available = list_stems(src, db_name);
                            if available.is_empty() {
                                format!(
                                    "{}: entry '{}' has no stems defined (requested [$={}])",
                                    at, local_name, stem_name.node
                                )
                            } else {
                                format!(
                                    "{}: entry '{}' has no stem '{}' (available: {})",
                                    at, local_name, stem_name.node, available.join(", ")
                                )
                            }
                        })?;
                    parts.push(AnnotatedPart::Entry {
                        text: stem_value,
                        annotation: EntryAnnotation {
                            entry_name: db_name.to_string(),
                            headword: headword.clone(),
                            meaning: meaning.clone(),
                            form_tags: Some(format!("$={}", stem_name.node)),
                        },
                    });
                } else if entry_ref.slot_spec.is_some() {
                    // Phase 4 lazy compose render — same dispatch as
                    // `resolve_with_phon_ctx`. The annotated path keeps the
                    // entry's headword/meaning so glossary tooltips still
                    // work; the form tags are taken from the explicit
                    // `[axes]` (which together with the slot fills
                    // uniquely identify the rendered cell).
                    let pc = phon_ctx.ok_or_else(|| {
                        format!(
                            "{}: entry '{}' uses explicit `[slot=...]` filling — \
                             the renderer needs a HutPhonContext (call \
                             `resolve_annotated_with_phon_ctx` with `Some(ctx)`)",
                            at, local_name
                        )
                    })?;
                    let surface = render_lazy_compose(entry_ref, ctx, pc, source_map)?;
                    let form_tags = entry_ref
                        .form_spec
                        .as_ref()
                        .map(format_tags_display);
                    parts.push(AnnotatedPart::Entry {
                        text: surface,
                        annotation: EntryAnnotation {
                            entry_name: db_name.to_string(),
                            headword,
                            meaning,
                            form_tags,
                        },
                    });
                } else if let Some(surface) =
                    try_render_lazy_autofill(entry_ref, phon_ctx, source_map)?
                {
                    // Phase 7: legacy `[axes]` ref on an any-lazy compose
                    // entry — auto-fill every lazy slot from the cell.
                    let form_tags = entry_ref
                        .form_spec
                        .as_ref()
                        .map(format_tags_display);
                    parts.push(AnnotatedPart::Entry {
                        text: surface,
                        annotation: EntryAnnotation {
                            entry_name: db_name.to_string(),
                            headword,
                            meaning,
                            form_tags,
                        },
                    });
                } else {
                    match &entry_ref.form_spec {
                        None => {
                            parts.push(AnnotatedPart::Entry {
                                text: headword.clone(),
                                annotation: EntryAnnotation {
                                    entry_name: db_name.to_string(),
                                    headword,
                                    meaning,
                                    form_tags: None,
                                },
                            });
                        }
                        Some(form_spec) => {
                            let form_str = find_form_by_spec(&src.conn, db_name, form_spec, &at, local_name)?;
                            let tags_display = format_tags_display(form_spec);
                            parts.push(AnnotatedPart::Entry {
                                text: form_str,
                                annotation: EntryAnnotation {
                                    entry_name: db_name.to_string(),
                                    headword,
                                    meaning,
                                    form_tags: Some(tags_display),
                                },
                            });
                        }
                    }
                }
            }
            ast::Token::Tag { name, attrs, children, .. } => {
                parts.push(AnnotatedPart::TagOpen(name.clone(), attrs.clone()));
                parts.extend(resolve_annotated_with_phon_ctx(children, ctx, phon_ctx, source_map)?);
                parts.push(AnnotatedPart::TagClose(name.clone()));
            }
            ast::Token::SelfClosingTag { name, attrs, .. } => {
                parts.push(AnnotatedPart::SelfClosingTag(name.clone(), attrs.clone()));
            }
            ast::Token::PhonCall { rule, inner, .. } => {
                parts.push(AnnotatedPart::PhonCallStart(rule.clone()));
                parts.extend(resolve_annotated_with_phon_ctx(inner, ctx, phon_ctx, source_map)?);
                parts.push(AnnotatedPart::PhonCallEnd);
            }
            ast::Token::ApplyBlock { rule, inner, .. } => {
                parts.push(AnnotatedPart::ApplyBlockStart(rule.clone()));
                parts.extend(resolve_annotated_with_phon_ctx(inner, ctx, phon_ctx, source_map)?);
                parts.push(AnnotatedPart::ApplyBlockEnd);
            }
        }
    }
    Ok(parts)
}

/// List available stem names for an entry (for error messages).
fn list_stems(src: &EntrySource, db_name: &str) -> Vec<String> {
    let mut stmt = match src.conn.prepare(
        "SELECT s.stem_name FROM stems s \
         JOIN entries e ON s.entry_id = e.id \
         WHERE e.name = ?1 ORDER BY s.stem_name",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([db_name], |row| row.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.flatten().collect()
}

/// Read render config from the first available `.huc` source in the context,
/// falling back to defaults.
pub fn read_render_config(ctx: &ResolveContext) -> (String, String) {
    // Try namespaced sources first, then default sources
    let all_conns = ctx
        .namespaced
        .values()
        .map(|s| &s.conn)
        .chain(ctx.default_sources.iter().map(|s| &s.conn));

    for conn in all_conns {
        let sep = conn.query_row(
            "SELECT value FROM render_config WHERE key = 'separator'",
            [],
            |row| row.get::<_, String>(0),
        );
        let no_sep = conn.query_row(
            "SELECT value FROM render_config WHERE key = 'no_separator_before'",
            [],
            |row| row.get::<_, String>(0),
        );
        if let (Ok(s), Ok(n)) = (sep, no_sep) {
            return (s, n);
        }
    }

    (" ".to_string(), ".,;:!?".to_string())
}

// ---------------------------------------------------------------------------
// Phonrule chain (F1b)
// ---------------------------------------------------------------------------

/// Per-`.hut` phonrule context. Holds the phase1/phase2 state that backs
/// `@apply` lookups for the file-level phonrule chain (F1b).
///
/// Constructed once per `.hut` render; the `PhonRuleResolver` impl borrows
/// from the inner `Phase1Result` / `PhonemeInventory`, so the context must
/// outlive any call to [`apply_phonrule_chain`].
pub struct HutPhonContext {
    p1: crate::phase1::Phase1Result,
    virtual_file_id: crate::span::FileId,
    inventory: crate::phoneme::PhonemeInventory,
    /// Phase 6 — resolved axis metadata (slot names + infix_positions per
    /// structural axis value). Used by the lazy-compose render path to
    /// build the per-stem structural map (`{root.C1}`, `{root.after_C1}`).
    axes: std::collections::HashMap<String, crate::phase2::ResolvedAxis>,
}

impl HutPhonContext {
    /// Build the context by running phase1/phase2 against the `.hut` file's
    /// `@reference` + `@use` directives. Returns `Err` if phase1 emits any
    /// diagnostic (e.g. missing `@use` target).
    pub fn build(hut_file: &HutFile, hut_dir: &Path) -> Result<Self, String> {
        let p1 = crate::phase1::run_phase1_virtual_with_uses_and_items(
            &hut_file.references,
            &hut_file.uses,
            &hut_file.inline_items,
            hut_dir,
        );
        if p1.diagnostics.has_errors() {
            return Err(p1.diagnostics.render_all(&p1.source_map));
        }
        let virtual_path = hut_dir.join("<hut-virtual>");
        let virtual_file_id = p1
            .path_to_id
            .get(&virtual_path)
            .copied()
            .ok_or_else(|| "internal: virtual .hut file not registered".to_string())?;
        let p2 = crate::phase2::run_phase2(&p1);
        if p2.diagnostics.has_errors() {
            return Err(p2.diagnostics.render_all(&p1.source_map));
        }
        Ok(Self {
            p1,
            virtual_file_id,
            inventory: p2.phonemes,
            axes: p2.axes,
        })
    }

    /// Access to resolved axis metadata (slots + infix_positions per
    /// structural axis value). Used by the lazy-compose render path to
    /// build the per-stem structural map at render time.
    pub(crate) fn axes(
        &self,
    ) -> &std::collections::HashMap<String, crate::phase2::ResolvedAxis> {
        &self.axes
    }

    /// PhonRuleResolver backed by this context.
    pub fn resolver(&self) -> HutPhonResolver<'_> {
        HutPhonResolver { ctx: self }
    }

    /// Look up an [`ast::Entry`] by local name across every loaded source
    /// file. `namespace` is currently treated as a hint when resolving via
    /// the virtual file's scope first — but `@reference` only imports
    /// entries (not the entry's *own* file scope), so we fall through to
    /// a global walk over `p1.files` when scope lookup misses. Returns the
    /// entry AST **and** the `FileId` of the file the entry was declared
    /// in (needed so the caller can resolve `inflection_class` against
    /// that file's local scope: inflections live in the source file, not
    /// in `@reference` chains).
    ///
    /// Returns `None` if no entry by that name exists in any loaded file.
    pub fn find_entry_ast(
        &self,
        namespace: &[crate::ast::Ident],
        name: &str,
    ) -> Option<(&crate::ast::Entry, crate::span::FileId)> {
        // 1. Try the virtual file's scope first — this honors namespaced
        //    `@reference` aliases.
        if let Some(scope) = self.p1.symbol_table.scope(self.virtual_file_id) {
            let resolved = if namespace.is_empty() {
                scope.resolve(name)
            } else {
                scope.resolve_qualified(&namespace[0].node, name)
            };
            for sym in resolved {
                if sym.kind == crate::symbol_table::SymbolKind::Entry {
                    if let Some(file) = self.p1.files.get(&sym.file_id) {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let crate::ast::Item::Entry(entry) = &item.node {
                                return Some((entry, sym.file_id));
                            }
                        }
                    }
                }
            }
        }

        // 2. Fall through to a global walk over every loaded file's local
        //    scope. `@reference * from "main.hu"` doesn't transitively pull
        //    main.hu's *imports* into the virtual scope (only its locals
        //    and exports), so transitively-imported entries are missed by
        //    step 1 even though the SQLite forms-table path finds them via
        //    `name_resolution`. The global walk mirrors that behavior for
        //    the AST world: the entry name is unique per compilation unit
        //    (phase1 already errored on duplicates) so the first hit wins.
        for (file_id, file) in &self.p1.files {
            if let Some(scope) = self.p1.symbol_table.scope(*file_id) {
                if let Some(sym) = scope.locals.get(name) {
                    if sym.kind == crate::symbol_table::SymbolKind::Entry {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let crate::ast::Item::Entry(entry) = &item.node {
                                return Some((entry, *file_id));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Iterate every inflectionless entry across every loaded `.hu` file in
    /// declaration order. These are the morpheme candidates for Phase 7
    /// render-time auto-fill (per the §3 "morpheme = inflectionless tagged
    /// entry" rule).
    ///
    /// Yields entries from all files visible through `phase1` — i.e. every
    /// file the `.hut`'s `@reference` chain transitively pulled in, plus the
    /// virtual file's own inline items. The iteration is deduplication-free
    /// because phase1 already enforces unique entry names per compilation
    /// unit (cross-file duplicates emit a diagnostic before render runs).
    pub fn iter_morpheme_entries(&self) -> impl Iterator<Item = &crate::ast::Entry> {
        self.p1.files.values().flat_map(|file| {
            file.items.iter().filter_map(|item| match &item.node {
                crate::ast::Item::Entry(e) if e.inflection.is_none() => {
                    Some(e.as_ref())
                }
                _ => None,
            })
        })
    }

    /// Look up an [`ast::Inflection`] by class name, scoped to `file_id`'s
    /// local + imported symbols. Inflections live in the file that
    /// declared (or `@use`d) them; entries reach them via local scope, not
    /// via `@reference` (which only carries entries).
    pub fn find_inflection_ast(
        &self,
        name: &str,
        file_id: crate::span::FileId,
    ) -> Option<&crate::ast::Inflection> {
        let scope = self.p1.symbol_table.scope(file_id)?;
        for sym in scope.resolve(name) {
            if sym.kind == crate::symbol_table::SymbolKind::Inflection {
                if let Some(file) = self.p1.files.get(&sym.file_id) {
                    if let Some(item) = file.items.get(sym.item_index) {
                        if let crate::ast::Item::Inflection(infl) = &item.node {
                            return Some(infl);
                        }
                    }
                }
            }
        }
        None
    }
}

/// `PhonRuleResolver` implementation backed by a [`HutPhonContext`].
pub struct HutPhonResolver<'a> {
    ctx: &'a HutPhonContext,
}

impl<'a> crate::inflection_eval::PhonRuleResolver for HutPhonResolver<'a> {
    fn resolve(&self, name: &str) -> Option<&crate::ast::PhonRule> {
        // 1. Virtual file scope — what an explicit `@use phonrule from "..."`
        //    in the `.hut` puts in reach.
        if let Some(scope) = self.ctx.p1.symbol_table.scope(self.ctx.virtual_file_id) {
            for sym in scope.resolve(name) {
                if sym.kind == crate::symbol_table::SymbolKind::PhonRule {
                    if let Some(file) = self.ctx.p1.files.get(&sym.file_id) {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let crate::ast::Item::PhonRule(pr) = &item.node {
                                return Some(pr);
                            }
                        }
                    }
                }
            }
        }
        // 2. Phase 7 fallback: walk every loaded file's local scope. A
        //    compose body wrapped in `harmony(elision(...))` references
        //    phonrules from the inflection's own file (which the .hut
        //    transitively pulled in via `@reference`), so the global walk
        //    finds them even without an explicit `@use` in the .hut. Same
        //    pattern as `HutPhonContext::find_entry_ast`'s fallback.
        find_phonrule_global(&self.ctx.p1, name)
    }

    fn inventory(&self) -> Option<&crate::phoneme::PhonemeInventory> {
        Some(&self.ctx.inventory)
    }

    fn resolve_syllable(&self, name: &str) -> Option<&crate::ast::Syllable> {
        if let Some(scope) = self.ctx.p1.symbol_table.scope(self.ctx.virtual_file_id) {
            for sym in scope.resolve(name) {
                if sym.kind == crate::symbol_table::SymbolKind::Syllable {
                    if let Some(file) = self.ctx.p1.files.get(&sym.file_id) {
                        if let Some(item) = file.items.get(sym.item_index) {
                            if let crate::ast::Item::Syllable(syl) = &item.node {
                                return Some(syl);
                            }
                        }
                    }
                }
            }
        }
        find_syllable_global(&self.ctx.p1, name)
    }
}

/// Walk every loaded file's locals looking for a phonrule by name. Used as
/// the Phase 7 fallback when an inflection's compose body references a
/// phonrule that's transitively loaded but not `@use`d into the virtual
/// .hut scope.
fn find_phonrule_global<'a>(
    p1: &'a crate::phase1::Phase1Result,
    name: &str,
) -> Option<&'a crate::ast::PhonRule> {
    for (file_id, file) in &p1.files {
        if let Some(scope) = p1.symbol_table.scope(*file_id) {
            if let Some(sym) = scope.locals.get(name) {
                if sym.kind == crate::symbol_table::SymbolKind::PhonRule {
                    if let Some(item) = file.items.get(sym.item_index) {
                        if let crate::ast::Item::PhonRule(pr) = &item.node {
                            return Some(pr);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Sibling to [`find_phonrule_global`] for syllable lookups.
fn find_syllable_global<'a>(
    p1: &'a crate::phase1::Phase1Result,
    name: &str,
) -> Option<&'a crate::ast::Syllable> {
    for (file_id, file) in &p1.files {
        if let Some(scope) = p1.symbol_table.scope(*file_id) {
            if let Some(sym) = scope.locals.get(name) {
                if sym.kind == crate::symbol_table::SymbolKind::Syllable {
                    if let Some(item) = file.items.get(sym.item_index) {
                        if let crate::ast::Item::Syllable(syl) = &item.node {
                            return Some(syl);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Apply the file-level `@apply` phonrule chain (F1b) to a resolved part list.
///
/// `~` (Glue) is reinterpreted as an *agglutination marker*: a maximal run of
/// `Text`-and-`Glue` parts forms one phonological word. For each phonological
/// word:
///   1. concatenate inner `Text` segments with [`phonrule_eval::BOUNDARY`]
///      (`\0`) markers in place of each `Glue`;
///   2. apply each phonrule in `apply_chain` order via
///      [`apply_phonrule_with_resolver`];
///   3. strip remaining boundary markers and collapse the run into a single
///      `Text` part (the inner `Glue` markers are consumed — the word is now
///      one token from the renderer's perspective).
///
/// `Newline` / tag parts terminate the current phonological word and are
/// passed through unchanged. If `apply_chain` is empty the input is returned
/// untouched — this preserves legacy `.hut` behaviour exactly.
///
/// F1c semantics:
///   * `PhonCallStart(rule)..PhonCallEnd` — the inner part list is collapsed
///     into a *single* phonological word; only `rule` (not the outer chain
///     or active `@apply` stack) is applied. Nested phon_calls are evaluated
///     innermost-first.
///   * `ApplyBlockStart(rule)..ApplyBlockEnd` — `rule` is pushed onto the
///     active apply stack while evaluating the inner parts. Phonological
///     words inside the block see the file-level chain followed by every
///     active block rule in nesting order.
///
/// `phonrules` are looked up in `apply_chain` order. Unknown names return an
/// error string with source location of the `@apply` directive.
pub fn apply_phonrule_chain(
    parts: Vec<ResolvedPart>,
    apply_chain: &[ast::Ident],
    resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<Vec<ResolvedPart>, String> {
    // Empty file-level chain *and* no F1c markers => legacy fast path
    // (preserves byte-for-byte behaviour for pre-F1c `.hut` files).
    let has_f1c = parts.iter().any(|p| {
        matches!(
            p,
            ResolvedPart::PhonCallStart(_)
                | ResolvedPart::PhonCallEnd
                | ResolvedPart::ApplyBlockStart(_)
                | ResolvedPart::ApplyBlockEnd
        )
    });
    if apply_chain.is_empty() && !has_f1c {
        return Ok(parts);
    }

    let mut out: Vec<ResolvedPart> = Vec::with_capacity(parts.len());
    // Active `@apply` block stack. File-level chain is the prefix; block
    // rules are pushed on top per nesting level.
    let mut stack: Vec<ast::Ident> = apply_chain.to_vec();
    let file_level_depth = stack.len();
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            ResolvedPart::Text(_) => {
                // Start of a phonological word. Collect the first Text plus
                // any (`Glue+Text`) extensions. A `Text` *not* preceded by
                // `Glue` is a new phonological word, so we stop the run.
                let mut buf = String::new();
                let mut have_first = false;
                while i < parts.len() {
                    match &parts[i] {
                        ResolvedPart::Text(s) => {
                            if have_first {
                                // Two adjacent Text parts without a Glue
                                // between them = separate phon words.
                                break;
                            }
                            buf.push_str(s);
                            have_first = true;
                            i += 1;
                        }
                        ResolvedPart::Glue => {
                            // Look ahead: extend the word only if the next
                            // non-Glue part is another Text. Trailing Glue
                            // (no following Text in the run) breaks out so
                            // smart_join can still suppress the separator
                            // the legacy way.
                            let mut j = i;
                            while j < parts.len() && matches!(parts[j], ResolvedPart::Glue) {
                                j += 1;
                            }
                            if j < parts.len() {
                                if let ResolvedPart::Text(s) = &parts[j] {
                                    buf.push(crate::phonrule_eval::BOUNDARY);
                                    buf.push_str(s);
                                    i = j + 1;
                                    continue;
                                }
                            }
                            break;
                        }
                        _ => break,
                    }
                }
                // Apply file-level + active block rules in stack order.
                let final_text = apply_rules_resolved(&buf, &stack, resolver, source_map)?;
                out.push(ResolvedPart::Text(final_text));
            }
            ResolvedPart::Glue => {
                // Leading or stranded Glue (no preceding Text) — pass through
                // so that smart_join's separator suppression still fires.
                out.push(ResolvedPart::Glue);
                i += 1;
            }
            ResolvedPart::PhonCallStart(rule) => {
                // Find matching PhonCallEnd (respecting nested PhonCall and
                // ApplyBlock pairs). Inner sub-list is evaluated as one
                // phonological word, with *only* `rule` applied (outer stack
                // is ignored: explicit phon_call overrides ambient apply).
                let (end, sub) = take_balanced(&parts, i, true);
                let single = resolved_to_single_word(&sub, resolver, source_map)?;
                let just_this = vec![rule.clone()];
                let final_text = apply_rules_resolved(&single, &just_this, resolver, source_map)?;
                out.push(ResolvedPart::Text(final_text));
                i = end + 1; // skip past the matching PhonCallEnd
            }
            ResolvedPart::PhonCallEnd => {
                // Top-level dangling End — should not happen because parser
                // pairs them. Defensively drop.
                i += 1;
            }
            ResolvedPart::ApplyBlockStart(rule) => {
                stack.push(rule.clone());
                i += 1;
            }
            ResolvedPart::ApplyBlockEnd => {
                if stack.len() > file_level_depth {
                    stack.pop();
                }
                i += 1;
            }
            ResolvedPart::Newline
            | ResolvedPart::TagOpen(..)
            | ResolvedPart::TagClose(_)
            | ResolvedPart::SelfClosingTag(..) => {
                out.push(parts[i].clone());
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Resolve a list of `chain` idents against `resolver`, returning an error
/// (with source location) on the first unknown name.
fn resolve_rules<'a>(
    chain: &[ast::Ident],
    resolver: &'a dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<Vec<&'a crate::ast::PhonRule>, String> {
    let mut rules: Vec<&crate::ast::PhonRule> = Vec::with_capacity(chain.len());
    for ident in chain {
        match resolver.resolve(&ident.node) {
            Some(rule) => rules.push(rule),
            None => {
                let at = loc(source_map, &ident.span);
                return Err(format!(
                    "{}: @apply refers to undefined phonrule '{}'",
                    at, ident.node
                ));
            }
        }
    }
    Ok(rules)
}

/// Apply each phonrule in `chain` order to a buffer that already contains
/// the phonological word (with `BOUNDARY` markers between agglutinated
/// segments), then strip the boundaries.
fn apply_rules_resolved(
    buf: &str,
    chain: &[ast::Ident],
    resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<String, String> {
    let rules = resolve_rules(chain, resolver, source_map)?;
    let mut s = buf.to_string();
    for rule in &rules {
        s = crate::phonrule_eval::apply_phonrule_with_resolver(&s, rule, resolver)
            .map_err(|d| d.render(source_map))?;
    }
    Ok(crate::phonrule_eval::strip_boundaries(&s))
}

/// Locate the matching `*End` marker for the `Start` at `start_idx`.
///
/// Returns `(end_idx, inner_parts)` where `inner_parts` is a fresh `Vec` of
/// the parts strictly between the open and close (exclusive). Nested phon_call
/// and apply_block pairs are balanced. `phon_call` selects which Start/End
/// kind to balance: `true` for [`ResolvedPart::PhonCallStart`], `false` for
/// [`ResolvedPart::ApplyBlockStart`].
fn take_balanced(
    parts: &[ResolvedPart],
    start_idx: usize,
    phon_call: bool,
) -> (usize, Vec<ResolvedPart>) {
    let mut depth = 1usize;
    let mut j = start_idx + 1;
    while j < parts.len() {
        match (&parts[j], phon_call) {
            (ResolvedPart::PhonCallStart(_), true) => depth += 1,
            (ResolvedPart::PhonCallEnd, true) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            (ResolvedPart::ApplyBlockStart(_), false) => depth += 1,
            (ResolvedPart::ApplyBlockEnd, false) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    let inner = parts[start_idx + 1..j.min(parts.len())].to_vec();
    (j.min(parts.len()), inner)
}

/// Collapse a sub-sequence of `ResolvedPart` into a single phonological-word
/// buffer (with `BOUNDARY` markers between concatenated segments).
///
/// Used to evaluate the `inner` of a `PhonCall`: the inner sequence is
/// forced to *one* phon-word. Inner `PhonCall`s are evaluated recursively
/// (innermost first) and contribute their finished text; inner `ApplyBlock`s
/// contribute their inner text under the ambient block rules. Inner `Newline`
/// and tag parts are skipped (they have no phonological meaning inside a
/// `phon_call`).
fn resolved_to_single_word(
    parts: &[ResolvedPart],
    resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<String, String> {
    let mut buf = String::new();
    let mut first = true;
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            ResolvedPart::Text(s) => {
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(s);
                first = false;
                i += 1;
            }
            ResolvedPart::Glue => {
                // Inside a phon_call, `~` is a boundary marker between
                // segments — same role as the implicit boundary above. We
                // just skip it; the next Text segment will emit a boundary.
                i += 1;
            }
            ResolvedPart::PhonCallStart(rule) => {
                let (end, sub) = take_balanced(parts, i, true);
                let inner_buf = resolved_to_single_word(&sub, resolver, source_map)?;
                let just_this = vec![rule.clone()];
                let inner_text = apply_rules_resolved(&inner_buf, &just_this, resolver, source_map)?;
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(&inner_text);
                first = false;
                i = end + 1;
            }
            ResolvedPart::PhonCallEnd => {
                i += 1;
            }
            ResolvedPart::ApplyBlockStart(rule) => {
                // Inside a phon_call, an inner `@apply` block applies its
                // rule to its inner segment before that segment is folded
                // into the surrounding phon-word.
                let (end, sub) = take_balanced(parts, i, false);
                let inner_buf = resolved_to_single_word(&sub, resolver, source_map)?;
                let just_this = vec![rule.clone()];
                let inner_text = apply_rules_resolved(&inner_buf, &just_this, resolver, source_map)?;
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(&inner_text);
                first = false;
                i = end + 1;
            }
            ResolvedPart::ApplyBlockEnd
            | ResolvedPart::Newline
            | ResolvedPart::TagOpen(..)
            | ResolvedPart::TagClose(_)
            | ResolvedPart::SelfClosingTag(..) => {
                // Structural / non-phonological parts have no role inside a
                // phon_call — drop them.
                i += 1;
            }
        }
    }
    Ok(buf)
}

/// Annotated-part variant of [`apply_phonrule_chain`] for the HTML pipeline.
///
/// Behaves identically to the plain version: maximal `Lit`/`Entry`/`Glue`
/// runs form a phonological word and are collapsed into one `Lit` part after
/// applying the chain. Any `Entry` annotation in the run is discarded — the
/// post-phonrule string no longer corresponds to a single dictionary entry,
/// so emitting a glossary tooltip on it would be misleading.
pub fn apply_phonrule_chain_annotated(
    parts: Vec<AnnotatedPart>,
    apply_chain: &[ast::Ident],
    resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<Vec<AnnotatedPart>, String> {
    let has_f1c = parts.iter().any(|p| {
        matches!(
            p,
            AnnotatedPart::PhonCallStart(_)
                | AnnotatedPart::PhonCallEnd
                | AnnotatedPart::ApplyBlockStart(_)
                | AnnotatedPart::ApplyBlockEnd
        )
    });
    if apply_chain.is_empty() && !has_f1c {
        return Ok(parts);
    }

    fn part_text(part: &AnnotatedPart) -> Option<&str> {
        match part {
            AnnotatedPart::Lit(t) | AnnotatedPart::Entry { text: t, .. } => Some(t.as_str()),
            _ => None,
        }
    }

    let mut out: Vec<AnnotatedPart> = Vec::with_capacity(parts.len());
    let mut stack: Vec<ast::Ident> = apply_chain.to_vec();
    let file_level_depth = stack.len();
    let mut i = 0;
    while i < parts.len() {
        if part_text(&parts[i]).is_some() {
            let mut buf = String::new();
            let mut have_first = false;
            while i < parts.len() {
                if let Some(t) = part_text(&parts[i]) {
                    if have_first {
                        break;
                    }
                    buf.push_str(t);
                    have_first = true;
                    i += 1;
                } else if matches!(parts[i], AnnotatedPart::Glue) {
                    let mut j = i;
                    while j < parts.len() && matches!(parts[j], AnnotatedPart::Glue) {
                        j += 1;
                    }
                    if j < parts.len() {
                        if let Some(t) = part_text(&parts[j]) {
                            buf.push(crate::phonrule_eval::BOUNDARY);
                            buf.push_str(t);
                            i = j + 1;
                            continue;
                        }
                    }
                    break;
                } else {
                    break;
                }
            }
            let final_text = apply_rules_resolved(&buf, &stack, resolver, source_map)?;
            out.push(AnnotatedPart::Lit(final_text));
        } else if matches!(parts[i], AnnotatedPart::Glue) {
            out.push(AnnotatedPart::Glue);
            i += 1;
        } else if let AnnotatedPart::PhonCallStart(rule) = &parts[i] {
            let (end, sub) = take_balanced_annotated(&parts, i, true);
            let single = annotated_to_single_word(&sub, resolver, source_map)?;
            let just_this = vec![rule.clone()];
            let final_text = apply_rules_resolved(&single, &just_this, resolver, source_map)?;
            out.push(AnnotatedPart::Lit(final_text));
            i = end + 1;
        } else if matches!(&parts[i], AnnotatedPart::PhonCallEnd) {
            i += 1;
        } else if let AnnotatedPart::ApplyBlockStart(rule) = &parts[i] {
            stack.push(rule.clone());
            i += 1;
        } else if matches!(&parts[i], AnnotatedPart::ApplyBlockEnd) {
            if stack.len() > file_level_depth {
                stack.pop();
            }
            i += 1;
        } else {
            out.push(parts[i].clone());
            i += 1;
        }
    }
    Ok(out)
}

/// Annotated-part counterpart to [`take_balanced`].
fn take_balanced_annotated(
    parts: &[AnnotatedPart],
    start_idx: usize,
    phon_call: bool,
) -> (usize, Vec<AnnotatedPart>) {
    let mut depth = 1usize;
    let mut j = start_idx + 1;
    while j < parts.len() {
        match (&parts[j], phon_call) {
            (AnnotatedPart::PhonCallStart(_), true) => depth += 1,
            (AnnotatedPart::PhonCallEnd, true) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            (AnnotatedPart::ApplyBlockStart(_), false) => depth += 1,
            (AnnotatedPart::ApplyBlockEnd, false) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    let inner = parts[start_idx + 1..j.min(parts.len())].to_vec();
    (j.min(parts.len()), inner)
}

/// Annotated-part counterpart to [`resolved_to_single_word`].
fn annotated_to_single_word(
    parts: &[AnnotatedPart],
    resolver: &dyn crate::inflection_eval::PhonRuleResolver,
    source_map: &SourceMap,
) -> Result<String, String> {
    let mut buf = String::new();
    let mut first = true;
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            AnnotatedPart::Lit(s) | AnnotatedPart::Entry { text: s, .. } => {
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(s);
                first = false;
                i += 1;
            }
            AnnotatedPart::Glue => {
                i += 1;
            }
            AnnotatedPart::PhonCallStart(rule) => {
                let (end, sub) = take_balanced_annotated(parts, i, true);
                let inner_buf = annotated_to_single_word(&sub, resolver, source_map)?;
                let just_this = vec![rule.clone()];
                let inner_text = apply_rules_resolved(&inner_buf, &just_this, resolver, source_map)?;
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(&inner_text);
                first = false;
                i = end + 1;
            }
            AnnotatedPart::PhonCallEnd => {
                i += 1;
            }
            AnnotatedPart::ApplyBlockStart(rule) => {
                let (end, sub) = take_balanced_annotated(parts, i, false);
                let inner_buf = annotated_to_single_word(&sub, resolver, source_map)?;
                let just_this = vec![rule.clone()];
                let inner_text = apply_rules_resolved(&inner_buf, &just_this, resolver, source_map)?;
                if !first {
                    buf.push(crate::phonrule_eval::BOUNDARY);
                }
                buf.push_str(&inner_text);
                first = false;
                i = end + 1;
            }
            AnnotatedPart::ApplyBlockEnd
            | AnnotatedPart::Newline
            | AnnotatedPart::TagOpen(..)
            | AnnotatedPart::TagClose(_)
            | AnnotatedPart::SelfClosingTag(..) => {
                i += 1;
            }
        }
    }
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Smart join
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// PartRenderer trait — format-agnostic rendering of AnnotatedPart sequences
// ---------------------------------------------------------------------------

/// Trait for rendering a sequence of resolved [`AnnotatedPart`]s into a
/// target format (HTML, plain text, etc.).
///
/// Library users can implement this trait to add custom output formats
/// (e.g. LaTeX, EPUB) without modifying the core resolution pipeline.
pub trait PartRenderer {
    /// Render annotated parts into the target format.
    ///
    /// * `separator` — default token separator (e.g. `" "`)
    /// * `no_sep_before` — characters that suppress the preceding separator
    ///   (e.g. `".,;:!?"`)
    fn render(&self, parts: &[AnnotatedPart], separator: &str, no_sep_before: &str) -> String;
}

/// Plain-text renderer: strips tags and joins text content with separators.
pub struct PlainTextRenderer;

impl PartRenderer for PlainTextRenderer {
    fn render(&self, parts: &[AnnotatedPart], separator: &str, no_sep_before: &str) -> String {
        let mut result = String::new();
        let mut glue_next = false;
        let mut newline_next = false;
        let mut has_content = false;

        for part in parts {
            match part {
                AnnotatedPart::Glue => {
                    glue_next = true;
                }
                AnnotatedPart::Newline => {
                    newline_next = true;
                    glue_next = false;
                }
                AnnotatedPart::Lit(text) | AnnotatedPart::Entry { text, .. } => {
                    if newline_next {
                        result.push('\n');
                        newline_next = false;
                    } else if has_content && !separator.is_empty() && !glue_next {
                        let suppress = text
                            .chars()
                            .next()
                            .map(|c| no_sep_before.contains(c))
                            .unwrap_or(false);
                        if !suppress {
                            result.push_str(separator);
                        }
                    }
                    glue_next = false;
                    has_content = true;
                    result.push_str(text);
                }
                // Tags are stripped in plain-text output.
                AnnotatedPart::TagOpen(..)
                | AnnotatedPart::TagClose(_)
                | AnnotatedPart::SelfClosingTag(..) => {}
                // F1c markers — consumed by `apply_phonrule_chain_annotated`;
                // any residual markers are dropped silently.
                AnnotatedPart::PhonCallStart(_)
                | AnnotatedPart::PhonCallEnd
                | AnnotatedPart::ApplyBlockStart(_)
                | AnnotatedPart::ApplyBlockEnd => {}
            }
        }
        result
    }
}

/// Join resolved parts using separator, suppressing it before certain characters
/// and around `Glue` markers.
pub fn smart_join(parts: &[ResolvedPart], separator: &str, no_sep_before: &str) -> String {
    let mut result = String::new();
    let mut glue_next = false;
    let mut newline_next = false;
    for part in parts {
        match part {
            ResolvedPart::Glue => {
                glue_next = true;
            }
            ResolvedPart::Newline => {
                newline_next = true;
                glue_next = false;
            }
            ResolvedPart::Text(text) => {
                if newline_next {
                    result.push('\n');
                    newline_next = false;
                } else if !result.is_empty() && !separator.is_empty() && !glue_next {
                    let first_char = text.chars().next();
                    let suppress = first_char
                        .map(|c| no_sep_before.contains(c))
                        .unwrap_or(false);
                    if !suppress {
                        result.push_str(separator);
                    }
                }
                glue_next = false;
                result.push_str(text);
            }
            // Tags are structural markers for HTML; plain-text join ignores them.
            ResolvedPart::TagOpen(..)
            | ResolvedPart::TagClose(_)
            | ResolvedPart::SelfClosingTag(..) => {}
            // F1c markers should already be consumed by `apply_phonrule_chain`.
            // If they reach this point (e.g. when the chain is empty), drop
            // them silently — they have no plain-text representation.
            ResolvedPart::PhonCallStart(_)
            | ResolvedPart::PhonCallEnd
            | ResolvedPart::ApplyBlockStart(_)
            | ResolvedPart::ApplyBlockEnd => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ResolvedPart {
        ResolvedPart::Text(s.to_string())
    }

    #[test]
    fn test_smart_join_basic() {
        let parts = vec![text("La"), text("hundo"), text("dormas"), text(".")];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "La hundo dormas.");
    }

    #[test]
    fn test_smart_join_glue() {
        // mal~bon~a hundo → "malbona hundo"
        let parts = vec![
            text("mal"),
            ResolvedPart::Glue,
            text("bon"),
            ResolvedPart::Glue,
            text("a"),
            text("hundo"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "malbona hundo");
    }

    #[test]
    fn test_smart_join_glue_with_punctuation() {
        // mal~bon~a hundo "."
        let parts = vec![
            text("mal"),
            ResolvedPart::Glue,
            text("bona"),
            text("hundo"),
            text("."),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "malbona hundo.");
    }

    #[test]
    fn test_smart_join_newline() {
        // "hello" // "world" → "hello\nworld"
        let parts = vec![
            text("hello"),
            ResolvedPart::Newline,
            text("world"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "hello\nworld");
    }

    #[test]
    fn test_smart_join_newline_no_extra_separator() {
        // Newline should replace separator, not add one
        let parts = vec![
            text("line1"),
            text("word"),
            ResolvedPart::Newline,
            text("line2"),
        ];
        assert_eq!(smart_join(&parts, " ", ".,;:!?"), "line1 word\nline2");
    }

    #[test]
    fn test_parse_hut_newline() {
        let (hut, _sm) = parse_hut(r#""hello" // "world""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 3);
        assert!(matches!(hut.tokens[0], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[1], ast::Token::Newline));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
    }

    #[test]
    fn test_parse_hut_stem_spec() {
        let (hut, _sm) = parse_hut(r#"gelmek[$=root]~"iyor""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 3);
        if let ast::Token::Ref(r) = &hut.tokens[0] {
            assert_eq!(r.entry_id.node, "gelmek");
            assert!(r.form_spec.is_none());
            assert_eq!(r.stem_spec.as_ref().unwrap().node, "root");
        } else {
            panic!("expected Ref token");
        }
        assert!(matches!(hut.tokens[1], ast::Token::Glue));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
    }

    #[test]
    fn test_parse_hut_glue() {
        let (hut, _sm) = parse_hut(r#""mal"~"bona" "hundo""#, "test.hut").unwrap();
        assert_eq!(hut.tokens.len(), 4);
        assert!(matches!(hut.tokens[0], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[1], ast::Token::Glue));
        assert!(matches!(hut.tokens[2], ast::Token::Lit(_)));
        assert!(matches!(hut.tokens[3], ast::Token::Lit(_)));
    }

    #[test]
    fn test_parse_hut_with_reference() {
        let src = r#"@reference * from "lang.hu"
"The" cat walk[tense=present, person=3, number=sg] "."
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 1);
        assert_eq!(hut.references[0].path.node, "lang.hu");
        assert!(hut.tokens.len() >= 3);
    }

    // F5: `;` is a statement separator in `.hu` only; `.hut` token lists are
    // not statement-based, so a `;` should surface as a parse error.
    #[test]
    fn test_parse_hut_semicolon_is_error() {
        let result = parse_hut(r#""a" ; "b""#, "test.hut");
        assert!(
            result.is_err(),
            "expected `;` inside `.hut` to produce a parse error"
        );
    }

    #[test]
    fn test_parse_hut_with_namespaced_reference() {
        let src = r#"@reference * as en from "english.hu"
en.cat en.walk[tense=present] "."
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 1);
        // Check namespace on the entry ref
        if let ast::Token::Ref(r) = &hut.tokens[0] {
            assert_eq!(r.namespace.len(), 1);
            assert_eq!(r.namespace[0].node, "en");
            assert_eq!(r.entry_id.node, "cat");
        } else {
            panic!("expected Ref token");
        }
    }

    // F1a: `.hut` accepts `@use` directives, parseable on their own.
    #[test]
    fn test_parse_hut_with_use_glob() {
        let src = r#"@use * from "phon/rules.hu"
"a"
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert!(hut.references.is_empty());
        assert_eq!(hut.uses.len(), 1);
        assert_eq!(hut.uses[0].path.node, "phon/rules.hu");
        assert!(matches!(hut.uses[0].target, ast::ImportTarget::Glob { alias: None }));
    }

    // F1a: `.hut` accepts `@use` with named imports.
    #[test]
    fn test_parse_hut_with_use_named() {
        let src = r#"@use lenition, palatalization as p from "phon/rules.hu"
"a"
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.uses.len(), 1);
        match &hut.uses[0].target {
            ast::ImportTarget::Named(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].name.node, "lenition");
                assert!(entries[0].alias.is_none());
                assert_eq!(entries[1].name.node, "palatalization");
                assert_eq!(entries[1].alias.as_ref().unwrap().node, "p");
            }
            other => panic!("expected Named, got {:?}", other),
        }
    }

    // F1a: `@reference` and `@use` can be mixed in any order at the top.
    #[test]
    fn test_parse_hut_reference_use_free_order() {
        let src = r#"@reference * as en from "english.hu"
@use lenition from "phon/rules.hu"
@reference * as ja from "japanese.hu"
@use * from "phon/more.hu"
en.cat
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 2);
        assert_eq!(hut.uses.len(), 2);
        assert_eq!(hut.references[0].path.node, "english.hu");
        assert_eq!(hut.references[1].path.node, "japanese.hu");
        assert_eq!(hut.uses[0].path.node, "phon/rules.hu");
        assert_eq!(hut.uses[1].path.node, "phon/more.hu");
    }

    // F1a: `@use` followed by `@reference` (reverse order) is also accepted.
    #[test]
    fn test_parse_hut_use_before_reference() {
        let src = r#"@use * from "phon/rules.hu"
@reference * as en from "english.hu"
en.cat
"#;
        let (hut, _sm) = parse_hut(src, "test.hut").unwrap();
        assert_eq!(hut.references.len(), 1);
        assert_eq!(hut.uses.len(), 1);
    }
}
