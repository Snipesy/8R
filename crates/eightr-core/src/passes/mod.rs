//! Un-passes. Each implements one or more registered rules.

mod kept_name;

pub use kept_name::NameStats;

use eightr_rules::Source;

use crate::error::Result;
use crate::labels::Labels;
use crate::pipeline::Evidence;
use crate::program::Program;

pub struct Context<'a> {
    pub program: &'a mut Program,
    pub evidence: &'a Evidence,
    pub labels: &'a mut Labels,
}

pub trait Pass {
    /// Stable name for logs and reports.
    fn name(&self) -> &'static str;
    /// The source whose transformation this pass undoes; passes run in source (undo) order.
    fn source(&self) -> Source;
    fn run(&self, cx: &mut Context) -> Result<()>;
}

/// All passes, in undo order: by source, then by name.
pub fn all() -> Vec<Box<dyn Pass>> {
    let mut v: Vec<Box<dyn Pass>> = vec![Box::new(kept_name::KeptName)];
    v.sort_by_key(|p| (p.source(), p.name()));
    v
}
