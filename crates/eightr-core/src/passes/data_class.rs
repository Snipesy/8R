//! `kotlinc/data-class-name` and `kotlinc/data-class-property` (S): a Kotlin data class's simple
//! name and property names from its generated `toString` (docs/sources/kotlinc.md §5).
//!
//! The compiler emits `"Name(p1=" + f1 + ", p2=" + f2 + … + ")"` over the primary-constructor
//! properties; R8 keeps the literals (it can't fold a field read). A hand-written `toString`
//! could imitate it, so S also needs the ensemble: `hashCode` or `equals` reading the same fields
//! in the same order, as the compiler generates them together. Without it the name is only a
//! structural-name hint (D).

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{FieldRef, InvokeKind, Op, Reg};
use eightr_rules::{Attribute, Source, DATA_CLASS_NAME, DATA_CLASS_PROPERTY};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub struct DataClass;

/// (class index, simple name, (property, field) pairs, ensemble proven).
type Found = (usize, String, Vec<(String, FieldRef)>, bool);

fn is_identifier(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// The `toString` template: (simple name, [(property name, field)]).
fn template(p: &Model, class: &str, body: &Body) -> Option<(String, Vec<(String, FieldRef)>)> {
    let s = &p.syms;
    let this = body.registers - body.ins;
    // Straight-line code only.
    if body.insns.iter().any(|x| x.op.is_branch()) {
        return None;
    }
    #[derive(Clone)]
    enum Val {
        Lit(String),
        Field(FieldRef),
    }
    let mut regs: BTreeMap<Reg, Val> = BTreeMap::new();
    let mut pieces: Vec<Val> = Vec::new();
    for x in &body.insns {
        match &x.op {
            Op::ConstString { dst, value } => {
                regs.insert(*dst, Val::Lit(s.get(*value).to_string()));
                continue;
            }
            Op::InstanceGet { dst, obj, field, .. } if *obj == this && s.get(field.class) == class => {
                regs.insert(*dst, Val::Field(*field));
                continue;
            }
            Op::Invoke { kind: InvokeKind::Virtual | InvokeKind::Direct, method, args } if s.get(method.class) == "Ljava/lang/StringBuilder;" => {
                let n = s.get(method.name);
                if (n == "append" || n == "<init>") && args.len() == 2 {
                    pieces.push(regs.get(&args[1])?.clone());
                }
            }
            _ => {}
        }
        if let Some((d, wide)) = x.op.def() {
            regs.remove(&d);
            if wide {
                regs.remove(&(d + 1));
            }
        }
    }
    // lit field (lit field)* lit
    if pieces.len() < 3 || pieces.len().is_multiple_of(2) {
        return None;
    }
    let lit = |v: &Val| match v {
        Val::Lit(t) => Some(t.clone()),
        Val::Field(_) => None,
    };
    let first = lit(&pieces[0])?;
    let (name, prop0) = first.strip_suffix('=')?.split_once('(')?;
    if !is_identifier(name) || !is_identifier(prop0) || lit(pieces.last()?)? != ")" {
        return None;
    }
    let mut props = Vec::new();
    let mut next = prop0.to_string();
    for (k, piece) in pieces[1..pieces.len() - 1].iter().enumerate() {
        if k % 2 == 0 {
            let Val::Field(f) = piece else { return None };
            props.push((next.clone(), *f));
        } else {
            let t = lit(piece)?;
            next = t.strip_prefix(", ")?.strip_suffix('=')?.to_string();
            if !is_identifier(&next) {
                return None;
            }
        }
    }
    let distinct: BTreeSet<String> = props.iter().map(|x| s.get(x.1.name).to_string()).collect();
    (distinct.len() == props.len()).then(|| (name.to_string(), props))
}

/// The sequence of fields of `class` a method reads (adjacent repeats merged: `equals` reads
/// each field of both objects).
fn reads(p: &Model, class: &str, body: &Body) -> Vec<FieldRef> {
    let mut out: Vec<FieldRef> = Vec::new();
    for x in &body.insns {
        if let Op::InstanceGet { field, .. } = &x.op {
            if p.syms.get(field.class) == class && out.last() != Some(field) {
                out.push(*field);
            }
        }
    }
    out
}

impl Pass for DataClass {
    fn name(&self) -> &'static str {
        "data-class"
    }

    fn source(&self) -> Source {
        Source::Kotlinc
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let p = &cx.program.model;
        let s = &p.syms;
        let pins = eightr_ir::reflect::pins(p);
        // Current simple names per package (a recovered name must not collide).
        let mut existing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for c in &p.classes {
            let d = s.get(c.ty);
            existing.entry(crate::program::package_of(d).to_string()).or_default().insert(crate::program::simple_name_of(d).to_string());
        }
        let mut found: Vec<Found> = Vec::new();
        for (ci, c) in p.classes.iter().enumerate() {
            let class = s.get(c.ty);
            let method = |name: &str, proto: &str| c.methods.iter().find(|m| s.get(m.name) == name && s.get(m.proto) == proto).and_then(|m| m.code.as_ref());
            let Some(ts) = method("toString", "()Ljava/lang/String;") else { continue };
            let Some((name, props)) = template(p, class, ts) else { continue };
            let seq: Vec<FieldRef> = props.iter().map(|x| x.1).collect();
            let ensemble = [method("hashCode", "()I"), method("equals", "(Ljava/lang/Object;)Z")].into_iter().flatten().any(|b| reads(p, class, b) == seq);
            found.push((ci, name, props, ensemble));
        }
        // A simple name claimed twice in one package, or already there: no S class name.
        let mut claims: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (ci, name, _, ensemble) in &found {
            if *ensemble {
                *claims.entry((crate::program::package_of(s.get(p.classes[*ci].ty)).to_string(), name.clone())).or_default() += 1;
            }
        }
        let mut class_labels = Vec::new();
        let mut field_labels = Vec::new();
        let mut hints = Vec::new();
        for (ci, name, props, ensemble) in found {
            let desc = s.get(p.classes[ci].ty);
            let pkg = crate::program::package_of(desc).to_string();
            let item = ItemId::Class { class: ClassId(ci as u32) };
            let own = crate::program::simple_name_of(desc) == name;
            let free = own || (!existing.get(&pkg).is_some_and(|e| e.contains(&name)) && claims.get(&(pkg, name.clone())) == Some(&1));
            let unlabelled = cx.labels.get(item, Attribute::ClassName).is_none();
            if ensemble && free && unlabelled && !pins.class(desc) && !crate::naming::is_platform_class(desc) {
                class_labels.push((item, name.clone()));
            } else if !ensemble {
                hints.push((ClassId(ci as u32), name.clone()));
            }
            if !ensemble {
                continue;
            }
            for (prop, f) in props {
                let Some(index) = p.classes[ci].fields.iter().position(|x| x.name == f.name && x.ty == f.ty) else { continue };
                let fitem = ItemId::Field { class: ClassId(ci as u32), index: index as u32 };
                if cx.labels.get(fitem, Attribute::MemberName).is_some() || pins.field(desc, s.get(f.name)) {
                    continue;
                }
                // Another field of the class already has the name.
                if p.classes[ci].fields.iter().enumerate().any(|(i, x)| i != index && s.get(x.name) == prop) {
                    continue;
                }
                field_labels.push((fitem, prop));
            }
        }
        for (item, name) in class_labels {
            cx.labels.record_value(item, Attribute::ClassName, DATA_CLASS_NAME, None, Some(name))?;
        }
        for (item, name) in field_labels {
            cx.labels.record_value(item, Attribute::MemberName, DATA_CLASS_PROPERTY, None, Some(name))?;
        }
        for (id, name) in hints {
            cx.program.class_hints.entry(id).or_insert(name);
        }
        Ok(())
    }
}
