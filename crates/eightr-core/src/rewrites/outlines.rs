//! `r8/outline-inline` and `r8/bu-outline-inline`: inline R8's outlines back into their call
//! sites (docs/sources/r8-desugar.md §4.5).
//!
//! Detection is purely structural, never by name:
//! * a method of a class R8 synthesized to hold outlines (see [`is_outline_holder`]);
//! * a static method with straight-line code, reached only by `invoke-static` from at least
//!   two sites;
//! * classic outline: ends in a return, at least 3 operations (R8's minimum outline size), at
//!   least one a call, and it only calls and instantiates *library* classes (R8 outlines
//!   sequences of library calls on their "holes"). This keeps out other synthetics of the same
//!   shape: interface companions (`$-CC`) call app code, and backports are pure arithmetic;
//! * bottom-up (throw) outline (R8 ≥ 9): ends in `throw` of a freshly built exception (which
//!   may be an app class, and may use other outlines to build its message).
//!
//! Inlining a static method preserves behavior provided no class initializer with an effect
//! would have run on the call (the holder and its superclasses have no `<clinit>`, or only one
//! filling their own statics with constants) and everything the body
//! references is accessible from the caller (checked per call site, resolving inherited
//! members). Outlines that call outlines are inlined callee-first, so every copy is final. A
//! hand-written helper can look like an outline, so these rules are D: the result is the same
//! program, just more readable.

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
/// them would be correct but less readable.
///
/// A static call initializes the holder and its superclasses, so none may have a `<clinit>`
/// with an observable effect: only one that fills the class's own static fields with constants
/// and constant arrays (R8 also puts outlines in e.g. an enum-unboxing utility holding such a
/// table). A holder with program subclasses is skipped: its statics could be referenced through
/// them.
fn is_outline_holder(p: &Model, ci: usize, subclassed: &std::collections::BTreeSet<&str>) -> bool {
    let c = &p.classes[ci];
    if c.access & access::SYNTHETIC == 0 || c.access & access::INTERFACE != 0 || subclassed.contains(p.syms.get(c.ty)) {
        return false;
    }
    let mut cur = Some(ci);
    let mut depth = 0;
    while let Some(i) = cur {
        let k = &p.classes[i];
        let impure = k.methods.iter().any(|m| p.syms.get(m.name) == "<clinit>" && !m.code.as_ref().is_some_and(|b| pure_clinit(p, k.ty, b)));
        if impure || depth > 64 {
            return false;
        }
        cur = k.superclass.and_then(|s| p.find(p.syms.get(s)));
        depth += 1;
    }
    true
}

/// A class initializer whose only effect is storing constants and constant arrays into the
/// class's own static fields: running it early, late or not at all is unobservable.
fn pure_clinit(p: &Model, owner: eightr_ir::sym::Sym, body: &Body) -> bool {
    body.tries.is_empty()
        && body.insns.iter().enumerate().all(|(at, x)| match &x.op {
            Op::Const { .. } | Op::ConstString { .. } | Op::Move { .. } | Op::FillArrayData { .. } | Op::ReturnVoid | Op::Nop => true,
            // Constant sizes only (a negative size would throw): the size register's only
            // definition before the allocation is a non-negative constant.
            Op::NewArray { size, .. } => {
                let mut defs = body.insns[..at].iter().filter(|y| y.op.def().is_some_and(|(d, w)| d == *size || (w && d + 1 == *size)));
                matches!((defs.next().map(|y| &y.op), defs.next()), (Some(Op::Const { value: eightr_ir::op::Const::Narrow(k), .. }), None) if *k >= 0)
            }
            Op::StaticPut { field, .. } => field.class == owner,
            _ => false,
        })
        && p.syms.get(owner).starts_with('L')
}

/// Not defined in the program (arrays by their element type).
fn is_library(p: &Model, desc: &str) -> bool {
    let base = desc.trim_start_matches('[');
    !base.starts_with('L') || p.find(base).is_none()
}

