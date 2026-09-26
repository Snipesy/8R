//! `r8/annotation-member-name`: R8 never renames annotation-interface methods.

use eightr_dex::class::access;
use eightr_rules::{Attribute, Source, ANNOTATION_MEMBER_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::ItemId;

pub struct AnnotationMember;

impl Pass for AnnotationMember {
    fn name(&self) -> &'static str {
        "annotation-member"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let mut items = Vec::new();
        for id in cx.program.class_ids() {
            let c = cx.program.class(id);
            if c.access & access::ANNOTATION == 0 {
                continue;
            }
            items.extend((0..c.methods.len() as u32).map(|index| ItemId::Method { class: id, index }));
        }
        for item in items {
            cx.labels.record(item, Attribute::MemberName, ANNOTATION_MEMBER_NAME, None)?;
        }
        Ok(())
    }
}
