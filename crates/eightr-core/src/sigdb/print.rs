//! Name-independent method and class fingerprints (docs/research/sigdb.md §2).
//!
//! A referenced class is *stable* when R8 can't have renamed it: it isn't defined in the program
//! being fingerprinted (app side), or, when building a DB, in the library or any library of the
//! DB universe. Stable references keep their names; program references keep only their shape
//! (`?` for the class, the proto with program types erased). So the fingerprints of a library
//! method are the same in the library's own R8 build and in an app that shrank it, whatever
//! names either gave it.

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{BinOp, Const, InvokeKind, MemKind, Op};
use eightr_ir::types::parse_proto;

/// Number of MinHash values per method.
pub const SKETCH: usize = 16;

/// 64-bit FNV-1a: stable across platforms and Rust versions.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn mix(mut x: u64) -> u64 {
    // splitmix64 finalizer
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A method's fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodPrint {
    pub class: usize,
    pub method: usize,
    /// Enough content for an exact match to mean something (≥ 8 ops, a string, or ≥ 2 stable refs).
    pub informative: bool,
    /// Ops, refs, strings, numbers and erased proto together.
    pub all: u64,
    /// Sorted string literals; 0 when there are none.
    pub strings: u64,
    /// Erased proto.
    pub proto: u64,
    /// MinHash of op 3-grams, ref tokens, strings and numbers (weighted by occurrence).
    pub sketch: [u32; SKETCH],
    /// Program callees in order: (erased call token, callee as referenced).
    pub callees: Vec<(u64, (String, String, String))>,
    /// Program field accesses in order: (erased access token, field as referenced).
    pub fields: Vec<(u64, (String, String, String))>,
}

/// A class's shape: (C2 with static field types, C3 without).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassPrint {
    pub class: usize,
    pub c2: u64,
    pub c3: u64,
}

/// The stable-reference rule both sides use: a reference keeps its name only if it names a
/// platform (android.jar) class; anything an app or library build may rename, or ship itself, is
/// erased. (A class defined in neither is erased on both sides alike.)
pub fn platform_stable(d: &str) -> bool {
    let base = d.trim_start_matches('[');
    !base.starts_with('L') || crate::naming::is_platform_class(base)
}

/// Erases program types in a type descriptor (arrays keep their dimensions).
pub fn erase_type(t: &str, stable: &dyn Fn(&str) -> bool) -> String {
    let dims = t.bytes().take_while(|&b| b == b'[').count();
    let base = &t[dims..];
    if base.starts_with('L') && !stable(base) {
        format!("{}L?;", &t[..dims])
    } else {
        t.to_string()
    }
}

pub fn erase_proto(proto: &str, stable: &dyn Fn(&str) -> bool) -> String {
    match parse_proto(proto) {
        Some((ps, r)) => format!("({}){}", ps.iter().map(|t| erase_type(t, stable)).collect::<String>(), erase_type(r, stable)),
        None => proto.to_string(),
    }
}

fn op_token(op: &Op) -> Option<&'static str> {
    Some(match op {
        Op::Nop | Op::Move { .. } | Op::Goto { .. } => return None,
        Op::MoveResult { .. } => "move-result",
        Op::MoveException { .. } => "move-exception",
        Op::ReturnVoid | Op::Return { .. } => "return",
        Op::Const { .. } => "const",
        Op::ConstString { .. } => "const-string",
        Op::ConstClass { .. } => "const-class",
        Op::ConstMethodHandle { .. } | Op::ConstMethodType { .. } => "const-method",
        Op::MonitorEnter { .. } | Op::MonitorExit { .. } => "monitor",
        Op::CheckCast { .. } => "check-cast",
        Op::InstanceOf { .. } => "instance-of",
        Op::ArrayLength { .. } => "array-length",
        Op::NewInstance { .. } => "new-instance",
        Op::NewArray { .. } => "new-array",
        Op::FilledNewArray { .. } => "filled-new-array",
        Op::FillArrayData { .. } => "fill-array-data",
        Op::Throw { .. } => "throw",
        Op::Switch { .. } => "switch",
        Op::Cmp { .. } => "cmp",
        Op::If { .. } => "if",
        Op::IfZ { .. } => "ifz",
        Op::ArrayGet { .. } => "aget",
        Op::ArrayPut { .. } => "aput",
        Op::InstanceGet { .. } => "iget",
        Op::InstancePut { .. } => "iput",
        Op::StaticGet { .. } => "sget",
        Op::StaticPut { .. } => "sput",
        Op::Invoke { .. } | Op::InvokePolymorphic { .. } | Op::InvokeCustom { .. } => "invoke",
        Op::Unop { .. } => "unop",
        Op::Binop { op, .. } => match op {
            BinOp::Add => "add",
            BinOp::Sub | BinOp::Rsub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Rem => "rem",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Xor => "xor",
            BinOp::Shl => "shl",
            BinOp::Shr => "shr",
            BinOp::Ushr => "ushr",
        },
    })
}

