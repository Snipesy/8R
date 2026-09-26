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
    /// Canonical name (`a.b.Outer.Inner`, not a binary name), from `valueOf`'s message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_name: Option<String>,
    /// Constant names by ordinal (empty when only the name is known).
    pub constants: Vec<String>,
    /// Whether `constants` are proven names: an inlined `valueOf` maps each literal to its
    /// value. Otherwise they are the strings an inlined chain yields per value — `name()`'s,
    /// or those of a `String` field — indistinguishable without `valueOf`.
    pub proven: bool,
}

const MESSAGE: &str = "No enum constant ";

/// A compared value: its register and the definitions reaching it.
type ValueKey = (Reg, Vec<usize>);


/// The narrow constant `reg` holds when `at` executes, found by walking back through code with
/// a single predecessor to its definition.
fn const_before(body: &Body, cfg: &Cfg, at: u32, reg: Reg) -> Option<i32> {
    let mut i = at;
    for _ in 0..256 {
        let blk = &cfg.blocks[cfg.block_of[i as usize] as usize];
        if i > blk.start {
            i -= 1;
        } else {
            let [pred] = blk.preds.as_slice() else { return None };
            i = cfg.blocks[*pred as usize].last();
        }
        match &body.insns[i as usize].op {
            Op::Const { dst, value: Const::Narrow(k) } if *dst == reg => return Some(*k),
            op if op.def().is_some_and(|(r, w)| r == reg || (w && r + 1 == reg)) => return None,
            _ => {}
        }
    }
    None
}

/// Inlined `valueOf` chains in one method: per tested string value, literal → the value `k` the
/// success branch yields (`"NAME".equals(s)` then `const k`, a copy of one, or a jump while the
/// result register holds it), with each success instruction and the result register (as
/// `compares`), and the message its failure path throws.
pub(crate) fn value_of_chains(p: &impl Strs, body: &Body) -> Vec<Chain> {
    let Ok(cfg) = Cfg::build(body) else { return Vec::new() };
    let rd = ReachingDefs::compute(body, &cfg);
    // Equals tests: (input value, literal, success instruction).
    let mut tests: BTreeMap<ValueKey, Vec<(String, u32, u32)>> = BTreeMap::new();
    for (i, insn) in body.insns.iter().enumerate() {
        let Op::Invoke { method, args, .. } = &insn.op else { continue };
        if p.str(method.name) != "equals" || p.str(method.proto) != "(Ljava/lang/Object;)Z" || args.len() != 2 {
            continue;
        }
        let i = i as u32;
        let lit = |r: Reg| string_at(p, body, &rd, i, r, 1).filter(|s| is_constant_name(s));
        let (name, input) = match (lit(args[0]), lit(args[1])) {
            (Some(n), None) => (n, args[1]),
            (None, Some(n)) => (n, args[0]),
            _ => continue,
        };
        let Some(Op::MoveResult { dst: r, .. }) = body.insns.get(i as usize + 1).map(|x| &x.op) else { continue };
        let (success, failure) = match body.insns.get(i as usize + 2).map(|x| &x.op) {
            Some(Op::IfZ { cond: Cond::Eq, a, target }) if a == r => (i + 3, *target),
            Some(Op::IfZ { cond: Cond::Ne, a, target }) if a == r => (*target, i + 3),
            _ => continue,
        };
        let defs = rd.uses[i as usize].as_ref().and_then(|u| u.iter().find(|(x, _)| *x == input)).map(|(_, d)| d.clone()).unwrap_or_default();
        tests.entry((input, defs)).or_default().push((name, success, failure));
    }
    let mut out = Vec::new();
    for (_, list) in tests {
        // The result register: the one successes define (`const`/copy of a constant). A success
        // that only jumps to the join yields that register's current constant.
        let defined: BTreeSet<Reg> = list
            .iter()
            .filter_map(|(_, s, _)| match body.insns.get(*s as usize).map(|x| &x.op) {
                Some(Op::Const { dst, value: Const::Narrow(_) }) | Some(Op::Move { dst, .. }) => Some(*dst),
                _ => None,
            })
            .collect();
        let [result] = defined.into_iter().collect::<Vec<_>>()[..] else { continue };
        let mut table: BTreeMap<i32, Option<String>> = BTreeMap::new();
        let mut produced = Vec::new();
        for (name, s, _) in &list {
            let k = match body.insns.get(*s as usize).map(|x| &x.op) {
                Some(Op::Const { dst, value: Const::Narrow(k) }) if *dst == result => Some(*k),
                Some(Op::Move { dst, src, .. }) if *dst == result => const_at(body, &rd, *s, *src),
                Some(Op::Goto { .. }) => const_before(body, &cfg, *s, result),
                _ => None,
            };
            let Some(k) = k.filter(|k| *k >= 1) else { continue };
            produced.push((*s, result));
            let slot = table.entry(k).or_insert_with(|| Some(name.clone()));
            if slot.as_deref() != Some(name.as_str()) {
                *slot = None;
            }
        }
        let Some(table) = table.into_iter().map(|(k, n)| n.map(|n| (k, n))).collect::<Option<BTreeMap<i32, String>>>() else { continue };
        // The message the failure path builds (the last test falls into `new
        // IllegalArgumentException("No enum constant X.")`, a few instructions in).
        let messages: BTreeSet<String> = list
            .iter()
            .flat_map(|(_, _, f)| body.insns.iter().skip(*f as usize).take(4))
            .filter_map(|x| match &x.op {
                Op::ConstString { value, .. } => p.str(*value).strip_prefix(MESSAGE).and_then(|r| r.strip_suffix('.')).filter(|n| !n.is_empty()).map(str::to_string),
                _ => None,
            })
            .collect();
        let message = (messages.len() == 1).then(|| messages.into_iter().next().unwrap());
        if table.len() >= 2 {
            out.push(Chain { table, compares: produced, from_value_of: false, message });
        }
    }
    out
}

