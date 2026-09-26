//! `r8/rebox-enum`: turn values of an enum R8 unboxed back into objects of a (re-created)
//! enum class (docs/sources/r8-desugar.md §4.3; Phase 3B).
//!
//! R8's enum unboxing represents a constant by its ordinal + 1 (0 = null) and removes the
//! class. Phase 3A recovers each enum's constants (by ordinal) and often its name. Here:
//! * the enum class is re-created in javac's shape (`values`, `valueOf`, `$VALUES`), named by
//!   its recovered FQN when known, its constants by their recovered names;
//! * a *web* of values is seeded by the value an inlined `name()` chain compares, and closed
//!   over register copies and merges. Its values may only come from constants `0..=N` and
//!   from elements of the utility's `values(N)` array; anything else refuses the web;
//! * inside the web, registers hold enum objects. Comparisons with a constant become identity
//!   comparisons with `E.CONST` (null checks stay as they are), `values(N)` becomes
//!   `E.values()`, and every other use of a value as an int reads `E.$8r$unboxed(x)`, which is
//!   exactly `x == null ? 0 : x.ordinal() + 1`: behavior is preserved exactly.
//!
//! Webs stay within one method (values flowing through fields, parameters or returns are
//! adapted to ints at that point). D: the enum is the original's shape; the class is S-named
//! only when its FQN was recovered.

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::cfg::Cfg;
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::lift::{Body, Insn};
use eightr_ir::liveness::Liveness;
use eightr_ir::model::{Class, Field, Method, Program as Model};
use eightr_ir::op::{Cond, Const, FieldRef, InvokeKind, MemKind, MethodRef, Op, Reg, Width};
use eightr_ir::sym::Sym;
use eightr_rules::{Source, REBOX_ENUM};

use super::{Rewrite, RewriteRecord};
use crate::error::Result;
use crate::passes::enum_unboxing::{self as eu, RecoveredEnum};

pub struct Rebox;

/// The re-created enum class.
struct Enum {
    ty: Sym,
    constants: Vec<Sym>, // field names, by ordinal
    values: MethodRef,
    unboxed: MethodRef,
}

impl Enum {
    fn constant(&self, k: i32) -> FieldRef {
        FieldRef { class: self.ty, name: self.constants[(k - 1) as usize], ty: self.ty }
    }
}

/// Planned edits of one method: instruction → (enum index, edit).
type MethodEdits = BTreeMap<u32, Vec<(usize, Edit)>>;

/// What to do at one instruction for one web.
#[derive(Debug, Clone)]
enum Edit {
    /// A constant `k` defined into the web: becomes `E.C_k` (or null for 0).
    ConstDef(i32),
    /// `move dst, src` from a (shared) constant `k` into the web: `sget-object dst, E.C_k`.
    MoveConst(i32),
    /// A copy of a web value: `move-object`.
    MoveObject,
    /// An element of the values array into the web: `aget-object`.
    AgetObject,
    /// `values(N)` of the utility: `E.values()`.
    Values,
    /// `if-eq/ne x, k` with `x` in the web and `k` a constant.
    CompareConst { web: Reg, k: i32 },
    /// Reads of web registers as ints: adapt each through `E.$8r$unboxed`.
    Adapt(Vec<Reg>),
}

/// The narrow constant all definitions in `defs` give.
fn const_of(body: &Body, rd: &ReachingDefs, defs: &[usize]) -> Option<i32> {
    let mut v = None;
    for &d in defs {
        let DefSite::Insn(j) = rd.defs[d].site else { return None };
        let Op::Const { value: Const::Narrow(k), .. } = body.insns[j as usize].op else { return None };
        if v.is_some_and(|x| x != k) {
            return None;
        }
        v = Some(k);
    }
    v
}

fn reaching(rd: &ReachingDefs, at: u32, reg: Reg) -> &[usize] {
    rd.uses[at as usize].as_ref().and_then(|u| u.iter().find(|(r, _)| *r == reg)).map_or(&[], |(_, d)| d.as_slice())
}

