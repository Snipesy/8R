//! `kotlinc/lateinit-field-name`: recovers the name of a `lateinit` property's backing field
//! from kotlinc's uninitialized-access message, bound through the field reference.

use std::collections::BTreeMap;

use eightr_ir::cfg::{Cfg, EdgeKind};
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::lift::Body;
use eightr_ir::op::{Cond, FieldRef, Op, Reg};
use eightr_rules::{Attribute, Source, LATEINIT_FIELD_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ItemId, Program};
use crate::report::{Finding, Severity};

pub struct Lateinit;

const PREFIX: &str = "lateinit property ";
const SUFFIX: &str = " has not been initialized";

/// Property name from the folded message, if it is exactly the template.
fn folded_name(s: &str) -> Option<&str> {
    s.strip_prefix(PREFIX)?.strip_suffix(SUFFIX).filter(|n| !n.is_empty())
}

/// Static `(Ljava/lang/String;)V` methods that build the message: stdlib's
/// `throwUninitializedPropertyAccessException`, possibly renamed. (owner, name)
fn helper_methods(p: &Program) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in &p.model.classes {
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            if !m.is_static() || p.str(m.proto) != "(Ljava/lang/String;)V" {
                continue;
            }
            let has = |needle: &str| b.insns.iter().any(|i| matches!(&i.op, Op::ConstString { value, .. } if p.str(*value) == needle));
            if has(PREFIX) && has(SUFFIX) {
                out.push((p.str(c.ty).to_string(), p.str(m.name).to_string()));
            }
        }
    }
    out
}

/// The field whose null value leads into `block`: every predecessor edge must be the null
/// branch of a test on a register read from the same field.
fn guard_field(body: &Body, cfg: &Cfg, rd: &ReachingDefs, block: u32) -> Option<FieldRef> {
    let preds = &cfg.blocks[block as usize].preds;
    if preds.is_empty() {
        return None;
    }
    let mut field: Option<FieldRef> = None;
    for &p in preds {
        let pb = &cfg.blocks[p as usize];
        let last = pb.last();
        let Op::IfZ { cond, a, .. } = &body.insns[last as usize].op else { return None };
        for e in pb.succs.iter().filter(|e| e.to == block) {
            let null_edge = matches!((cond, e.kind), (Cond::Eq, EdgeKind::Taken) | (Cond::Ne, EdgeKind::Fallthrough));
            if !null_edge {
                return None;
            }
        }
        let f = field_of(body, rd, last, *a, 1)?;
        match field {
            None => field = Some(f),
            Some(prev) if prev == f => {}
            Some(_) => return None,
        }
    }
    field
}

/// The field every reaching definition of `reg` at `at` reads (following at most `moves` moves).
fn field_of(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg, moves: u32) -> Option<FieldRef> {
    let uses = rd.uses[at as usize].as_ref()?;
    let (_, defs) = uses.iter().find(|(r, _)| *r == reg)?;
    let mut field: Option<FieldRef> = None;
    for &d in defs {
        let DefSite::Insn(site) = rd.defs[d].site else { return None };
        let f = match &body.insns[site as usize].op {
            Op::InstanceGet { field, .. } | Op::StaticGet { field, .. } => *field,
            Op::Move { src, .. } if moves > 0 => field_of(body, rd, site, *src, moves - 1)?,
            _ => return None,
        };
        match field {
            None => field = Some(f),
            Some(prev) if prev == f => {}
            Some(_) => return None,
        }
    }
    field
}

