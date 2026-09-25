//! Lifting a dex `code_item` into a [`Body`]: every index resolved and interned, branch and
//! payload offsets turned into instruction indices, try ranges and debug info attached.

use std::collections::BTreeMap;

use eightr_dex::code::CodeItem;
use eightr_dex::debug::LocalEvent;
use eightr_dex::insn::{Decoded, Instruction, Payload};
use eightr_dex::value::EncodedValue;
use eightr_dex::{Dex, DexError, ErrorKind, Result};

use crate::op::*;
use crate::sym::{Interner, Sym};

#[derive(Debug, Clone, PartialEq)]
pub struct Insn {
    /// Original address in code units (kept for debug info and reporting).
    pub pc: u32,
    pub op: Op,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handler {
    /// Caught type descriptor; `None` for catch-all.
    pub ty: Option<Sym>,
    /// Instruction index of the handler.
    pub target: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryRange {
    /// Instruction index range `[start, end)`.
    pub start: u32,
    pub end: u32,
    pub handlers: Vec<Handler>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub reg: Reg,
    pub name: Option<Sym>,
    pub ty: Option<Sym>,
    pub signature: Option<Sym>,
    /// pc range `[start, end)` where the variable is live.
    pub start_pc: u32,
    pub end_pc: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub registers: u16,
    pub ins: u16,
    pub outs: u16,
    pub insns: Vec<Insn>,
    pub tries: Vec<TryRange>,
    /// (instruction index, line) from the debug info's position table.
    pub positions: Vec<(u32, i64)>,
    pub locals: Vec<Local>,
    pub parameter_names: Vec<Option<Sym>>,
    /// Code length in code units (for closing debug ranges).
    pub code_units: u32,
}

impl Body {
    /// Index of the try range covering instruction `idx`. Dex try ranges don't overlap.
    pub fn try_covering(&self, idx: u32) -> Option<&TryRange> {
        self.tries.iter().find(|t| t.start <= idx && idx < t.end)
    }
}

struct Lifter<'a, 'd> {
    dex: &'a Dex<'d>,
    syms: &'a mut Interner,
    base: usize,
}

impl Lifter<'_, '_> {
    fn err(&self, pc: u32, what: &'static str) -> DexError {
        DexError::new(self.base + pc as usize * 2, ErrorKind::Malformed(what))
    }
    fn string(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.string(idx)?;
        Ok(self.syms.intern(&s))
    }
    fn ty(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.type_descriptor(idx)?;
        Ok(self.syms.intern(&s))
    }
    fn proto(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.proto_descriptor(idx)?;
        Ok(self.syms.intern(&s))
    }
    fn field(&mut self, idx: u32) -> Result<FieldRef> {
        let f = self.dex.field_id(idx)?;
        Ok(FieldRef { class: self.ty(f.class_idx.into())?, name: self.string(f.name_idx)?, ty: self.ty(f.type_idx.into())? })
    }
    fn method(&mut self, idx: u32) -> Result<MethodRef> {
        let m = self.dex.method_id(idx)?;
        Ok(MethodRef { class: self.ty(m.class_idx.into())?, name: self.string(m.name_idx)?, proto: self.proto(m.proto_idx.into())? })
    }
    fn method_handle(&mut self, idx: u32) -> Result<Sym> {
        const KINDS: [&str; 9] = [
            "static-put", "static-get", "instance-put", "instance-get", "invoke-static",
            "invoke-instance", "invoke-constructor", "invoke-direct", "invoke-interface",
        ];
        let h = self.dex.method_handle(idx)?;
        let kind = KINDS.get(h.kind as usize).copied().unwrap_or("unknown");
        let member = if h.kind <= 3 {
            self.dex.field_ref(h.field_or_method_idx.into())?
        } else {
            self.dex.method_ref(h.field_or_method_idx.into())?
        };
        Ok(self.syms.intern(&format!("{kind}@{member}")))
    }
}

/// Resolves a pc to an instruction index, requiring it to be an instruction boundary.
struct PcMap(Vec<u32>);

impl PcMap {
    fn get(&self, pc: i64) -> Option<u32> {
        usize::try_from(pc).ok().and_then(|p| self.0.get(p)).copied().filter(|&i| i != u32::MAX)
    }
}

pub fn lift(dex: &Dex, code: &CodeItem, syms: &mut Interner) -> Result<Body> {
    let decoded = code.decode()?;
    let base = code.off as usize + 16;
    let mut lx = Lifter { dex, syms, base };

    // pc → instruction index (payloads are not instructions).
    let mut pc_map = vec![u32::MAX; code.insns.len() + 1];
    let mut payloads: BTreeMap<u32, &Payload> = BTreeMap::new();
    let mut count = 0u32;
    for d in &decoded {
        match d {
            Decoded::Insn(i) => {
                pc_map[i.pc as usize] = count;
                count += 1;
            }
            Decoded::Payload { pc, payload, .. } => {
                payloads.insert(*pc, payload);
            }
        }
    }
    let pcs = PcMap(pc_map);

    let mut insns = Vec::with_capacity(count as usize);
    for d in &decoded {
        let Decoded::Insn(i) = d else { continue };
        let op = lift_insn(&mut lx, i, &pcs, &payloads)?;
        insns.push(Insn { pc: i.pc, op });
    }

    // Try ranges: pc ranges → instruction index ranges.
    let first_at_or_after = |pc: u32| -> u32 {
        insns.iter().position(|x| x.pc >= pc).map(|p| p as u32).unwrap_or(insns.len() as u32)
    };
    let mut tries = Vec::new();
    for t in &code.tries {
        let start = pcs.get(t.start_addr.into()).ok_or_else(|| lx.err(t.start_addr, "try start is not an instruction"))?;
        let end = first_at_or_after(t.start_addr + u32::from(t.insn_count));
        let h = code.handler(t.handler_off).ok_or_else(|| lx.err(t.start_addr, "missing catch handler"))?;
        let mut handlers = Vec::new();
        for &(ty, addr) in &h.catches {
            let target = pcs.get(addr.into()).ok_or_else(|| lx.err(addr, "handler is not an instruction"))?;
            handlers.push(Handler { ty: Some(lx.ty(ty)?), target });
        }
        if let Some(addr) = h.catch_all {
            let target = pcs.get(addr.into()).ok_or_else(|| lx.err(addr, "handler is not an instruction"))?;
            handlers.push(Handler { ty: None, target });
        }
        tries.push(TryRange { start, end, handlers });
    }

    // Debug info.
    let mut positions = Vec::new();
    let mut locals = Vec::new();
    let mut parameter_names = Vec::new();
    if let Some(dbg) = dex.debug_info(code.debug_info_off)? {
        for p in &dbg.positions {
            if let Some(idx) = pcs.get(p.addr.into()) {
                positions.push((idx, p.line));
            }
        }
        for n in &dbg.parameter_names {
            parameter_names.push(n.map(|i| lx.string(i)).transpose()?);
        }
        let code_units = code.insns.len() as u32;
        // reg → (currently open local, last closed local for DBG_RESTART_LOCAL)
        let mut open: BTreeMap<u32, Local> = BTreeMap::new();
        let mut last: BTreeMap<u32, Local> = BTreeMap::new();
        let close = |open: &mut BTreeMap<u32, Local>, last: &mut BTreeMap<u32, Local>, reg: u32, addr: u32, out: &mut Vec<Local>| {
            if let Some(mut l) = open.remove(&reg) {
                l.end_pc = addr;
                last.insert(reg, l.clone());
                if l.end_pc > l.start_pc {
                    out.push(l);
                }
            }
        };
        for ev in &dbg.locals {
            match *ev {
                LocalEvent::Start { addr, reg, name, ty, sig } => {
                    close(&mut open, &mut last, reg, addr, &mut locals);
                    let local = Local {
                        reg: reg as Reg,
                        name: name.map(|i| lx.string(i)).transpose()?,
                        ty: ty.map(|i| lx.ty(i)).transpose()?,
                        signature: sig.map(|i| lx.string(i)).transpose()?,
                        start_pc: addr,
                        end_pc: code_units,
                    };
                    open.insert(reg, local);
                }
                LocalEvent::End { addr, reg } => close(&mut open, &mut last, reg, addr, &mut locals),
                LocalEvent::Restart { addr, reg } => {
                    close(&mut open, &mut last, reg, addr, &mut locals);
                    if let Some(mut l) = last.get(&reg).cloned() {
                        l.start_pc = addr;
                        l.end_pc = code_units;
                        open.insert(reg, l);
                    }
                }
            }
        }
        for reg in open.keys().copied().collect::<Vec<_>>() {
            close(&mut open, &mut last, reg, code_units, &mut locals);
        }
        locals.sort_by_key(|l| (l.start_pc, l.reg, l.end_pc));
    }

    Ok(Body {
        registers: code.registers_size,
        ins: code.ins_size,
        outs: code.outs_size,
        insns,
        tries,
        positions,
        locals,
        parameter_names,
        code_units: code.insns.len() as u32,
    })
}

const MEM_KINDS: [MemKind; 7] =
    [MemKind::Narrow, MemKind::Wide, MemKind::Object, MemKind::Boolean, MemKind::Byte, MemKind::Char, MemKind::Short];
const INT_OPS: [BinOp; 11] = [
    BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem, BinOp::And, BinOp::Or, BinOp::Xor,
    BinOp::Shl, BinOp::Shr, BinOp::Ushr,
];
const FP_OPS: [BinOp; 5] = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem];
const LIT_OPS: [BinOp; 11] = [
    BinOp::Add, BinOp::Rsub, BinOp::Mul, BinOp::Div, BinOp::Rem, BinOp::And, BinOp::Or, BinOp::Xor,
    BinOp::Shl, BinOp::Shr, BinOp::Ushr,
];
const INVOKES: [InvokeKind; 5] =
    [InvokeKind::Virtual, InvokeKind::Super, InvokeKind::Direct, InvokeKind::Static, InvokeKind::Interface];

/// Binop number `i` within a 32-op block (23x or 2addr).
fn binop32(i: u8) -> (BinOp, NumType) {
    match i {
        0..=10 => (INT_OPS[i as usize], NumType::Int),
        11..=21 => (INT_OPS[i as usize - 11], NumType::Long),
        22..=26 => (FP_OPS[i as usize - 22], NumType::Float),
        _ => (FP_OPS[i as usize - 27], NumType::Double),
    }
}

fn lift_insn(lx: &mut Lifter, i: &Instruction, pcs: &PcMap, payloads: &BTreeMap<u32, &Payload>) -> Result<Op> {
    let r = i.regs.to_vec();
    let reg = |n: usize| r[n];
    let idx = i.index.unwrap_or(0);
    let lit = i.literal.unwrap_or(0);
    let target = |off: i32| -> Result<u32> {
        pcs.get(i64::from(i.pc) + i64::from(off)).ok_or_else(|| lx.err(i.pc, "branch target is not an instruction"))
    };
    let payload = |off: i32| -> Result<&Payload> {
        let pc = i64::from(i.pc) + i64::from(off);
        u32::try_from(pc).ok().and_then(|p| payloads.get(&p).copied()).ok_or_else(|| lx.err(i.pc, "missing payload"))
    };
    let off = i.offset.unwrap_or(0);
    Ok(match i.opcode {
        0x00 => Op::Nop,
        0x01..=0x09 => {
            let width = [Width::Single, Width::Wide, Width::Object][(i.opcode as usize - 1) / 3];
            Op::Move { width, dst: reg(0), src: reg(1) }
        }
        0x0a => Op::MoveResult { width: Width::Single, dst: reg(0) },
        0x0b => Op::MoveResult { width: Width::Wide, dst: reg(0) },
        0x0c => Op::MoveResult { width: Width::Object, dst: reg(0) },
        0x0d => Op::MoveException { dst: reg(0) },
        0x0e => Op::ReturnVoid,
        0x0f => Op::Return { width: Width::Single, src: reg(0) },
        0x10 => Op::Return { width: Width::Wide, src: reg(0) },
        0x11 => Op::Return { width: Width::Object, src: reg(0) },
        0x12..=0x15 => Op::Const { dst: reg(0), value: Const::Narrow(lit as i32) },
        0x16..=0x19 => Op::Const { dst: reg(0), value: Const::Wide(lit) },
        0x1a | 0x1b => Op::ConstString { dst: reg(0), value: lx.string(idx)? },
        0x1c => Op::ConstClass { dst: reg(0), ty: lx.ty(idx)? },
        0x1d => Op::MonitorEnter { obj: reg(0) },
        0x1e => Op::MonitorExit { obj: reg(0) },
        0x1f => Op::CheckCast { reg: reg(0), ty: lx.ty(idx)? },
        0x20 => Op::InstanceOf { dst: reg(0), obj: reg(1), ty: lx.ty(idx)? },
        0x21 => Op::ArrayLength { dst: reg(0), array: reg(1) },
        0x22 => Op::NewInstance { dst: reg(0), ty: lx.ty(idx)? },
        0x23 => Op::NewArray { dst: reg(0), size: reg(1), ty: lx.ty(idx)? },
        0x24 | 0x25 => Op::FilledNewArray { ty: lx.ty(idx)?, args: r },
        0x26 => match payload(off)? {
            Payload::FillArrayData { element_width, data } => {
                Op::FillArrayData { array: reg(0), element_width: *element_width, data: data.clone() }
            }
            _ => return Err(lx.err(i.pc, "fill-array-data points at a switch payload")),
        },
        0x27 => Op::Throw { src: reg(0) },
        0x28..=0x2a => Op::Goto { target: target(off)? },
        0x2b | 0x2c => {
            let cases = match (i.opcode, payload(off)?) {
                (0x2b, Payload::PackedSwitch { first_key, targets }) => targets
                    .iter()
                    .enumerate()
                    .map(|(n, &t)| Ok((first_key.wrapping_add(n as i32), target(t)?)))
                    .collect::<Result<_>>()?,
                (0x2c, Payload::SparseSwitch { keys, targets }) => {
                    keys.iter().zip(targets).map(|(&k, &t)| Ok((k, target(t)?))).collect::<Result<_>>()?
                }
                _ => return Err(lx.err(i.pc, "switch points at the wrong payload kind")),
            };
            Op::Switch { src: reg(0), packed: i.opcode == 0x2b, cases }
        }
        0x2d..=0x31 => {
            let kind = [CmpKind::LFloat, CmpKind::GFloat, CmpKind::LDouble, CmpKind::GDouble, CmpKind::Long]
                [(i.opcode - 0x2d) as usize];
            Op::Cmp { kind, dst: reg(0), a: reg(1), b: reg(2) }
        }
        0x32..=0x37 => Op::If { cond: cond(i.opcode - 0x32), a: reg(0), b: reg(1), target: target(off)? },
        0x38..=0x3d => Op::IfZ { cond: cond(i.opcode - 0x38), a: reg(0), target: target(off)? },
        0x44..=0x51 => {
            let n = i.opcode - 0x44;
            let kind = MEM_KINDS[(n % 7) as usize];
            if n < 7 {
                Op::ArrayGet { kind, dst: reg(0), array: reg(1), index: reg(2) }
            } else {
                Op::ArrayPut { kind, src: reg(0), array: reg(1), index: reg(2) }
            }
        }
        0x52..=0x5f => {
            let n = i.opcode - 0x52;
            let (kind, field) = (MEM_KINDS[(n % 7) as usize], lx.field(idx)?);
            if n < 7 {
                Op::InstanceGet { kind, dst: reg(0), obj: reg(1), field }
            } else {
                Op::InstancePut { kind, src: reg(0), obj: reg(1), field }
            }
        }
        0x60..=0x6d => {
            let n = i.opcode - 0x60;
            let (kind, field) = (MEM_KINDS[(n % 7) as usize], lx.field(idx)?);
            if n < 7 { Op::StaticGet { kind, dst: reg(0), field } } else { Op::StaticPut { kind, src: reg(0), field } }
        }
        0x6e..=0x72 => Op::Invoke { kind: INVOKES[(i.opcode - 0x6e) as usize], method: lx.method(idx)?, args: r },
        0x74..=0x78 => Op::Invoke { kind: INVOKES[(i.opcode - 0x74) as usize], method: lx.method(idx)?, args: r },
        0x7b..=0x8f => Op::Unop { op: UnOp::ALL[(i.opcode - 0x7b) as usize], dst: reg(0), src: reg(1) },
        0x90..=0xaf => {
            let (op, ty) = binop32(i.opcode - 0x90);
            Op::Binop { op, ty, dst: reg(0), a: reg(1), b: Operand::Reg(reg(2)) }
        }
        0xb0..=0xcf => {
            let (op, ty) = binop32(i.opcode - 0xb0);
            Op::Binop { op, ty, dst: reg(0), a: reg(0), b: Operand::Reg(reg(1)) }
        }
        0xd0..=0xd7 => Op::Binop {
            op: LIT_OPS[(i.opcode - 0xd0) as usize],
            ty: NumType::Int,
            dst: reg(0),
            a: reg(1),
            b: Operand::Lit(lit as i32),
        },
        0xd8..=0xe2 => Op::Binop {
            op: LIT_OPS[(i.opcode - 0xd8) as usize],
            ty: NumType::Int,
            dst: reg(0),
            a: reg(1),
            b: Operand::Lit(lit as i32),
        },
        0xfa | 0xfb => Op::InvokePolymorphic {
            method: lx.method(idx)?,
            proto: lx.proto(i.index2.unwrap_or(0))?,
            args: r,
        },
        0xfc | 0xfd => {
            let site = lx.dex.call_site(idx)?;
            let (Some(EncodedValue::MethodHandle(h)), Some(EncodedValue::String(name)), Some(EncodedValue::MethodType(proto))) =
                (site.first(), site.get(1), site.get(2))
            else {
                return Err(lx.err(i.pc, "malformed call site"));
            };
            let (h, name, proto) = (*h, *name, *proto);
            Op::InvokeCustom { name: lx.string(name)?, proto: lx.proto(proto)?, bootstrap: lx.method_handle(h)?, args: r }
        }
        0xfe => Op::ConstMethodHandle { dst: reg(0), handle: lx.method_handle(idx)? },
        0xff => Op::ConstMethodType { dst: reg(0), proto: lx.proto(idx)? },
        _ => return Err(lx.err(i.pc, "unused opcode")),
    })
}

fn cond(n: u8) -> Cond {
    [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Ge, Cond::Gt, Cond::Le][n as usize]
}