/// `sites_throw`: every call site throws the result (`throw f(msg)`).
fn classify(_p: &Model, body: &Body, sites_throw: bool) -> Option<Kind> {
    if !body.tries.is_empty() || body.insns.is_empty() {
        return None;
    }
    let last = &body.insns.last()?.op;
    // R8 ≥ 9 may also return the built exception for the call site to throw (`throw f(msg)`):
    // a throw outline too, once every call site is checked to throw the result.
    let returns_new = |src: &eightr_ir::op::Reg| {
        body.insns.iter().rev().skip(1).find(|i| i.op.def().is_some_and(|(r, _)| r == *src)).is_some_and(|i| matches!(i.op, Op::NewInstance { .. }))
    };
    let kind = match last {
        Op::Return { src, .. } if sites_throw && returns_new(src) => Kind::Throw,
        Op::Return { .. } | Op::ReturnVoid => Kind::Classic,
        Op::Throw { .. } => Kind::Throw,
        _ => return None,
    };
    let (mut work, mut calls, mut news) = (0, 0, 0);
    for (k, insn) in body.insns.iter().enumerate() {
        let is_last = k + 1 == body.insns.len();
        match &insn.op {
            Op::Invoke { .. } => {
                work += 1;
                calls += 1;
            }
            Op::NewInstance { .. } => {
                work += 1;
                news += 1;
            }
            Op::Binop { .. } | Op::Unop { .. } => work += 1,
            Op::Move { .. } | Op::MoveResult { .. } | Op::Const { .. } => {}
            Op::ConstString { .. } if kind == Kind::Throw => {}
            Op::Return { .. } | Op::ReturnVoid | Op::Throw { .. } if is_last => {}
            _ => return None, // branches, fields, arrays, casts, monitors, ...
        }
    }
    // A throw outline throws the exception it builds: the thrown register's (straight-line)
    // last definition is a `new-instance`.
    let throws_new = || {
        let (Some(Op::Throw { src }) | Some(Op::Return { src, .. })) = body.insns.last().map(|i| &i.op) else { return false };
        body.insns.iter().rev().skip(1).find(|i| i.op.def().is_some_and(|(r, _)| r == *src)).is_some_and(|i| matches!(i.op, Op::NewInstance { .. }))
    };
    let ok = match kind {
        // Program calls are checked by the caller (`app_calls`).
        Kind::Classic => work >= 3 && calls >= 1,
        Kind::Throw => news >= 1 && work >= 2 && throws_new(),
    };
    ok.then_some(kind)
}

/// Whether `body` calls or instantiates program classes (Object methods aside: R8's outliner
/// counts `x.hashCode()` as a library call on any type).
fn app_calls(p: &Model, body: &Body) -> bool {
    body.insns.iter().any(|i| match &i.op {
        Op::Invoke { method, .. } => {
            let object_method = matches!((p.syms.get(method.name), p.syms.get(method.proto)), ("hashCode", "()I") | ("equals", "(Ljava/lang/Object;)Z") | ("toString", "()Ljava/lang/String;"));
            !is_library(p, p.syms.get(method.class)) && !object_method
        }
        Op::NewInstance { ty, .. } => !is_library(p, p.syms.get(*ty)),
        _ => false,
    })
}

/// The lowest `min-api` among the input's D8/R8 build markers (`~~R8{"min-api":24,...}`).
fn min_api(p: &Model) -> Option<u32> {
    p.retained_strings
        .iter()
        .filter_map(|m| {
            let rest = &m[m.find("\"min-api\":")? + "\"min-api\":".len()..];
            rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        })
        .min()
}

fn package(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map_or("", |(p, _)| p)
}

/// Where a member reference resolves: a program class declaring it (with its flags), or the
/// library (the search left the program). `None`: not found anywhere in the program.
fn resolve_method(p: &Model, class: &str, name: eightr_ir::sym::Sym, proto: eightr_ir::sym::Sym, depth: u32) -> Option<Option<(String, u32)>> {
    let Some(i) = p.find(class) else { return Some(None) };
    let c = &p.classes[i];
    if let Some(m) = c.methods.iter().find(|m| m.name == name && m.proto == proto) {
        return Some(Some((class.to_string(), m.access)));
    }
    if depth > 64 {
        return None;
    }
    c.superclass.iter().chain(&c.interfaces).find_map(|s| resolve_method(p, p.syms.get(*s), name, proto, depth + 1))
}

