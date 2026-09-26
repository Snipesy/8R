//! Byte-level encoders: LEB128, encoded values, instructions (with branch relaxation), and
//! debug info.

use eightr_ir::lift::Body;
use eightr_ir::op::*;
use eightr_ir::value::{EncodedAnnotation, Value};
use eightr_ir::Interner;

use crate::pool::Pools;
use crate::WriteError;

pub fn uleb(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn sleb(out: &mut Vec<u8>, mut v: i32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        out.push(if done { b } else { b | 0x80 });
        if done {
            return;
        }
    }
}

pub fn uleb_p1(out: &mut Vec<u8>, v: Option<u32>) {
    uleb(out, v.map_or(0, |x| x + 1));
}

/// Fewest bytes that sign-extend back to `v`.
fn signed_len(v: i64) -> usize {
    (1..=8).find(|&n| {
        let shift = 64 - 8 * n as u32;
        (v << shift) >> shift == v
    }).expect("8 bytes always fit")
}

fn unsigned_len(v: u64) -> usize {
    (1..=8).find(|&n| n == 8 || v >> (8 * n) == 0).expect("8 bytes always fit")
}

fn put(out: &mut Vec<u8>, ty: u8, v: u64, len: usize) {
    out.push(((len as u8 - 1) << 5) | ty);
    for i in 0..len {
        out.push((v >> (8 * i)) as u8);
    }
}

/// Floats/doubles are stored right-zero-extended: drop low-order zero bytes.
fn put_right(out: &mut Vec<u8>, ty: u8, bits: u64, width: usize) {
    let mut len = width;
    let mut v = bits;
    while len > 1 && v & 0xff == 0 {
        v >>= 8;
        len -= 1;
    }
    put(out, ty, v, len);
}

pub fn value(out: &mut Vec<u8>, v: &Value, p: &Pools, s: &Interner) {
    match v {
        Value::Byte(x) => put(out, 0x00, *x as u8 as u64, 1),
        Value::Short(x) => put(out, 0x02, i64::from(*x) as u64, signed_len(i64::from(*x))),
        Value::Char(x) => put(out, 0x03, u64::from(*x), unsigned_len(u64::from(*x))),
        Value::Int(x) => put(out, 0x04, i64::from(*x) as u64, signed_len(i64::from(*x))),
        Value::Long(x) => put(out, 0x06, *x as u64, signed_len(*x)),
        Value::Float(b) => put_right(out, 0x10, u64::from(*b), 4),
        Value::Double(b) => put_right(out, 0x11, *b, 8),
        Value::MethodType(x) => index(out, 0x15, p.proto(s.get(*x))),
        Value::MethodHandle(h) => index(out, 0x16, p.handle(h, s)),
        Value::String(x) => index(out, 0x17, p.string(s.get(*x))),
        Value::Type(x) => index(out, 0x18, p.ty(s.get(*x))),
        Value::Field(f) => index(out, 0x19, p.field(f, s)),
        Value::Method(m) => index(out, 0x1a, p.method(m, s)),
        Value::Enum(f) => index(out, 0x1b, p.field(f, s)),
        Value::Array(a) => {
            out.push(0x1c);
            array(out, a, p, s);
        }
        Value::Annotation(a) => {
            out.push(0x1d);
            encoded_annotation(out, a, p, s);
        }
        Value::Null => out.push(0x1e),
        Value::Boolean(b) => out.push(0x1f | (u8::from(*b) << 5)),
    }
}

fn index(out: &mut Vec<u8>, ty: u8, i: u32) {
    put(out, ty, u64::from(i), unsigned_len(u64::from(i)));
}

pub fn array(out: &mut Vec<u8>, a: &[Value], p: &Pools, s: &Interner) {
    uleb(out, a.len() as u32);
    for x in a {
        value(out, x, p, s);
    }
}

