//! App composables: the Compose compiler plugin's synthetic parameters by role, and what they
//! prove about the original signature (docs/sources/compose.md §3 R-roles, `param-slot`,
//! `default-args`, `original-arity-bound`). Annotative only: nothing here changes code.
//!
//! * `$composer`: the parameter receiving `startRestartGroup` (the entry call). S.
//! * `$changed`: parameters receiving `updateChangedFlags(..)` at every restart-lambda call back
//!   to the composable. The role is S; its index (`$changed` vs `$changed1`) follows residual order
//!   and is D (R8 can drop a constant `$changedK`).
//! * `$default`: an int parameter used only in single-bit tests, one of which selects between a
//!   parameter's value and another (the default binding). D.
//! * Slot binding: `changed(p) ? 4<<3s : 2<<3s` ties `p` to slot `s` of a `$changed` int. D.
//! * Default binding: bit `i` of `$default` selecting `p`'s value ties `p` to regular index `i`. D.
//! * Slot lower bound: `10·(c−1)+1` for `c ≥ 2` `$changed` ints, the top slot of any mask applied to a
//!   `$changed`-derived value, the top `$default` bit + 1. Sound on the library truth.

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::cfg::Cfg;
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::lift::Body;
use eightr_ir::model::{Method, Program as Model};
use eightr_ir::op::{BinOp, Const, InvokeKind, NumType, Op, Operand, Reg};
use eightr_ir::types::parse_proto;
use serde::Serialize;

use crate::compose::{Composer, Roles};

/// A restartable composable's parameters by role. Parameter indices are the proto's (the
/// receiver excluded).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Composable {
    #[serde(skip)]
    pub class: usize,
    #[serde(skip)]
    pub method: usize,
    /// `Lclass;->name(proto)` in input names.
    pub signature: String,
    pub key: i32,
    pub composer: Option<usize>,
    pub changed: Vec<usize>,
    pub defaults: Vec<usize>,
    /// Restart lambdas calling back: `Lclass;->name(proto)`.
    pub restart: Vec<String>,
    #[serde(skip)]
    pub restart_methods: Vec<(usize, usize)>,
    /// (param, `$changed` param holding its bits, slot within that int).
    pub slots: Vec<(usize, usize, u32)>,
    /// (param, original regular index): its `$default` bit.
    pub default_bits: Vec<(usize, u32)>,
    /// Lower bound on the original slot count (parameters and receivers).
    pub slots_at_least: u32,
    /// Parameters R8 provably removed (at least).
    pub removed_at_least: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ComposeSummary {
    pub restartable: u64,
    pub with_restart_lambda: u64,
    pub changed_params: u64,
    pub default_params: u64,
    pub slot_bindings: u64,
    pub default_bindings: u64,
    pub removed_params_proven: u64,
    /// ComposableSingletons fields found (`lambda$K`; named only in the shouldExecute era).
    pub singleton_fields: u64,
}

pub fn summary(all: &[Composable]) -> ComposeSummary {
    let mut s = ComposeSummary { restartable: all.len() as u64, ..Default::default() };
    for c in all {
        s.with_restart_lambda += u64::from(!c.restart.is_empty());
        s.changed_params += c.changed.len() as u64;
        s.default_params += c.defaults.len() as u64;
        s.slot_bindings += c.slots.len() as u64;
        s.default_bindings += c.default_bits.len() as u64;
        s.removed_params_proven += u64::from(c.removed_at_least > 0);
    }
    s
}

/// Registers of each proto parameter (the first of a wide pair).
pub fn param_regs(p: &Model, m: &Method, b: &Body) -> Option<Vec<Reg>> {
    let (params, _) = parse_proto(p.syms.get(m.proto))?;
    let mut r = (b.registers - b.ins) + u16::from(!m.is_static());
    let mut out = Vec::new();
    for t in params {
        out.push(r);
        r += if t == "J" || t == "D" { 2 } else { 1 };
    }
    Some(out)
}

/// Argument registers of a call, per proto parameter (the receiver skipped).
fn call_args(static_call: bool, proto: &str, args: &[Reg]) -> Option<Vec<Reg>> {
    let (params, _) = parse_proto(proto)?;
    let mut k = usize::from(!static_call);
    let mut out = Vec::new();
    for t in params {
        out.push(*args.get(k)?);
        k += if t == "J" || t == "D" { 2 } else { 1 };
    }
    Some(out)
}

