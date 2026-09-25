//! A small forward dataflow solver shared by the analyses. The worklist is ordered by
//! reverse-postorder position, so results never depend on hash or insertion order.

use std::collections::BTreeSet;

use crate::cfg::{BlockId, Cfg};
use crate::lift::Body;

pub trait Forward {
    type State: Clone + PartialEq;

    fn entry_state(&self) -> Self::State;

    /// Joins `incoming` into `into` (`None` = no information yet). Returns whether `into`
    /// changed.
    fn join(&self, into: &mut Option<Self::State>, incoming: &Self::State) -> bool;

    /// Applies instruction `idx` (in block `block`) to `state`.
    fn transfer(&self, block: BlockId, idx: u32, state: &mut Self::State);
}

/// Returns the entry state of every block (`None` for unreachable blocks).
pub fn solve<F: Forward>(f: &F, cfg: &Cfg) -> Vec<Option<F::State>> {
    let mut entry: Vec<Option<F::State>> = vec![None; cfg.blocks.len()];
    entry[0] = Some(f.entry_state());
    let mut work: BTreeSet<u32> = BTreeSet::from([0]); // rpo positions
    while let Some(pos) = work.pop_first() {
        let b = cfg.rpo[pos as usize];
        let blk = &cfg.blocks[b as usize];
        let mut state = entry[b as usize].clone().expect("queued blocks have state");
        let mut before_last = None;
        for idx in blk.start..blk.end {
            if idx == blk.last() && blk.succs.iter().any(|e| e.is_exceptional()) {
                before_last = Some(state.clone());
            }
            f.transfer(b, idx, &mut state);
        }
        for e in &blk.succs {
            let incoming = if e.is_exceptional() { before_last.as_ref().expect("set above") } else { &state };
            if f.join(&mut entry[e.to as usize], incoming) {
                work.insert(cfg.rpo_index[e.to as usize]);
            }
        }
    }
    entry
}

/// The state before each instruction, replayed from block entry states.
pub fn per_instruction<F: Forward>(f: &F, body: &Body, cfg: &Cfg, entry: &[Option<F::State>]) -> Vec<Option<F::State>> {
    let mut out = vec![None; body.insns.len()];
    for (b, blk) in cfg.blocks.iter().enumerate() {
        let Some(mut state) = entry[b].clone() else { continue };
        for idx in blk.start..blk.end {
            out[idx as usize] = Some(state.clone());
            f.transfer(b as BlockId, idx, &mut state);
        }
    }
    out
}
