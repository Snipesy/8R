//! `r8/enum-unboxing-utility` (S) and enum-unboxing evidence (docs/sources/r8-desugar.md §4.3).
//!
//! R8's enum unboxing replaces an enum by `int`s (ordinal + 1, 0 = null) and removes its class.
//! What survives, structurally:
//! * the synthetic shared utility: a static `int[]` holding exactly `1..N` (`$VALUES`), a
//!   static `(I)I` that throws on 0 and returns `x - 1` (`ordinal`), and a static `(I)[I`
//!   copying a prefix of `$VALUES` (`values`). R8 generates these three names verbatim, and the
//!   shapes are exact fingerprints, so the names are S;
//! * `name()`/`toString()` inlined as a chain comparing the unboxed value against constants
//!   `k`, each arm yielding a string constant: an exact ordinal → constant-name table;
//! * `valueOf` inlined as `String.equals` tests ending in `"No enum constant <fqn>."`: the
//!   original enum's fully qualified name, tied to a table when the tested names match it.
//!
//! The enum class itself no longer exists, so the evidence is reported (Phase 3B re-boxes).

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::cfg::Cfg;
use eightr_ir::defs::{DefSite, ReachingDefs};
use eightr_ir::lift::Body;
use eightr_ir::op::{BinOp, Cond, Const, InvokeKind, Op, Operand, Reg};
use eightr_rules::{Attribute, Source, ENUM_UNBOXING_UTILITY};
use serde::Serialize;

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ItemId, Program};

pub struct EnumUnboxing;

/// An enum R8 unboxed, as far as the program still shows it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RecoveredEnum {
    /// Fully qualified name, from `valueOf`'s message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fqn: Option<String>,
    /// Constant names by ordinal (empty when only the name is known).
    pub constants: Vec<String>,
}

const MESSAGE: &str = "No enum constant ";

/// `$VALUES`: a static final `int[]` set in `<clinit>` to exactly `{1, 2, .., N}`.
fn values_field(p: &Program, c: &eightr_ir::model::Class) -> Option<usize> {
    let clinit = c.methods.iter().find(|m| p.str(m.name) == "<clinit>")?.code.as_ref()?;
    let cfg = Cfg::build(clinit).ok()?;
    let rd = ReachingDefs::compute(clinit, &cfg);
    for (i, insn) in clinit.insns.iter().enumerate() {
        let Op::StaticPut { src, field, .. } = &insn.op else { continue };
        if field.class != c.ty || p.str(field.ty) != "[I" {
            continue;
        }
        let Some(index) = c.fields.iter().position(|f| f.name == field.name && f.ty == field.ty && f.access & access::STATIC != 0) else { continue };
        if array_is_one_to_n(clinit, &rd, i as u32, *src) {
            return Some(index);
        }
    }
    None
}

