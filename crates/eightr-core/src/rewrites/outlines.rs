//! `r8/outline-inline` and `r8/bu-outline-inline`: inline R8's outlines back into their call
//! sites (docs/sources/r8-desugar.md §4.5).
//!
//! Detection is purely structural, never by name:
//! * a method of a class R8 synthesized to hold outlines (see [`is_outline_holder`]);
//! * a static method with straight-line code, reached only by `invoke-static` from at least
//!   two sites;
//! * classic outline: ends in a return; body is invokes / `new-instance` / arithmetic / moves;
//! * bottom-up (throw) outline (R8 ≥ 9): ends in `throw` of a freshly built exception.
//!
//! Inlining a static method preserves behavior provided its class has no `<clinit>` (the call
//! would have triggered class initialization) and everything its body references is
//! accessible from the caller. A hand-written helper can look like an outline, so these rules
//! are D: the result is the same program, just more readable.

use std::collections::BTreeMap;

use eightr_dex::class::access;
use eightr_ir::inline::inline_static_calls;
use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{InvokeKind, MethodRef, Op};
use eightr_ir::value::{HandleMember, Value};
use eightr_rules::{Source, BU_OUTLINE_INLINE, OUTLINE_INLINE};

use super::{Rewrite, RewriteRecord};
use crate::error::Result;

pub struct OutlineInline;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Classic,
    Throw,
}

type Key = (String, String, String); // (class, name, proto)

fn key(p: &Model, m: &MethodRef) -> Key {
    (p.syms.get(m.class).to_string(), p.syms.get(m.name).to_string(), p.syms.get(m.proto).to_string())
}