/// String lookup, so the analysis runs on the pass's `Program` and on the raw model (the
/// re-boxing rewrite runs before labels exist).
pub(crate) trait Strs {
    fn str(&self, s: eightr_ir::sym::Sym) -> &str;
}

impl Strs for Program {
    fn str(&self, s: eightr_ir::sym::Sym) -> &str {
        Program::str(self, s)
    }
}

impl Strs for eightr_ir::model::Program {
    fn str(&self, s: eightr_ir::sym::Sym) -> &str {
        self.syms.get(s)
    }
}

/// `$VALUES`: a static final `int[]` set in `<clinit>` to exactly `{1, 2, .., N}`.
pub(crate) fn values_field(p: &impl Strs, c: &eightr_ir::model::Class) -> Option<usize> {
    let clinit = c.methods.iter().find(|m| p.str(m.name) == "<clinit>")?.code.as_ref()?;
    let cfg = Cfg::build(clinit).ok()?;
    let rd = ReachingDefs::compute(clinit, &cfg);
    let mut found = Vec::new();
    for (i, insn) in clinit.insns.iter().enumerate() {
        let Op::StaticPut { src, field, .. } = &insn.op else { continue };
        if field.class != c.ty || p.str(field.ty) != "[I" {
            continue;
        }
        let Some(index) = c.fields.iter().position(|f| f.name == field.name && f.ty == field.ty && f.access & access::STATIC != 0) else { continue };
        if array_is_one_to_n(clinit, &rd, i as u32, *src) {
            found.push(index);
        }
    }
    // Exactly one: two such arrays (merged utilities) leave the roles ambiguous.
    found.dedup();
    (found.len() == 1).then(|| found[0])
}

