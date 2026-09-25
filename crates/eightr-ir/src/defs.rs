//! Reaching definitions: for every register read, which definitions may supply its value.

use crate::cfg::{BlockId, Cfg};
use crate::dataflow::{self, Forward};
use crate::lift::Body;
use crate::op::Reg;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DefSite {
    /// A method parameter (including `this`) in its incoming register.
    Param,
    /// Instruction index.
    Insn(u32),
}

/// One register written by one definition (a wide def produces two).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Def {
    pub site: DefSite,
    pub reg: Reg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BitSet(Vec<u64>);

impl BitSet {
    fn new(n: usize) -> Self {
        BitSet(vec![0; n.div_ceil(64)])
    }
    fn insert(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    fn remove(&mut self, i: usize) {
        self.0[i / 64] &= !(1 << (i % 64));
    }
    fn contains(&self, i: usize) -> bool {
        self.0[i / 64] & (1 << (i % 64)) != 0
    }
    fn union(&mut self, o: &BitSet) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            let n = *a | b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
}

/// A register read and the defs (indices into [`ReachingDefs::defs`]) that may reach it.
pub type UseDefs = (Reg, Vec<usize>);

pub struct ReachingDefs {
    pub defs: Vec<Def>,
    /// For each instruction, its register reads and their reaching defs. `None` for
    /// unreachable instructions.
    pub uses: Vec<Option<Vec<UseDefs>>>,
}

struct Analysis<'a> {
    body: &'a Body,
    defs: Vec<Def>,
    /// Register → indices of all defs of that register.
    by_reg: Vec<Vec<usize>>,
    /// Instruction → indices of the defs it generates.
    by_insn: Vec<Vec<usize>>,
    params: Vec<usize>,
}

impl Forward for Analysis<'_> {
    type State = BitSet;

    fn entry_state(&self) -> BitSet {
        let mut s = BitSet::new(self.defs.len());
        for &d in &self.params {
            s.insert(d);
        }
        s
    }

    fn join(&self, into: &mut Option<BitSet>, incoming: &BitSet) -> bool {
        match into {
            None => {
                *into = Some(incoming.clone());
                true
            }
            Some(cur) => cur.union(incoming),
        }
    }

    fn transfer(&self, _block: BlockId, idx: u32, s: &mut BitSet) {
        for &d in &self.by_insn[idx as usize] {
            for &k in &self.by_reg[self.defs[d].reg as usize] {
                s.remove(k);
            }
        }
        for &d in &self.by_insn[idx as usize] {
            s.insert(d);
        }
    }
}

impl ReachingDefs {
    pub fn compute(body: &Body, cfg: &Cfg) -> ReachingDefs {
        let nregs = body.registers as usize;
        let mut defs = Vec::new();
        let mut params = Vec::new();
        for r in body.registers.saturating_sub(body.ins)..body.registers {
            params.push(defs.len());
            defs.push(Def { site: DefSite::Param, reg: r });
        }
        let mut by_insn = vec![Vec::new(); body.insns.len()];
        for (i, insn) in body.insns.iter().enumerate() {
            if let Some((r, wide)) = insn.op.def() {
                for reg in [Some(r), wide.then(|| r.wrapping_add(1))].into_iter().flatten() {
                    if (reg as usize) < nregs {
                        by_insn[i].push(defs.len());
                        defs.push(Def { site: DefSite::Insn(i as u32), reg });
                    }
                }
            }
        }
        let mut by_reg = vec![Vec::new(); nregs];
        for (i, d) in defs.iter().enumerate() {
            by_reg[d.reg as usize].push(i);
        }
        let a = Analysis { body, defs, by_reg, by_insn, params };
        let entry = dataflow::solve(&a, cfg);
        let states = dataflow::per_instruction(&a, body, cfg, &entry);
        let uses = states
            .iter()
            .zip(&a.body.insns)
            .map(|(st, insn)| {
                let st = st.as_ref()?;
                Some(
                    insn.op
                        .uses()
                        .into_iter()
                        .map(|r| {
                            let reaching = a
                                .by_reg
                                .get(r as usize)
                                .map(|ds| ds.iter().copied().filter(|&d| st.contains(d)).collect())
                                .unwrap_or_default();
                            (r, reaching)
                        })
                        .collect(),
                )
            })
            .collect();
        ReachingDefs { defs: a.defs, uses }
    }
}
