//! Inlining a static callee's body at a call site (used to undo R8 outlining). The edit is
//! all-or-nothing: on any refusal the caller is left untouched.

use crate::edit::{recompute_outs, splice};
use crate::lift::Body;
use crate::op::{InvokeKind, Op, Reg, Width};
use crate::types::parse_proto;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotAStaticCall,
    CalleeHasTries,
    /// A wide value in the caller uses the register pair that would straddle the gap opened
    /// for the callee's registers.
    WidePairStraddles,
    TooManyRegisters,
    /// Some instruction can't be encoded after registers moved.
    NotEncodable,
}

fn width_of(desc: &str) -> Width {
    match desc.as_bytes().first() {
        Some(b'J' | b'D') => Width::Wide,
        Some(b'L' | b'[') => Width::Object,
        _ => Width::Single,
    }
}

/// Inlines `callee` (a static method with descriptor `callee_proto`) into `caller` at
/// instruction `call_at`, which must be an `invoke-static` of it. A following `move-result`
/// receives the callee's return value.
pub fn inline_static_call(caller: &mut Body, call_at: u32, callee: &Body, callee_proto: &str) -> Result<(), Refusal> {
    inline_static_calls(caller, &[(call_at, callee, callee_proto)]).remove(0)
}

/// Inlines several static call sites of one caller; returns each site's outcome, in the order
/// given. Sites are spliced from the highest index down (so the result doesn't depend on the
/// order given, and a refused site leaves the others' indices valid).
///
/// Each site first tries to run the callee in caller registers that are dead across the call
/// ([`in_place`]): the frame doesn't grow and no caller instruction changes. Otherwise it opens
/// a gap of fresh registers ([`with_gap`]); later sites can then reuse that gap in place.
pub fn inline_static_calls(caller: &mut Body, sites: &[(u32, &Body, &str)]) -> Vec<Result<(), Refusal>> {
    let mut order: Vec<usize> = (0..sites.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(sites[k].0));
    let mut results = vec![Ok(()); sites.len()];
    // Liveness of the body as it was before any splice. Splicing in place at a higher index
    // doesn't change liveness at lower ones (inlined code writes a borrowed register before
    // reading it), so it stays valid until a gap renumbers registers.
    let mut analysis: Option<(Body, crate::cfg::Cfg, crate::liveness::Liveness)> = None;
    for k in order {
        let (call_at, callee, proto) = sites[k];
        if analysis.is_none() {
            analysis = crate::cfg::Cfg::build(caller).ok().map(|cfg| {
                let live = crate::liveness::Liveness::compute(caller, &cfg);
                (caller.clone(), cfg, live)
            });
        }
        let placed = match &analysis {
            Some((orig, cfg, live)) => in_place(caller, call_at, callee, proto, &live.across(orig, cfg, call_at)),
            None => Err(Refusal::TooManyRegisters),
        };
        results[k] = match placed {
            Ok(edited) => {
                *caller = edited;
                Ok(())
            }
            Err(_) => with_gap(caller, call_at, callee, proto).map(|edited| {
                *caller = edited;
                analysis = None;
            }),
        };
    }
    recompute_outs(caller);
    results
}

fn check_site(body: &Body, call_at: u32, callee: &Body) -> Result<Vec<Reg>, Refusal> {
    let Some(Op::Invoke { kind: InvokeKind::Static, args, .. }) = body.insns.get(call_at as usize).map(|i| &i.op) else {
        return Err(Refusal::NotAStaticCall);
    };
    if !callee.tries.is_empty() {
        return Err(Refusal::CalleeHasTries);
    }
    Ok(args.clone())
}

