//! Structural rewrites: un-passes that change the program's shape (inline outlines back, split
//! merged classes, ...). They run on the model **before** any label is recorded and before
//! naming, because labels are keyed by position. Each records what it did for the report.

use eightr_ir::model::Program as Model;
use eightr_rules::Source;
use serde::Serialize;

use crate::error::{Error, Result};

mod merged;
mod outlines;
mod rebox;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RewriteRecord {
    pub rule: &'static str,
    /// The item rewritten, in input names.
    pub item: String,
    pub detail: String,
}

pub trait Rewrite {
    fn name(&self) -> &'static str;
    /// Rewrites run in source (undo) order, like passes.
    fn source(&self) -> Source;
    fn run(&self, model: &mut Model, records: &mut Vec<RewriteRecord>) -> Result<()>;
}

/// All rewrites, in undo order.
pub fn all() -> Vec<Box<dyn Rewrite>> {
    let mut v: Vec<Box<dyn Rewrite>> = vec![Box::new(outlines::OutlineInline), Box::new(rebox::Rebox), Box::new(merged::SplitMerged)];
    v.sort_by_key(|r| (r.source(), r.name()));
    v
}

/// Runs every rewrite, then restores the model's descriptor order. Every record must name a
/// registered rule.
pub fn run_all(model: &mut Model) -> Result<Vec<RewriteRecord>> {
    let mut records = Vec::new();
    for r in all() {
        r.run(model, &mut records)?;
    }
    model.sort();
    for rec in &records {
        if eightr_rules::lookup(rec.rule).is_none() {
            return Err(Error::UnregisteredRule { rule: rec.rule.to_string(), detail: "rewrite record".into() });
        }
    }
    records.sort();
    Ok(records)
}
