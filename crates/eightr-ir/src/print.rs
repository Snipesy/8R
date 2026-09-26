//! Canonical text rendering of the IR, for snapshot tests and debugging. Output depends only
//! on the IR's content (never on `Sym` ids).

use std::fmt::Write;

use crate::cfg::{Cfg, EdgeKind};
use crate::lift::Body;
use crate::op::*;
use crate::sym::Interner;
use crate::value::{HandleMember, MethodHandleRef};

fn regs(rs: &[Reg]) -> String {
    rs.iter().map(|r| format!("v{r}")).collect::<Vec<_>>().join(", ")
}

fn mem(k: MemKind) -> &'static str {
    match k {
        MemKind::Narrow => "",
        MemKind::Wide => "-wide",
        MemKind::Object => "-object",
        MemKind::Boolean => "-boolean",
        MemKind::Byte => "-byte",
        MemKind::Char => "-char",
        MemKind::Short => "-short",
    }
}

fn width(w: Width) -> &'static str {
    match w {
        Width::Single => "",
        Width::Wide => "-wide",
        Width::Object => "-object",
    }
}

pub fn method_handle(h: &MethodHandleRef, s: &Interner) -> String {
    let member = match h.member {
        HandleMember::Field(f) => format!("{}->{}:{}", s.get(f.class), s.get(f.name), s.get(f.ty)),
        HandleMember::Method(m) => format!("{}->{}{}", s.get(m.class), s.get(m.name), s.get(m.proto)),
    };
    format!("{:?}@{member}", h.kind)
}

pub fn op(o: &Op, s: &Interner) -> String {
    let f = |r: &FieldRef| format!("{}->{}:{}", s.get(r.class), s.get(r.name), s.get(r.ty));
    let m = |r: &MethodRef| format!("{}->{}{}", s.get(r.class), s.get(r.name), s.get(r.proto));
    match o {
        Op::Nop => "nop".into(),
        Op::Move { width: w, dst, src } => format!("move{} v{dst}, v{src}", width(*w)),
        Op::MoveResult { width: w, dst } => format!("move-result{} v{dst}", width(*w)),
        Op::MoveException { dst } => format!("move-exception v{dst}"),
        Op::ReturnVoid => "return-void".into(),
        Op::Return { width: w, src } => format!("return{} v{src}", width(*w)),
        Op::Const { dst, value: Const::Narrow(v) } => format!("const v{dst}, {v}"),
        Op::Const { dst, value: Const::Wide(v) } => format!("const-wide v{dst}, {v}"),
        Op::ConstString { dst, value } => format!("const-string v{dst}, {:?}", s.get(*value)),
        Op::ConstClass { dst, ty } => format!("const-class v{dst}, {}", s.get(*ty)),
        Op::ConstMethodHandle { dst, handle } => format!("const-method-handle v{dst}, {}", method_handle(handle, s)),
        Op::ConstMethodType { dst, proto } => format!("const-method-type v{dst}, {}", s.get(*proto)),
        Op::MonitorEnter { obj } => format!("monitor-enter v{obj}"),
        Op::MonitorExit { obj } => format!("monitor-exit v{obj}"),
        Op::CheckCast { reg, ty } => format!("check-cast v{reg}, {}", s.get(*ty)),
        Op::InstanceOf { dst, obj, ty } => format!("instance-of v{dst}, v{obj}, {}", s.get(*ty)),
        Op::ArrayLength { dst, array } => format!("array-length v{dst}, v{array}"),
        Op::NewInstance { dst, ty } => format!("new-instance v{dst}, {}", s.get(*ty)),
        Op::NewArray { dst, size, ty } => format!("new-array v{dst}, v{size}, {}", s.get(*ty)),
        Op::FilledNewArray { ty, args } => format!("filled-new-array {{{}}}, {}", regs(args), s.get(*ty)),
        Op::FillArrayData { array, element_width, data } => {
            format!("fill-array-data v{array}, width {element_width}, {} bytes", data.len())
        }
        Op::Throw { src } => format!("throw v{src}"),
        Op::Goto { target } => format!("goto @{target}"),
        Op::Switch { src, packed, cases } => {
            let c: Vec<String> = cases.iter().map(|(k, t)| format!("{k} -> @{t}")).collect();
            format!("{}-switch v{src}, [{}]", if *packed { "packed" } else { "sparse" }, c.join(", "))
        }
        Op::Cmp { kind, dst, a, b } => {
            let name = match kind {
                CmpKind::LFloat => "cmpl-float",
                CmpKind::GFloat => "cmpg-float",
                CmpKind::LDouble => "cmpl-double",
                CmpKind::GDouble => "cmpg-double",
                CmpKind::Long => "cmp-long",
            };
            format!("{name} v{dst}, v{a}, v{b}")
        }
        Op::If { cond, a, b, target } => format!("if-{} v{a}, v{b}, @{target}", format!("{cond:?}").to_lowercase()),
        Op::IfZ { cond, a, target } => format!("if-{}z v{a}, @{target}", format!("{cond:?}").to_lowercase()),
        Op::ArrayGet { kind, dst, array, index } => format!("aget{} v{dst}, v{array}, v{index}", mem(*kind)),
        Op::ArrayPut { kind, src, array, index } => format!("aput{} v{src}, v{array}, v{index}", mem(*kind)),
        Op::InstanceGet { kind, dst, obj, field } => format!("iget{} v{dst}, v{obj}, {}", mem(*kind), f(field)),
        Op::InstancePut { kind, src, obj, field } => format!("iput{} v{src}, v{obj}, {}", mem(*kind), f(field)),
        Op::StaticGet { kind, dst, field } => format!("sget{} v{dst}, {}", mem(*kind), f(field)),
        Op::StaticPut { kind, src, field } => format!("sput{} v{src}, {}", mem(*kind), f(field)),
        Op::Invoke { kind, method, args } => {
            format!("invoke-{} {{{}}}, {}", format!("{kind:?}").to_lowercase(), regs(args), m(method))
        }
        Op::InvokePolymorphic { method, proto, args } => {
            format!("invoke-polymorphic {{{}}}, {}, {}", regs(args), m(method), s.get(*proto))
        }
        Op::InvokeCustom { call_site, args } => format!(
            "invoke-custom {{{}}}, {}{} via {}{}",
            regs(args),
            s.get(call_site.name),
            s.get(call_site.proto),
            method_handle(&call_site.bootstrap, s),
            if call_site.extra.is_empty() { String::new() } else { format!(" +{} args", call_site.extra.len()) }
        ),
        Op::Unop { op, dst, src } => format!("{op:?} v{dst}, v{src}"),
        Op::Binop { op, ty, dst, a, b } => {
            let b = match b {
                Operand::Reg(r) => format!("v{r}"),
                Operand::Lit(l) => format!("#{l}"),
            };
            format!("{}-{} v{dst}, v{a}, {b}", format!("{op:?}").to_lowercase(), format!("{ty:?}").to_lowercase())
        }
    }
}