/// Defs carrying the value of each register-param (the param itself and moves of it), by
/// param register.
fn carriers(body: &Body, rd: &ReachingDefs) -> BTreeMap<Reg, BTreeSet<usize>> {
    let mut out: BTreeMap<Reg, BTreeSet<usize>> = BTreeMap::new();
    for (d, def) in rd.defs.iter().enumerate() {
        if def.site == DefSite::Param {
            out.entry(def.reg).or_default().insert(d);
        }
    }
    // Moves whose every reaching source def carries the same param.
    loop {
        let mut grew = false;
        for (d, def) in rd.defs.iter().enumerate() {
            let DefSite::Insn(i) = def.site else { continue };
            let Op::Move { src, .. } = &body.insns[i as usize].op else { continue };
            let Some(srcs) = rd.uses[i as usize].as_ref().and_then(|u| u.iter().find(|(r, _)| r == src)).map(|(_, v)| v) else { continue };
            for set in out.values_mut() {
                if !set.contains(&d) && !srcs.is_empty() && srcs.iter().all(|s| set.contains(s)) {
                    set.insert(d);
                    grew = true;
                }
            }
        }
        if !grew {
            return out;
        }
    }
}

/// Reaching defs of `reg` read at `at`.
fn reaching(rd: &ReachingDefs, at: usize, reg: Reg) -> &[usize] {
    rd.uses.get(at).and_then(|u| u.as_ref()).and_then(|u| u.iter().find(|(r, _)| *r == reg)).map_or(&[], |(_, v)| v.as_slice())
}

/// The int constant `reg` holds at `at` (a single reaching `const`).
fn const_at(body: &Body, rd: &ReachingDefs, at: usize, reg: Reg) -> Option<i32> {
    match reaching(rd, at, reg) {
        [d] => match rd.defs[*d].site {
            DefSite::Insn(j) => match body.insns[j as usize].op {
                Op::Const { value: Const::Narrow(k), .. } => Some(k),
                _ => None,
            },
            DefSite::Param => None,
        },
        _ => None,
    }
}

/// Whether the value of `reg` at `at` derives (through moves and `or`/`and`/`xor`) from one of
/// the param registers `from` on some path.
fn derives_from(body: &Body, rd: &ReachingDefs, at: usize, reg: Reg, from: &BTreeSet<Reg>) -> bool {
    let mut stack = vec![(at, reg)];
    let mut seen = BTreeSet::new();
    while let Some((i, r)) = stack.pop() {
        if !seen.insert((i, r)) {
            continue;
        }
        for &d in reaching(rd, i, r) {
            let def = rd.defs[d];
            match def.site {
                DefSite::Param => {
                    if from.contains(&def.reg) {
                        return true;
                    }
                }
                DefSite::Insn(j) => match &body.insns[j as usize].op {
                    Op::Move { src, .. } => stack.push((j as usize, *src)),
                    Op::Binop { op: BinOp::Or | BinOp::And | BinOp::Xor, ty: NumType::Int, a, b, .. } => {
                        stack.push((j as usize, *a));
                        if let Operand::Reg(b) = b {
                            stack.push((j as usize, *b));
                        }
                    }
                    _ => {}
                },
            }
        }
    }
    false
}

/// Instructions reachable from `start` (normal flow) without entering `stop`.
fn reachable(body: &Body, start: usize, stop: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(i) = stack.pop() {
        if stop.contains(&i) || i >= body.insns.len() || !seen.insert(i) {
            continue;
        }
        let op = &body.insns[i].op;
        match op {
            Op::Goto { target } => stack.push(*target as usize),
            Op::If { target, .. } | Op::IfZ { target, .. } => stack.extend([*target as usize, i + 1]),
            Op::Switch { cases, .. } => {
                stack.extend(cases.iter().map(|c| c.1 as usize));
                stack.push(i + 1);
            }
            _ if op.ends_flow() => {}
            _ => stack.push(i + 1),
        }
    }
    seen
}

fn single_bit(v: i32) -> Option<u32> {
    (v != 0 && (v as u32).is_power_of_two()).then(|| (v as u32).trailing_zeros())
}

fn method_sig(p: &Model, ci: usize, mi: usize) -> String {
    let s = &p.syms;
    let (c, m) = (&p.classes[ci], &p.classes[ci].methods[mi]);
    format!("{}->{}{}", s.get(c.ty), s.get(m.name), s.get(m.proto))
}

