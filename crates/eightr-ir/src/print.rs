//! Canonical text rendering of the IR, for snapshot tests and debugging. Output depends only
//! on the IR's content (never on `Sym` ids).

use std::fmt::Write;

use crate::cfg::{Cfg, EdgeKind};
use crate::lift::Body;
use crate::op::*;
use crate::sym::Interner;

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
        Op::ConstMethodHandle { dst, handle } => format!("const-method-handle v{dst}, {}", s.get(*handle)),
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
        Op::InvokeCustom { name, proto, bootstrap, args } => format!(
            "invoke-custom {{{}}}, {}{} via {}",
            regs(args),
            s.get(*name),
            s.get(*proto),
            s.get(*bootstrap)
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