/// Renders a body with its CFG.
pub fn body(b: &Body, cfg: &Cfg, s: &Interner) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "registers {} ins {} outs {}", b.registers, b.ins, b.outs);
    for (bi, blk) in cfg.blocks.iter().enumerate() {
        let succs: Vec<String> = blk
            .succs
            .iter()
            .map(|e| match e.kind {
                EdgeKind::Fallthrough => format!("B{}", e.to),
                EdgeKind::Jump => format!("B{} (goto)", e.to),
                EdgeKind::Taken => format!("B{} (taken)", e.to),
                EdgeKind::Case(k) => format!("B{} (case {k})", e.to),
                EdgeKind::Catch(None) => format!("B{} (catch-all)", e.to),
                EdgeKind::Catch(Some(t)) => format!("B{} (catch {})", e.to, s.get(t)),
            })
            .collect();
        let preds: Vec<String> = blk.preds.iter().map(|p| format!("B{p}")).collect();
        let reach = if cfg.is_reachable(bi as u32) { "" } else { " unreachable" };
        let _ = writeln!(out, "B{bi}{reach}  preds [{}]  succs [{}]", preds.join(", "), succs.join(", "));
        for idx in blk.start..blk.end {
            let insn = &b.insns[idx as usize];
            let _ = writeln!(out, "  @{idx:<3} {:04x}: {}", insn.pc, op(&insn.op, s));
        }
    }
    out
}

pub fn value(v: &crate::value::Value, s: &Interner) -> String {
    use crate::value::Value::*;
    match v {
        Byte(x) => format!("byte {x}"),
        Short(x) => format!("short {x}"),
        Char(x) => format!("char {x}"),
        Int(x) => format!("int {x}"),
        Long(x) => format!("long {x}"),
        Float(b) => format!("float {:#x}", b),
        Double(b) => format!("double {:#x}", b),
        MethodType(p) => format!("method-type {}", s.get(*p)),
        MethodHandle(h) => format!("method-handle {}", method_handle(h, s)),
        String(x) => format!("string {:?}", s.get(*x)),
        Type(t) => format!("type {}", s.get(*t)),
        Field(f) => format!("field {}->{}:{}", s.get(f.class), s.get(f.name), s.get(f.ty)),
        Method(m) => format!("method {}->{}{}", s.get(m.class), s.get(m.name), s.get(m.proto)),
        Enum(f) => format!("enum {}->{}:{}", s.get(f.class), s.get(f.name), s.get(f.ty)),
        Array(a) => format!("[{}]", a.iter().map(|x| value(x, s)).collect::<Vec<_>>().join(", ")),
        Annotation(a) => encoded_annotation(a, s),
        Null => "null".into(),
        Boolean(b) => format!("{b}"),
    }
}

