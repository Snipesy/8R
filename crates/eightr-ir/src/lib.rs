//! Register IR for 8R.
//!
//! [`lift`] turns a dex `code_item` into a [`Body`] of semantic [`Op`]s with every index
//! resolved and interned. On top of that: a [`Cfg`] with exceptional edges and dominators,
//! register [`types`] inference, and [`defs`] (reaching definitions).
//!
//! Everything here is deterministic: worklists are ordered by reverse postorder, and
//! interned [`Sym`] ids never influence ordering or output.

pub mod cfg;
pub mod dataflow;
pub mod defs;
pub mod lift;
pub mod model;
pub mod op;
pub mod print;
pub mod rename;
mod resolve;
pub mod sym;
pub mod types;
pub mod value;

pub use cfg::{BlockId, Cfg, Edge, EdgeKind};
pub use lift::{lift, Body, Insn};
pub use op::Op;
pub use sym::{Interner, Sym};