/// Callee registers that hold wide values (the first register of each pair).
fn wide_starts(callee: &Body, params: &[&str]) -> Vec<Reg> {
    let mut v = Vec::new();
    for insn in &callee.insns {
        if let Some((r, true)) = insn.op.def() {
            v.push(r);
        }
        let uses = insn.op.uses();
        v.extend(uses.windows(2).filter(|w| w[1] == w[0].wrapping_add(1)).map(|w| w[0]));
    }
    let mut word = callee.registers - callee.ins;
    for p in params {
        if width_of(p) == Width::Wide {
            v.push(word);
            word += 2;
        } else {
            word += 1;
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Runs the callee in registers that are dead across the call. Parameter and debug-local
/// registers are never borrowed (debuggers read them). An argument register that dies at the
/// call becomes the callee's parameter register directly.
/// `live`: registers live across the call.
fn in_place(body: &Body, call_at: u32, callee: &Body, proto: &str, live: &crate::liveness::Regs) -> Result<Body, Refusal> {
    let args = check_site(body, call_at, callee)?;
    let (params, _) = parse_proto(proto).ok_or(Refusal::NotAStaticCall)?;
    let base = body.registers - body.ins;
    let mut busy = live.clone();
    for l in body.locals.iter().filter(|l| l.start <= call_at && call_at < l.end) {
        busy.insert(l.reg);
    }
    let arg_count = |r: Reg| args.iter().filter(|&&a| a == r).count();
    let (r, i) = (callee.registers, callee.ins);
    let mut map: Vec<Option<Reg>> = vec![None; usize::from(r)];
    // Parameters: alias argument registers that die here (and appear only once).
    let mut word = 0usize;
    for p in &params {
        let n = if width_of(p) == Width::Wide { 2 } else { 1 };
        let srcs = &args[word..(word + n).min(args.len())];
        if srcs.len() == n && srcs.iter().all(|&a| a < base && !busy.contains(a) && arg_count(a) == 1) {
            for (j, &a) in srcs.iter().enumerate() {
                map[usize::from(r - i) + word + j] = Some(a);
            }
        }
        word += n;
    }
    let mut taken = busy.clone();
    for &a in &args {
        taken.insert(a); // other argument registers are read by the parameter moves
    }
    // Registers linked into wide pairs must stay adjacent; overlapping pairs (a callee reusing
    // (q-1, q) and (q, q+1)) chain into one block that needs contiguous registers.
    let wide = wide_starts(callee, &params);
    let linked = |q: Reg| wide.binary_search(&q).is_ok() && q + 1 < r;
    let mut q: Reg = 0;
    while q < r {
        let start = q;
        while linked(q) {
            q += 1;
        }
        let block = start..=q; // q: last register of the block
        q += 1;
        let len = *block.end() - start + 1;
        let mut anchor = block.clone().find(|&x| map[usize::from(x)].is_some());
        if let Some(x) = anchor {
            // Aliased parameters must line up with the rest of the block; otherwise they
            // get moves like any other parameter.
            let at = map[usize::from(x)].unwrap().checked_sub(x - start);
            let fits = at.is_some_and(|at| {
                at + len <= base
                    && block.clone().all(|y| match map[usize::from(y)] {
                        Some(m) => m == at + (y - start),
                        None => !taken.contains(at + (y - start)),
                    })
            });
            if !fits {
                for y in block.clone() {
                    map[usize::from(y)] = None;
                }
                anchor = None;
            }
        }
        let d = match anchor {
            // Part of the block is an aliased parameter: place the rest around it.
            Some(x) => map[usize::from(x)].unwrap() - (x - start),
            None => (0..base.saturating_sub(len - 1))
                .find(|&d| (d..d + len).all(|x| !taken.contains(x)))
                .ok_or(Refusal::TooManyRegisters)?,
        };
        for y in block {
            map[usize::from(y)] = Some(d + (y - start));
            taken.insert(d + (y - start));
        }
    }
    let map: Vec<Reg> = map.into_iter().map(|m| m.expect("every register mapped")).collect();
    let mut edited = body.clone();
    splice_one(&mut edited, call_at, callee, proto, &|q| map[usize::from(q)])?;
    if !edited.insns.iter().all(|x| x.op.encodable()) {
        return Err(Refusal::NotEncodable);
    }
    Ok(edited)
}

/// Opens a gap of fresh registers for the callee at position `g` (callee register `q` maps to
/// `g + q`), shifting caller registers `>= g` up. Tries the gap below the parameters first,
/// then the highest position that splits no wide pair, then the bottom.
fn with_gap(body: &Body, call_at: u32, callee: &Body, proto: &str) -> Result<Body, Refusal> {
    check_site(body, call_at, callee)?;
    let scratch = callee.registers;
    let new_total = u32::from(body.registers) + u32::from(scratch);
    if new_total > u32::from(u16::MAX) {
        return Err(Refusal::TooManyRegisters);
    }
    let base = body.registers - body.ins;
    // Gap positions that would split a wide pair (g - 1, g).
    let mut split = std::collections::BTreeSet::new();
    for insn in &body.insns {
        if let Some((reg, true)) = insn.op.def() {
            split.insert(reg + 1);
        }
        split.extend(insn.op.uses().windows(2).filter(|w| w[1] == w[0].wrapping_add(1)).map(|w| w[1]));
    }
    let straddles = |g: Reg| g > 0 && split.contains(&g);
    let mut candidates = vec![base];
    if let Some(g) = (0..base).rev().find(|&g| !straddles(g)) {
        candidates.push(g);
    }
    candidates.push(0);
    candidates.dedup();
    let mut last = Refusal::WidePairStraddles;
    for g in candidates {
        if straddles(g) {
            continue;
        }
        let mut edited = body.clone();
        let shift = |reg: Reg| if reg >= g { reg + scratch } else { reg };
        for insn in &mut edited.insns {
            insn.op.map_regs(&mut |reg| shift(reg));
        }
        for l in &mut edited.locals {
            l.reg = shift(l.reg);
        }
        splice_one(&mut edited, call_at, callee, proto, &|q| g + q)?;
        edited.registers = new_total as u16;
        if edited.insns.iter().all(|x| x.op.encodable()) {
            return Ok(edited);
        }
        last = Refusal::NotEncodable;
    }
    Err(last)
}

/// Splices one callee at `call_at`; callee register `q` maps to `map_callee(q)`.
fn splice_one(edited: &mut Body, call_at: u32, callee: &Body, callee_proto: &str, map_callee: &dyn Fn(Reg) -> Reg) -> Result<(), Refusal> {
    let args = check_site(edited, call_at, callee)?;
    let (params, ret) = parse_proto(callee_proto).ok_or(Refusal::NotAStaticCall)?;
    let result = match edited.insns.get(call_at as usize + 1).map(|i| &i.op) {
        Some(Op::MoveResult { width, dst }) if ret != "V" => Some((*width, *dst)),
        _ => None,
    };
    let (r, i) = (callee.registers, callee.ins);

    // Argument words → callee parameter registers (the callee's last `i` registers).
    let mut insert: Vec<Op> = Vec::new();
    let mut word = 0usize;
    for p in &params {
        let w = width_of(p);
        let src = *args.get(word).ok_or(Refusal::NotAStaticCall)?;
        let dst = map_callee(r - i + word as u16);
        if dst != src {
            insert.push(Op::Move { width: w, dst, src });
        }
        word += if w == Width::Wide { 2 } else { 1 };
    }

    // Local index of every callee instruction in `insert`.
    let prologue = insert.len() as u32;
    let mut local_of = Vec::with_capacity(callee.insns.len());
    let mut next = prologue;
    for (k, insn) in callee.insns.iter().enumerate() {
        local_of.push(next);
        let last = k + 1 == callee.insns.len();
        next += match &insn.op {
            Op::Return { src, .. } => u32::from(result.is_some_and(|(_, dst)| dst != map_callee(*src))) + u32::from(!last),
            Op::ReturnVoid => u32::from(!last),
            _ => 1,
        };
    }
    let region_end = next; // local index meaning "after the spliced region"
    for (k, insn) in callee.insns.iter().enumerate() {
        let last = k + 1 == callee.insns.len();
        let mut op = insn.op.clone();
        op.map_regs(&mut |reg| map_callee(reg));
        op.map_targets(&mut |t| local_of[t as usize]);
        match op {
            Op::Return { width, src } => {
                if let Some((_, dst)) = result.filter(|&(_, dst)| dst != src) {
                    insert.push(Op::Move { width, dst, src });
                }
                if !last {
                    insert.push(Op::Goto { target: region_end });
                }
            }
            Op::ReturnVoid => {
                if !last {
                    insert.push(Op::Goto { target: region_end });
                }
            }
            other => insert.push(other),
        }
    }
    if insert.is_empty() {
        insert.push(Op::Nop);
    }
    let remove = 1 + u32::from(result.is_some());
    let pc = edited.insns[call_at as usize].pc;
    splice(edited, call_at, remove, insert, pc);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lift::Insn;
    use crate::op::{BinOp, Const, MethodRef, NumType, Operand};
    use crate::sym::Interner;

    fn body(registers: u16, ins: u16, ops: Vec<Op>) -> Body {
        Body {
            registers,
            ins,
            outs: 0,
            insns: ops.into_iter().enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(),
            tries: Vec::new(),
            positions: Vec::new(),
            locals: Vec::new(),
            parameter_names: Vec::new(),
        }
    }

    #[test]
    fn inlines_add_with_result_and_shifts_caller_params() {
        let mut s = Interner::default();
        let m = MethodRef { class: s.intern("LO;"), name: s.intern("m"), proto: s.intern("(II)I") };
        // callee: v0 = p0 + p1 (p0 = v1, p1 = v2); return v0
        let callee = body(3, 2, vec![
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 1, b: Operand::Reg(2) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        // caller (one param in v2): v0 = 1; v1 = p0; v0 = m(v0, v1); return v0
        let mut caller = body(3, 1, vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::Move { width: Width::Single, dst: 1, src: 2 },
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![0, 1] },
            Op::MoveResult { width: Width::Single, dst: 0 },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        inline_static_call(&mut caller, 2, &callee, "(II)I").unwrap();
        // Callee regs occupy v2..v4; the caller's parameter moved from v2 to v5.
        assert_eq!(caller.registers, 6);
        assert_eq!(caller.ins, 1);
        let ops: Vec<&Op> = caller.insns.iter().map(|i| &i.op).collect();
        assert_eq!(ops, vec![
            &Op::Const { dst: 0, value: Const::Narrow(1) },
            &Op::Move { width: Width::Single, dst: 1, src: 5 },
            &Op::Move { width: Width::Single, dst: 3, src: 0 },
            &Op::Move { width: Width::Single, dst: 4, src: 1 },
            &Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 2, a: 3, b: Operand::Reg(4) },
            &Op::Move { width: Width::Single, dst: 0, src: 2 },
            &Op::Return { width: Width::Single, src: 0 },
        ]);
        assert_eq!(caller.outs, 0);
    }

    #[test]
    fn gap_moves_below_a_wide_pair_it_would_split() {
        let mut s = Interner::default();
        let m = MethodRef { class: s.intern("LO;"), name: s.intern("m"), proto: s.intern("()V") };
        let callee = body(1, 0, vec![Op::Const { dst: 0, value: Const::Narrow(7) }, Op::ReturnVoid]);
        // caller: 2 registers, 1 param (v1); the wide pair v0/v1 is live across the call, so
        // there is no dead register and the gap at v1 would split the pair: it goes to v0.
        let mut caller = body(2, 1, vec![
            Op::Const { dst: 0, value: Const::Wide(5) },
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![] },
            Op::Return { width: Width::Wide, src: 0 },
        ]);
        inline_static_call(&mut caller, 1, &callee, "()V").unwrap();
        assert_eq!(caller.registers, 3);
        let ops: Vec<&Op> = caller.insns.iter().map(|i| &i.op).collect();
        assert_eq!(ops, vec![
            &Op::Const { dst: 1, value: Const::Wide(5) },
            &Op::Const { dst: 0, value: Const::Narrow(7) },
            &Op::Return { width: Width::Wide, src: 1 },
        ]);
    }

    #[test]
    fn runs_in_dead_registers_and_aliases_dying_arguments() {
        let mut s = Interner::default();
        let m = MethodRef { class: s.intern("LO;"), name: s.intern("m"), proto: s.intern("(II)I") };
        // callee: v0 = p0 + p1; return v0
        let callee = body(3, 2, vec![
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 1, b: Operand::Reg(2) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        // caller: v3 is live across the call; v0/v1 die at it; v2 is free.
        let mut caller = body(5, 1, vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::Const { dst: 1, value: Const::Narrow(2) },
            Op::Const { dst: 3, value: Const::Narrow(3) },
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![0, 1] },
            Op::MoveResult { width: Width::Single, dst: 0 },
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(3) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        inline_static_call(&mut caller, 3, &callee, "(II)I").unwrap();
        assert_eq!(caller.registers, 5);
        let ops: Vec<&Op> = caller.insns.iter().map(|i| &i.op).collect();
        assert_eq!(ops[3..], [
            &Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 2, a: 0, b: Operand::Reg(1) },
            &Op::Move { width: Width::Single, dst: 0, src: 2 },
            &Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(3) },
            &Op::Return { width: Width::Single, src: 0 },
        ]);
    }

    #[test]
    fn overlapping_wide_pairs_stay_contiguous() {
        let mut s = Interner::default();
        let m = MethodRef { class: s.intern("LO;"), name: s.intern("m"), proto: s.intern("(J)V") };
        let sink = MethodRef { class: s.intern("LO;"), name: s.intern("sink"), proto: s.intern("(J)V") };
        // callee (4 regs, long param in v2/v3): uses pairs (v1,v2) and (v2,v3).
        let callee = body(4, 2, vec![
            Op::Move { width: Width::Wide, dst: 0, src: 2 },
            Op::Move { width: Width::Wide, dst: 1, src: 2 },
            Op::Invoke { kind: InvokeKind::Static, method: sink, args: vec![1, 2] },
            Op::ReturnVoid,
        ]);
        // caller: v0 live across; v1, v2 dead; v3, v4, v5 dead; the long argument (v6/v7) is a
        // parameter, so it can't be aliased.
        let mut caller = body(8, 2, vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![6, 7] },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        inline_static_call(&mut caller, 1, &callee, "(J)V").unwrap();
        let invoke = caller.insns.iter().find_map(|i| match &i.op {
            Op::Invoke { args, .. } => Some(args.clone()),
            _ => None,
        });
        let args = invoke.unwrap();
        assert_eq!(args[1], args[0] + 1, "{:?}", caller.insns);
        // Block v0..v3 of the callee needs four contiguous dead registers: v1..v4.
        assert_eq!(caller.registers, 8);
    }
}