/// The narrow constant every definition of `reg` at `at` gives.
pub(crate) fn const_at(body: &Body, rd: &ReachingDefs, at: u32, reg: Reg) -> Option<i32> {
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
pub(crate) fn is_ordinal(body: &Body) -> bool {
    let p0 = body.registers - body.ins;
    let ops: Vec<&Op> = body.insns.iter().map(|i| &i.op).collect();
    match ops.as_slice() {
        [Op::IfZ { cond: Cond::Eq, a, target: 3 }, Op::Binop { op: BinOp::Add, dst: r, a: x, b: Operand::Lit(-1), .. }, Op::Return { src, .. }, Op::Const { dst: t, value: Const::Narrow(0) }, Op::Throw { src: u }] => {
            *a == p0 && *x == p0 && src == r && t == u
        }
        _ => false,
    }
}

/// `values`: static `(I)[I` copying the first `n` of `$VALUES` with `System.arraycopy`.
pub(crate) fn is_values(p: &impl Strs, body: &Body, owner: eightr_ir::sym::Sym, field: eightr_ir::sym::Sym) -> bool {
    // new int[n]; System.arraycopy($VALUES, 0, copy, 0, n); return copy — straight-line.
    let p0 = body.registers - body.ins;
    let (mut copy, mut vals, mut zero) = (None, None, None);
    for insn in &body.insns {
        match &insn.op {
            Op::NewArray { dst, size, ty } if *size == p0 && p.str(*ty) == "[I" => copy = Some(*dst),
            Op::StaticGet { dst, field: f, .. } if f.class == owner && f.name == field => vals = Some(*dst),
            Op::Const { dst, value: Const::Narrow(0) } => zero = Some(*dst),
            Op::Invoke { kind: InvokeKind::Static, method, args } if p.str(method.class) == "Ljava/lang/System;" && p.str(method.name) == "arraycopy" => {
                if args.as_slice() != [vals.unwrap_or(u16::MAX), zero.unwrap_or(u16::MAX), copy.unwrap_or(u16::MAX), zero.unwrap_or(u16::MAX), p0] {
                    return false;
                }
            }
            Op::Return { src, .. } => return Some(*src) == copy && body.insns.len() <= 6,
            _ => return false,
        }
    }
    false
}

/// The string constant `reg` holds at `at`, following one register copy.
fn string_at(p: &impl Strs, body: &Body, rd: &ReachingDefs, at: u32, reg: Reg, depth: u32) -> Option<String> {
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
fn block_string(p: &impl Strs, body: &Body, rd: &ReachingDefs, start: u32) -> Option<String> {
    match &body.insns.get(start as usize)?.op {
        Op::ConstString { value, .. } => Some(p.str(*value).to_string()),
        Op::Move { src, .. } => string_at(p, body, rd, start, *src, 1),
        _ => None,
    }
}

pub(crate) fn is_constant_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
}

/// Name tables in one method: per compared value, unboxed value `k` → constant name, from
/// `if-eq x, k → "NAME"` and `if-ne x, k` falling through to `"NAME"`, for values whose chain
/// treats 0 as null (default arm `throw null` or `"null"`).
/// A name chain: unboxed value → constant name, and the `if` instructions comparing the value
/// (instruction index, compared register).
pub(crate) struct Chain {
    pub table: BTreeMap<i32, String>,
    pub compares: Vec<(u32, Reg)>,
    /// `compares` are the definitions of the values an inlined `valueOf` produces.
    pub from_value_of: bool,
    /// For a `valueOf` chain: the canonical name in the message its failure path throws.
    pub message: Option<String>,
}

fn name_tables(p: &impl Strs, body: &Body) -> Vec<BTreeMap<i32, String>> {
    chains(p, body).into_iter().map(|c| c.table).collect()
}

pub(crate) fn chains(p: &impl Strs, body: &Body) -> Vec<Chain> {
    let Ok(cfg) = Cfg::build(body) else { return Vec::new() };
    let rd = ReachingDefs::compute(body, &cfg);
    // Keyed by the value compared (register and its reaching definitions): R8 reuses registers.
    let mut by_reg: BTreeMap<ValueKey, BTreeMap<i32, Option<String>>> = BTreeMap::new();
    let mut sites: BTreeMap<ValueKey, Vec<(u32, Reg)>> = BTreeMap::new();
    let mut nullable: BTreeSet<ValueKey> = BTreeSet::new();
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
            Some(Op::Const { dst, value: Const::Narrow(0) }) => {
                matches!(body.insns.get(other as usize + 1).map(|x| &x.op), Some(Op::Throw { src }) if src == dst)
            }
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
        sites.entry((x, defs.clone())).or_default().push((i, x));
        let slot = by_reg.entry((x, defs)).or_default().entry(k).or_insert_with(|| name.clone());
        if *slot != name {
            *slot = None; // conflicting names for one value: not a name table
        }
    }
    by_reg
        .into_iter()
        .filter(|(key, _)| nullable.contains(key))
        .filter_map(|(key, t)| {
            let table = t.into_iter().map(|(k, n)| n.map(|n| (k, n))).collect::<Option<BTreeMap<i32, String>>>()?;
            (table.len() >= 2).then(|| Chain { table, compares: sites.remove(&key).unwrap_or_default(), from_value_of: false, message: None })
        })
        .collect()
}