/// Whether an int constant is an app resource id (`0x7fTTEEEE`: package 0x7f, a small type id,
/// entry). The numbers are assigned per app build (library code's `R.id.x` folds into them), so
/// fingerprints keep only that it is one. (aapt's type ids are small: `Integer.MAX_VALUE`,
/// `0x7fffffff`, isn't one.)
pub fn is_app_resource_id(v: i32) -> bool {
    let v = v as u32;
    v >> 24 == 0x7f && (1..=0x40).contains(&((v >> 16) & 0xff))
}

/// The fingerprint token of every app resource id.
const APP_RESOURCE_ID: i64 = 0x7f00_0000;

struct Tokens {
    ops: Vec<&'static str>,
    refs: Vec<String>,
    strings: Vec<String>,
    nums: Vec<i64>,
    stable_refs: usize,
    callees: Vec<(u64, (String, String, String))>,
    fields: Vec<(u64, (String, String, String))>,
}

fn tokens(p: &Model, body: &Body, stable: &dyn Fn(&str) -> bool, reflective: &dyn Fn(u32) -> bool) -> Tokens {
    let s = &p.syms;
    let mut t = Tokens { ops: Vec::new(), refs: Vec::new(), strings: Vec::new(), nums: Vec::new(), stable_refs: 0, callees: Vec::new(), fields: Vec::new() };
    let ty_ref = |t: &mut Tokens, kind: &str, ty: &str| {
        let e = erase_type(ty, stable);
        if e == ty {
            t.stable_refs += 1;
        }
        t.refs.push(format!("{kind}:{e}"));
    };
    for (k, x) in body.insns.iter().enumerate() {
        if let Some(o) = op_token(&x.op) {
            t.ops.push(o);
        }
        match &x.op {
            Op::Const { value, .. } => {
                let v = match value {
                    Const::Narrow(k) if is_app_resource_id(*k) => APP_RESOURCE_ID,
                    Const::Narrow(k) => i64::from(*k),
                    Const::Wide(k) => *k,
                };
                if !(-1..=1).contains(&v) {
                    t.nums.push(v);
                }
            }
            Op::Binop { b: eightr_ir::op::Operand::Lit(v), .. } if !(-1..=1).contains(v) => t.nums.push(i64::from(*v)),
            // A reflective lookup's name string follows the program's renames: erased.
            Op::ConstString { .. } if reflective(k as u32) => t.refs.push("string:?".into()),
            Op::ConstString { value, .. } => t.strings.push(s.get(*value).to_string()),
            Op::ConstClass { ty, .. } => ty_ref(&mut t, "class", s.get(*ty)),
            Op::CheckCast { ty, .. } => ty_ref(&mut t, "cast", s.get(*ty)),
            Op::InstanceOf { ty, .. } => ty_ref(&mut t, "instanceof", s.get(*ty)),
            Op::NewInstance { ty, .. } => ty_ref(&mut t, "new", s.get(*ty)),
            Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => ty_ref(&mut t, "newarray", s.get(*ty)),
            Op::InstanceGet { field, kind, .. } | Op::InstancePut { field, kind, .. } | Op::StaticGet { field, kind, .. } | Op::StaticPut { field, kind, .. } => {
                let dir = match &x.op {
                    Op::InstanceGet { .. } => "iget",
                    Op::InstancePut { .. } => "iput",
                    Op::StaticGet { .. } => "sget",
                    _ => "sput",
                };
                let _: &MemKind = kind;
                let (c, n, ft) = (s.get(field.class), s.get(field.name), s.get(field.ty));
                if stable(c.trim_start_matches('[')) {
                    t.stable_refs += 1;
                    t.refs.push(format!("{dir}:{c}->{n}:{ft}"));
                } else {
                    let tok = format!("{dir}:?:{}", erase_type(ft, stable));
                    t.fields.push((fnv(tok.as_bytes()), (c.to_string(), n.to_string(), ft.to_string())));
                    t.refs.push(tok);
                }
            }
            Op::Invoke { kind, method, .. } => {
                let (c, n, pr) = (s.get(method.class), s.get(method.name), s.get(method.proto));
                // An array receiver (`[LFoo;->clone()`) is as stable as its element type.
                let c_stable = { let base = c.trim_start_matches('['); !base.starts_with('L') || stable(base) };
                let k = match kind {
                    InvokeKind::Static => "S",
                    InvokeKind::Direct => "D",
                    InvokeKind::Super => "U",
                    _ => "V",
                };
                if c_stable {
                    t.stable_refs += 1;
                    t.refs.push(format!("{k}:{}->{n}{}", erase_type(c, stable), erase_proto(pr, stable)));
                } else {
                    // Constructors keep their name: R8 can't rename <init>.
                    let name = if n == "<init>" || n == "<clinit>" { n } else { "?" };
                    let tok = format!("{k}:?{name}{}", erase_proto(pr, stable));
                    t.callees.push((fnv(tok.as_bytes()), (c.to_string(), n.to_string(), pr.to_string())));
                    t.refs.push(tok);
                }
            }
            Op::InvokeCustom { call_site, .. } => t.refs.push(format!("indy:{}", erase_proto(s.get(call_site.proto), stable))),
            Op::InvokePolymorphic { method, .. } => t.refs.push(format!("poly:{}->{}", s.get(method.class), s.get(method.name))),
            _ => {}
        }
    }
    for tr in &body.tries {
        for h in &tr.handlers {
            match h.ty {
                Some(ty) => ty_ref(&mut t, "catch", s.get(ty)),
                None => t.refs.push("catch:*".into()),
            }
        }
    }
    t.strings.sort();
    t.nums.sort();
    t
}