/// The narrow constant every definition of `reg` at `at` gives.
fn const_at(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> Option<i32> {
    let uses = rd.uses.get(at as usize)?.as_ref()?;
    let (_, defs) = uses.iter().find(|(r, _)| *r == reg)?;
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

/// Is the array in `reg` at `at` `filled-new-array {1..N}` (N ≥ 1)?
fn array_is_one_to_n(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> bool {
    let Some(uses) = rd.uses.get(at as usize).and_then(|u| u.as_ref()) else { return false };
    let Some((_, defs)) = uses.iter().find(|(r, _)| *r == reg) else { return false };
    let [d] = defs.as_slice() else { return false };
    let DefSite::Insn(j) = rd.defs[*d].site else { return false };
    // `move-result-object` after `filled-new-array`.
    let Some(Op::FilledNewArray { args, .. }) = j.checked_sub(1).map(|k| &body.insns[k as usize].op) else { return false };
    let at = j - 1;
    !args.is_empty() && args.iter().enumerate().all(|(k, &r)| const_at(body, rd, at, r) == Some(k as i32 + 1))
}

/// `ordinal`: static `(I)I` = `if (x == 0) throw null; return x - 1;`.
fn is_ordinal(body: &Body) -> bool {
    let p0 = body.registers - body.ins;
    let ops: Vec<&Op> = body.insns.iter().map(|i| &i.op).collect();
    let has = |f: &dyn Fn(&Op) -> bool| ops.iter().any(|o| f(o));
    ops.len() <= 6
        && has(&|o| matches!(o, Op::IfZ { a, .. } if *a == p0))
        && has(&|o| matches!(o, Op::Binop { op: BinOp::Add, a, b: Operand::Lit(-1), .. } if *a == p0))
        && has(&|o| matches!(o, Op::Throw { .. }))
        && has(&|o| matches!(o, Op::Return { .. }))
        && ops.iter().all(|o| matches!(o, Op::IfZ { .. } | Op::Binop { .. } | Op::Return { .. } | Op::Throw { .. } | Op::Const { value: Const::Narrow(0), .. }))
}

/// `values`: static `(I)[I` copying the first `n` of `$VALUES` with `System.arraycopy`.
fn is_values(p: &Program, body: &Body, owner: eightr_ir::sym::Sym, field: eightr_ir::sym::Sym) -> bool {
    let reads = body.insns.iter().any(|i| matches!(&i.op, Op::StaticGet { field: f, .. } if f.class == owner && f.name == field));
    let copies = body.insns.iter().any(|i| {
        matches!(&i.op, Op::Invoke { kind: InvokeKind::Static, method, .. }
            if p.str(method.class) == "Ljava/lang/System;" && p.str(method.name) == "arraycopy")
    });
    let news = body.insns.iter().any(|i| matches!(&i.op, Op::NewArray { .. }));
    reads && copies && news && body.insns.len() <= 8
}

/// The string constant `reg` holds at `at`, following one register copy.
fn string_at(p: &Program, body: &Body, rd: &ReachingDefs, at: u32, reg: Reg, depth: u32) -> Option<String> {
    let uses = rd.uses.get(at as usize)?.as_ref()?;
    let (_, defs) = uses.iter().find(|(r, _)| *r == reg)?;
    let mut v: Option<String> = None;
    for &d in defs {
        let DefSite::Insn(j) = rd.defs[d].site else { return None };
        let s = match &body.insns[j as usize].op {
            Op::ConstString { value, .. } => p.str(*value).to_string(),
            Op::Move { src, .. } if depth > 0 => string_at(p, body, rd, j, *src, depth - 1)?,
            _ => return None,
        };
        if v.as_ref().is_some_and(|x| *x != s) {
            return None;
        }
        v = Some(s);
    }
    v
}

/// The string a block yields first: its first instruction is a `const-string`, or a move
/// from a register holding one.
fn block_string(p: &Program, body: &Body, rd: &ReachingDefs, start: u32) -> Option<String> {
    match &body.insns.get(start as usize)?.op {
        Op::ConstString { value, .. } => Some(p.str(*value).to_string()),
        Op::Move { src, .. } => string_at(p, body, rd, start, *src, 1),
        _ => None,
    }
}

fn is_constant_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
}

/// Name tables in one method: per compared value, unboxed value `k` → constant name, from
/// `if-eq x, k → "NAME"` and `if-ne x, k` falling through to `"NAME"`, for values whose chain
/// treats 0 as null (default arm `throw null` or `"null"`).
fn name_tables(p: &Program, body: &Body) -> Vec<BTreeMap<i32, String>> {
    let Ok(cfg) = Cfg::build(body) else { return Vec::new() };
    let rd = ReachingDefs::compute(body, &cfg);
    // Keyed by the value compared (register and its reaching definitions): R8 reuses registers.
    let mut by_reg: BTreeMap<(Reg, Vec<usize>), BTreeMap<i32, Option<String>>> = BTreeMap::new();
    let mut nullable: BTreeSet<(Reg, Vec<usize>)> = BTreeSet::new();
    for (i, insn) in body.insns.iter().enumerate() {
        let i = i as u32;
        let Op::If { cond, a, b, target } = insn.op else { continue };
        let (x, k) = match (const_at(body, &rd, i, a), const_at(body, &rd, i, b)) {
            (None, Some(k)) => (a, k),
            (Some(k), None) => (b, k),
            _ => continue,
        };
        let (arm, other) = match cond {
            Cond::Eq => (target, i + 1),
            Cond::Ne => (i + 1, target),
            _ => continue,
        };
        if k < 1 {
            continue;
        }
        let defs = rd.uses[i as usize].as_ref().and_then(|u| u.iter().find(|(r, _)| *r == x)).map(|(_, d)| d.clone()).unwrap_or_default();
        // The unboxed null (0) matches no constant: the chain's default throws null (`name()`)
        // or yields "null" (string conversion).
        let null_default = match body.insns.get(other as usize).map(|x| &x.op) {
            Some(Op::Throw { src }) => const_at(body, &rd, other, *src) == Some(0),
            Some(_) => block_string(p, body, &rd, other).is_some_and(|s| s == "null"),
            None => false,
        };
        if null_default {
            nullable.insert((x, defs.clone()));
        }
        // Arms yielding something else (a field's value, a computation) belong to other
        // switches on the same value: skip them. Conflicting names cancel the table.
        let Some(name) = block_string(p, body, &rd, arm) else { continue };
        let name = Some(name).filter(|s| is_constant_name(s));
        let slot = by_reg.entry((x, defs)).or_default().entry(k).or_insert_with(|| name.clone());
        if *slot != name {
            *slot = None; // conflicting names for one value: not a name table
        }
    }
    by_reg
        .into_iter()
        .filter(|(key, _)| nullable.contains(key))
        .map(|(_, t)| t)
        .filter_map(|t| t.into_iter().map(|(k, n)| n.map(|n| (k, n))).collect::<Option<BTreeMap<i32, String>>>())
        .filter(|t| t.len() >= 2)
        .collect()
}

/// A table covering exactly `1..=N` with distinct names, as names by ordinal.
fn complete(t: &BTreeMap<i32, String>) -> Option<Vec<String>> {
    let names: BTreeSet<&String> = t.values().collect();
    (t.keys().copied().eq(1..=t.len() as i32) && names.len() == t.len()).then(|| t.values().cloned().collect())
}

impl Pass for EnumUnboxing {
    fn name(&self) -> &'static str {
        "enum-unboxing"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let p = &*cx.program;
        // Shared utilities: S member names.
        let mut labels: Vec<(ItemId, &'static str)> = Vec::new();
        for id in p.class_ids() {
            let c = p.class(id);
            if c.access & access::SYNTHETIC == 0 {
                continue;
            }
            let Some(fi) = values_field(p, c) else { continue };
            let field = c.fields[fi].name;
            let mut found = vec![(ItemId::Field { class: id, index: fi as u32 }, "$VALUES")];
            for (mi, m) in c.methods.iter().enumerate() {
                let Some(b) = &m.code else { continue };
                if m.access & access::STATIC == 0 {
                    continue;
                }
                let item = ItemId::Method { class: id, index: mi as u32 };
                match p.str(m.proto) {
                    "(I)I" if is_ordinal(b) => found.push((item, "ordinal")),
                    "(I)[I" if is_values(p, b, c.ty, field) => found.push((item, "values")),
                    _ => {}
                }
            }
            // The array alone could be anything: require one of the methods using it.
            if found.len() >= 2 {
                labels.extend(found);
            }
        }
        // Evidence: name tables and valueOf messages.
        let mut tables: BTreeSet<Vec<String>> = BTreeSet::new();
        let mut named: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new(); // tested names → fqns
        let mut fqns: BTreeSet<String> = BTreeSet::new();
        for c in &p.model.classes {
            for m in &c.methods {
                let Some(b) = &m.code else { continue };
                let message = b.insns.iter().find_map(|i| match &i.op {
                    Op::ConstString { value, .. } => p.str(*value).strip_prefix(MESSAGE).and_then(|r| r.strip_suffix('.')).map(str::to_string),
                    _ => None,
                });
                let compares = b.insns.iter().any(|i| matches!(&i.op, Op::If { .. }));
                if !compares && message.is_none() {
                    continue;
                }
                for t in name_tables(p, b) {
                    if let Some(names) = complete(&t) {
                        tables.insert(names);
                    }
                }
                if let Some(fqn) = message.filter(|f| !f.is_empty()) {
                    fqns.insert(fqn.clone());
                    // Names `valueOf` tests with `String.equals` (one side a literal).
                    let mut tested: Vec<String> = Vec::new();
                    if let Ok(cfg) = Cfg::build(b) {
                        let rd = ReachingDefs::compute(b, &cfg);
                        for (i, insn) in b.insns.iter().enumerate() {
                            let Op::Invoke { method, args, .. } = &insn.op else { continue };
                            if p.str(method.name) != "equals" || args.len() != 2 {
                                continue;
                            }
                            for &r in args {
                                if let Some(s) = string_at(p, b, &rd, i as u32, r, 1).filter(|s| is_constant_name(s)) {
                                    tested.push(s);
                                }
                            }
                        }
                    }
                    tested.sort();
                    tested.dedup();
                    if !tested.is_empty() {
                        named.entry(tested).or_default().insert(fqn);
                    }
                }
            }
        }
        let mut enums: Vec<RecoveredEnum> = Vec::new();
        let mut used_fqns = BTreeSet::new();
        for t in &tables {
            let mut key = t.clone();
            key.sort();
            let fqn = named.get(&key).filter(|f| f.len() == 1).and_then(|f| f.iter().next().cloned());
            if let Some(f) = &fqn {
                used_fqns.insert(f.clone());
            }
            enums.push(RecoveredEnum { fqn, constants: t.clone() });
        }
        for f in fqns.difference(&used_fqns) {
            enums.push(RecoveredEnum { fqn: Some(f.clone()), constants: Vec::new() });
        }
        enums.sort();
        enums.dedup();
        for (item, name) in labels {
            cx.labels.record_value(item, Attribute::MemberName, ENUM_UNBOXING_UTILITY, None, Some(name.to_string()))?;
        }
        cx.enums.extend(enums);
        Ok(())
    }
}
