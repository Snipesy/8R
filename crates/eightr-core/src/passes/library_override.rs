//! `r8/library-override-name`: a method that overrides a library method keeps its name.
//!
//! Evidence is conservative: the dex must reference a library method with the same name and
//! descriptor on one of the class's library supertypes. (Without the platform's API surface,
//! an unreferenced library method can't be seen; that only costs coverage, never soundness.)

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::op::Op;
use eightr_ir::value::HandleMember;
use eightr_rules::{Attribute, Source, LIBRARY_OVERRIDE_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId, Program};

pub struct LibraryOverride;

/// Library (non-program) types reachable through `id`'s supertypes, walking through program
/// classes.
fn library_supertypes<'p>(p: &'p Program, id: ClassId, memo: &mut BTreeMap<ClassId, BTreeSet<&'p str>>) -> BTreeSet<&'p str> {
    if let Some(s) = memo.get(&id) {
        return s.clone();
    }
    memo.insert(id, BTreeSet::new()); // cycle guard for malformed input
    let c = p.class(id);
    let mut out = BTreeSet::new();
    for sup in c.superclass.iter().chain(&c.interfaces) {
        let d = p.str(*sup);
        match p.find(d) {
            Some(sid) => out.extend(library_supertypes(p, sid, memo)),
            None => {
                out.insert(d);
            }
        }
    }
    memo.insert(id, out.clone());
    out
}

impl Pass for LibraryOverride {
    fn name(&self) -> &'static str {
        "library-override"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let p = &*cx.program;
        // Library methods the dex references: (owner, name, descriptor).
        let mut lib_methods: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
        for c in &p.model.classes {
            for m in c.methods.iter().filter_map(|m| m.code.as_ref()) {
                for i in &m.insns {
                    let r = match &i.op {
                        Op::Invoke { method, .. } | Op::InvokePolymorphic { method, .. } => Some(*method),
                        Op::ConstMethodHandle { handle, .. } => match handle.member {
                            HandleMember::Method(m) => Some(m),
                            HandleMember::Field(_) => None,
                        },
                        _ => None,
                    };
                    if let Some(r) = r {
                        if p.find(p.str(r.class)).is_none() {
                            lib_methods.insert((p.str(r.class), p.str(r.name), p.str(r.proto)));
                        }
                    }
                }
            }
        }
        let mut memo = BTreeMap::new();
        let mut items = Vec::new();
        for id in p.class_ids() {
            let supers = library_supertypes(p, id, &mut memo);
            if supers.is_empty() {
                continue;
            }
            for (index, m) in p.class(id).methods.iter().enumerate() {
                if m.access & (access::STATIC | access::PRIVATE | access::CONSTRUCTOR) != 0 {
                    continue;
                }
                let (name, proto) = (p.str(m.name), p.str(m.proto));
                if supers.iter().any(|l| lib_methods.contains(&(*l, name, proto))) {
                    items.push(ItemId::Method { class: id, index: index as u32 });
                }
            }
        }
        for item in items {
            cx.labels.record(item, Attribute::MemberName, LIBRARY_OVERRIDE_NAME, None)?;
        }
        Ok(())
    }
}