fn hash_parts(parts: &[&str]) -> u64 {
    fnv(parts.join("\u{1}").as_bytes())
}

/// `const-string`s naming members or classes for reflection (atomic field updaters,
/// `Class.forName`, ...): R8 rewrites them with its renames. (class, method, instruction).
pub fn reflective_strings(p: &Model) -> BTreeSet<(usize, usize, u32)> {
    eightr_ir::reflect::sites(p).into_iter().filter_map(|x| x.string_insn.map(|i| (x.class, x.method, i))).collect()
}

/// The fingerprint of one method body. `reflective` is [`reflective_strings`] of `p`.
pub fn method_print(p: &Model, ci: usize, mi: usize, stable: &dyn Fn(&str) -> bool, reflective: &BTreeSet<(usize, usize, u32)>) -> Option<MethodPrint> {
    let m = &p.classes[ci].methods[mi];
    let body = m.code.as_ref()?;
    let t = tokens(p, body, stable, &|k| reflective.contains(&(ci, mi, k)));
    let proto = erase_proto(p.syms.get(m.proto), stable);
    let ops = t.ops.join(" ");
    let refs = t.refs.join(" ");
    let strings = t.strings.join("\u{2}");
    let nums = t.nums.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let all = hash_parts(&[&ops, &refs, &strings, &nums, &proto]);
    // Weighted set: each element with its occurrence number.
    let mut elems: BTreeMap<String, u32> = BTreeMap::new();
    let mut add = |e: String| *elems.entry(e).or_default() += 1;
    for w in t.ops.windows(3) {
        add(format!("g:{} {} {}", w[0], w[1], w[2]));
    }
    for r in &t.refs {
        add(format!("r:{r}"));
    }
    for st in &t.strings {
        add(format!("s:{st}"));
    }
    for n in &t.nums {
        add(format!("n:{n}"));
    }
    let hashes: Vec<u64> = elems.iter().flat_map(|(e, &n)| (0..n).map(move |k| fnv(format!("{e}#{k}").as_bytes()))).collect();
    let mut sketch = [u32::MAX; SKETCH];
    for (i, slot) in sketch.iter_mut().enumerate() {
        let seed = mix(i as u64 + 1);
        if let Some(min) = hashes.iter().map(|h| mix(h ^ seed)).min() {
            *slot = (min >> 32) as u32;
        }
    }
    Some(MethodPrint {
        class: ci,
        method: mi,
        informative: t.ops.len() >= 8 || !t.strings.is_empty() || t.stable_refs >= 2,
        all,
        strings: if t.strings.is_empty() { 0 } else { fnv(strings.as_bytes()) },
        proto: fnv(proto.as_bytes()),
        sketch,
        callees: t.callees,
        fields: t.fields,
    })
}

