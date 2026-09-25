//! Control-flow graph over a [`Body`].
//!
//! Blocks are split after every instruction that can throw inside a try range, so an
//! exceptional edge always leaves from a block's *last* instruction. The state flowing along
//! it is the state *before* that instruction executes.

use std::collections::BTreeSet;

use eightr_dex::{DexError, ErrorKind, Result};

use crate::lift::Body;
use crate::op::Op;
use crate::sym::Sym;

pub type BlockId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Fallthrough,
    Jump,
    /// Conditional branch taken.
    Taken,
    Case(i32),
    /// Exceptional edge; `None` = catch-all.
    Catch(Option<Sym>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub to: BlockId,
    pub kind: EdgeKind,
}

impl Edge {
    pub fn is_exceptional(&self) -> bool {
        matches!(self.kind, EdgeKind::Catch(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Instruction index range `[start, end)`.
    pub start: u32,
    pub end: u32,
    /// Normal successors first (fallthrough, then branch/case targets in op order), then
    /// exceptional successors in handler order.
    pub succs: Vec<Edge>,
    /// Sorted, deduplicated.
    pub preds: Vec<BlockId>,
}

impl Block {
    pub fn last(&self) -> u32 {
        self.end - 1
    }
}

#[derive(Debug, Clone)]
pub struct Cfg {
    pub blocks: Vec<Block>,
    /// Instruction index → block.
    pub block_of: Vec<BlockId>,
    /// Reachable blocks in reverse postorder (entry first).
    pub rpo: Vec<BlockId>,
    /// Block → position in `rpo`, or `u32::MAX` if unreachable.
    pub rpo_index: Vec<u32>,
}

impl Cfg {
    pub fn build(body: &Body) -> Result<Cfg> {
        let n = body.insns.len() as u32;
        let err = |idx: u32, what| DexError::new(body.insns.get(idx as usize).map_or(0, |i| i.pc as usize), ErrorKind::Malformed(what));
        if n == 0 {
            return Err(err(0, "empty code"));
        }
        let covered = |idx: u32| body.try_covering(idx).is_some();

        let mut leaders: BTreeSet<u32> = BTreeSet::from([0]);
        for (i, insn) in body.insns.iter().enumerate() {
            let i = i as u32;
            leaders.extend(insn.op.targets());
            if insn.op.is_branch() || insn.op.ends_flow() || (insn.op.can_throw() && covered(i)) {
                leaders.insert(i + 1);
            }
        }
        for t in &body.tries {
            leaders.insert(t.start);
            leaders.insert(t.end);
            leaders.extend(t.handlers.iter().map(|h| h.target));
        }
        let starts: Vec<u32> = leaders.into_iter().filter(|&l| l < n).collect();

        let mut block_of = vec![0; n as usize];
        let mut blocks: Vec<Block> = Vec::with_capacity(starts.len());
        for (b, &start) in starts.iter().enumerate() {
            let end = starts.get(b + 1).copied().unwrap_or(n);
            for i in start..end {
                block_of[i as usize] = b as BlockId;
            }
            blocks.push(Block { start, end, succs: Vec::new(), preds: Vec::new() });
        }

        let mut falls_off = Vec::new();
        #[allow(clippy::needless_range_loop)] // blocks[b] is read, then written after other blocks are read
        for b in 0..blocks.len() {
            let last = blocks[b].last();
            let op = &body.insns[last as usize].op;
            let mut succs = Vec::new();
            if !op.ends_flow() {
                if last + 1 < n {
                    succs.push(Edge { to: block_of[last as usize + 1], kind: EdgeKind::Fallthrough });
                } else {
                    // Legal only if unreachable (e.g. alignment `nop`s before payloads);
                    // checked once reachability is known.
                    falls_off.push(b);
                }
            }
            match op {
                Op::Goto { target } => succs.push(Edge { to: block_of[*target as usize], kind: EdgeKind::Jump }),
                Op::If { target, .. } | Op::IfZ { target, .. } => {
                    succs.push(Edge { to: block_of[*target as usize], kind: EdgeKind::Taken })
                }
                Op::Switch { cases, .. } => {
                    for &(key, t) in cases {
                        succs.push(Edge { to: block_of[t as usize], kind: EdgeKind::Case(key) });
                    }
                }
                _ => {}
            }
            if op.can_throw() {
                if let Some(t) = body.try_covering(last) {
                    for h in &t.handlers {
                        succs.push(Edge { to: block_of[h.target as usize], kind: EdgeKind::Catch(h.ty) });
                    }
                }
            }
            blocks[b].succs = succs;
        }
        let edges: Vec<(BlockId, BlockId)> = blocks
            .iter()
            .enumerate()
            .flat_map(|(b, blk)| blk.succs.iter().map(move |e| (b as BlockId, e.to)))
            .collect();
        for (from, to) in edges {
            blocks[to as usize].preds.push(from);
        }
        for blk in &mut blocks {
            blk.preds.sort_unstable();
            blk.preds.dedup();
        }

        // Reverse postorder by iterative DFS, visiting successors in stored order.
        let mut post = Vec::with_capacity(blocks.len());
        let mut seen = vec![false; blocks.len()];
        let mut stack: Vec<(BlockId, usize)> = vec![(0, 0)];
        seen[0] = true;
        while let Some((b, i)) = stack.pop() {
            if let Some(e) = blocks[b as usize].succs.get(i) {
                stack.push((b, i + 1));
                if !seen[e.to as usize] {
                    seen[e.to as usize] = true;
                    stack.push((e.to, 0));
                }
            } else {
                post.push(b);
            }
        }
        post.reverse();
        let mut rpo_index = vec![u32::MAX; blocks.len()];
        for (i, &b) in post.iter().enumerate() {
            rpo_index[b as usize] = i as u32;
        }
        if let Some(&b) = falls_off.iter().find(|&&b| rpo_index[b] != u32::MAX) {
            return Err(err(blocks[b].last(), "control falls off the end of the code"));
        }
        Ok(Cfg { blocks, block_of, rpo: post, rpo_index })
    }

    pub fn is_reachable(&self, b: BlockId) -> bool {
        self.rpo_index[b as usize] != u32::MAX
    }

    /// Immediate dominators (Cooper, Harvey & Kennedy, "A Simple, Fast Dominance
    /// Algorithm"). `idom[entry] == entry`; unreachable blocks get `None`.
    pub fn dominators(&self) -> Vec<Option<BlockId>> {
        let mut idom: Vec<Option<BlockId>> = vec![None; self.blocks.len()];
        idom[0] = Some(0);
        let intersect = |idom: &[Option<BlockId>], mut a: BlockId, mut b: BlockId| {
            while a != b {
                while self.rpo_index[a as usize] > self.rpo_index[b as usize] {
                    a = idom[a as usize].expect("processed");
                }
                while self.rpo_index[b as usize] > self.rpo_index[a as usize] {
                    b = idom[b as usize].expect("processed");
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &b in self.rpo.iter().skip(1) {
                let mut new = None;
                for &p in &self.blocks[b as usize].preds {
                    if idom[p as usize].is_some() {
                        new = Some(match new {
                            None => p,
                            Some(n) => intersect(&idom, p, n),
                        });
                    }
                }
                if new.is_some() && idom[b as usize] != new {
                    idom[b as usize] = new;
                    changed = true;
                }
            }
        }
        idom
    }
}
