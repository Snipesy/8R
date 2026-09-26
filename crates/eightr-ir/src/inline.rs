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
    let Op::Invoke { kind: InvokeKind::Static, args, .. } = &caller.insns[call_at as usize].op else {
        return Err(Refusal::NotAStaticCall);
    };
    if !callee.tries.is_empty() {
        return Err(Refusal::CalleeHasTries);
    }
    let args = args.clone();
    let (params, ret) = parse_proto(callee_proto).ok_or(Refusal::NotAStaticCall)?;
    let result = match caller.insns.get(call_at as usize + 1).map(|i| &i.op) {
        Some(Op::MoveResult { width, dst }) if ret != "V" => Some((*width, *dst)),
        _ => None,
    };

    let (big_r, big_i) = (caller.registers, caller.ins);
    let (r, i) = (callee.registers, callee.ins);
    let base = big_r - big_i; // first caller parameter register
    let new_total = u32::from(big_r) + u32::from(r);
    if new_total > u32::from(u16::MAX) {
        return Err(Refusal::TooManyRegisters);
    }
    // The callee's registers go between the caller's locals and its parameters, so caller
    // parameters move up by `r`. A wide pair (base-1, base) would be split; refuse.
    if base > 0 && r > 0 {
        let straddles = caller.insns.iter().any(|insn| {
            let wide_def = insn.op.def().is_some_and(|(reg, wide)| wide && reg == base - 1);
            let uses = insn.op.uses();
            let wide_use = uses.windows(2).any(|w| w[0] == base - 1 && w[1] == base);
            wide_def || wide_use
        });
        if straddles {
            return Err(Refusal::WidePairStraddles);
        }
    }

    let mut edited = caller.clone();
    let shift_caller = |reg: Reg| if reg >= base { reg + r } else { reg };
    for insn in &mut edited.insns {
        insn.op.map_regs(&mut |reg| shift_caller(reg));
    }
    for l in &mut edited.locals {
        l.reg = shift_caller(l.reg);
    }
    let map_callee = |reg: Reg| base + reg;

    // Argument words → callee parameter registers (the callee's last `i` registers).
    let mut insert: Vec<Op> = Vec::new();
    let mut word = 0usize;
    for p in &params {
        let w = width_of(p);
        let src = shift_caller(*args.get(word).ok_or(Refusal::NotAStaticCall)?);
        let dst = map_callee(r - i + word as u16);
        insert.push(Op::Move { width: w, dst, src });
        word += if w == Width::Wide { 2 } else { 1 };
    }

    // Callee body: registers mapped; returns become (move to result) + jump past the region.
    // First pass: local index of every callee instruction in `insert`.
    let prologue = insert.len() as u32;
    let mut local_of = Vec::with_capacity(callee.insns.len());
    let mut next = prologue;
    for (k, insn) in callee.insns.iter().enumerate() {
        local_of.push(next);
        let last = k + 1 == callee.insns.len();
        next += match &insn.op {
            Op::Return { .. } => u32::from(result.is_some()) + u32::from(!last),
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
                if let Some((_, dst)) = result {
                    insert.push(Op::Move { width, dst: shift_caller(dst), src });
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
    debug_assert_eq!(insert.len() as u32, region_end);
    // An empty region would make `goto region_end` a self-loop; never happens (a callee has
    // at least a return), but keep the region non-empty.
    if insert.is_empty() {
        insert.push(Op::Nop);
    }

    let remove = 1 + u32::from(result.is_some());
    let pc = edited.insns[call_at as usize].pc;
    splice(&mut edited, call_at, remove, insert, pc);
    edited.registers = new_total as u16;
    recompute_outs(&mut edited);
    edited.outs = edited.outs.max(callee.outs);
    if !edited.insns.iter().all(|x| x.op.encodable()) {
        return Err(Refusal::NotEncodable);
    }
    *caller = edited;
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
    fn refuses_straddling_wide_pair_and_keeps_caller() {
        let mut s = Interner::default();
        let m = MethodRef { class: s.intern("LO;"), name: s.intern("m"), proto: s.intern("()V") };
        let callee = body(1, 0, vec![Op::ReturnVoid]);
        // caller: 2 registers, 1 param (v1); a wide const into v0/v1 straddles the gap.
        let mut caller = body(2, 1, vec![
            Op::Const { dst: 0, value: Const::Wide(5) },
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![] },
            Op::ReturnVoid,
        ]);
        let before = caller.clone();
        assert_eq!(inline_static_call(&mut caller, 1, &callee, "()V"), Err(Refusal::WidePairStraddles));
        assert_eq!(caller, before);
    }
}