/// A table covering exactly `1..=N` with distinct names, as names by ordinal.
pub(crate) fn complete(t: &BTreeMap<i32, String>) -> Option<Vec<String>> {
    let names: BTreeSet<&String> = t.values().collect();
    (t.keys().copied().eq(1..=t.len() as i32) && names.len() == t.len()).then(|| t.values().cloned().collect())
}

/// Enums R8 unboxed, recovered from inlined `valueOf` (proven constant names, and the canonical
/// name from its message) and inlined `name()`/`toString()` chains (unproven unless they
/// match a `valueOf` map).
pub(crate) fn recover_enums(p: &impl Strs, classes: &[eightr_ir::model::Class]) -> Vec<RecoveredEnum> {
    // Proven tables → canonical names seen with them (one method, one message, one map).
    let mut proven: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    let mut chains_seen: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut switch_maps: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut attached: BTreeSet<String> = BTreeSet::new();
    for c in classes {
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            let messages: BTreeSet<String> = b
                .insns
                .iter()
                .filter_map(|i| match &i.op {
                    Op::ConstString { value, .. } => p.str(*value).strip_prefix(MESSAGE).and_then(|r| r.strip_suffix('.')).filter(|n| !n.is_empty()).map(str::to_string),
                    _ => None,
                })
                .collect();
            let compares = b.insns.iter().any(|i| matches!(&i.op, Op::If { .. } | Op::IfZ { .. }));
            if !compares && messages.is_empty() {
                continue;
            }
            names.extend(messages.iter().cloned());
            // A string `switch` compiles to the same equals-then-const shape: a map is `valueOf`
            // only next to its message (one message, one map in the method). Otherwise it can
            // still corroborate an identical name chain.
            for chain in value_of_chains(p, b) {
                let Some(t) = complete(&chain.table) else { continue };
                match chain.message {
                    // Its own failure path names the enum: an inlined valueOf.
                    Some(m) => {
                        proven.entry(t).or_default().insert(m.clone());
                        attached.insert(m);
                    }
                    None => {
                        switch_maps.insert(t);
                    }
                }
            }
            for t in name_tables(p, b) {
                if let Some(t) = complete(&t) {
                    chains_seen.insert(t);
                }
            }
        }
    }
    // A name chain agreeing exactly (value by value) with an equals map is proven too.
    for t in chains_seen.intersection(&switch_maps) {
        proven.entry(t.clone()).or_default();
    }
    let mut enums: Vec<RecoveredEnum> = Vec::new();
    for (t, n) in &proven {
        let canonical_name = (n.len() == 1).then(|| n.iter().next().unwrap().clone());
        enums.push(RecoveredEnum { canonical_name, constants: t.clone(), proven: true });
    }
    for t in chains_seen.iter().filter(|t| !proven.contains_key(*t)) {
        enums.push(RecoveredEnum { canonical_name: None, constants: t.clone(), proven: false });
    }
    for n in names.difference(&attached) {
        enums.push(RecoveredEnum { canonical_name: Some(n.clone()), constants: Vec::new(), proven: false });
    }
    enums.sort();
    enums.dedup();
    enums
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
            // The array alone could be anything: require one of the methods using it, and one
            // method per role (two would both get the name: a duplicate method).
            let count = |n: &str| found.iter().filter(|(_, x)| *x == n).count();
            if found.len() >= 2 && count("ordinal") <= 1 && count("values") <= 1 {
                labels.extend(found);
            }
        }
        for (item, name) in labels {
            cx.labels.record_value(item, Attribute::MemberName, ENUM_UNBOXING_UTILITY, None, Some(name.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eightr_ir::lift::Insn;
    use eightr_ir::op::{MethodRef, Width};
    use eightr_ir::sym::Interner;

    struct S(Interner);
    impl Strs for S {
        fn str(&self, s: eightr_ir::sym::Sym) -> &str {
            self.0.get(s)
        }
    }

    fn body(registers: u16, ins: u16, ops: Vec<Op>) -> Body {
        Body { registers, ins, outs: 0, insns: ops.into_iter().enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(), tries: vec![], positions: vec![], locals: vec![], parameter_names: vec![] }
    }

    #[test]
    fn ordinal_fingerprint_is_exact() {
        let ok = body(2, 1, vec![
            Op::IfZ { cond: Cond::Eq, a: 1, target: 3 },
            Op::Binop { op: BinOp::Add, ty: eightr_ir::op::NumType::Int, dst: 1, a: 1, b: Operand::Lit(-1) },
            Op::Return { width: Width::Single, src: 1 },
            Op::Const { dst: 0, value: Const::Narrow(0) },
            Op::Throw { src: 0 },
        ]);
        assert!(is_ordinal(&ok));
        // `if-nez` (throws for every value but 0) has the same ops but isn't `ordinal`.
        let mut bad = ok.clone();
        bad.insns[0].op = Op::IfZ { cond: Cond::Ne, a: 1, target: 3 };
        assert!(!is_ordinal(&bad));
    }

    /// `name()` whose null default is its own `const 0; throw` block (R8's usual shape; review
    /// finding), and a string switch (no valueOf message) that must not prove anything.
    #[test]
    fn chain_with_throw_block_default_and_string_switch() {
        let mut syms = Interner::default();
        let [a, b, c] = ["A", "B", "C"].map(|n| syms.intern(n));
        // switch-like chain on p0: 1 → "A", 2 → "B", else (3) → "C"; 0 → const 0; throw.
        let chain = body(3, 1, vec![
            Op::Const { dst: 0, value: Const::Narrow(1) },
            Op::If { cond: Cond::Eq, a: 2, b: 0, target: 9 },
            Op::Const { dst: 0, value: Const::Narrow(2) },
            Op::If { cond: Cond::Eq, a: 2, b: 0, target: 11 },
            Op::Const { dst: 0, value: Const::Narrow(3) },
            Op::If { cond: Cond::Ne, a: 2, b: 0, target: 13 },
            Op::ConstString { dst: 1, value: c },
            Op::Return { width: Width::Object, src: 1 },
            Op::Nop,
            Op::ConstString { dst: 1, value: a },
            Op::Return { width: Width::Object, src: 1 },
            Op::ConstString { dst: 1, value: b },
            Op::Return { width: Width::Object, src: 1 },
            Op::Const { dst: 1, value: Const::Narrow(0) },
            Op::Throw { src: 1 },
        ]);
        let s = S(syms);
        let t = name_tables(&s, &chain);
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(complete(&t[0]), Some(vec!["A".to_string(), "B".to_string(), "C".to_string()]));
        let _ = MethodRef { class: a, name: a, proto: a };
    }
}