fn def_index(rd: &ReachingDefs, at: u32, reg: Reg) -> Option<usize> {
    rd.defs.iter().position(|d| d.site == DefSite::Insn(at) && d.reg == reg)
}

/// `op` reading `to` wherever it read `from` (its destination, if `from`, is kept).
fn replace_reads(op: &Op, from: Reg, to: Reg) -> Op {
    let def = op.def().map(|(r, _)| r);
    let mut out = op.clone();
    out.map_regs(&mut |r| if r == from { to } else { r });
    if def == Some(from) {
        if let Op::Binop { dst, .. } | Op::Unop { dst, .. } | Op::Move { dst, .. } | Op::Cmp { dst, .. } | Op::InstanceOf { dst, .. }
        | Op::ArrayLength { dst, .. } | Op::ArrayGet { dst, .. } | Op::InstanceGet { dst, .. } = &mut out
        {
            *dst = from;
        }
    }
    out
}

/// One method's webs for one enum: closes them and plans edits. `None` refuses.
fn plan_webs(body: &Body, rd: &ReachingDefs, seeds: &[usize], n: i32, values_calls: &BTreeSet<u32>) -> Option<BTreeMap<u32, Vec<Edit>>> {
    // Uses of every definition: instructions reading its register with it reaching.
    let mut uses_of: Vec<Vec<u32>> = vec![Vec::new(); rd.defs.len()];
    for (j, u) in rd.uses.iter().enumerate() {
        for (_, defs) in u.iter().flatten() {
            for &d in defs {
                uses_of[d].push(j as u32);
            }
        }
    }
    let mut web: BTreeSet<usize> = BTreeSet::new();
    let mut arrays: BTreeSet<usize> = BTreeSet::new();
    let mut move_const: BTreeMap<u32, i32> = BTreeMap::new();
    let mut work: Vec<usize> = seeds.to_vec();
    let mut array_work: Vec<usize> = Vec::new();
    while !work.is_empty() || !array_work.is_empty() {
        while let Some(a) = array_work.pop() {
            if !arrays.insert(a) {
                continue;
            }
            let DefSite::Insn(j) = rd.defs[a].site else { return None };
            match &body.insns[j as usize].op {
                Op::MoveResult { .. } if j > 0 && values_calls.contains(&(j - 1)) => {}
                Op::Move { src, .. } => array_work.extend(reaching(rd, j, *src)),
                _ => return None,
            }
            for &u in &uses_of[a] {
                let reg = rd.defs[a].reg;
                array_work.extend(reaching(rd, u, reg));
                match &body.insns[u as usize].op {
                    Op::ArrayGet { array, dst, .. } if *array == reg => work.push(def_index(rd, u, *dst)?),
                    Op::ArrayLength { .. } => {}
                    Op::Move { src, dst, .. } if *src == reg => array_work.push(def_index(rd, u, *dst)?),
                    _ => return None,
                }
            }
        }
        let Some(d) = work.pop() else { continue };
        if !web.insert(d) {
            continue;
        }
        let DefSite::Insn(j) = rd.defs[d].site else { return None };
        match &body.insns[j as usize].op {
            Op::Const { value: Const::Narrow(k), .. } if (0..=n).contains(k) => {}
            Op::Move { src, .. } => {
                let defs = reaching(rd, j, *src);
                match const_of(body, rd, defs) {
                    Some(k) if (0..=n).contains(&k) => {
                        move_const.insert(j, k);
                    }
                    _ => work.extend(defs),
                }
            }
            Op::ArrayGet { array, .. } => array_work.extend(reaching(rd, j, *array)),
            _ => return None,
        }
        let reg = rd.defs[d].reg;
        for &u in &uses_of[d] {
            // Every definition of the register reaching a use of a web value is in the web.
            work.extend(reaching(rd, u, reg));
            if let Op::Move { src, dst, .. } = &body.insns[u as usize].op {
                if *src == reg {
                    work.push(def_index(rd, u, *dst)?);
                }
            }
        }
    }
    // Constants in the web must not be read as anything but web values... they are: every use
    // of a web definition is handled below (as an object, or adapted to an int).
    let web_reg_at = |u: u32, r: Reg| -> bool { reaching(rd, u, r).iter().any(|d| web.contains(d)) };
    let mut edits: BTreeMap<u32, Vec<Edit>> = BTreeMap::new();
    for &d in &web {
        let DefSite::Insn(j) = rd.defs[d].site else { continue };
        let e = match &body.insns[j as usize].op {
            Op::Const { value: Const::Narrow(k), .. } => Edit::ConstDef(*k),
            Op::Move { .. } => match move_const.get(&j) {
                Some(k) => Edit::MoveConst(*k),
                None => Edit::MoveObject,
            },
            Op::ArrayGet { .. } => Edit::AgetObject,
            _ => return None,
        };
        edits.entry(j).or_default().push(e);
    }
    for &a in &arrays {
        let DefSite::Insn(j) = rd.defs[a].site else { continue };
        if matches!(body.insns[j as usize].op, Op::MoveResult { .. }) {
            edits.entry(j - 1).or_default().push(Edit::Values);
        }
    }
    // Uses of web values that aren't copies or array reads.
    let used: BTreeSet<u32> = web.iter().flat_map(|&d| uses_of[d].iter().copied()).collect();
    for u in used {
        let op = &body.insns[u as usize].op;
        let reads: Vec<Reg> = op.uses().into_iter().filter(|&r| web_reg_at(u, r)).collect::<BTreeSet<_>>().into_iter().collect();
        match op {
            Op::Move { .. } => {}
            Op::IfZ { cond: Cond::Eq | Cond::Ne, .. } => {}
            Op::If { cond: Cond::Eq | Cond::Ne, a, b, .. } => {
                let (wa, wb) = (web_reg_at(u, *a), web_reg_at(u, *b));
                if wa && wb {
                    // Identity comparison of two web values.
                } else {
                    let (w, other) = if wa { (*a, *b) } else { (*b, *a) };
                    match const_of(body, rd, reaching(rd, u, other)) {
                        Some(k) if (0..=n).contains(&k) => edits.entry(u).or_default().push(Edit::CompareConst { web: w, k }),
                        _ => edits.entry(u).or_default().push(Edit::Adapt(vec![w])),
                    }
                }
            }
            _ => edits.entry(u).or_default().push(Edit::Adapt(reads)),
        }
    }
    Some(edits)
}

