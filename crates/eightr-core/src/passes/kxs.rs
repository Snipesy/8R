//! kotlinx.serialization descriptors (docs/sources/kotlinx-serialization.md §2.3, §3.3): a
//! generated `$serializer`'s `<clinit>` builds `new D(serialName, INSTANCE, n)` and adds exactly
//! `n` elements `D.m(elementName, optional)`. A serial name can be overridden (`@SerialName`), so
//! it only hints structural names (D): the serialized class gets the name's last segment, the
//! serializer that plus `Serializer`. (Proven simple names come from `kotlinc/data-class-name`
//! when the class's `toString` survives.)

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::model::Program as Model;
use eightr_ir::op::{Const, InvokeKind, Op};
use eightr_rules::Source;

use super::{Context, Pass};
use crate::error::Result;
use crate::program::ClassId;

pub struct Kxs;

/// A serializer: (serializer class, serial name, element names, serialized class).
pub type Descriptor = (usize, String, Vec<String>, Option<usize>);

pub fn descriptors(p: &Model) -> Vec<Descriptor> {
    let s = &p.syms;
    let mut out = Vec::new();
    for (xi, x) in p.classes.iter().enumerate() {
        let Some(clinit) = x.methods.iter().find(|m| s.get(m.name) == "<clinit>").and_then(|m| m.code.as_ref()) else { continue };
        let mut strings: BTreeMap<u16, String> = BTreeMap::new();
        let mut ints: BTreeMap<u16, i32> = BTreeMap::new();
        let mut own: BTreeSet<u16> = BTreeSet::new(); // registers holding the class's own instance
        let mut found: Option<(u16, String, i32, Vec<String>)> = None;
        for insn in &clinit.insns {
            match &insn.op {
                Op::NewInstance { dst, ty } if *ty == x.ty => {
                    own.insert(*dst);
                }
                Op::Invoke { kind: InvokeKind::Direct, method, args } if found.is_none() && s.get(method.name) == "<init>" && s.get(method.proto).starts_with("(Ljava/lang/String;") && args.len() == 4 => {
                    if let (Some(name), true, Some(&n)) = (strings.get(&args[1]), own.contains(&args[2]), ints.get(&args[3])) {
                        found = Some((args[0], name.clone(), n, Vec::new()));
                    }
                }
                Op::Invoke { kind: InvokeKind::Virtual, method, args } if s.get(method.proto) == "(Ljava/lang/String;Z)V" && args.len() == 3 => {
                    if let (Some((obj, _, _, elements)), Some(e)) = (found.as_mut(), strings.get(&args[1])) {
                        if args[0] == *obj {
                            elements.push(e.clone());
                        }
                    }
                }
                _ => {}
            }
            if let Some((d, wide)) = insn.op.def() {
                strings.remove(&d);
                ints.remove(&d);
                if wide {
                    strings.remove(&(d + 1));
                    ints.remove(&(d + 1));
                }
                match &insn.op {
                    Op::ConstString { dst, value } => {
                        strings.insert(*dst, s.get(*value).to_string());
                    }
                    Op::Const { dst, value: Const::Narrow(k) } => {
                        ints.insert(*dst, *k);
                    }
                    Op::NewInstance { .. } => {}
                    _ => {
                        own.remove(&d);
                    }
                }
            }
        }
        let Some((_, name, n, elements)) = found else { continue };
        if elements.len() != n as usize {
            continue;
        }
        // The serialized class: the one class the serializer constructs through a constructor
        // whose first parameter is the seen-mask int.
        let built: BTreeSet<usize> = x
            .methods
            .iter()
            .filter_map(|m| m.code.as_ref())
            .flat_map(|b| &b.insns)
            .filter_map(|i| match &i.op {
                Op::Invoke { kind: InvokeKind::Direct, method, .. } if s.get(method.name) == "<init>" && s.get(method.proto).starts_with("(I") && method.class != x.ty => p.find(s.get(method.class)),
                _ => None,
            })
            .collect();
        let target = (built.len() == 1).then(|| *built.iter().next().unwrap());
        out.push((xi, name, elements, target));
    }
    out
}

/// Room's schema validation messages name each entity class: `"table(com.x.Entity).\n Expected:\n"`.
pub fn room_entities(p: &Model) -> Vec<String> {
    let s = &p.syms;
    let mut out = BTreeSet::new();
    for b in p.classes.iter().flat_map(|c| &c.methods).filter_map(|m| m.code.as_ref()) {
        for x in &b.insns {
            if let Op::ConstString { value, .. } = &x.op {
                let v = s.get(*value);
                if let Some(head) = v.strip_suffix(").\n Expected:\n") {
                    if let Some((table, fqn)) = head.split_once('(') {
                        let ident = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$');
                        if ident(table) && ident(fqn) && fqn.contains('.') {
                            out.insert(fqn.to_string());
                        }
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

impl Pass for Kxs {
    fn name(&self) -> &'static str {
        "kxs"
    }

    fn source(&self) -> Source {
        Source::KotlinxSerialization
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let found = descriptors(&cx.program.model);
        for (xi, name, _, target) in found {
            let last = name.rsplit(['.', '$']).next().unwrap_or(&name).to_string();
            if let Some(ci) = target {
                cx.program.class_hints.entry(ClassId(ci as u32)).or_insert_with(|| last.clone());
            }
            cx.program.class_hints.entry(ClassId(xi as u32)).or_insert_with(|| format!("{last}Serializer"));
        }
        Ok(())
    }
}