fn package(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// Method references that aren't plain `invoke-static` calls (method handles, values,
/// annotations): a method referenced that way must keep existing.
fn escaping_refs(p: &Model) -> std::collections::BTreeSet<Key> {
    fn value(p: &Model, v: &Value, out: &mut std::collections::BTreeSet<Key>) {
        match v {
            Value::Method(m) => {
                out.insert(key(p, m));
            }
            Value::MethodHandle(h) => {
                if let HandleMember::Method(m) = h.member {
                    out.insert(key(p, &m));
                }
            }
            Value::Array(a) => a.iter().for_each(|x| value(p, x, out)),
            Value::Annotation(a) => a.elements.iter().for_each(|(_, x)| value(p, x, out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for c in &p.classes {
        for a in c.annotations.iter().chain(c.fields.iter().flat_map(|f| &f.annotations)).chain(c.methods.iter().flat_map(|m| &m.annotations)) {
            for (_, v) in &a.annotation.elements {
                value(p, v, &mut out);
            }
        }
        for f in &c.fields {
            if let Some(v) = &f.static_value {
                value(p, v, &mut out);
            }
        }
        for insn in c.methods.iter().filter_map(|m| m.code.as_ref()).flat_map(|b| &b.insns) {
            match &insn.op {
                Op::Invoke { kind, method, .. } if *kind != InvokeKind::Static => {
                    out.insert(key(p, method));
                }
                Op::InvokePolymorphic { method, .. } => {
                    out.insert(key(p, method));
                }
                Op::ConstMethodHandle { handle, .. } => {
                    if let HandleMember::Method(m) = handle.member {
                        out.insert(key(p, &m));
                    }
                }
                Op::InvokeCustom { call_site, .. } => {
                    for v in std::iter::once(&Value::MethodHandle(call_site.bootstrap)).chain(&call_site.extra) {
                        value(p, v, &mut out);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// R8 and D8 put outlines in classes they synthesize (`$$ExternalSyntheticOutline0`, `Foo$0`),
/// which the horizontal class merger may fold into a merged lambda group; either way the
/// holder is marked synthetic. Hand-written helpers (`throwIllegalArgumentException`,
/// `copyInto`) share the shape of an outline but never live in a synthetic class; inlining
/// them would be correct but less readable. No `<clinit>`: the call would have triggered it.
fn is_outline_holder(p: &Model, c: &eightr_ir::model::Class) -> bool {
    c.access & access::SYNTHETIC != 0 && c.access & access::INTERFACE == 0 && !c.methods.iter().any(|m| p.syms.get(m.name) == "<clinit>")
}

fn classify(body: &Body) -> Option<Kind> {
    if !body.tries.is_empty() || body.insns.is_empty() {
        return None;
    }
    let last = &body.insns.last()?.op;
    let kind = match last {
        Op::Return { .. } | Op::ReturnVoid => Kind::Classic,
        Op::Throw { .. } => Kind::Throw,
        _ => return None,
    };
    let mut work = 0;
    let mut news = 0;
    for (k, insn) in body.insns.iter().enumerate() {
        let is_last = k + 1 == body.insns.len();
        match &insn.op {
            Op::Invoke { .. } | Op::Binop { .. } | Op::Unop { .. } => work += 1,
            Op::NewInstance { .. } => {
                work += 1;
                news += 1;
            }
            Op::Move { .. } | Op::MoveResult { .. } | Op::Const { .. } => {}
            Op::ConstString { .. } if kind == Kind::Throw => {}
            Op::Return { .. } | Op::ReturnVoid | Op::Throw { .. } if is_last => {}
            _ => return None, // branches, fields, arrays, casts, monitors, ...
        }
    }
    let ok = match kind {
        Kind::Classic => work >= 2,
        Kind::Throw => news >= 1 && work >= 2,
    };
    ok.then_some(kind)
}

/// Can code in `caller_class` reference everything `body` references?
fn accessible_from(p: &Model, caller_class: &str, body: &Body) -> bool {
    let s = &p.syms;
    let class_ok = |desc: &str| -> bool {
        let base = desc.trim_start_matches('[');
        match p.find(base) {
            Some(i) => p.classes[i].access & access::PUBLIC != 0 || package(base) == package(caller_class),
            None => true, // library: outlines only reference accessible library code
        }
    };
    let member_ok = |owner: &str, flags: Option<u32>| -> bool {
        match flags {
            None => true,
            Some(f) if f & access::PUBLIC != 0 => true,
            Some(f) if f & access::PRIVATE != 0 => owner == caller_class,
            Some(_) => package(owner) == package(caller_class),
        }
    };
    for insn in &body.insns {
        match &insn.op {
            Op::Invoke { method, .. } => {
                let owner = s.get(method.class);
                if !class_ok(owner) {
                    return false;
                }
                let flags = p.find(owner).and_then(|i| {
                    p.classes[i].methods.iter().find(|m| m.name == method.name && m.proto == method.proto).map(|m| m.access)
                });
                if !member_ok(owner, flags) {
                    return false;
                }
            }
            Op::NewInstance { ty, .. } if !class_ok(s.get(*ty)) => return false,
            _ => {}
        }
    }
    true
}

impl Rewrite for OutlineInline {
    fn name(&self) -> &'static str {
        "outline-inline"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, p: &mut Model, records: &mut Vec<RewriteRecord>) -> Result<()> {
        let escaping = escaping_refs(p);
        // Static call sites of every method: key → [(class, method, insn)].
        let mut sites: BTreeMap<Key, Vec<(usize, usize, u32)>> = BTreeMap::new();
        for (ci, c) in p.classes.iter().enumerate() {
            for (mi, m) in c.methods.iter().enumerate() {
                for (k, insn) in m.code.iter().flat_map(|b| b.insns.iter().enumerate()) {
                    if let Op::Invoke { kind: InvokeKind::Static, method, .. } = &insn.op {
                        sites.entry(key(p, method)).or_default().push((ci, mi, k as u32));
                    }
                }
            }
        }
        // Candidate outlines: key → (class, method, kind).
        let mut outlines: BTreeMap<Key, (usize, usize, Kind)> = BTreeMap::new();
        for (ci, c) in p.classes.iter().enumerate() {
            if !is_outline_holder(p, c) {
                continue;
            }
            for (mi, m) in c.methods.iter().enumerate() {
                let Some(body) = &m.code else { continue };
                if m.access & access::STATIC == 0 || m.access & (access::NATIVE | access::ABSTRACT | access::CONSTRUCTOR) != 0 {
                    continue;
                }
                let k = (p.syms.get(c.ty).to_string(), p.syms.get(m.name).to_string(), p.syms.get(m.proto).to_string());
                let calls = sites.get(&k).map_or(0, Vec::len);
                if calls < 2 || escaping.contains(&k) {
                    continue;
                }
                // Not self-referencing, and not calling another candidate-shaped method (R8 never
                // nests outlines; refusing keeps inlining order-independent).
                let calls_static = body.insns.iter().any(|i| matches!(&i.op, Op::Invoke { kind: InvokeKind::Static, method, .. } if key(p, method) == k));
                if calls_static {
                    continue;
                }
                if let Some(kind) = classify(body) {
                    outlines.insert(k, (ci, mi, kind));
                }
            }
        }
        if outlines.is_empty() {
            return Ok(());
        }
        // Callee bodies (cloned so callers can be edited freely).
        let bodies: BTreeMap<Key, (Body, String, Kind)> = outlines
            .iter()
            .map(|(k, &(ci, mi, kind))| (k.clone(), (p.classes[ci].methods[mi].code.clone().expect("has code"), k.2.clone(), kind)))
            .collect();

        // Per caller: every outline site, if accessible from the caller.
        let mut per_caller: BTreeMap<(usize, usize), Vec<(u32, Key)>> = BTreeMap::new();
        for (k, list) in &sites {
            let Some((_, body_proto_kind)) = bodies.get_key_value(k) else { continue };
            for &(ci, mi, insn) in list {
                if accessible_from(p, p.syms.get(p.classes[ci].ty), &body_proto_kind.0) {
                    per_caller.entry((ci, mi)).or_default().push((insn, k.clone()));
                }
            }
        }
        let mut inlined: BTreeMap<Key, usize> = BTreeMap::new();
        let mut refused: BTreeMap<Key, BTreeMap<String, usize>> = BTreeMap::new();
        for ((ci, mi), list) in per_caller {
            let Some(caller) = p.classes[ci].methods[mi].code.as_mut() else { continue };
            let batch: Vec<(u32, &Body, &str)> = list.iter().map(|(i, k)| (*i, &bodies[k].0, bodies[k].1.as_str())).collect();
            let mut done: Vec<&Key> = Vec::new();
            for ((_, k), res) in list.iter().zip(inline_static_calls(caller, &batch)) {
                match res {
                    Ok(()) => done.push(k),
                    Err(e) => *refused.entry(k.clone()).or_default().entry(format!("{e:?}")).or_default() += 1,
                }
            }
            if done.iter().any(|k| bodies[*k].2 == Kind::Throw) {
                eightr_ir::edit::remove_unreachable(caller);
            }
            for k in done {
                *inlined.entry(k.clone()).or_default() += 1;
            }
        }

        // Delete outlines with no remaining callers; record everything.
        let mut delete: Vec<(usize, usize)> = Vec::new();
        for (k, &(ci, mi, kind)) in &outlines {
            let total = sites[k].len();
            let n = inlined.get(k).copied().unwrap_or(0);
            if n == 0 {
                continue;
            }
            let rule = if kind == Kind::Throw { BU_OUTLINE_INLINE } else { OUTLINE_INLINE };
            let detail = if n == total {
                delete.push((ci, mi));
                format!("inlined at all {total} call sites; method removed")
            } else {
                let why: Vec<String> = refused.get(k).into_iter().flatten().map(|(r, c)| format!("{c} {r}")).collect();
                let why = if why.is_empty() { "others inaccessible from their caller".to_string() } else { format!("refused: {}", why.join(", ")) };
                format!("inlined at {n} of {total} call sites; {why}")
            };
            records.push(RewriteRecord { rule, item: format!("{}->{}{}", k.0, k.1, k.2), detail });
        }
        delete.sort_by(|a, b| b.cmp(a));
        for (ci, mi) in delete {
            p.classes[ci].methods.remove(mi);
        }
        remove_empty_unreferenced_classes(p);
        Ok(())
    }
}

/// Removes classes left with no members, no annotations, and no references to their type.
fn remove_empty_unreferenced_classes(p: &mut Model) {
    let empty: Vec<usize> = p
        .classes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.fields.is_empty() && c.methods.is_empty() && c.annotations.is_empty() && c.interfaces.is_empty())
        .map(|(i, _)| i)
        .collect();
    if empty.is_empty() {
        return;
    }
    // Descriptors hide inside protos and generic signatures, so look for them in the text of
    // every other class.
    let mut referenced = vec![false; empty.len()];
    for (ci, c) in p.classes.iter().enumerate() {
        let text = type_text(p, c);
        for (k, &e) in empty.iter().enumerate() {
            referenced[k] |= ci != e && text.contains(p.syms.get(p.classes[e].ty));
        }
    }
    let mut remove: Vec<usize> = empty.into_iter().zip(referenced).filter(|(_, r)| !r).map(|(i, _)| i).collect();
    remove.sort_by(|a, b| b.cmp(a));
    for i in remove {
        p.classes.remove(i);
    }
}

/// Everything in `c` that can mention a type, as text: the class header and annotations,
/// fields, method signatures and annotations, instructions, and catch types.
fn type_text(p: &Model, c: &eightr_ir::model::Class) -> String {
    use eightr_ir::print;
    let s = &p.syms;
    let header = eightr_ir::model::Class {
        ty: c.ty,
        access: c.access,
        superclass: c.superclass,
        interfaces: c.interfaces.clone(),
        source_file: c.source_file,
        annotations: c.annotations.clone(),
        fields: Vec::new(),
        methods: Vec::new(),
        origin: c.origin,
    };
    let mut t = print::class(&header, s);
    for f in &c.fields {
        t.push_str(&print::field(f, s));
    }
    for m in &c.methods {
        t.push_str(&print::method_header(m, s));
        if let Some(b) = &m.code {
            for insn in &b.insns {
                t.push_str(&print::op(&insn.op, s));
                t.push('\n');
            }
            for h in b.tries.iter().flat_map(|x| &x.handlers) {
                if let Some(ty) = h.ty {
                    t.push_str(s.get(ty));
                    t.push('\n');
                }
            }
        }
    }
    t
}