/// Registers free before instruction `at`: not live into it and not touched by it.
fn free_before(body: &Body, cfg: &Cfg, live: &Liveness, at: u32) -> Vec<Reg> {
    let op = &body.insns[at as usize].op;
    let mut busy = if at == 0 {
        // Live into the entry: parameters and anything read before written.
        let mut b = live.across(body, cfg, 0);
        for r in op.uses() {
            b.insert(r);
        }
        b
    } else {
        let mut b = live.across(body, cfg, at);
        if let Some((r, wide)) = op.def() {
            b.remove(r);
            if wide {
                b.remove(r + 1);
            }
        }
        for r in op.uses() {
            b.insert(r);
        }
        b
    };
    if let Some((r, wide)) = op.def() {
        busy.insert(r);
        if wide {
            busy.insert(r + 1);
        }
    }
    let base = body.registers - body.ins;
    (0..base).filter(|&r| !busy.contains(r)).collect()
}

/// Applies one method's edits. `None` if registers run out or something can't be encoded.
fn apply(body: &Body, edits: &MethodEdits, enums: &[Option<Enum>]) -> Option<Body> {
    let cfg = Cfg::build(body).ok()?;
    let live = Liveness::compute(body, &cfg);
    let mut out = body.clone();
    // Highest index first: insertions don't move lower ones.
    for (&j, es) in edits.iter().rev() {
        let mut op = out.insns[j as usize].op.clone();
        let mut prefix: Vec<Op> = Vec::new();
        let mut free = free_before(body, &cfg, &live, j).into_iter();
        for (e, edit) in es {
            let en = enums[*e].as_ref()?;
            match edit {
                Edit::ConstDef(k) | Edit::MoveConst(k) => {
                    let dst = op.def()?.0;
                    op = if *k == 0 {
                        Op::Const { dst, value: Const::Narrow(0) }
                    } else {
                        Op::StaticGet { kind: MemKind::Object, dst, field: en.constant(*k) }
                    };
                }
                Edit::MoveObject => {
                    if let Op::Move { width, .. } = &mut op {
                        *width = Width::Object;
                    }
                }
                Edit::AgetObject => {
                    if let Op::ArrayGet { kind, .. } = &mut op {
                        *kind = MemKind::Object;
                    }
                }
                Edit::Values => {
                    op = Op::Invoke { kind: InvokeKind::Static, method: en.values, args: vec![] };
                }
                Edit::CompareConst { web, k } => {
                    let Op::If { cond, target, .. } = op else { return None };
                    if *k == 0 {
                        op = Op::IfZ { cond, a: *web, target };
                    } else {
                        let t = free.next()?;
                        prefix.push(Op::StaticGet { kind: MemKind::Object, dst: t, field: en.constant(*k) });
                        op = Op::If { cond, a: *web, b: t, target };
                    }
                }
                Edit::Adapt(regs) => {
                    for &r in regs {
                        let t = free.next()?;
                        prefix.push(Op::Invoke { kind: InvokeKind::Static, method: en.unboxed, args: vec![r] });
                        prefix.push(Op::MoveResult { width: Width::Single, dst: t });
                        op = replace_reads(&op, r, t);
                    }
                }
            }
        }
        out.insns[j as usize].op = op;
        eightr_ir::edit::insert_before(&mut out, j, prefix);
    }
    eightr_ir::edit::recompute_outs(&mut out);
    out.insns.iter().all(|i| i.op.encodable()).then_some(out)
}

