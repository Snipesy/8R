//! Structural facts about Jetpack Compose code (docs/sources/compose.md): the runtime's
//! `Composer`, found by the compiler plugin's call shape, never by name (R8 renames it and merges
//! the `Composer` interface into `ComposerImpl`).
//!
//! Every restartable composable begins with `$composer.startRestartGroup(KEY)`: a call on the
//! composer parameter, taking an int constant (the durable group key) and returning the composer
//! type. The class whose such method opens the most methods, by a wide margin, is the composer.

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{Const, InvokeKind, Op};
use eightr_ir::types::parse_proto;

/// Role candidates with their votes, best first.
type Ranked = Vec<((String, String), usize)>;

/// Restartable composables opened by one method: (class index, method index, entry key).
type Opened = Vec<(usize, usize, i32)>;

/// The composer and its group-opening method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// Descriptor of the composer class.
    pub class: String,
    /// `startRestartGroup`: (name, proto) of the `(I)C` method opening restartable composables.
    pub start_restart_group: (String, String),
    /// Restartable composables: (class index, method index, entry key).
    pub restartable: Vec<(usize, usize, i32)>,
}

/// The first call in `body` if it is `recv.m(k)` on a parameter (or a copy of one) with an int
/// constant `k`. Straight-line code before it is allowed (R8 hoists constants, field loads,
/// arithmetic, null checks and library calls above it); a branch or an app call first is not.
/// Returns (method, key, the parameter register the receiver copies).
pub(crate) fn entry_call<'a>(p: &Model, body: &'a Body) -> Option<(&'a eightr_ir::op::MethodRef, i32, u16)> {
    let first_param = body.registers - body.ins;
    let mut consts: BTreeMap<u16, i32> = BTreeMap::new();
    // Register → the parameter it copies.
    let mut params: BTreeMap<u16, u16> = (first_param..body.registers).map(|r| (r, r)).collect();
    for (i, insn) in body.insns.iter().enumerate() {
        let op = &insn.op;
        // Parameter null checks (Kotlin's, or R8's discarded `getClass()`) and hoisted static
        // calls such as boxing come before the group call: none can be it.
        let skippable = match op {
            Op::Invoke { kind: InvokeKind::Virtual, method, .. } => {
                p.syms.get(method.name) == "getClass" && !matches!(body.insns.get(i + 1).map(|x| &x.op), Some(Op::MoveResult { .. }))
            }
            // Static calls (Kotlin's parameter null checks, boxing, ...) can't be the virtual
            // group call.
            Op::Invoke { kind: InvokeKind::Static, .. } => true,
            _ => false,
        };
        if skippable {
            continue;
        }
        match op {
            Op::Invoke { kind: InvokeKind::Virtual | InvokeKind::Interface, method, args } if args.len() == 2 => {
                let recv = *params.get(&args[0])?;
                return consts.get(&args[1]).map(|&k| (method, k, recv));
            }
            Op::Invoke { .. } | Op::InvokeCustom { .. } | Op::InvokePolymorphic { .. } => return None,
            _ if op.is_branch() || matches!(op, Op::Return { .. } | Op::ReturnVoid | Op::Throw { .. } | Op::Switch { .. }) => return None,
            _ => {}
        }
        // Track what the instruction defines.
        if let Some((dst, wide)) = op.def() {
            consts.remove(&dst);
            params.remove(&dst);
            if wide {
                consts.remove(&(dst + 1));
                params.remove(&(dst + 1));
            }
            match op {
                Op::Const { dst, value: Const::Narrow(k) } => {
                    consts.insert(*dst, *k);
                }
                Op::Move { dst, src, .. } => {
                    if let Some(&p0) = params.get(src) {
                        params.insert(*dst, p0);
                    }
                    if let Some(&k) = consts.get(src) {
                        consts.insert(*dst, k);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Finds the composer: the class `C` with a method `(I)C` (or `(I)` + a type `C` merged into)
/// that is the constant-key entry call of the most methods; it must dominate (≥ 5 methods and 3×
/// the runner-up).
pub fn find(p: &Model) -> Option<Composer> {
    let s = &p.syms;
    // (class, name, proto) → methods it opens.
    let mut opens: BTreeMap<(String, String, String), Opened> = BTreeMap::new();
    for (ci, c) in p.classes.iter().enumerate() {
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(b) = &m.code else { continue };
            let Some((call, key, _)) = entry_call(p, b) else { continue };
            let proto = s.get(call.proto);
            let Some((params, ret)) = parse_proto(proto) else { continue };
            // (I) returning the receiver's own class: startRestartGroup(I)Composer.
            if params == ["I"] && ret == s.get(call.class) {
                // The receiver must be a parameter of that type.
                if parse_proto(s.get(m.proto)).is_some_and(|(ps, _)| ps.contains(&ret)) {
                    opens.entry((ret.to_string(), s.get(call.name).to_string(), proto.to_string())).or_default().push((ci, mi, key));
                }
            }
        }
    }
    let mut ranked: Vec<(&(String, String, String), &Opened)> = opens.iter().collect();
    ranked.sort_by_key(|(k, v)| (std::cmp::Reverse(v.len()), (*k).clone()));
    let ((class, name, proto), methods) = ranked.first()?;
    let runner_up = ranked.iter().skip(1).find(|(k, _)| k.0 != *class).map_or(0, |(_, v)| v.len());
    if methods.len() < 5 || methods.len() < 3 * runner_up.max(1) {
        return None;
    }
    let mut restartable: Opened = (*methods).clone();
    restartable.sort();
    Some(Composer {
        class: class.clone(),
        start_restart_group: (name.clone(), proto.clone()),
        restartable,
    })
}

/// A composer member identified by the role the compiler plugin's calls give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    /// (name, proto) of the composer method in the input.
    pub method: (String, String),
    /// The runtime's name for it.
    pub name: &'static str,
}

/// The first call satisfying `on` from `start`, following gotos; `None` at any other branch or
/// after 16 instructions.
fn first_call<'a>(insns: &'a [eightr_ir::lift::Insn], start: usize, on: &dyn Fn(&eightr_ir::op::MethodRef) -> bool) -> Option<&'a eightr_ir::op::MethodRef> {
    let mut at = start;
    for _ in 0..16 {
        match &insns.get(at)?.op {
            Op::Invoke { method, .. } if on(method) => return Some(method),
            Op::Goto { target } => at = *target as usize,
            op if op.is_branch() || matches!(op, Op::Return { .. } | Op::ReturnVoid | Op::Throw { .. }) => return None,
            _ => at += 1,
        }
    }
    None
}

/// Whether the composer's `(Object)Z` method `m` compares by `equals` (`changed`) or by identity
/// only (`changedInstance`).
fn object_compare(p: &Model, c: &Composer, m: &(String, String)) -> Option<&'static str> {
    let s = &p.syms;
    let class = p.classes.iter().find(|k| s.get(k.ty) == c.class)?;
    let body = class.methods.iter().find(|x| s.get(x.name) == m.0 && s.get(x.proto) == m.1)?.code.as_ref()?;
    let equals = body.insns.iter().any(|x| {
        matches!(&x.op, Op::Invoke { method, .. } if (s.get(method.name) == "equals" && s.get(method.proto) == "(Ljava/lang/Object;)Z")
            // Intrinsics.areEqual, possibly renamed with the app's stdlib.
            || (s.get(method.proto) == "(Ljava/lang/Object;Ljava/lang/Object;)Z"))
    });
    let identity = body.insns.iter().any(|x| matches!(x.op, Op::If { cond: eightr_ir::op::Cond::Eq | eightr_ir::op::Cond::Ne, .. }));
    match (equals, identity) {
        (true, _) => Some("changed"),
        (false, true) => Some("changedInstance"),
        _ => None,
    }
}

/// Runtime members identified by role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    /// Composer members.
    pub composer: Vec<Role>,
    /// `updateChangedFlags(I)I`: (class, name, proto).
    pub update_changed_flags: Option<(String, String, String)>,
}

