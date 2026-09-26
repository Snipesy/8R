//! Editing method bodies: splicing instruction ranges while keeping everything that is keyed
//! by instruction index (branch targets, switch cases, try ranges, handlers, positions,
//! local-variable ranges) consistent.

use crate::lift::{Body, Insn};
use crate::op::Op;

/// Replaces instructions `[at, at + remove)` with `insert`.
///
/// Branch targets *inside* `insert` are local: `0..insert.len()` address the inserted
/// instructions, and `insert.len()` means "the instruction after the spliced region".
/// Existing references into the removed range are redirected to the start of the inserted
/// code (or to what follows, if nothing is inserted).
pub fn splice(body: &mut Body, at: u32, remove: u32, insert: Vec<Op>, pc: u32) {
    let n = insert.len() as u32;
    let end = at + remove;
    // Start-like index (a target, a range start): removed indices go to `at`.
    let f = |i: u32| -> u32 {
        if i < at {
            i
        } else if i < end {
            at
        } else {
            i - remove + n
        }
    };
    // Exclusive end of a range: an end inside the removed region covers all of `insert`.
    let g = |e: u32| -> u32 {
        if e <= at {
            e
        } else if e >= end {
            e - remove + n
        } else {
            at + n
        }
    };
    for insn in &mut body.insns {
        insn.op.map_targets(&mut |t| f(t));
    }
    // A range *starting* inside the removed region (after its first instruction) covered only
    // the tail of it, e.g. a try starting at a removed `move-result`: it starts after the
    // inserted code rather than growing over it.
    let range_start = |i: u32| -> u32 { if i > at && i < end { at + n } else { f(i) } };
    for t in &mut body.tries {
        t.start = range_start(t.start);
        t.end = g(t.end);
        for h in &mut t.handlers {
            h.target = f(h.target);
        }
    }
    body.tries.retain(|t| t.end > t.start);
    let mut positions = Vec::with_capacity(body.positions.len());
    for &(i, line) in &body.positions {
        // Interior of the removed range: the region keeps its first line (and nothing, if
        // nothing is inserted: the next instruction has its own position or inherits one).
        if i >= at && i < end && (i != at || n == 0) {
            continue;
        }
        positions.push((f(i), line));
    }
    // One position per instruction: a later entry for the same index wins, as when decoding.
    positions.dedup_by(|later, earlier| {
        let same = later.0 == earlier.0;
        if same {
            earlier.1 = later.1;
        }
        same
    });
    body.positions = positions;
    for l in &mut body.locals {
        l.start = range_start(l.start);
        l.end = g(l.end);
    }
    body.locals.retain(|l| l.end > l.start);
    let new: Vec<Insn> = insert
        .into_iter()
        .map(|mut op| {
            op.map_targets(&mut |t| at + t);
            Insn { pc, op }
        })
        .collect();
    body.insns.splice(at as usize..end as usize, new);
}

/// Deletes instructions no path reaches (e.g. the default `return` R8 left after a call it
/// knew always throws). Returns how many were removed. Leaves the body unchanged if its CFG
/// can't be built.
pub fn remove_unreachable(body: &mut Body) -> usize {
    let Ok(cfg) = crate::cfg::Cfg::build(body) else { return 0 };
    let mut dead: Vec<(u32, u32)> = cfg
        .blocks
        .iter()
        .enumerate()
        .filter(|(b, _)| !cfg.is_reachable(*b as u32))
        .map(|(_, blk)| (blk.start, blk.end))
        .collect();
    // Never delete everything, and keep at least one instruction.
    if dead.iter().map(|(s, e)| (e - s) as usize).sum::<usize>() >= body.insns.len() {
        return 0;
    }
    // A handler in a dead block catches nothing (no covered instruction can reach it): drop it,
    // and any try left without handlers, before its code goes.
    let is_dead = |i: u32| dead.iter().any(|&(s, e)| s <= i && i < e);
    for t in &mut body.tries {
        t.handlers.retain(|h| !is_dead(h.target));
    }
    body.tries.retain(|t| !t.handlers.is_empty());
    dead.sort_by_key(|d| std::cmp::Reverse(d.0));
    let mut removed = 0;
    for (start, end) in dead {
        splice(body, start, end - start, Vec::new(), 0);
        removed += (end - start) as usize;
    }
    removed
}