/// The javac shape of `enum E { C1, .., CN }`, plus the `$8r$unboxed` adapter.
fn enum_class(p: &mut Model, ty: Sym, names: &[String], origin: usize) -> (Class, Enum) {
    let s = &mut p.syms;
    let tyd = s.get(ty).to_string();
    let arr = s.intern(&format!("[{tyd}"));
    let jenum = s.intern("Ljava/lang/Enum;");
    let init = s.intern("<init>");
    let si = s.intern("(Ljava/lang/String;I)V");
    let values_field = s.intern("$VALUES");
    let constants: Vec<Sym> = names.iter().map(|n| s.intern(n)).collect();
    let values = MethodRef { class: ty, name: s.intern("values"), proto: s.intern(&format!("()[{tyd}")) };
    let unboxed = MethodRef { class: ty, name: s.intern("$8r$unboxed"), proto: s.intern(&format!("({tyd})I")) };
    let value_of = MethodRef { class: ty, name: s.intern("valueOf"), proto: s.intern(&format!("(Ljava/lang/String;){tyd}")) };
    let ordinal = MethodRef { class: jenum, name: s.intern("ordinal"), proto: s.intern("()I") };
    let clone = MethodRef { class: arr, name: s.intern("clone"), proto: s.intern("()Ljava/lang/Object;") };
    let enum_value_of = MethodRef { class: jenum, name: s.intern("valueOf"), proto: s.intern("(Ljava/lang/Class;Ljava/lang/String;)Ljava/lang/Enum;") };
    let ctor = MethodRef { class: ty, name: init, proto: si };
    let super_ctor = MethodRef { class: jenum, name: init, proto: si };
    let vf = FieldRef { class: ty, name: values_field, ty: arr };
    let name_syms: Vec<Sym> = names.iter().map(|n| s.intern(n)).collect();

    let code = |registers: u16, ins: u16, ops: Vec<Op>| Body {
        registers,
        ins,
        outs: 0,
        insns: ops.into_iter().map(|op| Insn { pc: 0, op }).collect(),
        tries: vec![],
        positions: vec![],
        locals: vec![],
        parameter_names: vec![],
    };
    // <clinit>: C_k = new E("NAME", k - 1); $VALUES = { C_1, .., C_N }.
    let mut ops = Vec::new();
    for (i, &c) in constants.iter().enumerate() {
        ops.push(Op::NewInstance { dst: 0, ty });
        ops.push(Op::ConstString { dst: 1, value: name_syms[i] });
        ops.push(Op::Const { dst: 2, value: Const::Narrow(i as i32) });
        ops.push(Op::Invoke { kind: InvokeKind::Direct, method: ctor, args: vec![0, 1, 2] });
        ops.push(Op::StaticPut { kind: MemKind::Object, src: 0, field: FieldRef { class: ty, name: c, ty } });
    }
    ops.push(Op::Const { dst: 0, value: Const::Narrow(names.len() as i32) });
    ops.push(Op::NewArray { dst: 0, size: 0, ty: arr });
    for (i, &c) in constants.iter().enumerate() {
        ops.push(Op::StaticGet { kind: MemKind::Object, dst: 1, field: FieldRef { class: ty, name: c, ty } });
        ops.push(Op::Const { dst: 2, value: Const::Narrow(i as i32) });
        ops.push(Op::ArrayPut { kind: MemKind::Object, src: 1, array: 0, index: 2 });
    }
    ops.push(Op::StaticPut { kind: MemKind::Object, src: 0, field: vf });
    ops.push(Op::ReturnVoid);
    let mut clinit = code(3, 0, ops);
    clinit.outs = 3;
    let mut ctor_body = code(3, 3, vec![Op::Invoke { kind: InvokeKind::Direct, method: super_ctor, args: vec![0, 1, 2] }, Op::ReturnVoid]);
    ctor_body.outs = 3;
    let mut values_body = code(1, 0, vec![
        Op::StaticGet { kind: MemKind::Object, dst: 0, field: vf },
        Op::Invoke { kind: InvokeKind::Virtual, method: clone, args: vec![0] },
        Op::MoveResult { width: Width::Object, dst: 0 },
        Op::CheckCast { reg: 0, ty: arr },
        Op::Return { width: Width::Object, src: 0 },
    ]);
    values_body.outs = 1;
    let mut value_of_body = code(2, 1, vec![
        Op::ConstClass { dst: 0, ty },
        Op::Invoke { kind: InvokeKind::Static, method: enum_value_of, args: vec![0, 1] },
        Op::MoveResult { width: Width::Object, dst: 0 },
        Op::CheckCast { reg: 0, ty },
        Op::Return { width: Width::Object, src: 0 },
    ]);
    value_of_body.outs = 2;
    // $8r$unboxed(e) = e == null ? 0 : e.ordinal() + 1
    let mut unboxed_body = code(2, 1, vec![
        Op::IfZ { cond: Cond::Eq, a: 1, target: 5 },
        Op::Invoke { kind: InvokeKind::Virtual, method: ordinal, args: vec![1] },
        Op::MoveResult { width: Width::Single, dst: 0 },
        Op::Binop { op: eightr_ir::op::BinOp::Add, ty: eightr_ir::op::NumType::Int, dst: 0, a: 0, b: eightr_ir::op::Operand::Lit(1) },
        Op::Return { width: Width::Single, src: 0 },
        Op::Const { dst: 0, value: Const::Narrow(0) },
        Op::Return { width: Width::Single, src: 0 },
    ]);
    unboxed_body.outs = 1;

    let m = |name: Sym, proto: Sym, access: u32, body: Body| Method { name, proto, access, code: Some(body), annotations: vec![], parameter_annotations: None };
    let mut fields: Vec<Field> = constants
        .iter()
        .map(|&c| Field { name: c, ty, access: access::PUBLIC | access::STATIC | access::FINAL | access::ENUM, static_value: None, annotations: vec![] })
        .collect();
    fields.push(Field { name: values_field, ty: arr, access: access::PRIVATE | access::STATIC | access::FINAL | access::SYNTHETIC, static_value: None, annotations: vec![] });
    let clinit_name = p.syms.intern("<clinit>");
    let void = p.syms.intern("()V");
    let methods = vec![
        m(clinit_name, void, access::STATIC | access::CONSTRUCTOR, clinit),
        m(init, si, access::PRIVATE | access::CONSTRUCTOR, ctor_body),
        m(values.name, values.proto, access::PUBLIC | access::STATIC, values_body),
        m(value_of.name, value_of.proto, access::PUBLIC | access::STATIC, value_of_body),
        m(unboxed.name, unboxed.proto, access::PUBLIC | access::STATIC | access::SYNTHETIC, unboxed_body),
    ];
    let class = Class {
        ty,
        access: access::PUBLIC | access::FINAL | access::ENUM,
        superclass: Some(jenum),
        interfaces: vec![],
        source_file: None,
        annotations: vec![],
        fields,
        methods,
        origin,
    };
    (class, Enum { ty, constants, values, unboxed })
}