/// Restartable composables with their parameter roles.
pub fn composables(p: &Model, c: &Composer, roles: &Roles) -> Vec<Composable> {
    let s = &p.syms;
    let key3 = |m: &eightr_ir::op::MethodRef| (s.get(m.class).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string());
    // (class, name, proto) → index in `out`.
    let mut by_ref: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut out: Vec<Composable> = Vec::new();
    for &(ci, mi, key) in &c.restartable {
        let (cls, m) = (&p.classes[ci], &p.classes[ci].methods[mi]);
        let Some(b) = &m.code else { continue };
        let Some(regs) = param_regs(p, m, b) else { continue };
        let composer = crate::compose::entry_call(p, b).and_then(|(_, _, recv)| regs.iter().position(|&r| r == recv));
        by_ref.insert((s.get(cls.ty).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string()), out.len());
        out.push(Composable { class: ci, method: mi, signature: method_sig(p, ci, mi), key, composer, ..Default::default() });
    }

    // Restart lambdas: calls back to a composable passing updateChangedFlags(..) results.
    // Per composable: params fed by updateChangedFlags at each site.
    let mut sites: BTreeMap<usize, Vec<BTreeSet<usize>>> = BTreeMap::new();
    if let Some(ucf) = &roles.update_changed_flags {
        for (ci, cls) in p.classes.iter().enumerate() {
            for (mi, m) in cls.methods.iter().enumerate() {
                let Some(b) = &m.code else { continue };
                let calls_ucf = b.insns.iter().any(|x| matches!(&x.op, Op::Invoke { method, .. } if key3(method) == *ucf));
                if !calls_ucf {
                    continue;
                }
                let Ok(cfg) = Cfg::build(b) else { continue };
                let rd = ReachingDefs::compute(b, &cfg);
                // Is the value read at `at` from `reg` an updateChangedFlags result on every path?
                let from_ucf = |at: usize, reg: Reg| -> bool {
                    let mut stack = vec![(at, reg)];
                    let mut seen = BTreeSet::new();
                    let mut any = false;
                    while let Some((i, r)) = stack.pop() {
                        if !seen.insert((i, r)) {
                            continue;
                        }
                        let defs = reaching(&rd, i, r);
                        if defs.is_empty() {
                            return false;
                        }
                        for &d in defs {
                            let DefSite::Insn(j) = rd.defs[d].site else { return false };
                            let j = j as usize;
                            match &b.insns[j].op {
                                Op::Move { src, .. } => stack.push((j, *src)),
                                Op::MoveResult { .. } if j > 0 && matches!(&b.insns[j - 1].op, Op::Invoke { method, .. } if key3(method) == *ucf) => any = true,
                                _ => return false,
                            }
                        }
                    }
                    any
                };
                for (i, x) in b.insns.iter().enumerate() {
                    let Op::Invoke { kind, method, args } = &x.op else { continue };
                    let Some(&k) = by_ref.get(&key3(method)) else { continue };
                    let Some(argv) = call_args(*kind == InvokeKind::Static, s.get(method.proto), args) else { continue };
                    let fed: BTreeSet<usize> = argv.iter().enumerate().filter(|(_, r)| from_ucf(i, **r)).map(|(j, _)| j).collect();
                    if fed.is_empty() {
                        continue; // an ordinary call (e.g. a content lambda calling it)
                    }
                    sites.entry(k).or_default().push(fed);
                    let site = (ci, mi);
                    if !out[k].restart_methods.contains(&site) {
                        out[k].restart_methods.push(site);
                        out[k].restart.push(method_sig(p, ci, mi));
                    }
                }
            }
        }
    }

    // Composer members whose results feed slot bindings.
    let changed_calls: BTreeSet<(String, String)> =
        roles.composer.iter().filter(|r| r.name == "changed" || r.name == "changedInstance").map(|r| r.method.clone()).collect();

    for (k, comp) in out.iter_mut().enumerate() {
        let m = &p.classes[comp.class].methods[comp.method];
        let b = m.code.as_ref().unwrap();
        let regs = param_regs(p, m, b).unwrap_or_default();
        let (ptypes, _) = parse_proto(s.get(m.proto)).unwrap_or_default();
        // $changed: fed at every site.
        if let Some(v) = sites.get(&k) {
            let mut all: BTreeSet<usize> = v[0].clone();
            for x in &v[1..] {
                all = all.intersection(x).copied().collect();
            }
            all.remove(&comp.composer.unwrap_or(usize::MAX));
            comp.changed = all.into_iter().collect();
        }
        comp.restart.sort();
        comp.restart_methods.sort();
        let Ok(cfg) = Cfg::build(b) else { continue };
        let rd = ReachingDefs::compute(b, &cfg);
        let carry = carriers(b, &rd);
        let param_of = |defs: &[usize]| -> Option<usize> {
            if defs.is_empty() {
                return None;
            }
            regs.iter().position(|r| carry.get(r).is_some_and(|set| defs.iter().all(|d| set.contains(d))))
        };
        let role = |j: usize| comp.composer == Some(j) || comp.changed.contains(&j);

        // Bit tests on each candidate int param: (insn of the `and`, bit, its IfZ).
        let mut bit_tests: BTreeMap<usize, Vec<(usize, u32)>> = BTreeMap::new();
        let mut other_use: BTreeSet<usize> = BTreeSet::new();
        for (i, x) in b.insns.iter().enumerate() {
            let Some(uses) = rd.uses[i].as_ref() else { continue };
            for (r, defs) in uses {
                let Some(j) = regs.iter().position(|pr| carry.get(pr).is_some_and(|set| defs.iter().any(|d| set.contains(d)))) else { continue };
                if matches!(x.op, Op::Move { .. }) && param_of(defs) == Some(j) {
                    continue;
                }
                // Captured by the restart lambda (constructor argument, field store, indy capture):
                // neutral.
                let captured = match &x.op {
                    Op::Invoke { kind: InvokeKind::Direct, method, .. } => s.get(method.name) == "<init>",
                    Op::InstancePut { src, .. } => src == r,
                    Op::InvokeCustom { .. } => true,
                    _ => false,
                };
                if captured {
                    continue;
                }
                let bit = match &x.op {
                    Op::Binop { op: BinOp::And, ty: NumType::Int, dst, a, b: rhs } if a == r || matches!(rhs, Operand::Reg(q) if q == r) => {
                        let other = match rhs {
                            Operand::Lit(v) => Some(*v),
                            Operand::Reg(q) if a == r => const_at(b, &rd, i, *q),
                            Operand::Reg(_) => const_at(b, &rd, i, *a),
                        };
                        // The result only feeds zero tests.
                        let only_tested = b.insns.iter().enumerate().all(|(u, y)| {
                            reaching(&rd, u, *dst).iter().all(|d| rd.defs[*d].site != DefSite::Insn(i as u32)) || matches!(y.op, Op::IfZ { a, .. } if a == *dst)
                        });
                        other.and_then(single_bit).filter(|_| only_tested && param_of(defs) == Some(j))
                    }
                    _ => None,
                };
                match bit {
                    Some(bit) => bit_tests.entry(j).or_default().push((i, bit)),
                    None => {
                        other_use.insert(j);
                    }
                }
            }
        }

        let changed_regs: BTreeSet<Reg> = comp.changed.iter().map(|&j| regs[j]).collect();
        // Whether `x` (at `i`) ORs one slot's "static" bits `0b110 << 3s` into a `$changed`-derived
        // value: the compiler's `if ($default & (1<<i)) $dirty |= 6 << 3s`.
        let dirty_tie = |i: usize, x: &Op| -> bool {
            let Op::Binop { op: BinOp::Or, ty: NumType::Int, a, b: rhs, .. } = x else { return false };
            let (v, other) = match rhs {
                Operand::Lit(v) => (Some(*v), *a),
                Operand::Reg(q) => match const_at(b, &rd, i, *q) {
                    Some(v) => (Some(v), *a),
                    None => (const_at(b, &rd, i, *a), *q),
                },
            };
            v.is_some_and(|v| (0..10).any(|sl| v == 6 << (3 * sl))) && derives_from(b, &rd, i, other, &changed_regs)
        };
        // Default bindings of a candidate (bit → the param whose value it selects), and whether a
        // bit's default arm feeds the dirty bits (what makes the int a `$default`).
        let bindings_of = |tests: &[(usize, u32)]| -> (Vec<(usize, u32)>, bool) {
            let mut found: Vec<(usize, u32)> = Vec::new();
            let mut tied = false;
            for &(i, bit) in tests {
                // Every zero test of the `and` result (the dirty-bits test, then the selection).
                let Op::Binop { dst, .. } = b.insns[i].op else { continue };
                let mut selected: BTreeSet<usize> = BTreeSet::new();
                for (t, y) in b.insns.iter().enumerate() {
                    let Op::IfZ { a, cond, target } = y.op else { continue };
                    if a != dst || !reaching(&rd, t, a).iter().all(|d| rd.defs[*d].site == DefSite::Insn(i as u32)) {
                        continue;
                    }
                    let target = target as usize;
                    // `if-eqz bit` jumps when the bit is clear: the default arm falls through.
                    let start = match cond {
                        eightr_ir::op::Cond::Eq => t + 1,
                        eightr_ir::op::Cond::Ne => target,
                        _ => continue,
                    };
                    // Defs made only on the default arm: reachable from its start without entering
                    // code the other arm reaches (the join and after).
                    let other_start = if start == t + 1 { target } else { t + 1 };
                    let joined = reachable(b, other_start, &BTreeSet::new());
                    let arm = reachable(b, start, &joined);
                    let arm_defs: BTreeSet<usize> =
                        rd.defs.iter().enumerate().filter(|(_, d)| matches!(d.site, DefSite::Insn(k) if arm.contains(&(k as usize)))).map(|(d, _)| d).collect();
                    tied |= arm.iter().any(|&k| dirty_tie(k, &b.insns[k].op));
                    // Or it guards the param's `changed(p)` whose result goes into the dirty bits
                    // (`$default & bit == 0 && changed(p) ? 4<<3s : 2<<3s`, for defaults that
                    // call composables). `remember(key)` calls `changed` too, but never ORs the
                    // result into a `$changed`-derived value.
                    let from_default = reachable(b, start, &BTreeSet::new());
                    let other = reachable(b, other_start, &from_default);
                    let changed_call = other.iter().any(|&k| {
                        matches!(&b.insns[k].op, Op::Invoke { method, .. } if s.get(method.class) == c.class && changed_calls.contains(&(s.get(method.name).to_string(), s.get(method.proto).to_string())))
                    });
                    // The OR happens in an arm or right where they join, not later in the body.
                    let join = joined.intersection(&from_default).next().copied().unwrap_or(usize::MAX);
                    let at_join = (join..join.saturating_add(3)).filter(|k| joined.contains(k));
                    let dirty_or = arm.iter().chain(&other).copied().chain(at_join).any(|k| match &b.insns[k].op {
                        Op::Binop { op: BinOp::Or, ty: NumType::Int, a, b: Operand::Reg(q), .. } => {
                            derives_from(b, &rd, k, *a, &changed_regs) || derives_from(b, &rd, k, *q, &changed_regs)
                        }
                        _ => false,
                    });
                    tied |= changed_call && dirty_or;
                    // Phis: a later read reached by an arm def and by a def carrying param `j`.
                    for u in rd.uses.iter().flatten() {
                        for (_, defs) in u {
                            if !defs.iter().any(|d| arm_defs.contains(d)) {
                                continue;
                            }
                            for (j, pr) in regs.iter().enumerate() {
                                if !role(j) && carry.get(pr).is_some_and(|set| defs.iter().any(|d| set.contains(d))) {
                                    selected.insert(j);
                                }
                            }
                        }
                    }
                }
                if selected.len() == 1 {
                    found.push((*selected.iter().next().unwrap(), bit));
                }
            }
            (found, tied)
        };

        let mut top_default_bit: Option<u32> = None;
        for (&j, tests) in &bit_tests {
            if role(j) || other_use.contains(&j) || ptypes.get(j) != Some(&"I") {
                continue;
            }
            // A real int bit-tested to pick a value looks the same; only `$default` also feeds the
            // dirty bits (review witness: `flags and 1 != 0`).
            let (bound, tied) = bindings_of(tests);
            if !tied {
                continue;
            }
            comp.defaults.push(j);
            comp.default_bits.extend(bound);
            top_default_bit = top_default_bit.max(tests.iter().map(|t| t.1).max());
        }
        // Unambiguous bindings only: one bit per param, one param per bit.
        let bits = std::mem::take(&mut comp.default_bits);
        let count = |f: &dyn Fn(&(usize, u32)) -> bool| bits.iter().filter(|x| f(x)).count();
        comp.default_bits = bits
            .iter()
            .copied()
            .filter(|x| !comp.defaults.contains(&x.0) && count(&|y| y.0 == x.0) == 1 && count(&|y| y.1 == x.1) == 1)
            .collect();
        comp.default_bits.sort();
        comp.default_bits.dedup();

        // The skip check: masks after it belong to the body, e.g. to a non-restartable callee R8
        // inlined whose own `$changed` derives from ours (review witness Outer3).
        let skip_calls: BTreeSet<(String, String)> =
            roles.composer.iter().filter(|r| r.name == "shouldExecute" || r.name == "getSkipping").map(|r| r.method.clone()).collect();
        let skip_at = b.insns.iter().position(|x| {
            matches!(&x.op, Op::Invoke { method, .. } if s.get(method.class) == c.class && skip_calls.contains(&(s.get(method.name).to_string(), s.get(method.proto).to_string())))
        });
        // Slot bindings: changed(p) → `? 4<<3s : 2<<3s`.
        let mut slots: Vec<(usize, usize, u32)> = Vec::new();
        let mut top_slot: Option<u32> = None;
        for (i, x) in b.insns.iter().enumerate() {
            match &x.op {
                Op::Invoke { method, args, .. } if s.get(method.class) == c.class && changed_calls.contains(&(s.get(method.name).to_string(), s.get(method.proto).to_string())) => {
                    let Some(&a) = args.get(1) else { continue };
                    let Some(j) = param_of(reaching(&rd, i, a)) else { continue };
                    if role(j) {
                        continue;
                    }
                    let Some(Op::MoveResult { dst, .. }) = b.insns.get(i + 1).map(|y| &y.op) else { continue };
                    if !matches!(b.insns.get(i + 2).map(|y| &y.op), Some(Op::IfZ { a, .. }) if a == dst) {
                        continue;
                    }
                    let consts: BTreeSet<i32> = b.insns.iter().skip(i + 3).take(6).filter_map(|y| match y.op {
                        Op::Const { value: Const::Narrow(k), .. } => Some(k),
                        _ => None,
                    }).collect();
                    let cands: Vec<u32> = (0..10).filter(|s| consts.contains(&(2 << (3 * s))) && consts.contains(&(4 << (3 * s)))).collect();
                    // The `$changed` int holding the slot: the one the preceding mask tests.
                    let q = b.insns[..i].iter().enumerate().rev().take(8).find_map(|(u, y)| match &y.op {
                        Op::Binop { op: BinOp::And, ty: NumType::Int, a, .. } => comp.changed.iter().copied().find(|&q| derives_from(b, &rd, u, *a, &BTreeSet::from([regs[q]]))),
                        _ => None,
                    });
                    if let ([sl], Some(q)) = (cands.as_slice(), q) {
                        slots.push((j, q, *sl));
                    }
                }
                Op::Binop { op: BinOp::And, ty: NumType::Int, a, b: rhs, .. } if !changed_regs.is_empty() && skip_at.is_some_and(|k| i < k) => {
                    let (v, r) = match rhs {
                        Operand::Lit(v) => (Some(*v), *a),
                        Operand::Reg(q) => match const_at(b, &rd, i, *q) {
                            Some(v) => (Some(v), *a),
                            None => (const_at(b, &rd, i, *a), *q),
                        },
                    };
                    let Some(v) = v else { continue };
                    let hb = 31 - (v as u32).leading_zeros();
                    if v == 0 || hb == 0 || hb > 30 {
                        continue;
                    }
                    if derives_from(b, &rd, i, r, &changed_regs) {
                        top_slot = top_slot.max(Some((hb - 1) / 3));
                    }
                }
                _ => {}
            }
        }
        let count = |f: &dyn Fn(&(usize, usize, u32)) -> bool| slots.iter().filter(|x| f(x)).count();
        comp.slots = slots.iter().copied().filter(|x| count(&|y| y.0 == x.0) == 1 && count(&|y| y.1 == x.1 && y.2 == x.2) == 1).collect();
        comp.slots.sort();

        let c_count = comp.changed.len() as u32;
        // One `$changed` exists even with no params; each further one follows 10 slots.
        let mut bound = if c_count > 1 { 10 * (c_count - 1) + 1 } else { 0 };
        bound = bound.max(top_slot.map_or(0, |t| t + 1));
        bound = bound.max(top_default_bit.map_or(0, |t| t + 1));
        comp.slots_at_least = bound;
        let real = regs.len() as u32 - u32::from(comp.composer.is_some()) - c_count - comp.defaults.len() as u32;
        comp.removed_at_least = bound.saturating_sub(real + u32::from(!m.is_static()));
    }
    out
}
