//! Structural facts about Jetpack Compose code (docs/sources/compose.md): the runtime's
//! `Composer`, found by the compiler plugin's call shape, never by name (R8 renames it and merges
//! the `Composer` interface into `ComposerImpl`).
//!
//! Every restartable composable begins with `$composer.startRestartGroup(KEY)`: a call on the
//! composer parameter, taking an int constant (the durable group key) and returning the composer
//! type. The class whose such method opens the most methods, by a wide margin, is the composer.

use std::collections::BTreeMap;

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{Const, InvokeKind, Op};
use eightr_ir::types::parse_proto;

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
/// Returns (method, key).
fn entry_call<'a>(p: &Model, body: &'a Body) -> Option<(&'a eightr_ir::op::MethodRef, i32)> {
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
                return params.contains_key(&args[0]).then_some(()).and(consts.get(&args[1]).map(|&k| (method, k)));
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
            let Some((call, key)) = entry_call(p, b) else { continue };
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