/// Descriptor for a canonical name (`a.b.Outer.Inner`): the longest prefix naming an existing
/// class is the outer class and the rest are nested (`$`). Without one (the outer class may be
/// renamed), Java's convention decides: the first segment starting upper-case is the top-level
/// class, later ones are nested.
fn binary_name(p: &Model, canonical: &str) -> String {
    let parts: Vec<&str> = canonical.split('.').collect();
    for k in (1..parts.len()).rev() {
        let outer = format!("L{};", parts[..k].join("/"));
        if p.find(&outer).is_some() {
            return format!("{}${};", outer.trim_end_matches(';'), parts[k..].join("$"));
        }
    }
    let top = parts.iter().position(|s| s.starts_with(|c: char| c.is_ascii_uppercase())).unwrap_or(parts.len() - 1);
    format!("L{}{}{};", parts[..top].iter().map(|s| format!("{s}/")).collect::<String>(), parts[top], parts[top + 1..].iter().map(|s| format!("${s}")).collect::<String>())
}

impl Rewrite for Rebox {
    fn name(&self) -> &'static str {
        "rebox-enum"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, p: &mut Model, records: &mut Vec<RewriteRecord>) -> Result<()> {
        // Only proven constant names: a String field's per-value strings look the same.
        let recovered: Vec<RecoveredEnum> = eu::recover_enums(p, &p.classes).into_iter().filter(|e| e.proven && !e.constants.is_empty()).collect();
        if recovered.is_empty() {
            return Ok(());
        }
        // The shared utility's `values(n)` methods, by fingerprint.
        let mut values_methods: Vec<(Sym, Sym, Sym)> = Vec::new();
        for c in &p.classes {
            if c.access & access::SYNTHETIC == 0 {
                continue;
            }
            let Some(fi) = eu::values_field(p, c) else { continue };
            for m in &c.methods {
                if m.access & access::STATIC != 0 && p.syms.get(m.proto) == "(I)[I" && m.code.as_ref().is_some_and(|b| eu::is_values(p, b, c.ty, c.fields[fi].name)) {
                    values_methods.push((c.ty, m.name, m.proto));
                }
            }
        }
        let by_names: BTreeMap<&Vec<String>, usize> = recovered.iter().enumerate().map(|(i, e)| (&e.constants, i)).collect();

        // Plan every method: (class, method) → edits tagged with the enum index.
        let mut planned: BTreeMap<(usize, usize), MethodEdits> = BTreeMap::new();
        let mut webs = vec![0usize; recovered.len()];
        for (ci, c) in p.classes.iter().enumerate() {
            for (mi, m) in c.methods.iter().enumerate() {
                let Some(body) = &m.code else { continue };
                // Seeds: values an inlined name() chain compares, and values an inlined valueOf
                // produces (the definition in each success branch).
                let mut chains = eu::chains(p, body);
                let produced = eu::value_of_chains(p, body);
                if chains.is_empty() && produced.is_empty() {
                    continue;
                }
                let Ok(cfg) = Cfg::build(body) else { continue };
                let rd = ReachingDefs::compute(body, &cfg);
                let mut method_edits: MethodEdits = BTreeMap::new();
                let mut claimed: BTreeSet<u32> = BTreeSet::new();
                chains.extend(produced.into_iter().map(|mut c| {
                    c.from_value_of = true;
                    c
                }));
                for chain in chains {
                    let Some(names) = eu::complete(&chain.table) else { continue };
                    let Some(&e) = by_names.get(&names) else { continue };
                    let n = names.len() as i32;
                    let seeds: Vec<usize> = if chain.from_value_of {
                        // The success branch defines the result, or (a jump) it already holds it.
                        chain.compares.iter().flat_map(|&(i, r)| match def_index(&rd, i, r) {
                            Some(d) => vec![d],
                            None => reaching(&rd, i, r).to_vec(),
                        }).collect()
                    } else {
                        chain.compares.iter().flat_map(|&(i, r)| reaching(&rd, i, r).iter().copied()).collect()
                    };
                    let values_calls: BTreeSet<u32> = body
                        .insns
                        .iter()
                        .enumerate()
                        .filter(|(j, x)| match &x.op {
                            Op::Invoke { kind: InvokeKind::Static, method, args } => {
                                values_methods.contains(&(method.class, method.name, method.proto))
                                    && args.len() == 1
                                    && eu::const_at(body, &rd, *j as u32, args[0]) == Some(n)
                            }
                            _ => false,
                        })
                        .map(|(j, _)| j as u32)
                        .collect();
                    let Some(edits) = plan_webs(body, &rd, &seeds, n, &values_calls) else { continue };
                    // Two webs (of different enums) editing one instruction's definition would
                    // conflict: refuse the later one.
                    let defs_touched: BTreeSet<u32> = edits.iter().filter(|(_, es)| es.iter().any(|x| !matches!(x, Edit::Adapt(_) | Edit::CompareConst { .. }))).map(|(j, _)| *j).collect();
                    if !claimed.is_disjoint(&defs_touched) {
                        continue;
                    }
                    claimed.extend(defs_touched);
                    for (j, es) in edits {
                        method_edits.entry(j).or_default().extend(es.into_iter().map(|x| (e, x)));
                    }
                    webs[e] += 1;
                }
                if !method_edits.is_empty() {
                    planned.insert((ci, mi), method_edits);
                }
            }
        }
        if planned.is_empty() {
            return Ok(());
        }
        // Create the classes (only enums with a web), then apply.
        let origin = 0;
        let mut classes: Vec<Option<Enum>> = (0..recovered.len()).map(|_| None).collect();
        let mut order: Vec<usize> = (0..recovered.len()).filter(|&e| webs[e] > 0).collect();
        order.sort_by(|a, b| recovered[*a].constants.cmp(&recovered[*b].constants));
        let mut new_classes = Vec::new();
        for (i, &e) in order.iter().enumerate() {
            let desc = recovered[e]
                .canonical_name
                .as_ref()
                .map(|f| binary_name(p, f))
                .filter(|d| p.find(d).is_none())
                .unwrap_or_else(|| format!("L$8r$Enum{i};"));
            let ty = p.syms.intern(&desc);
            let (class, en) = enum_class(p, ty, &recovered[e].constants, origin);
            new_classes.push(class);
            classes[e] = Some(en);
        }
        let enums = classes;
        let mut done = vec![0usize; recovered.len()];
        for ((ci, mi), edits) in planned {
            let body = p.classes[ci].methods[mi].code.as_ref().expect("planned methods have code");
            if let Some(new) = apply(body, &edits, &enums) {
                for e in edits.values().flatten().map(|(e, _)| *e).collect::<BTreeSet<_>>() {
                    done[e] += 1;
                }
                p.classes[ci].methods[mi].code = Some(new);
            }
        }
        for &e in &order {
            let d = p.syms.get(enums[e].as_ref().expect("created above").ty).to_string();
            records.push(RewriteRecord {
                rule: REBOX_ENUM,
                item: d,
                detail: format!("re-boxed in {} method(s) ({} constants: {})", done[e], recovered[e].constants.len(), recovered[e].constants.join(", ")),
            });
        }
        p.classes.extend(new_classes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eightr_ir::op::{BinOp, NumType, Operand};

    #[test]
    fn canonical_names_map_to_binary_names() {
        let p = Model::default();
        assert_eq!(binary_name(&p, "com.caverock.androidsvg.SVG.Unit"), "Lcom/caverock/androidsvg/SVG$Unit;");
        assert_eq!(binary_name(&p, "com.example.enums.Color"), "Lcom/example/enums/Color;");
        assert_eq!(binary_name(&p, "a.b.c"), "La/b/c;");
    }

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

    fn plan(b: &Body, compare_at: u32, reg: Reg) -> Option<BTreeMap<u32, Vec<Edit>>> {
        let cfg = Cfg::build(b).unwrap();
        let rd = ReachingDefs::compute(b, &cfg);
        let seeds = reaching(&rd, compare_at, reg).to_vec();
        plan_webs(b, &rd, &seeds, 3, &BTreeSet::new())
    }

    /// `x = p0 ? 1 : 2; if (x == 1) ..`: constants only — a web; the compare becomes identity
    /// with the constant, the int use (return) is adapted.
    #[test]
    fn constant_web_is_planned() {
        let b = body(3, 1, vec![
            Op::IfZ { cond: Cond::Eq, a: 2, target: 3 },
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::Goto { target: 4 },
            Op::Const { dst: 0, value: Const::Narrow(2) },
            Op::Const { dst: 1, value: Const::Narrow(1) },
            Op::If { cond: Cond::Eq, a: 0, b: 1, target: 7 },
            Op::Return { width: Width::Single, src: 0 },
            Op::Return { width: Width::Single, src: 1 },
        ]);
        let edits = plan(&b, 5, 0).expect("closed web");
        assert!(matches!(edits[&1].as_slice(), [Edit::ConstDef(1)]));
        assert!(matches!(edits[&3].as_slice(), [Edit::ConstDef(2)]));
        assert!(matches!(edits[&5].as_slice(), [Edit::CompareConst { web: 0, k: 1 }]));
        assert!(matches!(edits[&6].as_slice(), [Edit::Adapt(r)] if r == &[0]));
    }

    /// A value computed by arithmetic can't be an enum object: the web is refused.
    #[test]
    fn arithmetic_values_refuse_the_web() {
        let b = body(3, 1, vec![
            Op::Binop { op: BinOp::Add, ty: NumType::Int, dst: 0, a: 2, b: Operand::Lit(1) },
            Op::Const { dst: 1, value: Const::Narrow(1) },
            Op::If { cond: Cond::Eq, a: 0, b: 1, target: 4 },
            Op::Return { width: Width::Single, src: 0 },
            Op::Return { width: Width::Single, src: 1 },
        ]);
        assert!(plan(&b, 2, 0).is_none());
    }

    /// Constants out of the enum's range (N = 3) aren't its values.
    #[test]
    fn out_of_range_constants_refuse_the_web() {
        let b = body(2, 0, vec![
            Op::Const { dst: 0, value: Const::Narrow(9) },
            Op::Const { dst: 1, value: Const::Narrow(1) },
            Op::If { cond: Cond::Eq, a: 0, b: 1, target: 4 },
            Op::Return { width: Width::Single, src: 0 },
            Op::Return { width: Width::Single, src: 1 },
        ]);
        assert!(plan(&b, 2, 0).is_none());
    }
}