/// Composer members by role, from how restartable composables call them (compose.md §2
/// fingerprints), and `updateChangedFlags`. A role is reported only when one method clearly
/// plays it.
pub fn roles(p: &Model, c: &Composer) -> Roles {
    let s = &p.syms;
    let on_c = |m: &eightr_ir::op::MethodRef| s.get(m.class) == c.class;
    let key = |m: &eightr_ir::op::MethodRef| (s.get(m.name).to_string(), s.get(m.proto).to_string());
    // Candidates per role: method → number of composables showing the role.
    let mut votes: BTreeMap<&'static str, BTreeMap<(String, String), usize>> = BTreeMap::new();
    let mut vote = |role: &'static str, m: (String, String)| *votes.entry(role).or_default().entry(m).or_default() += 1;
    for &(ci, mi, _) in &c.restartable {
        let Some(b) = &p.classes[ci].methods[mi].code else { continue };
        let insns = &b.insns;
        // Only the first skip-decision-shaped call is the composable's skip check: a later `()Z`
        // is the defaults block's getDefaultsInvalid (whose else arm also skips to group end).
        let mut skip_seen = false;
        for (i, insn) in insns.iter().enumerate() {
            let Op::Invoke { method, .. } = &insn.op else { continue };
            if !on_c(method) {
                continue;
            }
            let proto = s.get(method.proto);
            let result = match insns.get(i + 1).map(|x| &x.op) {
                Some(Op::MoveResult { dst, .. }) => Some(*dst),
                _ => None,
            };
            let branch_on = |r: u16| insns.get(i + 2).map(|x| &x.op).and_then(|op| match op {
                Op::IfZ { a, cond, target } if *a == r => Some((*cond, *target)),
                _ => None,
            });
            match proto {
                // The skip decision: shouldExecute(Z, I)Z (compiler 2.2+) skips when false,
                // getSkipping()Z (older) when true. The skip path's first composer call is
                // skipToGroupEnd()V; a ()Z whose path starts otherwise (getInserting → createNode,
                // ...) isn't voted for.
                "(ZI)Z" | "()Z" => {
                    let Some((cond, target)) = result.and_then(branch_on) else { continue };
                    if std::mem::replace(&mut skip_seen, true) {
                        continue;
                    }
                    let skips_when_true = proto == "()Z";
                    // `if-eqz r` jumps when false; `if-nez r` when true.
                    let jump_when_true = match cond {
                        eightr_ir::op::Cond::Eq => false,
                        eightr_ir::op::Cond::Ne => true,
                        _ => continue,
                    };
                    let skip_at = if jump_when_true == skips_when_true { target as usize } else { i + 3 };
                    if let Some(m2) = first_call(insns, skip_at, &on_c) {
                        if s.get(m2.proto) == "()V" {
                            vote(if skips_when_true { "getSkipping" } else { "shouldExecute" }, key(method));
                            vote("skipToGroupEnd", key(m2));
                        }
                    }
                }
                // rememberedValue()Object compared with the Empty sentinel, then
                // updateRememberedValue(Object) on the fresh value.
                "()Ljava/lang/Object;" => {
                    let Some(r) = result else { continue };
                    let compared = insns.iter().skip(i + 2).take(6).any(|x| matches!(x.op, Op::If { a, b, .. } if a == r || b == r));
                    if compared {
                        vote("rememberedValue", key(method));
                        for x in insns.iter().skip(i + 2).take(24) {
                            if let Op::Invoke { method: m2, .. } = &x.op {
                                if on_c(m2) && s.get(m2.proto) == "(Ljava/lang/Object;)V" {
                                    vote("updateRememberedValue", key(m2));
                                    break;
                                }
                            }
                        }
                    }
                }
                // changed(X)Z feeding the $dirty bits (`? 4 : 2`).
                "(I)Z" | "(J)Z" | "(F)Z" | "(D)Z" | "(Z)Z" | "(C)Z" | "(B)Z" | "(S)Z" | "(Ljava/lang/Object;)Z" => {
                    if result.and_then(branch_on).is_some() {
                        vote(if proto == "(Ljava/lang/Object;)Z" { "changed(Object)" } else { "changed" }, key(method));
                    }
                }
                _ => {
                    // endRestartGroup()ScopeUpdateScope: an object result null-tested.
                    if proto.starts_with("()L") && !proto.ends_with(&format!("){}", c.class)) && proto != "()Ljava/lang/Object;" {
                        if let Some(r) = result {
                            if branch_on(r).is_some() {
                                vote("endRestartGroup", key(method));
                            }
                        }
                    }
                }
            }
        }
    }
    // updateChangedFlags(I)I: the restart lambda re-invokes its composable with
    // `updateChangedFlags($changed | 1)`, a static (I)I whose result is an argument of the call.
    let restartable: std::collections::BTreeSet<(String, String, String)> = c
        .restartable
        .iter()
        .map(|&(ci, mi, _)| {
            let m = &p.classes[ci].methods[mi];
            (s.get(p.classes[ci].ty).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string())
        })
        .collect();
    // Its body is the runtime's bit shuffle: masks 0x12492492 and 0x24924924 (the "same" and
    // "different" bits of every slot), which R8 keeps verbatim.
    let shuffles = |(class, name, proto): &(String, String, String)| -> bool {
        let Some(k) = p.find(class) else { return false };
        let Some(body) = p.classes[k].methods.iter().find(|x| s.get(x.name) == name && s.get(x.proto) == proto).and_then(|x| x.code.as_ref()) else { return false };
        let has = |v: i32| body.insns.iter().any(|x| matches!(x.op, Op::Const { value: eightr_ir::op::Const::Narrow(k), .. } if k == v));
        has(0x1249_2492) && has(0x2492_4924)
    };
    let mut flags: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for m in p.classes.iter().flat_map(|c| &c.methods) {
        let Some(b) = &m.code else { continue };
        // Register → the static (I)I call defining it (straight-line; cleared by any other def).
        let mut defined: BTreeMap<u16, &eightr_ir::op::MethodRef> = BTreeMap::new();
        let mut pending: Option<&eightr_ir::op::MethodRef> = None;
        for insn in &b.insns {
            let last = pending.take();
            match &insn.op {
                Op::Invoke { kind: InvokeKind::Static, method, .. } if s.get(method.proto) == "(I)I" => pending = Some(method),
                Op::Invoke { method, args, .. } if restartable.contains(&(s.get(method.class).to_string(), s.get(method.name).to_string(), s.get(method.proto).to_string())) => {
                    for a in args {
                        if let Some(f) = defined.get(a) {
                            *flags.entry((s.get(f.class).to_string(), s.get(f.name).to_string(), s.get(f.proto).to_string())).or_default() += 1;
                        }
                    }
                }
                _ => {}
            }
            if insn.op.is_branch() || insn.op.ends_flow() {
                defined.clear();
            }
            if let Some((dst, wide)) = insn.op.def() {
                defined.remove(&dst);
                if wide {
                    defined.remove(&(dst + 1));
                }
                if let (Op::MoveResult { .. }, Some(f)) = (&insn.op, last) {
                    defined.insert(dst, f);
                }
            }
        }
    }
    // A library reference (runtime not in the program) can't be checked, and isn't renamed.
    flags.retain(|m, _| p.find(&m.0).is_none() || shuffles(m));
    let mut ranked: Vec<_> = flags.into_iter().collect();
    ranked.sort_by_key(|(m, n)| (std::cmp::Reverse(*n), m.clone()));
    let update_changed_flags = match ranked.as_slice() {
        [(m, n), rest @ ..] if *n >= 3 && rest.first().is_none_or(|r| *n >= 4 * r.1) => Some(m.clone()),
        _ => None,
    };
    // A role holds when one method plays it in most composables that show it at all.
    let mut out = vec![Role { method: c.start_restart_group.clone(), name: "startRestartGroup" }];
    // The skip checks of the two compiler eras never meet in one program's composables compiled
    // by one compiler; if both have votes, the (ZI)Z shape (never a composer getter) decides.
    if votes.get("shouldExecute").is_some_and(|v| !v.is_empty()) {
        votes.remove("getSkipping");
    }
    let mut winners: Vec<Role> = Vec::new();
    let mut deferred: Vec<(&'static str, Ranked)> = Vec::new();
    for (role, cands) in votes {
        let mut ranked: Vec<_> = cands.into_iter().collect();
        ranked.sort_by_key(|(m, n)| (std::cmp::Reverse(*n), m.clone()));
        if role == "changed" || role == "changed(Object)" {
            deferred.push((role, ranked));
            continue;
        }
        let (m, n) = &ranked[0];
        if ranked.len() > 1 && *n < 4 * ranked[1].1 {
            continue; // not a clear winner
        }
        // rememberedValue / updateRememberedValue are thin wrappers over nextSlotForCache /
        // updateValue; where R8 inlined the wrapper everywhere, the callers reach the inner
        // method instead. The winner must itself call another composer method of its shape.
        let wraps = |proto: &str| {
            let Some(k) = p.find(&c.class) else { return false };
            let Some(body) = p.classes[k].methods.iter().find(|x| s.get(x.name) == m.0 && s.get(x.proto) == m.1).and_then(|x| x.code.as_ref()) else {
                return false;
            };
            body.insns.iter().any(|x| matches!(&x.op, Op::Invoke { method, .. } if s.get(method.class) == c.class && s.get(method.proto) == proto && s.get(method.name) != m.0))
        };
        match role {
            "rememberedValue" if !wraps("()Ljava/lang/Object;") => continue,
            "updateRememberedValue" if !wraps("(Ljava/lang/Object;)V") => continue,
            _ => {}
        }
        winners.push(Role { method: m.clone(), name: role });
    }
    let taken: BTreeSet<(String, String)> = winners.iter().map(|r| r.method.clone()).collect();
    for (role, ranked) in deferred {
        if role == "changed" {
            // Primitive `changed` overloads all share the name: each candidate is one (other roles'
            // winners excluded, e.g. a shouldExecute R8 narrowed to `(Z)Z`).
            for (m, _) in &ranked {
                if !taken.contains(m) {
                    out.push(Role { method: m.clone(), name: "changed" });
                }
            }
            continue;
        }
        // changed(Object) and changedInstance(Object) share the call shape; their bodies differ:
        // `nextSlot() != value` (equals) vs `nextSlot() !== value` (identity). Each candidate is
        // named by its body, when no other candidate has the same verdict.
        let kinds: Vec<Option<&'static str>> = ranked.iter().map(|(m, _)| object_compare(p, c, m)).collect();
        for ((m, _), kind) in ranked.iter().zip(&kinds) {
            let Some(kind) = kind else { continue };
            if kinds.iter().filter(|k| **k == Some(*kind)).count() == 1 && !taken.contains(m) {
                out.push(Role { method: m.clone(), name: kind });
            }
        }
    }
    out.extend(winners);
    out.sort_by(|a, b| a.method.cmp(&b.method));
    out.dedup();
    Roles { composer: out, update_changed_flags }
}

/// `ComposableSingletons$FileKt` fields (compose.md `compose/singletons`): static fields a
/// `<clinit>` sets to `new ComposableLambdaImpl(K, false, block)`, named `lambda$K` by compilers
/// ≥ 2.1.20. The lambda class is found by behavior: a method taking the composer opens a restart
/// group keyed by an int field its constructor stores from its first parameter. Returns
/// (class index, field index, K); a key stored into two fields is dropped.
pub fn singletons(p: &Model, c: &Composer) -> Vec<(usize, usize, i32)> {
    let s = &p.syms;
    let (srg_name, srg_proto) = &c.start_restart_group;
    // Lambda classes: descriptor → key field name.
    let mut lambda: BTreeMap<String, String> = BTreeMap::new();
    for cls in &p.classes {
        let ty = s.get(cls.ty);
        for m in &cls.methods {
            let Some(b) = &m.code else { continue };
            for (i, x) in b.insns.iter().enumerate() {
                let Op::Invoke { method, args, .. } = &x.op else { continue };
                if s.get(method.class) != c.class || s.get(method.name) != srg_name || s.get(method.proto) != srg_proto || args.len() != 2 {
                    continue;
                }
                // The key: the last def of args[1] before the call, straight-line, an own int field.
                let key_field = b.insns[..i].iter().rev().take_while(|y| !y.op.is_branch()).find_map(|y| match &y.op {
                    Op::InstanceGet { dst, field, .. } if *dst == args[1] => Some(Some(field)),
                    op if op.def().is_some_and(|(d, _)| d == args[1]) => Some(None),
                    _ => None,
                });
                if let Some(Some(f)) = key_field {
                    if s.get(f.class) == ty && s.get(f.ty) == "I" {
                        lambda.insert(ty.to_string(), s.get(f.name).to_string());
                    }
                }
            }
        }
    }
    // Constructors storing their first (int) parameter into the key field.
    let mut ctors: BTreeSet<(String, String)> = BTreeSet::new();
    for cls in &p.classes {
        let ty = s.get(cls.ty);
        let Some(key_field) = lambda.get(ty) else { continue };
        for m in cls.methods.iter().filter(|m| s.get(m.name) == "<init>") {
            let Some(b) = &m.code else { continue };
            if !s.get(m.proto).starts_with("(I") {
                continue;
            }
            let first = b.registers - b.ins + 1;
            // The parameter register is never redefined before the store.
            let stores = b.insns.iter().take_while(|y| y.op.def().is_none_or(|(d, _)| d != first)).any(|y| {
                matches!(&y.op, Op::InstancePut { src, field, .. } if *src == first && s.get(field.name) == key_field && s.get(field.class) == ty)
            });
            if stores {
                ctors.insert((ty.to_string(), s.get(m.proto).to_string()));
            }
        }
    }
    // `<clinit>`: new L; const K; L.<init>(K, ...); sput-object → field.
    let mut found: BTreeMap<i32, Vec<(usize, usize)>> = BTreeMap::new();
    for cls in &p.classes {
        for m in cls.methods.iter().filter(|m| s.get(m.name) == "<clinit>") {
            let Some(b) = &m.code else { continue };
            let mut consts: BTreeMap<u16, i32> = BTreeMap::new();
            let mut made: BTreeMap<u16, i32> = BTreeMap::new();
            for x in &b.insns {
                match &x.op {
                    Op::Invoke { kind: InvokeKind::Direct, method, args } if ctors.contains(&(s.get(method.class).to_string(), s.get(method.proto).to_string())) && s.get(method.name) == "<init>" => {
                        if let (Some(&obj), Some(k)) = (args.first(), args.get(1).and_then(|r| consts.get(r))) {
                            made.insert(obj, *k);
                        }
                        continue;
                    }
                    Op::StaticPut { src, field, .. } => {
                        if let Some(&k) = made.get(src) {
                            if let Some(fc) = p.find(s.get(field.class)) {
                                if let Some(fi) = p.classes[fc].fields.iter().position(|f| f.name == field.name && f.ty == field.ty) {
                                    found.entry(k).or_default().push((fc, fi));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if x.op.is_branch() {
                    consts.clear();
                    made.clear();
                }
                if let Some((d, wide)) = x.op.def() {
                    consts.remove(&d);
                    made.remove(&d);
                    if wide {
                        consts.remove(&(d + 1));
                        made.remove(&(d + 1));
                    }
                    match &x.op {
                        Op::Const { dst, value: Const::Narrow(k) } => {
                            consts.insert(*dst, *k);
                        }
                        Op::NewInstance { .. } => {}
                        _ => {}
                    }
                }
            }
        }
    }
    let mut out: Vec<(usize, usize, i32)> = found.into_iter().filter(|(_, v)| v.len() == 1).map(|(k, v)| (v[0].0, v[0].1, k)).collect();
    out.sort();
    out
}