/// Recomputes `outs`: the most argument words any call in the body passes.
pub fn recompute_outs(body: &mut Body) {
    let outs = body
        .insns
        .iter()
        .map(|i| match &i.op {
            Op::Invoke { args, .. } | Op::InvokePolymorphic { args, .. } | Op::InvokeCustom { args, .. }
            | Op::FilledNewArray { args, .. } => args.len(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    body.outs = outs as u16;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lift::{Handler, Local, TryRange};
    use crate::op::{Cond, Const};

    fn body(ops: Vec<Op>) -> Body {
        Body {
            registers: 4,
            ins: 0,
            outs: 0,
            insns: ops.into_iter().enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(),
            tries: Vec::new(),
            positions: Vec::new(),
            locals: Vec::new(),
            parameter_names: Vec::new(),
        }
    }

    #[test]
    fn splice_renumbers_targets_ranges_and_debug_info() {
        // 0: if-eqz v0 -> 4 ; 1: const ; 2: nop (replaced) ; 3: const ; 4: return-void
        let mut b = body(vec![
            Op::IfZ { cond: Cond::Eq, a: 0, target: 4 },
            Op::Const { dst: 1, value: Const::Narrow(1) },
            Op::Nop,
            Op::Const { dst: 2, value: Const::Narrow(2) },
            Op::ReturnVoid,
        ]);
        b.tries.push(TryRange { start: 1, end: 4, handlers: vec![Handler { ty: None, target: 4 }] });
        b.positions = vec![(0, 10), (2, 11), (4, 12)];
        b.locals.push(Local { reg: 1, name: None, ty: None, signature: None, start: 2, end: 5 });
        // Replace instruction 2 with three instructions, the second jumping past the region.
        splice(&mut b, 2, 1, vec![Op::Nop, Op::Goto { target: 3 }, Op::Nop], 2);
        assert_eq!(b.insns.len(), 7);
        assert_eq!(b.insns[0].op, Op::IfZ { cond: Cond::Eq, a: 0, target: 6 });
        assert_eq!(b.insns[3].op, Op::Goto { target: 5 }); // local 3 = after region = old 3 -> 5
        assert_eq!((b.tries[0].start, b.tries[0].end, b.tries[0].handlers[0].target), (1, 6, 6));
        assert_eq!(b.positions, vec![(0, 10), (2, 11), (6, 12)]);
        assert_eq!((b.locals[0].start, b.locals[0].end), (2, 7));
    }

    #[test]
    fn removes_code_after_throw() {
        let mut b = body(vec![
            Op::Const { dst: 0, value: Const::Narrow(0) },
            Op::Throw { src: 0 },
            Op::Const { dst: 1, value: Const::Narrow(0) },
            Op::Return { width: crate::op::Width::Single, src: 1 },
        ]);
        assert_eq!(remove_unreachable(&mut b), 2);
        assert_eq!(b.insns.len(), 2);
    }

    #[test]
    fn deleting_keeps_one_position_per_instruction() {
        let mut b = body(vec![Op::Nop, Op::Nop, Op::ReturnVoid]);
        b.positions = vec![(0, 1), (1, 2), (2, 3)];
        splice(&mut b, 1, 1, vec![], 0);
        assert_eq!(b.positions, vec![(0, 1), (1, 3)]);
    }

    #[test]
    fn try_starting_at_removed_move_result_does_not_grow() {
        // 0: nop ; 1: nop (replaced) ; 2: nop (removed, like a move-result) ; 3: return-void
        let mut b = body(vec![Op::Nop, Op::Nop, Op::Nop, Op::ReturnVoid]);
        b.tries.push(TryRange { start: 2, end: 4, handlers: vec![Handler { ty: None, target: 3 }] });
        splice(&mut b, 1, 2, vec![Op::Nop, Op::Nop, Op::Nop], 1);
        assert_eq!((b.tries[0].start, b.tries[0].end), (4, 5));
    }

    #[test]
    fn dead_handler_and_its_try_are_dropped() {
        // No instruction in the try can throw, so the handler block is dead.
        let mut b = body(vec![Op::Nop, Op::ReturnVoid, Op::MoveException { dst: 0 }, Op::ReturnVoid]);
        b.tries.push(TryRange { start: 0, end: 1, handlers: vec![Handler { ty: None, target: 2 }] });
        assert_eq!(remove_unreachable(&mut b), 2);
        assert!(b.tries.is_empty());
        assert_eq!(b.insns.len(), 2);
    }

    #[test]
    fn splice_delete_redirects_to_following_instruction() {
        let mut b = body(vec![Op::Goto { target: 1 }, Op::Nop, Op::ReturnVoid]);
        splice(&mut b, 1, 1, vec![], 0);
        assert_eq!(b.insns.len(), 2);
        assert_eq!(b.insns[0].op, Op::Goto { target: 1 });
    }
}