pub fn encoded_annotation(out: &mut Vec<u8>, a: &EncodedAnnotation, p: &Pools, s: &Interner) {
    uleb(out, p.ty(s.get(a.ty)));
    let mut els: Vec<(u32, &Value)> = a.elements.iter().map(|(n, v)| (p.string(s.get(*n)), v)).collect();
    els.sort_by_key(|(n, _)| *n);
    uleb(out, els.len() as u32);
    for (n, v) in els {
        uleb(out, n);
        value(out, v, p, s);
    }
}

// ---- instructions ----

fn fits4(r: Reg) -> bool {
    r < 16
}
fn fits8(r: Reg) -> bool {
    r < 256
}
fn fits_i8(v: i64) -> bool {
    (-128..=127).contains(&v)
}
fn fits_i16(v: i64) -> bool {
    (-32768..=32767).contains(&v)
}

const INT_OPS: [BinOp; 11] = [
    BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem, BinOp::And, BinOp::Or, BinOp::Xor,
    BinOp::Shl, BinOp::Shr, BinOp::Ushr,
];
const LIT_OPS: [BinOp; 11] = [
    BinOp::Add, BinOp::Rsub, BinOp::Mul, BinOp::Div, BinOp::Rem, BinOp::And, BinOp::Or, BinOp::Xor,
    BinOp::Shl, BinOp::Shr, BinOp::Ushr,
];

fn binop_index(op: BinOp, ty: NumType) -> Option<u8> {
    let i = INT_OPS.iter().position(|&o| o == op)? as u8;
    match ty {
        NumType::Int => Some(i),
        NumType::Long => Some(11 + i),
        NumType::Float if i < 5 => Some(22 + i),
        NumType::Double if i < 5 => Some(27 + i),
        _ => None,
    }
}

fn mem_index(k: MemKind) -> u16 {
    match k {
        MemKind::Narrow => 0,
        MemKind::Wide => 1,
        MemKind::Object => 2,
        MemKind::Boolean => 3,
        MemKind::Byte => 4,
        MemKind::Char => 5,
        MemKind::Short => 6,
    }
}

fn width_index(w: Width) -> u16 {
    match w {
        Width::Single => 0,
        Width::Wide => 1,
        Width::Object => 2,
    }
}

fn cond_index(c: Cond) -> u16 {
    [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Ge, Cond::Gt, Cond::Le].iter().position(|&x| x == c).unwrap() as u16
}

/// A payload attached to an instruction.
enum Payload<'a> {
    Switch(&'a [(i32, u32)], bool),
    Fill(u16, &'a [u8]),
}

fn is_packable(cases: &[(i32, u32)]) -> bool {
    cases.windows(2).all(|w| w[0].0.checked_add(1) == Some(w[1].0))
}

struct Enc<'a> {
    p: &'a Pools,
    s: &'a Interner,
    out: Vec<u16>,
}

impl Enc<'_> {
    fn u(&mut self, x: u16) {
        self.out.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.out.push(x as u16);
        self.out.push((x >> 16) as u16);
    }
    fn idx16(&self, i: u32, what: &str) -> Result<u16, WriteError> {
        u16::try_from(i).map_err(|_| WriteError(format!("{what} index {i} exceeds 16 bits (needs multidex)")))
    }
    fn regs35(&mut self, op: u16, idx: u16, args: &[Reg], extra: Option<u16>) {
        let g = if args.len() == 5 { args[4] } else { 0 };
        self.u(op | (g << 8) | ((args.len() as u16) << 12));
        self.u(idx);
        let r = |i: usize| args.get(i).copied().unwrap_or(0);
        self.u(r(0) | (r(1) << 4) | (r(2) << 8) | (r(3) << 12));
        if let Some(e) = extra {
            self.u(e);
        }
    }
    /// Encodes an invoke-style instruction: 35c if it fits, else 3rc (needs contiguous regs).
    fn invoke(&mut self, op35: u16, op3r: u16, idx: u16, args: &[Reg], extra: Option<u16>) -> Result<(), WriteError> {
        if args.len() <= 5 && args.iter().all(|&r| fits4(r)) {
            self.regs35(op35, idx, args, extra);
            return Ok(());
        }
        let contiguous = args.windows(2).all(|w| w[1] == w[0].wrapping_add(1));
        if !contiguous || args.len() > 255 {
            return Err(WriteError("invoke arguments neither fit 35c nor are contiguous".into()));
        }
        self.u(op3r | ((args.len() as u16) << 8));
        self.u(idx);
        self.u(args.first().copied().unwrap_or(0));
        if let Some(e) = extra {
            self.u(e);
        }
        Ok(())
    }
}

