//! `r8/split-merged-class`: split classes R8's horizontal class merger combined (merged
//! siblings and lambda groups) back into one class per original (docs/sources/r8-desugar.md
//! §4.1).
//!
//! R8 merges classes of the same shape into one, adding a synthetic `$r8$classId` field that
//! each constructor stores (from a constant or a constructor argument), and switch dispatch on
//! it in every method whose behavior differed. Detection is structural:
//! * an instance field of type `byte`/`short`/`int`, synthetic and final, written only by the
//!   class's own constructors, once each, from a constant or one of their parameters (never
//!   delegating to another constructor of the class);
//! * every instantiation (`new-instance C` + `invoke-direct C.<init>`) passes a constant id;
//! * no program subclass, no `const-class C` (identity or reflection), and no `getClass()` with
//!   a used result on a value typed `C`.
//!
//! The split keeps `C` as an abstract base (its fields, constructors, statics and methods that
//! don't depend on the id), adds one final subclass per id seen at instantiation sites whose
//! constructor supplies the id, and overrides every method reading the id with a copy
//! specialized to that id (the dispatch folded away). The base still stores the id, so reads
//! anywhere else stay correct. Subclass names never use the id: naming is structural.
//!
//! S facts (reported): at least as many classes were merged as there are ids, and each
//! override is exactly that id's behavior. The split form itself is D.

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::cfg::Cfg;
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::lift::{Body, Insn};
use eightr_ir::model::{Class, Method, Program as Model};
use eightr_ir::op::{Const, FieldRef, InvokeKind, MethodRef, Op, Reg, Width};
use eightr_ir::sym::Sym;
use eightr_ir::types::parse_proto;
use eightr_rules::{Source, SPLIT_MERGED_CLASS};

use super::{Rewrite, RewriteRecord};
use crate::error::Result;

pub struct SplitMerged;

/// Where a constructor gets the class id from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdSource {
    /// Argument word (0 = `this`).
    Param(usize),
    Const(i32),
}

/// One instantiation: `new-instance` at `new_at` and `invoke-direct <init>` at `init_at`.
#[derive(Debug, Clone)]
struct Site {
    class: usize,
    method: usize,
    new_at: u32,
    init_at: u32,
    ctor: usize,
    id: i32,
}

struct Plan {
    class: usize,
    field: FieldRef,
    ctors: BTreeMap<usize, IdSource>,
    sites: Vec<Site>,
}

