//! `PhonMapDef` → transducer FST (F2a step 3).
//!
//! Per the F2 plan §2 table row for `PhonMapDef`: a map is a single-symbol
//! transducer (one input symbol → one output symbol).
//!
//! ## Semantics — matches `phonrule_eval::apply_map` exactly
//!
//! `apply_map` (`src/phonrule_eval.rs:523`) is a **total** function over
//! single-character inputs. For any `ch`:
//!
//!   1. Find the named map (if absent, return `ch` unchanged — identity
//!      fallback). This case doesn't arise inside the FST: the map FST is
//!      only built when the map exists; the "missing map" branch is
//!      enforced one layer up.
//!   2. Walk the arms in declaration order; the first arm whose `from`
//!      equals `ch` returns its `to` (literal or, for `Var`, identity).
//!   3. If no arm matched but `else_arm` is present, return its result
//!      (literal or, for `Var`, identity).
//!   4. **If no arm matched and there is no `else_arm`, return `ch` unchanged**
//!      (eval `phonrule_eval.rs:544-545`).
//!
//! Point 4 is the subtle one and the task brief calls it out. The plan's
//! §2 table description ("`else_arm` = identity over class complement;
//! arms cause non-acceptance otherwise") is not what the evaluator does.
//! The evaluator's behaviour is **identity over the complement of arm
//! inputs, regardless of `else_arm` presence**; an explicit `else -> X`
//! changes the output to `X` instead of identity. F2 is validated against
//! the evaluator (plan §6.3 "100% match"), so the FST implements eval's
//! semantics — not the plan's table description. (Plan §6.3 explicitly
//! anticipates this kind of divergence — "if divergence is a phonrule_eval
//! bug … fix phonrule_eval to match the FST" — but for F2a we mirror eval
//! and document the divergence; any policy change is a follow-up
//! conversation, not an F2a code change.)
//!
//! ## FST construction
//!
//! Two-state FST: start `s0`, final `s1`. One arc per case:
//!
//!   * For every arm `from -> to`: arc `s0 -[from:to]-> s1`. (For
//!     `Var`-result arms, `to_label == from_label` — identity.)
//!   * For every symbol in Σ NOT covered by an arm: an identity arc
//!     `s0 -[sym:sym]-> s1`, encoding either the explicit `else -> Var`
//!     case or the implicit no-`else_arm` fallback (eval line 545).
//!   * For `else -> Literal(lit)`: each uncovered Σ symbol gets arc
//!     `s0 -[sym:lit_label]-> s1`. (Note: this collapses many inputs to a
//!     single output — an unusual map shape but eval supports it; the FST
//!     reproduces it faithfully.)
//!
//! ### Alphabet timing
//!
//! "Every symbol in Σ NOT covered by an arm" is evaluated **at the time of
//! the call**. If new symbols are interned into `alpha` after `compile_map`
//! returns (e.g. by later rule compilation), the returned FST will NOT have
//! identity arcs for them. In F2a this is acceptable: tests build the
//! alphabet first, then compile maps. F2b will likely want to defer map
//! arc-laying until after all alphabet-extending rule compilation, or to
//! re-emit maps after the alphabet stabilises — but that's an F2b
//! scheduling concern, not a semantics concern.

use std::collections::HashSet;

use crate::ast::{PhonMapBody, PhonMapDef, PhonMapElse, PhonMapResult};

use super::super::alphabet::PhonruleAlphabet;
use super::super::backend::{FstBuilder, Label};
use super::super::rustfst_backend::{RustFstBackend, RustFstWrapper};
use super::super::FstBackend;

/// Compile a `PhonMapDef` to a two-state transducer FST.
///
/// Behaviour matches `phonrule_eval::apply_map` (`src/phonrule_eval.rs:523`)
/// on every single-character input. See module docs for the construction.
pub fn compile_map(map: &PhonMapDef, alpha: &mut PhonruleAlphabet) -> RustFstWrapper {
    let PhonMapBody::Match { arms, else_arm } = &map.body;

    // Build the explicit-arm arcs. The first arm wins on a given input
    // (eval line 530 "if arm.from.node == ch ... return"). We achieve this
    // in the FST by emitting each arm's arc and deduplicating by input
    // symbol — only the first occurrence is retained, matching eval's
    // first-arm-wins semantics.
    let mut covered_inputs: HashSet<Label> = HashSet::new();
    let mut arm_arcs: Vec<(Label, Label)> = Vec::new();

    for arm in arms {
        let from_label = alpha.intern(&arm.from.node);
        if !covered_inputs.insert(from_label) {
            // Duplicate `from` in arm list — eval only ever fires the first
            // arm (line 530-535). We honour that by skipping subsequent
            // arcs on the same input.
            continue;
        }
        let to_label = match &arm.to {
            PhonMapResult::Literal(lit) => alpha.intern(&lit.node),
            // `Var(name)`: the `name` must be the map's parameter, which
            // means "identity on this input". Eval treats this as
            // returning `ch.to_string()` (line 533). The FST encodes the
            // same as input=output.
            PhonMapResult::Var(_) => from_label,
        };
        arm_arcs.push((from_label, to_label));
    }

    // Determine the else-arm output for inputs not covered by any arm.
    // Two cases produce identity arcs over the uncovered set:
    //   * `else_arm == Some(Var(_))` — eval line 540 returns `ch`.
    //   * `else_arm == None` — eval falls through to line 545 returning `ch`.
    // The remaining case is `else_arm == Some(Literal(lit))`, which maps
    // every uncovered input to the same literal output label.
    enum ElseBehaviour {
        Identity,
        ToLabel(Label),
    }
    let else_behaviour = match else_arm {
        None => ElseBehaviour::Identity,
        Some(PhonMapElse::Var(_)) => ElseBehaviour::Identity,
        Some(PhonMapElse::Literal(lit)) => ElseBehaviour::ToLabel(alpha.intern(&lit.node)),
    };

    // Snapshot Σ for the else-arc emission. The list is taken at the time
    // of call; see module docs on "Alphabet timing".
    let sigma_snapshot: Vec<Label> = alpha.sigma().collect();

    let mut b = RustFstBackend::builder();
    let s0 = b.add_state();
    let s1 = b.add_state();
    b.set_start(s0).expect("set_start");
    b.set_final(s1).expect("set_final");

    // Lay arm arcs first (declaration order, preserved).
    for (from, to) in &arm_arcs {
        b.add_arc(s0, *from, *to, s1).expect("add_arc arm");
    }
    // Lay else arcs over uncovered Σ.
    for sym in sigma_snapshot {
        if covered_inputs.contains(&sym) {
            continue;
        }
        let out = match else_behaviour {
            ElseBehaviour::Identity => sym,
            ElseBehaviour::ToLabel(l) => l,
        };
        b.add_arc(s0, sym, out, s1).expect("add_arc else");
    }
    b.finish().expect("finish")
}