/// Can code in `caller` (a class descriptor) reference everything `body` references? Library
/// references are accessible: the outline's holder, an unrelated app class, could make them.
/// Program classes must be public or in the caller's package; program members are checked
/// against their declaring class (protected ones conservatively as package-private).
fn accessible_from(p: &Model, caller: &str, body: &Body) -> bool {
    let class_ok = |desc: &str| {
        let base = desc.trim_start_matches('[');
        p.find(base).is_none_or(|i| p.classes[i].access & access::PUBLIC != 0 || package(base) == package(caller))
    };
    body.insns.iter().all(|insn| match &insn.op {
        Op::NewInstance { ty, .. } => class_ok(p.syms.get(*ty)),
        Op::Invoke { method, .. } => {
            let owner = p.syms.get(method.class);
            class_ok(owner)
                && match resolve_method(p, owner, method.name, method.proto, 0) {
                    None => false,
                    Some(None) => true,
                    Some(Some((declaring, flags))) => {
                        flags & access::PUBLIC != 0
                            || if flags & access::PRIVATE != 0 { declaring == caller } else { package(&declaring) == package(caller) }
                    }
                }
        }
        _ => true,
    })
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
        let subclassed: std::collections::BTreeSet<&str> = p.classes.iter().filter_map(|c| c.superclass).map(|t| p.syms.get(t)).collect();
        // R8 ≥ 9 also outlines calls of program methods. Below min-api 24, D8/R8 move default and
        // static interface methods into companion classes whose methods look the same; the build
        // marker's min-api rules them out.
        let companions_impossible = min_api(p).is_some_and(|a| a >= 24);
        // Candidate outlines: key → (class, method, kind).
        let mut outlines: BTreeMap<Key, (usize, usize, Kind)> = BTreeMap::new();
        for (ci, c) in p.classes.iter().enumerate() {
            if !is_outline_holder(p, ci, &subclassed) {
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
                // Every call site throws the result: a throw outline returning its exception.
                let thrown = |&(sci, smi, at): &(usize, usize, u32)| {
                    let insns = p.classes[sci].methods[smi].code.as_ref().map_or(&[][..], |b| &b.insns[..]);
                    let at = at as usize;
                    matches!((insns.get(at + 1).map(|i| &i.op), insns.get(at + 2).map(|i| &i.op)), (Some(Op::MoveResult { dst, .. }), Some(Op::Throw { src })) if dst == src)
                };
                if let Some(kind) = classify(p, body, sites[&k].iter().all(thrown)) {
                    if kind == Kind::Classic && app_calls(p, body) && !companions_impossible {
                        continue;
                    }
                    outlines.insert(k, (ci, mi, kind));
                }
            }
        }
        // Outlines calling outlines (a throw outline building its message with a classic one)
        // are inlined callee-first, so each body is final before it's copied. Cycles (never
        // produced by R8) are dropped.
        let calls_of = |p: &Model, ci: usize, mi: usize| -> Vec<Key> {
            p.classes[ci].methods[mi].code.iter().flat_map(|b| &b.insns).filter_map(|i| match &i.op {
                Op::Invoke { kind: InvokeKind::Static, method, .. } => Some(key(p, method)),
                _ => None,
            }).collect()
        };
        let mut order: Vec<Key> = Vec::new();
        {
            let deps: BTreeMap<Key, Vec<Key>> = outlines
                .iter()
                .map(|(k, &(ci, mi, _))| (k.clone(), calls_of(p, ci, mi).into_iter().filter(|d| outlines.contains_key(d)).collect()))
                .collect();
            let mut placed: std::collections::BTreeSet<Key> = std::collections::BTreeSet::new();
            loop {
                let ready: Vec<Key> = deps.iter().filter(|(k, d)| !placed.contains(*k) && d.iter().all(|x| placed.contains(x))).map(|(k, _)| k.clone()).collect();
                if ready.is_empty() {
                    break;
                }
                for k in ready {
                    placed.insert(k.clone());
                    order.push(k);
                }
            }
            outlines.retain(|k, _| placed.contains(k));
        }
        if outlines.is_empty() {
            return Ok(());
        }

        // Every outline call site, per caller.
        let mut per_caller: BTreeMap<(usize, usize), Vec<(u32, Key)>> = BTreeMap::new();
        for (k, list) in &sites {
            if outlines.contains_key(k) {
                for &(ci, mi, insn) in list {
                    per_caller.entry((ci, mi)).or_default().push((insn, k.clone()));
                }
            }
        }
        // Callers that are outlines go first, callee-first; then everyone else.
        let mut callers: Vec<(usize, usize)> = order.iter().map(|k| (outlines[k].0, outlines[k].1)).filter(|c| per_caller.contains_key(c)).collect();
        let first: std::collections::BTreeSet<(usize, usize)> = callers.iter().copied().collect();
        callers.extend(per_caller.keys().copied().filter(|c| !first.contains(c)));

        let mut inlined: BTreeMap<Key, usize> = BTreeMap::new();
        let mut refused: BTreeMap<Key, BTreeMap<String, usize>> = BTreeMap::new();
        for (ci, mi) in callers {
            let list = &per_caller[&(ci, mi)];
            let caller_class = p.syms.get(p.classes[ci].ty).to_string();
            // Current (final) bodies of the callees.
            let bodies: BTreeMap<&Key, Body> = list
                .iter()
                .map(|(_, k)| (k, p.classes[outlines[k].0].methods[outlines[k].1].code.clone().expect("has code")))
                .collect();
            let mut batch: Vec<(u32, &Body, &str)> = Vec::new();
            let mut keys: Vec<&Key> = Vec::new();
            for (i, k) in list {
                if accessible_from(p, &caller_class, &bodies[k]) {
                    batch.push((*i, &bodies[k], k.2.as_str()));
                    keys.push(k);
                } else {
                    *refused.entry(k.clone()).or_default().entry("Inaccessible".to_string()).or_default() += 1;
                }
            }
            let Some(caller) = p.classes[ci].methods[mi].code.as_mut() else { continue };
            let mut throws = false;
            for (k, res) in keys.into_iter().zip(inline_static_calls(caller, &batch)) {
                match res {
                    Ok(()) => {
                        throws |= outlines[k].2 == Kind::Throw;
                        *inlined.entry(k.clone()).or_default() += 1;
                    }
                    Err(e) => *refused.entry(k.clone()).or_default().entry(format!("{e:?}")).or_default() += 1,
                }
            }
            if throws {
                eightr_ir::edit::remove_unreachable(caller);
            }
        }

        // Delete outlines with no remaining callers; record everything.
        let mut delete: Vec<(usize, usize)> = Vec::new();
        for (k, &(ci, mi, kind)) in &outlines {
            let total = sites[k].len();
            let n = inlined.get(k).copied().unwrap_or(0);

            let rule = if kind == Kind::Throw { BU_OUTLINE_INLINE } else { OUTLINE_INLINE };
            let detail = if n == total {
                delete.push((ci, mi));
                format!("inlined at all {total} call sites; method removed")
            } else {
                let why: Vec<String> = refused.get(k).into_iter().flatten().map(|(r, c)| format!("{c} {r}")).collect();
                format!("inlined at {n} of {total} call sites; refused: {}", why.join(", "))
            };
            records.push(RewriteRecord { rule, item: format!("{}->{}{}", k.0, k.1, k.2), detail });
        }
        delete.sort_by(|a, b| b.cmp(a));
        let mut emptied = Vec::new();
        for (ci, mi) in delete {
            p.classes[ci].methods.remove(mi);
            emptied.push(p.syms.get(p.classes[ci].ty).to_string());
        }
        remove_emptied_holders(p, &emptied);
        Ok(())
    }
}