/// Size in code units of a non-branch op (branches are sized by relaxation).
fn fixed_units(op: &Op, p: &Pools, s: &Interner) -> Result<u32, WriteError> {
    let mut e = Enc { p, s, out: Vec::new() };
    encode_op(&mut e, op, 0, &[])?;
    Ok(e.out.len() as u32)
}

/// Encodes `op` at `pc`. `targets` gives the pc of every instruction; payload ops encode a
/// placeholder offset that `encode_body` patches.
fn encode_op(e: &mut Enc, op: &Op, pc: u32, targets: &[u32]) -> Result<(), WriteError> {
    let bad = |what: &str| WriteError(format!("cannot encode {what}"));
    let rel = |t: u32| -> i64 { i64::from(targets.get(t as usize).copied().unwrap_or(0)) - i64::from(pc) };
    let (p, s) = (e.p, e.s);
    match op {
        Op::Nop => e.u(0),
        Op::Move { width, dst, src } => {
            let base = 0x01 + 3 * width_index(*width);
            if fits4(*dst) && fits4(*src) {
                e.u(base | (dst << 8) | (src << 12));
            } else if fits8(*dst) {
                e.u((base + 1) | (dst << 8));
                e.u(*src);
            } else {
                e.u(base + 2);
                e.u(*dst);
                e.u(*src);
            }
        }
        Op::MoveResult { width, dst } if fits8(*dst) => e.u((0x0a + width_index(*width)) | (dst << 8)),
        Op::MoveException { dst } if fits8(*dst) => e.u(0x0d | (dst << 8)),
        Op::ReturnVoid => e.u(0x0e),
        Op::Return { width, src } if fits8(*src) => e.u((0x0f + width_index(*width)) | (src << 8)),
        Op::Const { dst, value: Const::Narrow(v) } => {
            let v64 = i64::from(*v);
            if fits4(*dst) && (-8..=7).contains(&v64) {
                e.u(0x12 | (dst << 8) | (((*v as u16) & 0xf) << 12));
            } else if !fits8(*dst) {
                return Err(bad("const into register >= 256"));
            } else if fits_i16(v64) {
                e.u(0x13 | (dst << 8));
                e.u(*v as u16);
            } else if v & 0xffff == 0 {
                e.u(0x15 | (dst << 8));
                e.u((*v >> 16) as u16);
            } else {
                e.u(0x14 | (dst << 8));
                e.u32(*v as u32);
            }
        }
        Op::Const { dst, value: Const::Wide(v) } if fits8(*dst) => {
            if fits_i16(*v) {
                e.u(0x16 | (dst << 8));
                e.u(*v as u16);
            } else if i32::try_from(*v).is_ok() {
                e.u(0x17 | (dst << 8));
                e.u32(*v as u32);
            } else if v & 0xffff_ffff_ffff == 0 {
                e.u(0x19 | (dst << 8));
                e.u((*v >> 48) as u16);
            } else {
                e.u(0x18 | (dst << 8));
                e.u32(*v as u32);
                e.u32((*v >> 32) as u32);
            }
        }
        Op::ConstString { dst, value } if fits8(*dst) => {
            let i = p.string(s.get(*value));
            if let Ok(i16) = u16::try_from(i) {
                e.u(0x1a | (dst << 8));
                e.u(i16);
            } else {
                e.u(0x1b | (dst << 8));
                e.u32(i);
            }
        }
        Op::ConstClass { dst, ty } if fits8(*dst) => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.u(0x1c | (dst << 8));
            e.u(i);
        }
        Op::MonitorEnter { obj } if fits8(*obj) => e.u(0x1d | (obj << 8)),
        Op::MonitorExit { obj } if fits8(*obj) => e.u(0x1e | (obj << 8)),
        Op::CheckCast { reg, ty } if fits8(*reg) => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.u(0x1f | (reg << 8));
            e.u(i);
        }
        Op::InstanceOf { dst, obj, ty } if fits4(*dst) && fits4(*obj) => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.u(0x20 | (dst << 8) | (obj << 12));
            e.u(i);
        }
        Op::ArrayLength { dst, array } if fits4(*dst) && fits4(*array) => e.u(0x21 | (dst << 8) | (array << 12)),
        Op::NewInstance { dst, ty } if fits8(*dst) => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.u(0x22 | (dst << 8));
            e.u(i);
        }
        Op::NewArray { dst, size, ty } if fits4(*dst) && fits4(*size) => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.u(0x23 | (dst << 8) | (size << 12));
            e.u(i);
        }
        Op::FilledNewArray { ty, args } => {
            let i = e.idx16(p.ty(s.get(*ty)), "type")?;
            e.invoke(0x24, 0x25, i, args, None)?;
        }
        Op::FillArrayData { array, .. } if fits8(*array) => {
            e.u(0x26 | (array << 8));
            e.u32(0); // patched
        }
        Op::Throw { src } if fits8(*src) => e.u(0x27 | (src << 8)),
        Op::Goto { target } => {
            let off = rel(*target);
            if off != 0 && fits_i8(off) {
                e.u(0x28 | (((off as i8) as u8 as u16) << 8));
            } else if off != 0 && fits_i16(off) {
                e.u(0x29);
                e.u(off as u16);
            } else {
                e.u(0x2a);
                e.u32(off as u32);
            }
        }
        Op::Switch { src, packed, cases } if fits8(*src) => {
            let op = if *packed && is_packable(cases) { 0x2b } else { 0x2c };
            e.u(op | (src << 8));
            e.u32(0); // patched
        }
        Op::Cmp { kind, dst, a, b } if fits8(*dst) && fits8(*a) && fits8(*b) => {
            let k = [CmpKind::LFloat, CmpKind::GFloat, CmpKind::LDouble, CmpKind::GDouble, CmpKind::Long]
                .iter()
                .position(|x| x == kind)
                .unwrap() as u16;
            e.u((0x2d + k) | (dst << 8));
            e.u(a | (b << 8));
        }
        Op::If { cond, a, b, target } if fits4(*a) && fits4(*b) => {
            let off = rel(*target);
            if !targets.is_empty() && (off == 0 || !fits_i16(off)) {
                return Err(bad("if-test branch offset (zero or beyond 16 bits)"));
            }
            e.u((0x32 + cond_index(*cond)) | (a << 8) | (b << 12));
            e.u(off as u16);
        }
        Op::IfZ { cond, a, target } if fits8(*a) => {
            let off = rel(*target);
            if !targets.is_empty() && (off == 0 || !fits_i16(off)) {
                return Err(bad("if-testz branch offset (zero or beyond 16 bits)"));
            }
            e.u((0x38 + cond_index(*cond)) | (a << 8));
            e.u(off as u16);
        }
        Op::ArrayGet { kind, dst, array, index } if fits8(*dst) && fits8(*array) && fits8(*index) => {
            e.u((0x44 + mem_index(*kind)) | (dst << 8));
            e.u(array | (index << 8));
        }
        Op::ArrayPut { kind, src, array, index } if fits8(*src) && fits8(*array) && fits8(*index) => {
            e.u((0x4b + mem_index(*kind)) | (src << 8));
            e.u(array | (index << 8));
        }
        Op::InstanceGet { kind, dst, obj, field } if fits4(*dst) && fits4(*obj) => {
            let i = e.idx16(p.field(field, s), "field")?;
            e.u((0x52 + mem_index(*kind)) | (dst << 8) | (obj << 12));
            e.u(i);
        }
        Op::InstancePut { kind, src, obj, field } if fits4(*src) && fits4(*obj) => {
            let i = e.idx16(p.field(field, s), "field")?;
            e.u((0x59 + mem_index(*kind)) | (src << 8) | (obj << 12));
            e.u(i);
        }
        Op::StaticGet { kind, dst, field } if fits8(*dst) => {
            let i = e.idx16(p.field(field, s), "field")?;
            e.u((0x60 + mem_index(*kind)) | (dst << 8));
            e.u(i);
        }
        Op::StaticPut { kind, src, field } if fits8(*src) => {
            let i = e.idx16(p.field(field, s), "field")?;
            e.u((0x67 + mem_index(*kind)) | (src << 8));
            e.u(i);
        }
        Op::Invoke { kind, method, args } => {
            let k = [InvokeKind::Virtual, InvokeKind::Super, InvokeKind::Direct, InvokeKind::Static, InvokeKind::Interface]
                .iter()
                .position(|x| x == kind)
                .unwrap() as u16;
            let i = e.idx16(p.method(method, s), "method")?;
            e.invoke(0x6e + k, 0x74 + k, i, args, None)?;
        }
        Op::InvokePolymorphic { method, proto, args } => {
            let i = e.idx16(p.method(method, s), "method")?;
            let pr = e.idx16(p.proto(s.get(*proto)), "proto")?;
            e.invoke(0xfa, 0xfb, i, args, Some(pr))?;
        }
        Op::InvokeCustom { call_site, args } => {
            let i = e.idx16(p.call_site(call_site), "call site")?;
            e.invoke(0xfc, 0xfd, i, args, None)?;
        }
        Op::ConstMethodHandle { dst, handle } if fits8(*dst) => {
            let i = e.idx16(p.handle(handle, s), "method handle")?;
            e.u(0xfe | (dst << 8));
            e.u(i);
        }
        Op::ConstMethodType { dst, proto } if fits8(*dst) => {
            let i = e.idx16(p.proto(s.get(*proto)), "proto")?;
            e.u(0xff | (dst << 8));
            e.u(i);
        }
        Op::Unop { op, dst, src } if fits4(*dst) && fits4(*src) => {
            let i = UnOp::ALL.iter().position(|x| x == op).unwrap() as u16;
            e.u((0x7b + i) | (dst << 8) | (src << 12));
        }
        Op::Binop { op, ty, dst, a, b: Operand::Reg(b) } => {
            let i = u16::from(binop_index(*op, *ty).ok_or_else(|| bad("binop/type combination"))?);
            if dst == a && fits4(*dst) && fits4(*b) {
                e.u((0xb0 + i) | (dst << 8) | (b << 12));
            } else if fits8(*dst) && fits8(*a) && fits8(*b) {
                e.u((0x90 + i) | (dst << 8));
                e.u(a | (b << 8));
            } else {
                return Err(bad("binop registers"));
            }
        }
        Op::Binop { op, ty: NumType::Int, dst, a, b: Operand::Lit(lit) } => {
            let i = LIT_OPS.iter().position(|x| x == op).ok_or_else(|| bad("literal binop"))? as u16;
            let lit = i64::from(*lit);
            if fits_i8(lit) && fits8(*dst) && fits8(*a) {
                e.u((0xd8 + i) | (dst << 8));
                e.u(a | (((lit as i8) as u8 as u16) << 8));
            } else if i < 8 && fits_i16(lit) && fits4(*dst) && fits4(*a) {
                e.u((0xd0 + i) | (dst << 8) | (a << 12));
                e.u(lit as u16);
            } else {
                return Err(bad("literal binop operands"));
            }
        }
        other => return Err(WriteError(format!("cannot encode {other:?} (register or index out of range)"))),
    }
    Ok(())
}