fn encoded_annotation(a: &crate::value::EncodedAnnotation, s: &Interner) -> String {
    // A generic signature is split into arbitrary string pieces; only the joined text is
    // semantic, so render it joined.
    if s.get(a.ty) == "Ldalvik/annotation/Signature;" {
        if let [(n, crate::value::Value::Array(parts))] = a.elements.as_slice() {
            let joined: Option<String> = parts
                .iter()
                .map(|p| match p {
                    crate::value::Value::String(x) => Some(s.get(*x)),
                    _ => None,
                })
                .collect();
            if let Some(j) = joined {
                return format!("@{}({}=signature {j:?})", s.get(a.ty), s.get(*n));
            }
        }
    }
    // Element order is not semantic (the writer sorts by name), so render sorted.
    let mut els: Vec<String> = a.elements.iter().map(|(n, v)| format!("{}={}", s.get(*n), value(v, s))).collect();
    els.sort();
    format!("@{}({})", s.get(a.ty), els.join(", "))
}

fn annotations(out: &mut String, indent: &str, anns: &[crate::value::Annotation], s: &Interner) {
    // Set order is not semantic either.
    let mut v: Vec<String> = anns.iter().map(|a| format!("{:?} {}", a.visibility, encoded_annotation(&a.annotation, s))).collect();
    v.sort();
    for a in v {
        let _ = writeln!(out, "{indent}{a}");
    }
}

/// Canonical rendering of a method body without pcs (which change on re-encoding): ops,
/// tries, positions, locals, parameter names.
pub fn body_semantic(b: &Body, s: &Interner) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "registers {} ins {} outs {}", b.registers, b.ins, b.outs);
    for (i, insn) in b.insns.iter().enumerate() {
        let _ = writeln!(out, "  @{i:<3} {}", op(&insn.op, s));
    }
    for t in &b.tries {
        let hs: Vec<String> = t
            .handlers
            .iter()
            .map(|h| format!("{} -> @{}", h.ty.map_or("<any>", |t| s.get(t)), h.target))
            .collect();
        let _ = writeln!(out, "  try @{}..@{}: {}", t.start, t.end, hs.join(", "));
    }
    for (i, line) in &b.positions {
        let _ = writeln!(out, "  line @{i} {line}");
    }
    for l in &b.locals {
        let name = l.name.map_or("?", |n| s.get(n));
        let ty = l.ty.map_or("?", |n| s.get(n));
        let sig = l.signature.map(|n| format!(" sig {}", s.get(n))).unwrap_or_default();
        let _ = writeln!(out, "  local v{} {name} {ty}{sig} @{}..@{}", l.reg, l.start, l.end);
    }
    if !b.parameter_names.is_empty() {
        let names: Vec<&str> = b.parameter_names.iter().map(|n| n.map_or("?", |n| s.get(n))).collect();
        let _ = writeln!(out, "  params {}", names.join(", "));
    }
    out
}

/// Canonical rendering of one field (declaration, static value, annotations).
pub fn field(f: &crate::model::Field, s: &Interner) -> String {
    let mut o = format!("  field {}:{} access {:#x}", s.get(f.name), s.get(f.ty), f.access);
    // An explicit default and an absent initial value mean the same thing.
    if let Some(v) = f.static_value.as_ref().filter(|v| !crate::value::is_default(v)) {
        o.push_str(&format!(" = {}", value(v, s)));
    }
    o.push('\n');
    annotations(&mut o, "    ", &f.annotations, s);
    o
}

/// Canonical rendering of one method (signature, annotations, body).
pub fn method(m: &crate::model::Method, s: &Interner) -> String {
    let mut o = format!("  method {}{} access {:#x}\n", s.get(m.name), s.get(m.proto), m.access);
    annotations(&mut o, "    ", &m.annotations, s);
    if let Some(ps) = &m.parameter_annotations {
        for (i, set) in ps.iter().enumerate() {
            let _ = writeln!(o, "    param {i}:");
            annotations(&mut o, "      ", set, s);
        }
    }
    if let Some(b) = &m.code {
        o.push_str(&body_semantic(b, s));
    }
    o
}

/// Canonical rendering of one class. Member order isn't semantic, so members are sorted.
pub fn class(c: &crate::model::Class, s: &Interner) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "class {} access {:#x}", s.get(c.ty), c.access);
    if let Some(sup) = c.superclass {
        let _ = writeln!(out, "  extends {}", s.get(sup));
    }
    for i in &c.interfaces {
        let _ = writeln!(out, "  implements {}", s.get(*i));
    }
    if let Some(f) = c.source_file {
        let _ = writeln!(out, "  source {:?}", s.get(f));
    }
    annotations(&mut out, "  ", &c.annotations, s);
    let mut fields: Vec<String> = c.fields.iter().map(|f| field(f, s)).collect();
    fields.sort();
    out.extend(fields);
    let mut methods: Vec<String> = c.methods.iter().map(|m| method(m, s)).collect();
    methods.sort();
    out.extend(methods);
    out
}

/// Canonical rendering of a whole program. Two programs are semantically equal (for the
/// round-trip tests) iff their renderings are equal.
pub fn program(p: &crate::model::Program) -> String {
    let mut out = String::new();
    for r in &p.retained_strings {
        let _ = writeln!(out, "retain {r:?}");
    }
    for c in &p.classes {
        out.push_str(&class(c, &p.syms));
    }
    out
}