/// Instructions defining `reg` before `at`, following register copies back to their source.
/// `None` when a definition can't be followed (merges of different sources count as several).
fn origins(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> Vec<DefSite> {
    let mut out = Vec::new();
    let mut stack = vec![(at, reg)];
    let mut seen = BTreeSet::new();
    while let Some((i, r)) = stack.pop() {
        if !seen.insert((i, r)) {
            continue;
        }
        let Some(uses) = rd.uses.get(i as usize).and_then(|u| u.as_ref()) else { continue };
        for (ur, defs) in uses {
            if *ur != r {
                continue;
            }
            for &d in defs {
                let def = rd.defs[d];
                match def.site {
                    DefSite::Insn(j) => match &body.insns[j as usize].op {
                        Op::Move { src, .. } => stack.push((j, *src)),
                        _ => out.push(def.site),
                    },
                    DefSite::Param => out.push(DefSite::Param),
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Does the value `def_at` defines in `reg` only feed branch operands (through copies)?
fn only_branch_uses(body: &Body, rd: &ReachingDefs, def_at: u32, reg: Reg, depth: u32) -> bool {
    let Some(d) = rd.defs.iter().position(|x| x.site == DefSite::Insn(def_at) && x.reg == reg) else { return false };
    for (j, insn) in body.insns.iter().enumerate() {
        let Some(uses) = rd.uses[j].as_ref() else { continue };
        if !uses.iter().any(|(r, defs)| *r == reg && defs.contains(&d)) {
            continue;
        }
        let ok = match &insn.op {
            Op::If { .. } | Op::IfZ { .. } | Op::Switch { .. } => true,
            Op::Move { dst, src, .. } if *src == reg && depth > 0 => only_branch_uses(body, rd, j as u32, *dst, depth - 1),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// The value of `reg` at `at` if every definition reaching it is the same narrow constant.
fn const_value(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> Option<i32> {
    let o = origins(body, rd, at, reg);
    let mut v = None;
    for d in &o {
        let DefSite::Insn(j) = d else { return None };
        let Op::Const { value: Const::Narrow(k), .. } = body.insns[*j as usize].op else { return None };
        if v.is_some_and(|x| x != k) {
            return None;
        }
        v = Some(k);
    }
    v
}

/// Is `reg` at `at` the method's receiver (`this`, never reassigned on any path)?
fn is_this(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> bool {
    let this = body.registers - body.ins;
    reg == this && origins(body, rd, at, reg) == [DefSite::Param]
}

fn analyze(body: &Body) -> Option<ReachingDefs> {
    let cfg = Cfg::build(body).ok()?;
    Some(ReachingDefs::compute(body, &cfg))
}

fn is_ctor(p: &Model, m: &Method) -> bool {
    m.access & access::STATIC == 0 && p.syms.get(m.name) == "<init>"
}

/// Argument word index of each parameter register of a method with `ins` words.
fn arg_word(body: &Body, reg: Reg) -> Option<usize> {
    let first = body.registers - body.ins;
    (reg >= first).then(|| usize::from(reg - first))
}

/// Finds the class id field and its constructors' id sources, if `C` is a merged class.
/// Every field of `C` that could be its class id, with its constructors' id sources.
fn id_fields(p: &Model, ci: usize) -> Vec<(FieldRef, BTreeMap<usize, IdSource>)> {
    let mut out = Vec::new();
    let c = &p.classes[ci];
    if c.access & (access::INTERFACE | access::ABSTRACT | access::ANNOTATION | access::ENUM) != 0 {
        return out;
    }
    'field: for f in &c.fields {
        let ty = p.syms.get(f.ty);
        let wanted = access::SYNTHETIC | access::FINAL;
        if f.access & access::STATIC != 0 || f.access & wanted != wanted || !matches!(ty, "B" | "S" | "I") {
            continue;
        }
        let fref = FieldRef { class: c.ty, name: f.name, ty: f.ty };
        let mut ctors = BTreeMap::new();
        let mut delegations: Vec<(usize, usize)> = Vec::new();
        for (mi, m) in c.methods.iter().enumerate() {
            if !is_ctor(p, m) {
                continue;
            }
            let Some(body) = m.code.as_ref() else { return out };
            let Ok(cfg) = Cfg::build(body) else { return out };
            let rd = ReachingDefs::compute(body, &cfg);
            let entry = cfg.rpo.first().copied().unwrap_or(0);
            let this = body.registers - body.ins;
            let mut src = None;
            // `this` escaped (passed to a call other than a constructor) before the id store?
            let mut escaped = false;
            for (i, insn) in body.insns.iter().enumerate() {
                let i = i as u32;
                match &insn.op {
                    Op::InstancePut { src: v, obj, field, .. } if *field == fref => {
                        // The store must run on every path, before anything could read the id:
                        // in the entry block, before `this` escapes.
                        if src.is_some() || !is_this(body, &rd, i, *obj) || cfg.block_of[i as usize] != entry || escaped {
                            continue 'field;
                        }
                        let o = origins(body, &rd, i, *v);
                        src = match o.as_slice() {
                            [DefSite::Param] => arg_word(body, *v).map(IdSource::Param),
                            _ => const_value(body, &rd, i, *v).map(IdSource::Const),
                        };
                        if src.is_none() {
                            continue 'field;
                        }
                    }
                    // Delegating to another constructor of the class (on `this`); allowed from a
                    // constructor that stores the id itself to one that doesn't (checked below).
                    // (`new C(..)` inside a constructor is an ordinary site.)
                    Op::Invoke { kind: InvokeKind::Direct, method, args } if method.class == c.ty && p.syms.get(method.name) == "<init>" && is_this(body, &rd, i, args[0]) => {
                        let Some(to) = c.methods.iter().position(|x| x.name == method.name && x.proto == method.proto) else { continue 'field };
                        delegations.push((mi, to));
                    }
                    Op::Invoke { method, args, .. } if p.syms.get(method.name) != "<init>" && args.contains(&this) => escaped = true,
                    _ => {}
                }
            }
            if let Some(src) = src {
                ctors.insert(mi, src);
            }
        }
        // Constructors that don't store the id may only be reached by delegation from one
        // that does (R8 adds a dummy parameter to keep merged constructors apart); sites that
        // call them directly are refused later (no id source).
        if ctors.is_empty() || delegations.iter().any(|(from, to)| !ctors.contains_key(from) || ctors.contains_key(to)) {
            continue;
        }
        out.push((fref, ctors));
    }
    out
}

impl Rewrite for SplitMerged {
    fn name(&self) -> &'static str {
        // Runs after `outline-inline` (rewrites of one source run by name): R8 outlines after
        // merging, so outlines are undone first.
        "split-merged-class"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, p: &mut Model, records: &mut Vec<RewriteRecord>) -> Result<()> {
        let subclassed: BTreeSet<&str> = p.classes.iter().filter_map(|c| c.superclass).map(|t| p.syms.get(t)).collect();
        let mut plans: Vec<Plan> = Vec::new();
        // Java serialization names the class in streams and skips constructors: leave those.
        let serializable = |ci: usize| {
            let mut stack = vec![p.classes[ci].ty];
            let mut seen = BTreeSet::new();
            while let Some(t) = stack.pop() {
                let d = p.syms.get(t);
                if d == "Ljava/io/Serializable;" {
                    return true;
                }
                if !seen.insert(d.to_string()) {
                    continue;
                }
                if let Some(i) = p.find(d) {
                    stack.extend(p.classes[i].superclass.iter().chain(&p.classes[i].interfaces).copied());
                }
            }
            false
        };
        for ci in 0..p.classes.len() {
            if subclassed.contains(p.syms.get(p.classes[ci].ty)) || serializable(ci) {
                continue;
            }
            for (field, ctors) in id_fields(p, ci) {
                plans.push(Plan { class: ci, field, ctors, sites: Vec::new() });
            }
        }
        if plans.is_empty() {
            return Ok(());
        }
        // R8's class id is only ever a dispatch key: every read of it, anywhere, feeds only
        // `if`/`switch` operands. (A captured int that happens to be constant at every
        // instantiation is used as a value.)
        {
            let plans_of = |f: &FieldRef| -> Vec<usize> { plans.iter().enumerate().filter(|(_, pl)| pl.field == *f).map(|(k, _)| k).collect() };
            let mut bad: BTreeSet<usize> = BTreeSet::new();
            for c in &p.classes {
                for m in &c.methods {
                    let Some(body) = &m.code else { continue };
                    let reads: Vec<(u32, Reg, Vec<usize>)> = body
                        .insns
                        .iter()
                        .enumerate()
                        .filter_map(|(i, x)| match &x.op {
                            Op::InstanceGet { dst, field, .. } => Some((i as u32, *dst, plans_of(field))).filter(|r| !r.2.is_empty()),
                            _ => None,
                        })
                        .collect();
                    if reads.is_empty() {
                        continue;
                    }
                    let Some(rd) = analyze(body) else {
                        bad.extend(reads.iter().flat_map(|r| r.2.iter().copied()));
                        continue;
                    };
                    for (i, dst, ks) in reads {
                        if !only_branch_uses(body, &rd, i, dst, 4) {
                            bad.extend(ks.iter().copied());
                        }
                    }
                }
            }
            if !bad.is_empty() {
                let keep: Vec<Plan> = plans.drain(..).enumerate().filter(|(k, _)| !bad.contains(k)).map(|(_, pl)| pl).collect();
                plans = keep;
            }
            if plans.is_empty() {
                return Ok(());
            }
        }
        // Candidate plans per class (a class may have several id-shaped fields).
        let mut by_type: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (k, pl) in plans.iter().enumerate() {
            by_type.entry(p.syms.get(p.classes[pl.class].ty)).or_default().push(k);
        }
        let mut rejected: BTreeSet<usize> = BTreeSet::new();
        let mut news: BTreeMap<usize, usize> = BTreeMap::new(); // plan → new-instance count
        let get_class = |p: &Model, m: &MethodRef| p.syms.get(m.name) == "getClass" && p.syms.get(m.proto) == "()Ljava/lang/Class;";

        // One pass over all code: instantiation sites, identity observations.
        for (ci, c) in p.classes.iter().enumerate() {
            for (mi, m) in c.methods.iter().enumerate() {
                let Some(body) = &m.code else { continue };
                let touches = body.insns.iter().any(|i| match &i.op {
                    Op::NewInstance { ty, .. } | Op::ConstClass { ty, .. } => by_type.contains_key(p.syms.get(*ty)),
                    Op::Invoke { method, .. } => by_type.contains_key(p.syms.get(method.class)) || get_class(p, method),
                    _ => false,
                });
                if !touches {
                    continue;
                }
                let rd = analyze(body);
                for (i, insn) in body.insns.iter().enumerate() {
                    let i = i as u32;
                    match &insn.op {
                        Op::ConstClass { ty, .. } => {
                            rejected.extend(by_type.get(p.syms.get(*ty)).into_iter().flatten());
                        }
                        Op::NewInstance { ty, .. } => {
                            for &k in by_type.get(p.syms.get(*ty)).into_iter().flatten() {
                                *news.entry(k).or_default() += 1;
                            }
                        }
                        Op::Invoke { method, args, .. } if get_class(p, method) => {
                            // Only a used result can observe the class.
                            let used = matches!(body.insns.get(i as usize + 1).map(|x| &x.op), Some(Op::MoveResult { .. }));
                            if !used {
                                continue;
                            }
                            let Some(rd) = &rd else {
                                rejected.extend(by_type.values().flatten().copied());
                                continue;
                            };
                            // A syntactic approximation of "the value is typed as a merged class":
                            // its definition says so, or the method casts that register to it.
                            for x in body.insns.iter() {
                                if let Op::CheckCast { reg, ty } = &x.op {
                                    if *reg == args[0] {
                                        rejected.extend(by_type.get(p.syms.get(*ty)).into_iter().flatten());
                                    }
                                }
                            }
                            let param_types: Vec<String> = {
                                let mut v = Vec::new();
                                if m.access & access::STATIC == 0 {
                                    v.push(p.syms.get(c.ty).to_string());
                                }
                                for t in parse_proto(p.syms.get(m.proto)).map(|x| x.0).unwrap_or_default() {
                                    v.push(t.to_string());
                                    if matches!(t, "J" | "D") {
                                        v.push(String::new());
                                    }
                                }
                                v
                            };
                            for o in origins(body, rd, i, args[0]) {
                                let typed = match o {
                                    DefSite::Param => arg_word(body, args[0]).and_then(|w| param_types.get(w)).and_then(|t| by_type.get(t.as_str())),
                                    DefSite::Insn(j) => match &body.insns[j as usize].op {
                                        Op::NewInstance { ty, .. } => by_type.get(p.syms.get(*ty)),
                                        Op::InstanceGet { field, .. } | Op::StaticGet { field, .. } => by_type.get(p.syms.get(field.ty)),
                                        // An element of an array of the class.
                                        Op::ArrayGet { array, .. } => {
                                            let arrays: Vec<&str> = body.insns.iter().filter_map(|y| match &y.op {
                                                Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => Some(p.syms.get(*ty)),
                                                Op::StaticGet { field, .. } | Op::InstanceGet { field, .. } => Some(p.syms.get(field.ty)),
                                                _ => None,
                                            }).collect();
                                            let _ = array;
                                            arrays.iter().find_map(|a| a.strip_prefix('[').and_then(|e| by_type.get(e)))
                                        }
                                        Op::MoveResult { .. } => match body.insns.get(j as usize - 1).map(|x| &x.op) {
                                            Some(Op::Invoke { method, .. }) => parse_proto(p.syms.get(method.proto)).and_then(|(_, r)| by_type.get(r)),
                                            _ => None,
                                        },
                                        _ => None,
                                    },
                                };
                                rejected.extend(typed.into_iter().flatten());
                            }
                        }
                        Op::Invoke { kind: InvokeKind::Direct, method, args } if p.syms.get(method.name) == "<init>" => {
                            let Some(ks) = by_type.get(p.syms.get(method.class)) else { continue };
                            let Some(rd) = &rd else {
                                rejected.extend(ks);
                                continue;
                            };
                            for &k in ks {
                                let plan = &plans[k];
                                // Delegation between the class's own constructors (validated by
                                // `id_fields`), not an instantiation.
                                if ci == plan.class && is_ctor(p, m) && is_this(body, rd, i, args[0]) {
                                    continue;
                                }
                                let ctor = p.classes[plan.class].methods.iter().position(|x| x.name == method.name && x.proto == method.proto);
                                let new_at = match origins(body, rd, i, args[0]).as_slice() {
                                    [DefSite::Insn(j)] if matches!(body.insns[*j as usize].op, Op::NewInstance { ty, .. } if ty == method.class) => Some(*j),
                                    _ => None,
                                };
                                let id = ctor.and_then(|ctor| match plan.ctors.get(&ctor)? {
                                    IdSource::Const(v) => Some(*v),
                                    IdSource::Param(w) => args.get(*w).and_then(|&r| const_value(body, rd, i, r)),
                                });
                                match (ctor, new_at, id) {
                                    (Some(ctor), Some(new_at), Some(id)) => {
                                        plans[k].sites.push(Site { class: ci, method: mi, new_at, init_at: i, ctor, id });
                                    }
                                    _ => {
                                        rejected.insert(k);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // Every `new-instance` must pair with exactly one instantiation site.
        for (k, plan) in plans.iter().enumerate() {
            let sites_new: BTreeSet<(usize, usize, u32)> = plan.sites.iter().map(|s| (s.class, s.method, s.new_at)).collect();
            if sites_new.len() != plan.sites.len() || news.get(&k).copied().unwrap_or(0) != plan.sites.len() || plan.sites.is_empty() {
                rejected.insert(k);
            }
        }
        // Method handles or constant values naming the class: refuse.
        let handles = handle_refs(p);
        for (k, plan) in plans.iter().enumerate() {
            if handles.contains(p.syms.get(p.classes[plan.class].ty)) {
                rejected.insert(k);
            }
        }

        // A class with more than one field that works as its id is ambiguous: refused.
        for ks in by_type.values() {
            let ok: Vec<usize> = ks.iter().copied().filter(|k| !rejected.contains(k)).collect();
            if ok.len() > 1 {
                rejected.extend(ok);
            }
        }
        // Prepare every split first, then rewrite all instantiation sites (a site may sit in
        // another merged class's dispatching method), then build the subclasses from the
        // rewritten bodies.
        let prepared: Vec<(usize, Prepared)> =
            plans.iter().enumerate().filter(|(k, _)| !rejected.contains(k)).filter_map(|(k, plan)| prepare(p, plan).map(|x| (k, x))).collect();
        for (k, prep) in &prepared {
            rewrite_sites(p, &plans[*k], prep);
        }
        let mut new_classes: Vec<Class> = Vec::new();
        for (k, prep) in prepared {
            records.push(finish(p, &plans[k], prep, &mut new_classes));
        }
        p.classes.extend(new_classes);
        Ok(())
    }
}

/// Class descriptors named by method handles, call-site arguments, annotation values or
/// constant values.
fn handle_refs(p: &Model) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in &p.classes {
        for insn in c.methods.iter().filter_map(|m| m.code.as_ref()).flat_map(|b| &b.insns) {
            match &insn.op {
                Op::ConstMethodHandle { handle, .. } => {
                    if let eightr_ir::value::HandleMember::Method(m) = handle.member {
                        out.insert(p.syms.get(m.class).to_string());
                    }
                }
                Op::InvokeCustom { .. } | Op::InvokePolymorphic { .. } => {
                    // Their static arguments may name the class: collect every type.
                    eightr_ir::refs::op_types(p, &insn.op, |d| {
                        out.insert(d.to_string());
                    });
                }
                _ => {}
            }
        }
        for v in c.fields.iter().filter_map(|f| f.static_value.as_ref()) {
            eightr_ir::refs::value_types(p, v, |d| {
                out.insert(d.to_string());
            });
        }
        // Annotations (e.g. `@JsonSubTypes(C.class)`, then reflective instantiation).
        let anns = c
            .annotations
            .iter()
            .chain(c.fields.iter().flat_map(|f| &f.annotations))
            .chain(c.methods.iter().flat_map(|m| m.annotations.iter().chain(m.parameter_annotations.iter().flatten().flatten())));
        for a in anns {
            for (_, v) in &a.annotation.elements {
                eightr_ir::refs::value_types(p, v, |d| {
                    out.insert(d.to_string());
                });
            }
        }
    }
    out
}

fn width_of(desc: &str) -> Width {
    match desc.as_bytes().first() {
        Some(b'J' | b'D') => Width::Wide,
        Some(b'L' | b'[') => Width::Object,
        _ => Width::Single,
    }
}

struct Prepared {
    ids: BTreeSet<i32>,
    /// Base methods reading the id on `this`: overridden per id.
    dispatch: Vec<usize>,
    sub_of: BTreeMap<i32, Sym>,
    /// (id, base constructor) → (subclass constructor body, proto).
    ctors: BTreeMap<(i32, usize), (Body, Sym)>,
    /// Whether subclass constructors drop the id parameter (false when that would leave some
    /// instantiation's argument registers unencodable: they keep it and ignore it).
    drop_id: bool,
}

fn prepare(p: &mut Model, plan: &Plan) -> Option<Prepared> {
    let base = &p.classes[plan.class];
    let base_desc = p.syms.get(base.ty).to_string();
    let ids: BTreeSet<i32> = plan.sites.iter().map(|s| s.id).collect();
    let dispatch: Vec<usize> = base
        .methods
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.access & (access::STATIC | access::PRIVATE | access::CONSTRUCTOR | access::ABSTRACT) == 0
                && p.syms.get(m.name) != "<init>"
                && m.code.as_ref().is_some_and(|b| {
                    analyze(b).is_some_and(|rd| {
                        b.insns.iter().enumerate().any(|(i, x)| matches!(&x.op, Op::InstanceGet { field, obj, .. } if *field == plan.field && is_this(b, &rd, i as u32, *obj)))
                    })
                })
        })
        .map(|(i, _)| i)
        .collect();
    let used: BTreeSet<(i32, usize)> = plan.sites.iter().map(|s| (s.id, s.ctor)).collect();
    if used.iter().any(|&(_, ctor)| base.methods[ctor].access & access::PRIVATE != 0) {
        return None;
    }
    let inner = base_desc.strip_suffix(';').unwrap_or(&base_desc).to_string();
    let mut sub_of = BTreeMap::new();
    for &id in &ids {
        // A placeholder descriptor: naming replaces it structurally, so the id never shows.
        let tag = if id < 0 { format!("m{}", -(i64::from(id))) } else { id.to_string() };
        let desc = format!("{inner}$$Split{tag};");
        if p.find(&desc).is_some() {
            return None; // an existing class has that name
        }
        sub_of.insert(id, p.syms.intern(&desc));
    }
    let drop_id = plan.sites.iter().all(|s| match plan.ctors[&s.ctor] {
        IdSource::Const(_) => true,
        IdSource::Param(w) => match &p.classes[s.class].methods[s.method].code.as_ref().map(|b| &b.insns[s.init_at as usize].op) {
            Some(Op::Invoke { kind, method, args }) => {
                let mut args = args.clone();
                args.remove(w);
                Op::Invoke { kind: *kind, method: *method, args }.encodable()
            }
            _ => false,
        },
    });
    // Subclass constructors of one id must have distinct protos (dropping the id can make two
    // base constructors collide): keep the id parameter then, or give up.
    let build = |p: &mut Model, drop_id: bool| -> Option<BTreeMap<(i32, usize), (Body, Sym)>> {
        let mut ctors = BTreeMap::new();
        for &(id, ctor) in &used {
            ctors.insert((id, ctor), sub_ctor(p, plan, ctor, id, drop_id)?);
        }
        let mut seen = BTreeSet::new();
        let unique = ctors.iter().all(|((id, _), (_, proto))| seen.insert((*id, p.syms.get(*proto).to_string())));
        unique.then_some(ctors)
    };
    let (ctors, drop_id) = match build(p, drop_id) {
        Some(c) => (c, drop_id),
        None if drop_id => (build(p, false)?, false),
        None => return None,
    };
    Some(Prepared { ids, dispatch, sub_of, ctors, drop_id })
}

fn rewrite_sites(p: &mut Model, plan: &Plan, prep: &Prepared) {
    for s in &plan.sites {
        let sub = prep.sub_of[&s.id];
        let proto = prep.ctors[&(s.id, s.ctor)].1;
        let src = plan.ctors[&s.ctor];
        let Some(body) = p.classes[s.class].methods[s.method].code.as_mut() else { continue };
        if let Op::NewInstance { ty, .. } = &mut body.insns[s.new_at as usize].op {
            *ty = sub;
        }
        if let Op::Invoke { method, args, .. } = &mut body.insns[s.init_at as usize].op {
            method.class = sub;
            method.proto = proto;
            if let (IdSource::Param(w), true) = (src, prep.drop_id) {
                args.remove(w);
            }
        }
    }
}

/// Specialized copies move into subclasses, which can't reach the base's private members.
/// Widens those to package-private (as R8's access modification does): fields lose `private`;
/// methods lose it and become final, and their `invoke-direct` calls become `invoke-virtual`.
/// A method that would then override a supertype's method can't be widened: dispatching
/// methods that need it stay in the base, unspecialized, like those using `invoke-super`.
/// Returns the methods to specialize.
fn widen_privates(p: &mut Model, class: usize, dispatch: &[usize]) -> Vec<usize> {
    let ty = p.classes[class].ty;
    let supers = {
        let mut out: Vec<(Sym, Sym)> = Vec::new();
        let mut stack: Vec<Sym> = p.classes[class].superclass.iter().chain(&p.classes[class].interfaces).copied().collect();
        let mut seen = BTreeSet::new();
        while let Some(t) = stack.pop() {
            let d = p.syms.get(t).to_string();
            if !seen.insert(d.clone()) {
                continue;
            }
            if let Some(i) = p.find(&d) {
                let c = &p.classes[i];
                out.extend(c.methods.iter().filter(|m| m.access & access::PRIVATE == 0).map(|m| (m.name, m.proto)));
                stack.extend(c.superclass.iter().chain(&c.interfaces).copied());
            }
        }
        out
    };
    let package = |t: Sym| {
        let d = p.syms.get(t);
        d.rsplit_once('/').map_or("", |(a, _)| a).to_string()
    };
    let own_package = package(ty);
    let protected_elsewhere = |p: &Model, owner: Sym, is: &dyn Fn(&Method) -> bool| -> bool {
        package(owner) != own_package
            && p.find(p.syms.get(owner)).is_some_and(|i| p.classes[i].methods.iter().any(|m| is(m) && m.access & access::PROTECTED != 0))
    };
    let c = &p.classes[class];
    let private_field = |f: &FieldRef| f.class == ty && c.fields.iter().any(|x| x.name == f.name && x.ty == f.ty && x.access & access::PRIVATE != 0);
    let private_method = |m: &MethodRef| {
        if m.class != ty {
            return None;
        }
        c.methods.iter().position(|x| x.name == m.name && x.proto == m.proto && x.access & access::PRIVATE != 0)
    };
    let mut keep = Vec::new();
    let mut fields: Vec<(Sym, Sym)> = Vec::new();
    let mut methods: BTreeSet<usize> = BTreeSet::new();
    for &mi in dispatch {
        let (mut f, mut m, mut ok) = (Vec::new(), Vec::new(), true);
        for insn in c.methods[mi].code.iter().flat_map(|b| &b.insns) {
            match &insn.op {
                Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. } | Op::StaticPut { field, .. } if private_field(field) => {
                    f.push((field.name, field.ty));
                }
                // `invoke-super` resolves from the class holding the code: moved into a
                // subclass it would reach the base (whose method becomes abstract).
                Op::Invoke { kind: InvokeKind::Super, .. } => ok = false,
                // Protected members of another package are reachable from the base through
                // its own receivers only; a moved copy's receiver type changes. Keep it simple.
                Op::Invoke { method, .. } if protected_elsewhere(p, method.class, &|m: &Method| m.name == method.name && m.proto == method.proto) => ok = false,
                Op::Invoke { method, .. } => {
                    if let Some(k) = private_method(method) {
                        let x = &c.methods[k];
                        ok &= x.access & access::STATIC != 0 || !supers.contains(&(x.name, x.proto));
                        m.push(k);
                    }
                }
                _ => {}
            }
        }
        if ok {
            keep.push(mi);
            fields.extend(f);
            methods.extend(m);
        }
    }
    let c = &mut p.classes[class];
    for f in c.fields.iter_mut().filter(|x| fields.contains(&(x.name, x.ty))) {
        f.access &= !access::PRIVATE;
    }
    let mut devirt: Vec<(Sym, Sym)> = Vec::new();
    for &k in &methods {
        let m = &mut c.methods[k];
        m.access &= !access::PRIVATE;
        if m.access & access::STATIC == 0 {
            m.access |= access::FINAL;
            devirt.push((m.name, m.proto));
        }
    }
    for insn in c.methods.iter_mut().filter_map(|m| m.code.as_mut()).flat_map(|b| b.insns.iter_mut()) {
        if let Op::Invoke { kind, method, .. } = &mut insn.op {
            if *kind == InvokeKind::Direct && method.class == ty && devirt.contains(&(method.name, method.proto)) {
                *kind = InvokeKind::Virtual;
            }
        }
    }
    keep
}

fn finish(p: &mut Model, plan: &Plan, mut prep: Prepared, new_classes: &mut Vec<Class>) -> RewriteRecord {
    prep.dispatch = widen_privates(p, plan.class, &prep.dispatch);
    let init = p.syms.intern("<init>");
    let base = &p.classes[plan.class];
    let base_desc = p.syms.get(base.ty).to_string();
    for &id in &prep.ids {
        let mut methods = Vec::new();
        for (_, (body, proto)) in prep.ctors.iter().filter(|((cid, _), _)| *cid == id) {
            methods.push(Method { name: init, proto: *proto, access: access::PUBLIC | access::CONSTRUCTOR, code: Some(body.clone()), annotations: vec![], parameter_annotations: None });
        }
        for &mi in &prep.dispatch {
            let bm = &base.methods[mi];
            methods.push(Method {
                name: bm.name,
                proto: bm.proto,
                access: bm.access | access::FINAL,
                code: Some(specialize(bm.code.as_ref().expect("dispatching methods have code"), &plan.field, id)),
                annotations: vec![],
                parameter_annotations: None,
            });
        }
        new_classes.push(Class {
            ty: prep.sub_of[&id],
            // Not synthetic: each subclass stands for an original class (decompilers hide
            // synthetic classes, expecting to inline them as anonymous classes or lambdas).
            access: access::FINAL | (base.access & access::PUBLIC),
            superclass: Some(base.ty),
            interfaces: vec![],
            source_file: base.source_file,
            annotations: vec![],
            fields: vec![],
            methods,
            origin: base.origin,
        });
    }
    // The base becomes abstract; its dispatching methods, overridden everywhere, too.
    let base = &mut p.classes[plan.class];
    base.access = (base.access & !(access::FINAL | access::SYNTHETIC)) | access::ABSTRACT;
    for &mi in &prep.dispatch {
        let m = &mut base.methods[mi];
        m.access = (m.access & !(access::FINAL | access::SYNCHRONIZED | access::DECLARED_SYNCHRONIZED | access::NATIVE | access::STRICT)) | access::ABSTRACT;
        m.code = None;
    }
    let n = prep.ids.len();
    RewriteRecord {
        rule: SPLIT_MERGED_CLASS,
        item: base_desc,
        detail: format!(
            "split into {n} classes by class id ({} instantiation sites, {} dispatching method(s) specialized); at least {n} classes were merged (S)",
            plan.sites.len(),
            prep.dispatch.len()
        ),
    }
}

/// A subclass constructor for base constructor `ctor` and class id `id`: the same parameters
/// minus the id (when the base takes it as a parameter); it calls the base constructor with the
/// id constant. Returns the body and the proto.
fn sub_ctor(p: &mut Model, plan: &Plan, ctor: usize, id: i32, drop_id: bool) -> Option<(Body, Sym)> {
    let base = &p.classes[plan.class];
    let bm = &base.methods[ctor];
    let base_proto = p.syms.get(bm.proto).to_string();
    let (params, _) = parse_proto(&base_proto)?;
    let src = plan.ctors[&ctor];
    // Words of the base constructor's arguments (with `this`), and which param is the id.
    let mut words: Vec<(Width, Option<usize>)> = vec![(Width::Object, None)]; // (width, param index)
    for (pi, t) in params.iter().enumerate() {
        let w = width_of(t);
        words.push((w, Some(pi)));
        if w == Width::Wide {
            words.push((Width::Wide, None)); // high half
        }
    }
    let id_word = match src {
        IdSource::Param(w) => Some(w),
        IdSource::Const(_) => None,
    };
    // Keeping the id parameter: the subclass takes the base's parameters and ignores the id.
    let kept_id = if drop_id { None } else { id_word };
    let id_param = id_word.and_then(|w| words.get(w).and_then(|x| x.1));
    let sub_params: Vec<&str> = params.iter().enumerate().filter(|(i, _)| Some(*i) != id_param || kept_id.is_some()).map(|(_, t)| *t).collect();
    let proto = format!("({})V", sub_params.concat());
    let n_base = words.len() as u16;
    let n_sub = n_base - u16::from(id_word.is_some() && kept_id.is_none());
    let registers = n_base + n_sub;
    // Subclass argument registers start at n_base; copy them in base order to v0..n_base-1.
    let mut ops = Vec::new();
    let mut from = n_base;
    let mut j = 0usize;
    while j < words.len() {
        let (w, _) = words[j];
        if Some(j) == id_word {
            ops.push(Op::Const { dst: j as Reg, value: Const::Narrow(id) });
            j += 1;
            from += u16::from(kept_id.is_some()); // skip the ignored id argument
            continue;
        }
        ops.push(Op::Move { width: w, dst: j as Reg, src: from });
        let step = if w == Width::Wide { 2 } else { 1 };
        from += step;
        j += step as usize;
    }
    let args: Vec<Reg> = (0..n_base).collect();
    ops.push(Op::Invoke { kind: InvokeKind::Direct, method: MethodRef { class: base.ty, name: bm.name, proto: bm.proto }, args });
    ops.push(Op::ReturnVoid);
    let body = Body {
        registers,
        ins: n_sub,
        outs: n_base,
        insns: ops.into_iter().map(|op| Insn { pc: 0, op }).collect(),
        tries: vec![],
        positions: vec![],
        locals: vec![],
        parameter_names: vec![],
    };
    let proto = p.syms.intern(&proto);
    Some((body, proto))
}

/// `body` with reads of the id field on `this` replaced by `id`, and the dispatch folded.
fn specialize(body: &Body, field: &FieldRef, id: i32) -> Body {
    let mut b = body.clone();
    if let Some(rd) = analyze(&b) {
        let at: Vec<usize> = b
            .insns
            .iter()
            .enumerate()
            .filter(|(i, x)| matches!(&x.op, Op::InstanceGet { field: f, obj, .. } if f == field && is_this(&b, &rd, *i as u32, *obj)))
            .map(|(i, _)| i)
            .collect();
        for i in at {
            if let Op::InstanceGet { dst, .. } = b.insns[i].op {
                b.insns[i].op = Op::Const { dst, value: Const::Narrow(id) };
            }
        }
    }
    eightr_ir::edit::fold_constant_branches(&mut b);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use eightr_ir::op::{BinOp, NumType, Operand};

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

    fn meth(p: &mut Model, name: &str, proto: &str, access: u32, code: Option<Body>) -> Method {
        Method { name: p.syms.intern(name), proto: p.syms.intern(proto), access, code, annotations: vec![], parameter_annotations: None }
    }

    /// A lambda group `LG;` (id field `a:B`, capture `b:I`) implementing `get()I` as
    /// `id == 0 ? b + 1 : helper(b)`, created as `new G(n, 0)` and `new G(n, 1)` in `LU;`.
    /// `arm1` is the id-1 arm's call (a private helper of the group, or `super.hashCode()`).
    fn group(arm1: impl Fn(&mut Model) -> (Vec<Op>, Vec<Method>)) -> Model {
        let mut p = Model::default();
        let g = p.syms.intern("LG;");
        let id = FieldRef { class: g, name: p.syms.intern("a"), ty: p.syms.intern("B") };
        let cap = FieldRef { class: g, name: p.syms.intern("b"), ty: p.syms.intern("I") };
        let obj_init = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("<init>"), proto: p.syms.intern("()V") };
        let ctor = body(3, 3, vec![
            Op::InstancePut { kind: eightr_ir::op::MemKind::Byte, src: 2, obj: 0, field: id },
            Op::InstancePut { kind: eightr_ir::op::MemKind::Narrow, src: 1, obj: 0, field: cap },
            Op::Invoke { kind: InvokeKind::Direct, method: obj_init, args: vec![0] },
            Op::ReturnVoid,
        ]);
        let (arm, extra) = arm1(&mut p);
        let mut get = vec![
            Op::InstanceGet { kind: eightr_ir::op::MemKind::Byte, dst: 0, obj: 2, field: id },
            Op::InstanceGet { kind: eightr_ir::op::MemKind::Narrow, dst: 1, obj: 2, field: cap },
            Op::IfZ { cond: eightr_ir::op::Cond::Ne, a: 0, target: 4 },
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 1, a: 1, b: Operand::Lit(1) },
        ];
        get.push(Op::Return { width: Width::Single, src: 1 });
        let n = get.len() as u32;
        get[2] = Op::IfZ { cond: eightr_ir::op::Cond::Ne, a: 0, target: n };
        get.extend(arm);
        let mut methods = vec![
            meth(&mut p, "<init>", "(IB)V", access::PUBLIC | access::CONSTRUCTOR, Some(ctor)),
            meth(&mut p, "get", "()I", access::PUBLIC | access::FINAL, Some(body(3, 1, get))),
        ];
        methods.extend(extra);
        let fields = vec![
            eightr_ir::model::Field { name: id.name, ty: id.ty, access: access::PUBLIC | access::FINAL | access::SYNTHETIC, static_value: None, annotations: vec![] },
            eightr_ir::model::Field { name: cap.name, ty: cap.ty, access: access::PUBLIC | access::FINAL | access::SYNTHETIC, static_value: None, annotations: vec![] },
        ];
        let object = p.syms.intern("Ljava/lang/Object;");
        let gc = Class { ty: g, access: access::PUBLIC | access::FINAL | access::SYNTHETIC, superclass: Some(object), interfaces: vec![], source_file: None, annotations: vec![], fields, methods, origin: 0 };
        let g_init = MethodRef { class: g, name: p.syms.intern("<init>"), proto: p.syms.intern("(IB)V") };
        let make = |k: i32| {
            body(3, 1, vec![
                Op::NewInstance { dst: 0, ty: g },
                Op::Const { dst: 1, value: Const::Narrow(k) },
                Op::Invoke { kind: InvokeKind::Direct, method: g_init, args: vec![0, 2, 1] },
                Op::Return { width: Width::Object, src: 0 },
            ])
        };
        let u = p.syms.intern("LU;");
        let m0 = meth(&mut p, "m0", "(I)LG;", access::PUBLIC | access::STATIC, Some(make(0)));
        let m1 = meth(&mut p, "m1", "(I)LG;", access::PUBLIC | access::STATIC, Some(make(1)));
        let uc = Class { ty: u, access: access::PUBLIC, superclass: Some(object), interfaces: vec![], source_file: None, annotations: vec![], fields: vec![], methods: vec![m0, m1], origin: 0 };
        p.classes = vec![gc, uc];
        p.sort();
        p
    }

    fn run(p: &mut Model) -> Vec<RewriteRecord> {
        let mut rec = vec![];
        SplitMerged.run(p, &mut rec).unwrap();
        p.sort();
        rec
    }

    fn plain_arm(_: &mut Model) -> (Vec<Op>, Vec<Method>) {
        (vec![Op::Binop { op: BinOp::Mul, ty: NumType::Int, dst: 1, a: 1, b: Operand::Lit(3) }, Op::Return { width: Width::Single, src: 1 }], vec![])
    }

    #[test]
    fn splits_a_lambda_group_into_one_class_per_id() {
        let mut p = group(plain_arm);
        let rec = run(&mut p);
        assert_eq!(rec.len(), 1, "{rec:?}");
        let g = &p.classes[p.find("LG;").unwrap()];
        assert!(g.access & access::ABSTRACT != 0);
        let get = g.methods.iter().find(|m| p.syms.get(m.name) == "get").unwrap();
        assert!(get.code.is_none() && get.access & access::ABSTRACT != 0);
        let subs: Vec<&Class> = p.classes.iter().filter(|c| c.superclass == Some(g.ty)).collect();
        assert_eq!(subs.len(), 2);
        // Each override is its own arm, with the dispatch folded away.
        for s in subs {
            let body = s.methods.iter().find(|m| p.syms.get(m.name) == "get").unwrap().code.as_ref().unwrap();
            assert!(body.insns.iter().all(|i| !i.op.is_branch()), "{:?}", body.insns);
            assert!(!body.insns.iter().any(|i| matches!(&i.op, Op::InstanceGet { field, .. } if p.syms.get(field.ty) == "B")));
        }
        // Instantiations name the subclasses and no longer pass the id.
        let u = &p.classes[p.find("LU;").unwrap()];
        for m in &u.methods {
            let b = m.code.as_ref().unwrap();
            let Op::Invoke { method, args, .. } = &b.insns[2].op else { panic!() };
            assert_ne!(p.syms.get(method.class), "LG;");
            assert_eq!(p.syms.get(method.proto), "(I)V");
            assert_eq!(args.len(), 2);
        }
    }

    /// An arm calling a private helper of the group: the helper must stay reachable from the
    /// subclass (widened, called virtually) — an IllegalAccessError otherwise (found on a real
    /// app).
    #[test]
    fn private_helpers_used_by_arms_are_widened() {
        let mut p = group(|p| {
            let helper = MethodRef { class: p.syms.intern("LG;"), name: p.syms.intern("h"), proto: p.syms.intern("(I)I") };
            let hb = body(3, 2, vec![Op::Binop { op: BinOp::Mul, ty: NumType::Int, dst: 0, a: 2, b: Operand::Lit(3) }, Op::Return { width: Width::Single, src: 0 }]);
            let h = meth(p, "h", "(I)I", access::PRIVATE, Some(hb));
            (
                vec![
                    Op::Invoke { kind: InvokeKind::Direct, method: helper, args: vec![2, 1] },
                    Op::MoveResult { width: Width::Single, dst: 1 },
                    Op::Return { width: Width::Single, src: 1 },
                ],
                vec![h],
            )
        });
        assert_eq!(run(&mut p).len(), 1);
        let g = &p.classes[p.find("LG;").unwrap()];
        let h = g.methods.iter().find(|m| p.syms.get(m.name) == "h").unwrap();
        assert!(h.access & access::PRIVATE == 0 && h.access & access::FINAL != 0);
        for c in p.classes.iter().filter(|c| c.superclass == Some(g.ty)) {
            for i in c.methods.iter().filter_map(|m| m.code.as_ref()).flat_map(|b| &b.insns) {
                if let Op::Invoke { kind, method, .. } = &i.op {
                    if p.syms.get(method.name) == "h" {
                        assert_eq!(*kind, InvokeKind::Virtual);
                    }
                }
            }
        }
    }

    /// An arm using `invoke-super` stays in the base: moved to a subclass it would reach the
    /// base's (now abstract) method — an AbstractMethodError (found on a real app).
    #[test]
    fn arms_using_invoke_super_stay_in_the_base() {
        let mut p = group(|p| {
            let hash = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("hashCode"), proto: p.syms.intern("()I") };
            (
                vec![
                    Op::Invoke { kind: InvokeKind::Super, method: hash, args: vec![2] },
                    Op::MoveResult { width: Width::Single, dst: 1 },
                    Op::Return { width: Width::Single, src: 1 },
                ],
                vec![],
            )
        });
        assert_eq!(run(&mut p).len(), 1);
        let g = &p.classes[p.find("LG;").unwrap()];
        let get = g.methods.iter().find(|m| p.syms.get(m.name) == "get").unwrap();
        assert!(get.code.is_some() && get.access & access::ABSTRACT == 0);
        assert!(p.classes.iter().filter(|c| c.superclass == Some(g.ty)).all(|c| c.methods.iter().all(|m| p.syms.get(m.name) != "get")));
    }

    // Findings of the Phase 2 review, as regression tests.
    fn add_to_u(p: &mut Model, m: Method) {
        let u = p.find("LU;").unwrap();
        p.classes[u].methods.push(m);
        p.sort();
    }

    #[test]
    fn review_getclass_on_param_typed_as_class_refuses() {
        let mut p = group(plain_arm);
        let gc = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("getClass"), proto: p.syms.intern("()Ljava/lang/Class;") };
        let b = body(2, 1, vec![
            Op::Invoke { kind: InvokeKind::Virtual, method: gc, args: vec![1] },
            Op::MoveResult { width: Width::Object, dst: 0 },
            Op::Return { width: Width::Object, src: 0 },
        ]);
        let m = meth(&mut p, "k", "(LG;)Ljava/lang/Class;", access::PUBLIC | access::STATIC, Some(b));
        add_to_u(&mut p, m);
        assert_eq!(run(&mut p).len(), 0, "split despite getClass() on an LG;-typed parameter");
    }

    #[test]
    fn review_getclass_on_checkcast_refuses() {
        let mut p = group(plain_arm);
        let g = p.syms.intern("LG;");
        let gc = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("getClass"), proto: p.syms.intern("()Ljava/lang/Class;") };
        let b = body(2, 1, vec![
            Op::CheckCast { reg: 1, ty: g },
            Op::Invoke { kind: InvokeKind::Virtual, method: gc, args: vec![1] },
            Op::MoveResult { width: Width::Object, dst: 0 },
            Op::Return { width: Width::Object, src: 0 },
        ]);
        let m = meth(&mut p, "k", "(Ljava/lang/Object;)Ljava/lang/Class;", access::PUBLIC | access::STATIC, Some(b));
        add_to_u(&mut p, m);
        assert_eq!(run(&mut p).len(), 0, "split despite getClass() on a check-cast LG; value");
    }

    #[test]
    fn review_subclass_constructor_protos_are_unique() {
        let mut p = group(plain_arm);
        let gi = p.find("LG;").unwrap();
        let g = p.classes[gi].ty;
        let id = FieldRef { class: g, name: p.syms.intern("a"), ty: p.syms.intern("B") };
        let cap = FieldRef { class: g, name: p.syms.intern("b"), ty: p.syms.intern("I") };
        let obj_init = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("<init>"), proto: p.syms.intern("()V") };
        let c2 = body(3, 2, vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::InstancePut { kind: eightr_ir::op::MemKind::Byte, src: 0, obj: 1, field: id },
            Op::InstancePut { kind: eightr_ir::op::MemKind::Narrow, src: 2, obj: 1, field: cap },
            Op::Invoke { kind: InvokeKind::Direct, method: obj_init, args: vec![1] },
            Op::ReturnVoid,
        ]);
        let m = meth(&mut p, "<init>", "(I)V", access::PUBLIC | access::CONSTRUCTOR, Some(c2));
        p.classes[gi].methods.push(m);
        let g_init1 = MethodRef { class: g, name: p.syms.intern("<init>"), proto: p.syms.intern("(I)V") };
        let b = body(3, 1, vec![
            Op::NewInstance { dst: 0, ty: g },
            Op::Invoke { kind: InvokeKind::Direct, method: g_init1, args: vec![0, 2] },
            Op::Return { width: Width::Object, src: 0 },
        ]);
        let m = meth(&mut p, "m2", "(I)LG;", access::PUBLIC | access::STATIC, Some(b));
        add_to_u(&mut p, m);
        if run(&mut p).is_empty() { return; }
        for c in &p.classes {
            let mut seen = BTreeSet::new();
            for m in &c.methods {
                assert!(seen.insert((p.syms.get(m.name).to_string(), p.syms.get(m.proto).to_string())), "{}: duplicate {}{}", p.syms.get(c.ty), p.syms.get(m.name), p.syms.get(m.proto));
            }
        }
    }

    #[test]
    fn review_conditional_id_write_refuses() {
        let mut p = group(plain_arm);
        let gi = p.find("LG;").unwrap();
        let g = p.classes[gi].ty;
        let id = FieldRef { class: g, name: p.syms.intern("a"), ty: p.syms.intern("B") };
        let cap = FieldRef { class: g, name: p.syms.intern("b"), ty: p.syms.intern("I") };
        let obj_init = MethodRef { class: p.syms.intern("Ljava/lang/Object;"), name: p.syms.intern("<init>"), proto: p.syms.intern("()V") };
        let ctor = body(3, 3, vec![
            Op::IfZ { cond: eightr_ir::op::Cond::Eq, a: 1, target: 2 },
            Op::InstancePut { kind: eightr_ir::op::MemKind::Byte, src: 2, obj: 0, field: id },
            Op::InstancePut { kind: eightr_ir::op::MemKind::Narrow, src: 1, obj: 0, field: cap },
            Op::Invoke { kind: InvokeKind::Direct, method: obj_init, args: vec![0] },
            Op::ReturnVoid,
        ]);
        let k = p.classes[gi].methods.iter().position(|m| p.syms.get(m.name) == "<init>").unwrap();
        p.classes[gi].methods[k].code = Some(ctor);
        assert_eq!(run(&mut p).len(), 0, "split although the id store doesn't dominate the constructor's exit");
    }

    #[test]
    fn review_arithmetic_capture_is_not_a_class_id() {
        let mut p = group(plain_arm);
        let gi = p.find("LG;").unwrap();
        let g = p.classes[gi].ty;
        let id = FieldRef { class: g, name: p.syms.intern("a"), ty: p.syms.intern("B") };
        let cap = FieldRef { class: g, name: p.syms.intern("b"), ty: p.syms.intern("I") };
        let get = body(3, 1, vec![
            Op::InstanceGet { kind: eightr_ir::op::MemKind::Byte, dst: 0, obj: 2, field: id },
            Op::InstanceGet { kind: eightr_ir::op::MemKind::Narrow, dst: 1, obj: 2, field: cap },
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 1, a: 1, b: Operand::Reg(0) },
            Op::Return { width: Width::Single, src: 1 },
        ]);
        let k = p.classes[gi].methods.iter().position(|m| p.syms.get(m.name) == "get").unwrap();
        p.classes[gi].methods[k].code = Some(get);
        assert_eq!(run(&mut p).len(), 0, "an arithmetic capture was split as a class id");
    }
}