/// Estimated weighted Jaccard similarity of two sketches.
pub fn similarity(a: &[u32; SKETCH], b: &[u32; SKETCH]) -> f64 {
    a.iter().zip(b).filter(|(x, y)| x == y && **x != u32::MAX).count() as f64 / SKETCH as f64
}

/// Class shape: first stable ancestor, stable interfaces, erased field types (C2: all fields;
/// C3: instance fields only).
pub fn class_print(p: &Model, ci: usize, stable: &dyn Fn(&str) -> bool) -> ClassPrint {
    let s = &p.syms;
    let c = &p.classes[ci];
    // First stable ancestor, walking program superclasses.
    let mut sup = c.superclass.map(|t| s.get(t).to_string());
    let mut guard = 0;
    while let Some(t) = &sup {
        if stable(t) || guard > 64 {
            break;
        }
        guard += 1;
        sup = p.find(t).and_then(|k| p.classes[k].superclass).map(|x| s.get(x).to_string());
    }
    let mut ifaces: Vec<String> = c.interfaces.iter().map(|t| s.get(*t)).filter(|t| stable(t)).map(str::to_string).collect();
    ifaces.sort();
    let is_static = |f: &eightr_ir::model::Field| f.access & eightr_dex::class::access::STATIC != 0;
    let mut stat: Vec<String> = c.fields.iter().filter(|f| is_static(f)).map(|f| erase_type(s.get(f.ty), stable)).collect();
    let mut inst: Vec<String> = c.fields.iter().filter(|f| !is_static(f)).map(|f| erase_type(s.get(f.ty), stable)).collect();
    stat.sort();
    inst.sort();
    let sup = sup.unwrap_or_default();
    let base = [sup.as_str(), &ifaces.join(","), &inst.join(",")];
    // The kind is part of the exact shape: an abstract class merged into its only subclass, whose
    // own members were all inlined, has the abstract class's fields and protos but is concrete.
    let kind = c.access & (eightr_dex::class::access::ABSTRACT | eightr_dex::class::access::INTERFACE);
    ClassPrint { class: ci, c2: hash_parts(&[base[0], base[1], base[2], &stat.join(","), &kind.to_string()]), c3: hash_parts(&base) }
}

#[cfg(test)]
mod resource_id_tests {
    #[test]
    fn app_resource_ids() {
        assert!(super::is_app_resource_id(0x7f0a01e1));
        assert!(!super::is_app_resource_id(0x7fffffff));
        assert!(!super::is_app_resource_id(0x7f000000));
        assert!(!super::is_app_resource_id(0x0101_0000));
    }
}