pub struct EncodedCode {
    pub insns: Vec<u16>,
    /// New pc of every instruction, plus the end of instructions (before payloads).
    pub pcs: Vec<u32>,
}

/// Encodes a body's instructions: sizes branches by iterative relaxation, then appends
/// payloads (each at an even pc).
pub fn encode_body(body: &Body, p: &Pools, s: &Interner) -> Result<EncodedCode, WriteError> {
    let n = body.insns.len();
    let mut sizes: Vec<u32> = Vec::with_capacity(n);
    for i in &body.insns {
        sizes.push(match &i.op {
            Op::Goto { .. } => 1,
            _ => fixed_units(&i.op, p, s)?,
        });
    }
    // Relaxation: grow gotos until every offset fits. Sizes only increase, so this ends.
    let pcs = loop {
        let mut pcs = Vec::with_capacity(n + 1);
        let mut pc = 0u32;
        for &sz in &sizes {
            pcs.push(pc);
            pc += sz;
        }
        pcs.push(pc);
        let mut changed = false;
        for (i, insn) in body.insns.iter().enumerate() {
            if let Op::Goto { target } = insn.op {
                let off = i64::from(pcs[target as usize]) - i64::from(pcs[i]);
                let need = if off != 0 && fits_i8(off) {
                    1
                } else if off != 0 && fits_i16(off) {
                    2
                } else {
                    3
                };
                if need > sizes[i] {
                    sizes[i] = need;
                    changed = true;
                }
            }
        }
        if !changed {
            break pcs;
        }
    };

    let mut e = Enc { p, s, out: Vec::with_capacity(pcs[n] as usize) };
    let mut patches: Vec<(usize, u32, Payload)> = Vec::new(); // (unit index of offset, insn pc, payload)
    for (i, insn) in body.insns.iter().enumerate() {
        let before = e.out.len();
        encode_op(&mut e, &insn.op, pcs[i], &pcs)?;
        // Offsets only grow during relaxation, so each op re-encodes at its relaxed size.
        if e.out.len() - before != sizes[i] as usize {
            return Err(WriteError(format!("internal: instruction {i} changed size after relaxation")));
        }
        match &insn.op {
            Op::Switch { cases, packed, .. } => {
                patches.push((before + 1, pcs[i], Payload::Switch(cases, *packed && is_packable(cases))))
            }
            Op::FillArrayData { element_width, data, .. } => {
                patches.push((before + 1, pcs[i], Payload::Fill(*element_width, data)))
            }
            _ => {}
        }
    }
    for (at, insn_pc, payload) in patches {
        if e.out.len() % 2 == 1 {
            e.u(0); // nop: payloads must be 4-byte aligned
        }
        let payload_pc = e.out.len() as u32;
        let off = payload_pc.wrapping_sub(insn_pc);
        e.out[at] = off as u16;
        e.out[at + 1] = (off >> 16) as u16;
        match payload {
            Payload::Switch(cases, true) => {
                e.u(0x0100);
                e.u(cases.len() as u16);
                e.u32(cases.first().map_or(0, |c| c.0) as u32);
                for &(_, t) in cases {
                    e.u32(pcs[t as usize].wrapping_sub(insn_pc));
                }
            }
            Payload::Switch(cases, false) => {
                let mut sorted: Vec<(i32, u32)> = cases.to_vec();
                sorted.sort_by_key(|c| c.0);
                e.u(0x0200);
                e.u(sorted.len() as u16);
                for &(k, _) in &sorted {
                    e.u32(k as u32);
                }
                for &(_, t) in &sorted {
                    e.u32(pcs[t as usize].wrapping_sub(insn_pc));
                }
            }
            Payload::Fill(width, data) => {
                let count = if width == 0 { 0 } else { data.len() as u32 / u32::from(width) };
                e.u(0x0300);
                e.u(width);
                e.u32(count);
                for chunk in data.chunks(2) {
                    e.u(u16::from(chunk[0]) | (u16::from(*chunk.get(1).unwrap_or(&0)) << 8));
                }
            }
        }
    }
    Ok(EncodedCode { insns: e.out, pcs })
}