/// Removes holders this rewrite emptied, if nothing else mentions them: no members,
/// annotations or interfaces left, no other class mentions their type (structurally, see
/// [`eightr_ir::refs::class_types`]), and no reflective lookup names them.
fn remove_emptied_holders(p: &mut Model, emptied: &[String]) {
    use std::collections::BTreeSet;
    let empty: BTreeSet<&str> = emptied
        .iter()
        .map(String::as_str)
        .filter(|d| {
            p.find(d).is_some_and(|i| {
                let c = &p.classes[i];
                c.fields.is_empty() && c.methods.is_empty() && c.annotations.is_empty() && c.interfaces.is_empty()
            })
        })
        .collect();
    if empty.is_empty() {
        return;
    }
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for c in &p.classes {
        let own = p.syms.get(c.ty);
        eightr_ir::refs::class_types(p, c, |d| {
            if d != own && empty.contains(d) {
                referenced.insert(d.to_string());
            }
        });
    }
    // Reflective lookups (`Class.forName("a.b.C")`) with a constant name.
    for site in eightr_ir::reflect::sites(p) {
        if site.kind == eightr_ir::reflect::Kind::Class {
            let desc = format!("L{};", site.name.replace('.', "/"));
            if empty.contains(desc.as_str()) {
                referenced.insert(desc);
            }
        }
    }
    let remove: BTreeSet<String> = empty.iter().filter(|d| !referenced.contains(**d)).map(|d| d.to_string()).collect();
    p.classes.retain(|c| !remove.contains(p.syms.get(c.ty)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use eightr_ir::lift::Insn;
    use eightr_ir::model::{Class, Method};
    use eightr_ir::op::{BinOp, NumType, Operand, Width};

    fn body(registers: u16, ins: u16, ops: Vec<Op>) -> Body {
        Body {
            registers,
            ins,
            outs: 0,
            insns: ops.into_iter().enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(),
            tries: vec![],
            positions: vec![],
            locals: vec![],
            parameter_names: vec![],
        }
    }

    fn class(p: &mut Model, ty: &str, access: u32, methods: Vec<Method>) -> Class {
        Class {
            ty: p.syms.intern(ty),
            access,
            superclass: Some(p.syms.intern("Ljava/lang/Object;")),
            interfaces: vec![],
            source_file: None,
            annotations: vec![],
            fields: vec![],
            methods,
            origin: 0,
        }
    }

    fn meth(p: &mut Model, name: &str, proto: &str, access: u32, code: Option<Body>) -> Method {
        Method { name: p.syms.intern(name), proto: p.syms.intern(proto), access, code, annotations: vec![], parameter_annotations: None }
    }

    fn mref(p: &mut Model, class: &str, name: &str, proto: &str) -> MethodRef {
        MethodRef { class: p.syms.intern(class), name: p.syms.intern(name), proto: p.syms.intern(proto) }
    }

    const ST: u32 = access::STATIC | access::PUBLIC;

    /// `(II)I` caller: `return m(p0, p1)`.
    fn calls(m: MethodRef) -> Body {
        body(3, 2, vec![
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![1, 2] },
            Op::MoveResult { width: Width::Single, dst: 0 },
            Op::Return { width: Width::Single, src: 0 },
        ])
    }

    /// `(II)I` outline-shaped body: `Integer.hashCode(p0) + p1) * p1` (library call + arithmetic).
    fn outline_body(p: &mut Model) -> Body {
        let hash = mref(p, "Ljava/lang/Integer;", "hashCode", "(I)I");
        body(3, 2, vec![
            Op::Invoke { kind: InvokeKind::Static, method: hash, args: vec![1] },
            Op::MoveResult { width: Width::Single, dst: 0 },
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(2) },
            Op::Binop { op: BinOp::Mul, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(2) },
            Op::Return { width: Width::Single, src: 0 },
        ])
    }

    /// Every invoke of a program class names a method that still exists.
    fn assert_no_dangling_calls(p: &Model) {
        for c in &p.classes {
            for m in c.methods.iter().filter_map(|m| m.code.as_ref()) {
                for i in &m.insns {
                    if let Op::Invoke { method, .. } = &i.op {
                        if let Some(k) = p.find(p.syms.get(method.class)) {
                            assert!(
                                p.classes[k].methods.iter().any(|x| x.name == method.name && x.proto == method.proto),
                                "{} calls deleted {}->{}",
                                p.syms.get(c.ty),
                                p.syms.get(method.class),
                                p.syms.get(method.name)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn inlines_a_library_only_outline_and_removes_its_holder() {
        let mut p = Model::default();
        let o = mref(&mut p, "LH;", "o", "(II)I");
        let ob = outline_body(&mut p);
        let mo = meth(&mut p, "o", "(II)I", ST, Some(ob));
        let h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![mo]);
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(o)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(o)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2]);
        p.classes = vec![h, u];
        let mut rec = vec![];
        OutlineInline.run(&mut p, &mut rec).unwrap();
        assert_eq!(rec.len(), 1);
        assert!(p.find("LH;").is_none(), "emptied holder should go");
        assert_no_dangling_calls(&p);
    }

    /// A synthetic static calling another synthetic static is app code, not an outline: neither
    /// is inlined, so no copy can call a deleted method (review finding).
    #[test]
    fn candidates_calling_program_code_are_not_outlines() {
        let mut p = Model::default();
        let a = mref(&mut p, "LH;", "a", "(II)I");
        let b = mref(&mut p, "LH;", "b", "(II)I");
        let bb = outline_body(&mut p);
        let ab = body(3, 2, vec![
            Op::Invoke { kind: InvokeKind::Static, method: b, args: vec![1, 2] },
            Op::MoveResult { width: Width::Single, dst: 0 },
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(1) },
            Op::Binop { op: BinOp::Mul, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(2) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        let ma = meth(&mut p, "a", "(II)I", ST, Some(ab));
        let mb = meth(&mut p, "b", "(II)I", ST, Some(bb));
        let h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![ma, mb]);
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(a)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(a)));
        let c3 = meth(&mut p, "c3", "(II)I", ST, Some(calls(b)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2, c3]);
        p.classes = vec![h, u];
        let mut rec = vec![];
        OutlineInline.run(&mut p, &mut rec).unwrap();
        assert!(rec.iter().all(|r| !r.item.contains("->a(")), "{rec:?}");
        assert_no_dangling_calls(&p);
    }

    /// Only holders this rewrite emptied are removed (review finding).
    #[test]
    fn unrelated_empty_class_is_kept() {
        let mut p = Model::default();
        let e = class(&mut p, "LEmpty;", access::PUBLIC, vec![]);
        let o = mref(&mut p, "LH;", "o", "(II)I");
        let ob = outline_body(&mut p);
        let mo = meth(&mut p, "o", "(II)I", ST, Some(ob));
        let h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![mo]);
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(o)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(o)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2]);
        p.classes = vec![e, h, u];
        OutlineInline.run(&mut p, &mut vec![]).unwrap();
        assert!(p.find("LEmpty;").is_some());
        assert!(p.find("LH;").is_none());
    }

    /// An emptied holder looked up reflectively stays.
    #[test]
    fn holder_looked_up_reflectively_is_kept() {
        let mut p = Model::default();
        let o = mref(&mut p, "La/H;", "o", "(II)I");
        let ob = outline_body(&mut p);
        let mo = meth(&mut p, "o", "(II)I", ST, Some(ob));
        let h = class(&mut p, "La/H;", access::PUBLIC | access::SYNTHETIC, vec![mo]);
        let name = p.syms.intern("a.H");
        let for_name = mref(&mut p, "Ljava/lang/Class;", "forName", "(Ljava/lang/String;)Ljava/lang/Class;");
        let r = meth(&mut p, "r", "()V", ST, Some(body(1, 0, vec![
            Op::ConstString { dst: 0, value: name },
            Op::Invoke { kind: InvokeKind::Static, method: for_name, args: vec![0] },
            Op::ReturnVoid,
        ])));
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(o)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(o)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2, r]);
        p.classes = vec![h, u];
        p.sort();
        OutlineInline.run(&mut p, &mut vec![]).unwrap();
        assert!(p.find("La/H;").is_some());
    }

    /// A holder whose superclass has a static initializer isn't an outline holder: the call
    /// would have run it (review finding).
    #[test]
    fn superclass_initializer_blocks_inlining() {
        let mut p = Model::default();
        // An initializer with an observable effect (a pure one, filling its own statics with
        // constants, would not block).
        let gc = mref(&mut p, "Ljava/lang/System;", "gc", "()V");
        let clinit = meth(
            &mut p,
            "<clinit>",
            "()V",
            access::STATIC | access::CONSTRUCTOR,
            Some(body(0, 0, vec![Op::Invoke { kind: InvokeKind::Static, method: gc, args: vec![] }, Op::ReturnVoid])),
        );
        let base = class(&mut p, "LBase;", access::PUBLIC, vec![clinit]);
        let o = mref(&mut p, "LH;", "o", "(II)I");
        let ob = outline_body(&mut p);
        let mo = meth(&mut p, "o", "(II)I", ST, Some(ob));
        let mut h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![mo]);
        h.superclass = Some(p.syms.intern("LBase;"));
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(o)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(o)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2]);
        p.classes = vec![base, h, u];
        let mut rec = vec![];
        OutlineInline.run(&mut p, &mut rec).unwrap();
        assert!(rec.is_empty(), "{rec:?}");
    }

    /// A holder whose initializer only fills its own static with a constant array doesn't block
    /// inlining; one whose array size register is redefined (maybe negative) does.
    #[test]
    fn pure_initializer_allows_inlining_only_with_one_constant_size() {
        for (sizes, inlined) in [(vec![5], true), (vec![5, -1], false)] {
            let mut p = Model::default();
            let f = eightr_ir::op::FieldRef { class: p.syms.intern("LH;"), name: p.syms.intern("t"), ty: p.syms.intern("[I") };
            let ty = p.syms.intern("[I");
            let mut ops: Vec<Op> = sizes.iter().map(|&k| Op::Const { dst: 0, value: eightr_ir::op::Const::Narrow(k) }).collect();
            ops.push(Op::NewArray { dst: 1, size: 0, ty });
            ops.push(Op::StaticPut { kind: eightr_ir::op::MemKind::Object, src: 1, field: f });
            ops.push(Op::ReturnVoid);
            let clinit = meth(&mut p, "<clinit>", "()V", access::STATIC | access::CONSTRUCTOR, Some(body(2, 0, ops)));
            let o = mref(&mut p, "LH;", "o", "(II)I");
            let ob = outline_body(&mut p);
            let mo = meth(&mut p, "o", "(II)I", ST, Some(ob));
            let h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![clinit, mo]);
            let c1 = meth(&mut p, "c1", "(II)I", ST, Some(calls(o)));
            let c2 = meth(&mut p, "c2", "(II)I", ST, Some(calls(o)));
            let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2]);
            p.classes = vec![h, u];
            let mut rec = vec![];
            OutlineInline.run(&mut p, &mut rec).unwrap();
            assert_eq!(!rec.is_empty(), inlined, "sizes {sizes:?}: {rec:?}");
        }
    }

    /// Backport-shaped (pure arithmetic) and too-small bodies aren't classic outlines.
    #[test]
    fn classic_outlines_need_a_library_call_and_three_operations() {
        let mut p = Model::default();
        let arith = body(3, 2, vec![
            Op::Binop { op: BinOp::Xor, ty: NumType::Int, dst: 0, a: 1, b: Operand::Reg(2) },
            Op::Binop { op: BinOp::Ushr, ty: NumType::Int, dst: 0, a: 0, b: Operand::Lit(16) },
            Op::Binop { op: BinOp::Xor, ty: NumType::Int, dst: 0, a: 0, b: Operand::Reg(1) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        assert_eq!(classify(&p, &arith, false), None);
        let ctor = mref(&mut p, "Ljava/lang/Object;", "<init>", "()V");
        let obj = p.syms.intern("Ljava/lang/Object;");
        let api_outline = body(1, 0, vec![
            Op::NewInstance { dst: 0, ty: obj },
            Op::Invoke { kind: InvokeKind::Direct, method: ctor, args: vec![0] },
            Op::Return { width: Width::Object, src: 0 },
        ]);
        assert_eq!(classify(&p, &api_outline, false), None);
        let ob = outline_body(&mut p);
        assert_eq!(classify(&p, &ob, false), Some(Kind::Classic));
    }

    /// A throw outline that builds its message with a classic outline: both are inlined,
    /// callee-first, so no copy calls a deleted method.
    #[test]
    fn nested_outlines_inline_callee_first() {
        let mut p = Model::default();
        let msg = mref(&mut p, "LH;", "msg", "(II)I");
        let fail = mref(&mut p, "LH;", "fail", "(II)V");
        let ctor = mref(&mut p, "Ljava/lang/IllegalStateException;", "<init>", "(I)V");
        let ise = p.syms.intern("Ljava/lang/IllegalStateException;");
        let mb = outline_body(&mut p);
        let fb = body(4, 2, vec![
            Op::Invoke { kind: InvokeKind::Static, method: msg, args: vec![2, 3] },
            Op::MoveResult { width: Width::Single, dst: 1 },
            Op::NewInstance { dst: 0, ty: ise },
            Op::Invoke { kind: InvokeKind::Direct, method: ctor, args: vec![0, 1] },
            Op::Throw { src: 0 },
        ]);
        let m_msg = meth(&mut p, "msg", "(II)I", ST, Some(mb));
        let m_fail = meth(&mut p, "fail", "(II)V", ST, Some(fb));
        let h = class(&mut p, "LH;", access::PUBLIC | access::SYNTHETIC, vec![m_fail, m_msg]);
        let thrower = |m: MethodRef| body(3, 2, vec![
            Op::Invoke { kind: InvokeKind::Static, method: m, args: vec![1, 2] },
            Op::Const { dst: 0, value: eightr_ir::op::Const::Narrow(0) },
            Op::Return { width: Width::Single, src: 0 },
        ]);
        let c1 = meth(&mut p, "c1", "(II)I", ST, Some(thrower(fail)));
        let c2 = meth(&mut p, "c2", "(II)I", ST, Some(thrower(fail)));
        let c3 = meth(&mut p, "c3", "(II)I", ST, Some(calls(msg)));
        let u = class(&mut p, "LU;", access::PUBLIC, vec![c1, c2, c3]);
        p.classes = vec![h, u];
        let mut rec = vec![];
        OutlineInline.run(&mut p, &mut rec).unwrap();
        assert_eq!(rec.len(), 2, "{rec:?}");
        assert!(rec.iter().all(|r| r.detail.contains("method removed")), "{rec:?}");
        assert!(p.find("LH;").is_none());
        assert_no_dangling_calls(&p);
    }

    /// A package-private method inherited through a public subclass isn't accessible from
    /// another package (review finding).
    #[test]
    fn inherited_package_private_member_is_not_accessible() {
        let mut p = Model::default();
        let base_m = meth(&mut p, "f", "()V", 0, Some(body(1, 1, vec![Op::ReturnVoid])));
        let base = class(&mut p, "La/Base;", access::PUBLIC, vec![base_m]);
        let mut sub = class(&mut p, "La/Sub;", access::PUBLIC, vec![]);
        sub.superclass = Some(p.syms.intern("La/Base;"));
        p.classes = vec![base, sub];
        p.sort();
        let f = mref(&mut p, "La/Sub;", "f", "()V");
        let callee = body(1, 1, vec![Op::Invoke { kind: InvokeKind::Virtual, method: f, args: vec![0] }, Op::ReturnVoid]);
        assert!(!accessible_from(&p, "Lb/Caller;", &callee));
        assert!(accessible_from(&p, "La/Caller;", &callee));
    }
}
