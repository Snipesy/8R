//! Register liveness (backward dataflow over the [`Cfg`]).
//!
//! An exceptional edge leaves from a block's last instruction and carries the state *before*
//! it (see [`crate::cfg`]), so a handler's live-in registers are live before that instruction.

use crate::cfg::Cfg;
use crate::lift::Body;
use crate::op::Reg;

/// A set of registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regs(Vec<u64>);

impl Regs {
    pub fn new(registers: u16) -> Regs {
        Regs(vec![0; usize::from(registers).div_ceil(64).max(1)])
    }

    pub fn contains(&self, r: Reg) -> bool {
        self.0.get(usize::from(r) / 64).is_some_and(|w| w >> (r % 64) & 1 == 1)
    }

    pub fn insert(&mut self, r: Reg) {
        if let Some(w) = self.0.get_mut(usize::from(r) / 64) {
            *w |= 1 << (r % 64);
        }
    }

    pub fn remove(&mut self, r: Reg) {
        if let Some(w) = self.0.get_mut(usize::from(r) / 64) {
            *w &= !(1 << (r % 64));
        }
    }

    /// `self |= other`; true if anything changed.
    pub fn union(&mut self, other: &Regs) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            let n = *a | b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
}

pub struct Liveness {
    /// Registers live on entry to each block.
    live_in: Vec<Regs>,
}

/// Applies instruction `i` backward to `live` (the state after it).
fn step(body: &Body, i: u32, live: &mut Regs) {
    let op = &body.insns[i as usize].op;
    if let Some((r, wide)) = op.def() {
        live.remove(r);
        if wide {
            live.remove(r + 1);
        }
    }
    for r in op.uses() {
        live.insert(r);
    }
}

impl Liveness {
    pub fn compute(body: &Body, cfg: &Cfg) -> Liveness {
        let n = cfg.blocks.len();
        let mut live_in: Vec<Regs> = vec![Regs::new(body.registers); n];
        let mut changed = true;
        while changed {
            changed = false;
            for &b in cfg.rpo.iter().rev() {
                let state = Self::block_in(body, cfg, &live_in, b);
                changed |= live_in[b as usize].union(&state);
            }
        }
        Liveness { live_in }
    }

    fn block_in(body: &Body, cfg: &Cfg, live_in: &[Regs], b: u32) -> Regs {
        let blk = &cfg.blocks[b as usize];
        let mut live = Regs::new(body.registers);
        let mut exc = Regs::new(body.registers);
        for e in &blk.succs {
            if e.is_exceptional() {
                exc.union(&live_in[e.to as usize]);
            } else {
                live.union(&live_in[e.to as usize]);
            }
        }
        for i in (blk.start..blk.end).rev() {
            step(body, i, &mut live);
            if i == blk.last() {
                live.union(&exc);
            }
        }
        live
    }

    /// Registers whose values instruction `i` must not disturb beyond its own def: those live
    /// after it on the normal path, plus those its exception handlers read.
    pub fn across(&self, body: &Body, cfg: &Cfg, i: u32) -> Regs {
        let b = cfg.block_of[i as usize];
        let blk = &cfg.blocks[b as usize];
        let mut live = Regs::new(body.registers);
        let mut exc = Regs::new(body.registers);
        for e in &blk.succs {
            if e.is_exceptional() {
                exc.union(&self.live_in[e.to as usize]);
            } else {
                live.union(&self.live_in[e.to as usize]);
            }
        }
        for k in (i + 1..blk.end).rev() {
            step(body, k, &mut live);
            if k == blk.last() {
                live.union(&exc);
            }
        }
        if i == blk.last() {
            live.union(&exc);
        }
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lift::Insn;
    use crate::op::{Const, Op, Width};

    #[test]
    fn value_dies_at_last_use() {
        let ops = vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::Const { dst: 1, value: Const::Narrow(2) },
            Op::Move { width: Width::Single, dst: 2, src: 0 },
            Op::Return { width: Width::Single, src: 1 },
        ];
        let body = Body {
            registers: 3,
            ins: 0,
            outs: 0,
            insns: ops.into_iter().enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(),
            tries: Vec::new(),
            positions: Vec::new(),
            locals: Vec::new(),
            parameter_names: Vec::new(),
        };
        let cfg = Cfg::build(&body).unwrap();
        let l = Liveness::compute(&body, &cfg);
        let after_move = l.across(&body, &cfg, 2);
        assert!(!after_move.contains(0) && after_move.contains(1) && !after_move.contains(2));
    }
}