// ---- debug info ----

const DBG_FIRST_SPECIAL: i64 = 0x0a;
const DBG_LINE_BASE: i64 = -4;
const DBG_LINE_RANGE: i64 = 15;

/// Encodes a `debug_info_item`, or `None` if the body has no debug information.
pub fn debug_info(body: &Body, pcs: &[u32], p: &Pools, s: &Interner) -> Option<Vec<u8>> {
    if body.positions.is_empty() && body.locals.is_empty() && body.parameter_names.is_empty() {
        return None;
    }
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Ev {
        // Ordering at equal pcs: close locals, then positions, then open locals.
        End(u16),
        Line(usize), // index into positions (keeps relative order)
        Start(usize), // index into locals
    }
    let mut events: Vec<(u32, Ev)> = Vec::new();
    for (k, &(idx, _)) in body.positions.iter().enumerate() {
        events.push((pcs[idx as usize], Ev::Line(k)));
    }
    for (k, l) in body.locals.iter().enumerate() {
        events.push((pcs[l.start as usize], Ev::Start(k)));
        events.push((pcs[l.end as usize], Ev::End(l.reg)));
    }
    events.sort();

    let mut out = Vec::new();
    let line_start = body.positions.first().map_or(0, |&(_, l)| l.max(0));
    uleb(&mut out, line_start as u32);
    uleb(&mut out, body.parameter_names.len() as u32);
    for n in &body.parameter_names {
        uleb_p1(&mut out, n.map(|n| p.string(s.get(n))));
    }
    let mut addr: u32 = 0;
    let mut line: i64 = line_start;
    let advance_pc = |out: &mut Vec<u8>, addr: &mut u32, to: u32| {
        if to > *addr {
            out.push(0x01);
            uleb(out, to - *addr);
            *addr = to;
        }
    };
    for (pc, ev) in events {
        match ev {
            Ev::Line(k) => {
                let target = body.positions[k].1;
                let dline = target - line;
                let daddr = i64::from(pc - addr);
                let special = |dl: i64, da: i64| DBG_FIRST_SPECIAL + (dl - DBG_LINE_BASE) + DBG_LINE_RANGE * da;
                if (DBG_LINE_BASE..DBG_LINE_BASE + DBG_LINE_RANGE).contains(&dline) && special(dline, daddr) <= 0xff {
                    out.push(special(dline, daddr) as u8);
                } else {
                    if dline != 0 {
                        out.push(0x02);
                        sleb(&mut out, dline as i32);
                    }
                    advance_pc(&mut out, &mut addr, pc);
                    out.push(special(0, 0) as u8);
                }
                addr = pc;
                line = target;
            }
            Ev::Start(k) => {
                let l = &body.locals[k];
                advance_pc(&mut out, &mut addr, pc);
                out.push(if l.signature.is_some() { 0x04 } else { 0x03 });
                uleb(&mut out, u32::from(l.reg));
                uleb_p1(&mut out, l.name.map(|n| p.string(s.get(n))));
                uleb_p1(&mut out, l.ty.map(|t| p.ty(s.get(t))));
                if let Some(sig) = l.signature {
                    uleb_p1(&mut out, Some(p.string(s.get(sig))));
                }
            }
            Ev::End(reg) => {
                advance_pc(&mut out, &mut addr, pc);
                out.push(0x05);
                uleb(&mut out, u32::from(reg));
            }
        }
    }
    out.push(0x00);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb_round_trips() {
        for v in [0u32, 1, 127, 128, 16256, u32::MAX] {
            let mut b = Vec::new();
            uleb(&mut b, v);
            assert!(b.len() <= 5);
        }
        let mut b = Vec::new();
        sleb(&mut b, -128);
        assert_eq!(b, vec![0x80, 0x7f]);
        let mut b = Vec::new();
        sleb(&mut b, -1);
        assert_eq!(b, vec![0x7f]);
        let mut b = Vec::new();
        sleb(&mut b, 64);
        assert_eq!(b, vec![0xc0, 0x00]);
    }

    #[test]
    fn value_widths() {
        assert_eq!(signed_len(0), 1);
        assert_eq!(signed_len(-1), 1);
        assert_eq!(signed_len(127), 1);
        assert_eq!(signed_len(128), 2);
        assert_eq!(signed_len(-32768), 2);
        assert_eq!(signed_len(i64::MIN), 8);
        assert_eq!(unsigned_len(0), 1);
        assert_eq!(unsigned_len(255), 1);
        assert_eq!(unsigned_len(256), 2);
    }
}