impl Pass for Lateinit {
    fn name(&self) -> &'static str {
        "lateinit"
    }

    fn source(&self) -> Source {
        Source::Kotlinc
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let p = &*cx.program;
        let helpers = helper_methods(p);
        // field → proposed names (sorted, deduplicated at the end).
        let mut proposals: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
        for c in &p.model.classes {
            for m in &c.methods {
                let Some(body) = &m.code else { continue };
                // Find message sites: (instruction index, property name).
                let mut sites: Vec<(u32, String)> = Vec::new();
                for (i, insn) in body.insns.iter().enumerate() {
                    if let Op::ConstString { value, .. } = &insn.op {
                        if let Some(n) = folded_name(p.str(*value)) {
                            sites.push((i as u32, n.to_string()));
                        }
                    }
                }
                let invokes_helper = body.insns.iter().any(|i| {
                    matches!(&i.op, Op::Invoke { method, .. }
                        if helpers.iter().any(|(o, n)| p.str(method.class) == o && p.str(method.name) == n))
                });
                if sites.is_empty() && !invokes_helper {
                    continue;
                }
                let Ok(cfg) = Cfg::build(body) else { continue };
                let rd = ReachingDefs::compute(body, &cfg);
                if invokes_helper {
                    for (i, insn) in body.insns.iter().enumerate() {
                        let Op::Invoke { method, args, .. } = &insn.op else { continue };
                        if !helpers.iter().any(|(o, n)| p.str(method.class) == o && p.str(method.name) == n) {
                            continue;
                        }
                        // The argument must be a single constant string.
                        let Some(&arg) = args.first() else { continue };
                        let Some(uses) = rd.uses[i].as_ref() else { continue };
                        let Some((_, defs)) = uses.iter().find(|(r, _)| *r == arg) else { continue };
                        let names: Vec<&str> = defs
                            .iter()
                            .filter_map(|&d| match rd.defs[d].site {
                                DefSite::Insn(s) => match &body.insns[s as usize].op {
                                    Op::ConstString { value, .. } => Some(p.str(*value)),
                                    _ => None,
                                },
                                DefSite::Param => None,
                            })
                            .collect();
                        if names.len() == defs.len() && !names.is_empty() && names.iter().all(|n| *n == names[0]) {
                            sites.push((i as u32, names[0].to_string()));
                        }
                    }
                }
                for (i, name) in sites {
                    let block = cfg.block_of[i as usize];
                    if !cfg.is_reachable(block) {
                        continue;
                    }
                    if let Some(f) = guard_field(body, &cfg, &rd, block) {
                        let key = (p.str(f.class).to_string(), p.str(f.name).to_string(), p.str(f.ty).to_string());
                        proposals.entry(key).or_default().push(name);
                    }
                }
            }
        }
        // Fields R8 widened to Object when merging classes (reads cast straight away): such a
        // field may hold several original fields, one per merged class, and the mapping keeps one
        // name. Refused.
        let mut widened: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
        for c in &p.model.classes {
            for m in &c.methods {
                let Some(body) = &m.code else { continue };
                for w in body.insns.windows(2) {
                    if let (Op::InstanceGet { dst, field, .. } | Op::StaticGet { dst, field, .. }, Op::CheckCast { reg, .. }) = (&w[0].op, &w[1].op) {
                        if dst == reg && p.str(field.ty) == "Ljava/lang/Object;" {
                            widened.insert((p.str(field.class).to_string(), p.str(field.name).to_string()));
                        }
                    }
                }
            }
        }
        let mut findings = Vec::new();
        let mut labels = Vec::new();
        for ((class, name, ty), mut names) in proposals {
            if widened.contains(&(class.clone(), name.clone())) {
                continue;
            }
            names.sort();
            names.dedup();
            let Some(cid) = p.find(&class) else { continue };
            let Some(index) = p.class(cid).fields.iter().position(|f| p.str(f.name) == name && p.str(f.ty) == ty) else { continue };
            if names.len() != 1 || !ty.starts_with(['L', '[']) {
                findings.push(Finding {
                    severity: Severity::Warning,
                    message: format!("{LATEINIT_FIELD_NAME}: {class}->{name}:{ty} has conflicting lateinit names {names:?}; refused"),
                });
                continue;
            }
            labels.push((ItemId::Field { class: cid, index: index as u32 }, names.remove(0)));
        }
        for (item, name) in labels {
            cx.labels.record_value(item, Attribute::MemberName, LATEINIT_FIELD_NAME, None, Some(name))?;
        }
        cx.findings.extend(findings);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template() {
        assert_eq!(folded_name("lateinit property db has not been initialized"), Some("db"));
        assert_eq!(folded_name("lateinit property  has not been initialized"), None);
        assert_eq!(folded_name("lateinit property db"), None);
    }
}
