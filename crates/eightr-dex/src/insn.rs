//! Dalvik bytecode decoding: the full opcode table (formats, index kinds, names) and a
//! linear-sweep decoder that also understands the three payload pseudo-instructions.

use crate::error::{DexError, ErrorKind, Result};

/// Instruction formats, named as in the "Dalvik bytecode formats" spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    F10x, F12x, F11n, F11x, F10t, F20t, F22x, F21t, F21s, F21h, F21c, F23x, F22b, F22t, F22s,
    F22c, F30t, F32x, F31i, F31t, F31c, F35c, F3rc, F45cc, F4rcc, F51l,
}

impl Format {
    /// Size in 16-bit code units.
    pub fn units(self) -> u32 {
        use Format::*;
        match self {
            F10x | F12x | F11n | F11x | F10t => 1,
            F20t | F22x | F21t | F21s | F21h | F21c | F23x | F22b | F22t | F22s | F22c => 2,
            F30t | F32x | F31i | F31t | F31c | F35c | F3rc => 3,
            F45cc | F4rcc => 4,
            F51l => 5,
        }
    }
}

/// What a constant-pool index operand refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    None,
    String,
    Type,
    Field,
    Method,
    CallSite,
    MethodHandle,
    Proto,
    /// `invoke-polymorphic`: a method index plus a proto index.
    MethodAndProto,
}

#[derive(Debug, Clone, Copy)]
pub struct OpInfo {
    pub name: &'static str,
    pub format: Format,
    pub index: IndexKind,
}

const fn op(name: &'static str, format: Format, index: IndexKind) -> OpInfo {
    OpInfo { name, format, index }
}

use Format::*;
use IndexKind as K;

const UNUSED: OpInfo = op("unused", F10x, K::None);

/// Opcode table indexed by opcode byte.
pub static OPCODES: [OpInfo; 256] = {
    let mut t = [UNUSED; 256];
    t[0x00] = op("nop", F10x, K::None);
    t[0x01] = op("move", F12x, K::None);
    t[0x02] = op("move/from16", F22x, K::None);
    t[0x03] = op("move/16", F32x, K::None);
    t[0x04] = op("move-wide", F12x, K::None);
    t[0x05] = op("move-wide/from16", F22x, K::None);
    t[0x06] = op("move-wide/16", F32x, K::None);
    t[0x07] = op("move-object", F12x, K::None);
    t[0x08] = op("move-object/from16", F22x, K::None);
    t[0x09] = op("move-object/16", F32x, K::None);
    t[0x0a] = op("move-result", F11x, K::None);
    t[0x0b] = op("move-result-wide", F11x, K::None);
    t[0x0c] = op("move-result-object", F11x, K::None);
    t[0x0d] = op("move-exception", F11x, K::None);
    t[0x0e] = op("return-void", F10x, K::None);
    t[0x0f] = op("return", F11x, K::None);
    t[0x10] = op("return-wide", F11x, K::None);
    t[0x11] = op("return-object", F11x, K::None);
    t[0x12] = op("const/4", F11n, K::None);
    t[0x13] = op("const/16", F21s, K::None);
    t[0x14] = op("const", F31i, K::None);
    t[0x15] = op("const/high16", F21h, K::None);
    t[0x16] = op("const-wide/16", F21s, K::None);
    t[0x17] = op("const-wide/32", F31i, K::None);
    t[0x18] = op("const-wide", F51l, K::None);
    t[0x19] = op("const-wide/high16", F21h, K::None);
    t[0x1a] = op("const-string", F21c, K::String);
    t[0x1b] = op("const-string/jumbo", F31c, K::String);
    t[0x1c] = op("const-class", F21c, K::Type);
    t[0x1d] = op("monitor-enter", F11x, K::None);
    t[0x1e] = op("monitor-exit", F11x, K::None);
    t[0x1f] = op("check-cast", F21c, K::Type);
    t[0x20] = op("instance-of", F22c, K::Type);
    t[0x21] = op("array-length", F12x, K::None);
    t[0x22] = op("new-instance", F21c, K::Type);
    t[0x23] = op("new-array", F22c, K::Type);
    t[0x24] = op("filled-new-array", F35c, K::Type);
    t[0x25] = op("filled-new-array/range", F3rc, K::Type);
    t[0x26] = op("fill-array-data", F31t, K::None);
    t[0x27] = op("throw", F11x, K::None);
    t[0x28] = op("goto", F10t, K::None);
    t[0x29] = op("goto/16", F20t, K::None);
    t[0x2a] = op("goto/32", F30t, K::None);
    t[0x2b] = op("packed-switch", F31t, K::None);
    t[0x2c] = op("sparse-switch", F31t, K::None);
    t[0x2d] = op("cmpl-float", F23x, K::None);
    t[0x2e] = op("cmpg-float", F23x, K::None);
    t[0x2f] = op("cmpl-double", F23x, K::None);
    t[0x30] = op("cmpg-double", F23x, K::None);
    t[0x31] = op("cmp-long", F23x, K::None);
    t[0x32] = op("if-eq", F22t, K::None);
    t[0x33] = op("if-ne", F22t, K::None);
    t[0x34] = op("if-lt", F22t, K::None);
    t[0x35] = op("if-ge", F22t, K::None);
    t[0x36] = op("if-gt", F22t, K::None);
    t[0x37] = op("if-le", F22t, K::None);
    t[0x38] = op("if-eqz", F21t, K::None);
    t[0x39] = op("if-nez", F21t, K::None);
    t[0x3a] = op("if-ltz", F21t, K::None);
    t[0x3b] = op("if-gez", F21t, K::None);
    t[0x3c] = op("if-gtz", F21t, K::None);
    t[0x3d] = op("if-lez", F21t, K::None);

    const ARR: [&str; 14] = [
        "aget", "aget-wide", "aget-object", "aget-boolean", "aget-byte", "aget-char", "aget-short",
        "aput", "aput-wide", "aput-object", "aput-boolean", "aput-byte", "aput-char", "aput-short",
    ];
    let mut i = 0;
    while i < 14 {
        t[0x44 + i] = op(ARR[i], F23x, K::None);
        i += 1;
    }
    const IFIELD: [&str; 14] = [
        "iget", "iget-wide", "iget-object", "iget-boolean", "iget-byte", "iget-char", "iget-short",
        "iput", "iput-wide", "iput-object", "iput-boolean", "iput-byte", "iput-char", "iput-short",
    ];
    let mut i = 0;
    while i < 14 {
        t[0x52 + i] = op(IFIELD[i], F22c, K::Field);
        i += 1;
    }
    const SFIELD: [&str; 14] = [
        "sget", "sget-wide", "sget-object", "sget-boolean", "sget-byte", "sget-char", "sget-short",
        "sput", "sput-wide", "sput-object", "sput-boolean", "sput-byte", "sput-char", "sput-short",
    ];
    let mut i = 0;
    while i < 14 {
        t[0x60 + i] = op(SFIELD[i], F21c, K::Field);
        i += 1;
    }
    t[0x6e] = op("invoke-virtual", F35c, K::Method);
    t[0x6f] = op("invoke-super", F35c, K::Method);
    t[0x70] = op("invoke-direct", F35c, K::Method);
    t[0x71] = op("invoke-static", F35c, K::Method);
    t[0x72] = op("invoke-interface", F35c, K::Method);
    t[0x74] = op("invoke-virtual/range", F3rc, K::Method);
    t[0x75] = op("invoke-super/range", F3rc, K::Method);
    t[0x76] = op("invoke-direct/range", F3rc, K::Method);
    t[0x77] = op("invoke-static/range", F3rc, K::Method);
    t[0x78] = op("invoke-interface/range", F3rc, K::Method);

    const UNOP: [&str; 21] = [
        "neg-int", "not-int", "neg-long", "not-long", "neg-float", "neg-double", "int-to-long",
        "int-to-float", "int-to-double", "long-to-int", "long-to-float", "long-to-double",
        "float-to-int", "float-to-long", "float-to-double", "double-to-int", "double-to-long",
        "double-to-float", "int-to-byte", "int-to-char", "int-to-short",
    ];
    let mut i = 0;
    while i < 21 {
        t[0x7b + i] = op(UNOP[i], F12x, K::None);
        i += 1;
    }
    const BINOP: [&str; 32] = [
        "add-int", "sub-int", "mul-int", "div-int", "rem-int", "and-int", "or-int", "xor-int",
        "shl-int", "shr-int", "ushr-int", "add-long", "sub-long", "mul-long", "div-long",
        "rem-long", "and-long", "or-long", "xor-long", "shl-long", "shr-long", "ushr-long",
        "add-float", "sub-float", "mul-float", "div-float", "rem-float", "add-double",
        "sub-double", "mul-double", "div-double", "rem-double",
    ];
    const BINOP_2ADDR: [&str; 32] = [
        "add-int/2addr", "sub-int/2addr", "mul-int/2addr", "div-int/2addr", "rem-int/2addr",
        "and-int/2addr", "or-int/2addr", "xor-int/2addr", "shl-int/2addr", "shr-int/2addr",
        "ushr-int/2addr", "add-long/2addr", "sub-long/2addr", "mul-long/2addr", "div-long/2addr",
        "rem-long/2addr", "and-long/2addr", "or-long/2addr", "xor-long/2addr", "shl-long/2addr",
        "shr-long/2addr", "ushr-long/2addr", "add-float/2addr", "sub-float/2addr",
        "mul-float/2addr", "div-float/2addr", "rem-float/2addr", "add-double/2addr",
        "sub-double/2addr", "mul-double/2addr", "div-double/2addr", "rem-double/2addr",
    ];
    let mut i = 0;
    while i < 32 {
        t[0x90 + i] = op(BINOP[i], F23x, K::None);
        t[0xb0 + i] = op(BINOP_2ADDR[i], F12x, K::None);
        i += 1;
    }
    const LIT16: [&str; 8] = [
        "add-int/lit16", "rsub-int", "mul-int/lit16", "div-int/lit16", "rem-int/lit16",
        "and-int/lit16", "or-int/lit16", "xor-int/lit16",
    ];
    let mut i = 0;
    while i < 8 {
        t[0xd0 + i] = op(LIT16[i], F22s, K::None);
        i += 1;
    }
    const LIT8: [&str; 11] = [
        "add-int/lit8", "rsub-int/lit8", "mul-int/lit8", "div-int/lit8", "rem-int/lit8",
        "and-int/lit8", "or-int/lit8", "xor-int/lit8", "shl-int/lit8", "shr-int/lit8",
        "ushr-int/lit8",
    ];
    let mut i = 0;
    while i < 11 {
        t[0xd8 + i] = op(LIT8[i], F22b, K::None);
        i += 1;
    }
    t[0xfa] = op("invoke-polymorphic", F45cc, K::MethodAndProto);
    t[0xfb] = op("invoke-polymorphic/range", F4rcc, K::MethodAndProto);
    t[0xfc] = op("invoke-custom", F35c, K::CallSite);
    t[0xfd] = op("invoke-custom/range", F3rc, K::CallSite);
    t[0xfe] = op("const-method-handle", F21c, K::MethodHandle);
    t[0xff] = op("const-method-type", F21c, K::Proto);
    t
};

pub fn is_unused(opcode: u8) -> bool {
    matches!(opcode, 0x3e..=0x43 | 0x73 | 0x79 | 0x7a | 0xe3..=0xf9)
}

/// Register operands. Non-range formats list up to five registers; range formats name a
/// contiguous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regs {
    List { regs: [u16; 5], len: u8 },
    Range { first: u16, count: u16 },
}

impl Regs {
    fn list(rs: &[u16]) -> Regs {
        let mut regs = [0; 5];
        regs[..rs.len()].copy_from_slice(rs);
        Regs::List { regs, len: rs.len() as u8 }
    }

    pub fn to_vec(&self) -> Vec<u16> {
        match *self {
            Regs::List { regs, len } => regs[..len as usize].to_vec(),
            Regs::Range { first, count } => (0..count).map(|i| first.wrapping_add(i)).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// Address in code units from the start of `insns`.
    pub pc: u32,
    pub opcode: u8,
    pub regs: Regs,
    pub literal: Option<i64>,
    pub index: Option<u32>,
    /// Second index (the proto of `invoke-polymorphic`).
    pub index2: Option<u32>,
    /// Branch/payload target, relative to `pc`.
    pub offset: Option<i32>,
}

impl Instruction {
    pub fn info(&self) -> &'static OpInfo {
        &OPCODES[self.opcode as usize]
    }
    pub fn name(&self) -> &'static str {
        self.info().name
    }
    pub fn units(&self) -> u32 {
        self.info().format.units()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    PackedSwitch { first_key: i32, targets: Vec<i32> },
    SparseSwitch { keys: Vec<i32>, targets: Vec<i32> },
    FillArrayData { element_width: u16, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Insn(Instruction),
    Payload { pc: u32, units: u32, payload: Payload },
}

impl Decoded {
    pub fn pc(&self) -> u32 {
        match self {
            Decoded::Insn(i) => i.pc,
            Decoded::Payload { pc, .. } => *pc,
        }
    }
    pub fn units(&self) -> u32 {
        match self {
            Decoded::Insn(i) => i.units(),
            Decoded::Payload { units, .. } => *units,
        }
    }
}

/// Decodes a method's `insns` array by linear sweep. `base` is the file offset of the
/// array, used only for error reporting.
pub fn decode_all(insns: &[u16], base: usize) -> Result<Vec<Decoded>> {
    let mut out = Vec::new();
    let mut pc = 0usize;
    while pc < insns.len() {
        let d = decode_one(insns, pc, base)?;
        pc += d.units() as usize;
        out.push(d);
    }
    Ok(out)
}

pub fn decode_one(insns: &[u16], pc: usize, base: usize) -> Result<Decoded> {
    let err = |what| DexError::new(base + pc * 2, ErrorKind::Malformed(what));
    let unit = |i: usize| insns.get(pc + i).copied().ok_or_else(|| err("instruction runs past end of code"));
    let w = unit(0)?;
    let opcode = (w & 0xff) as u8;
    if opcode == 0 && w != 0 {
        return decode_payload(insns, pc, base);
    }
    if is_unused(opcode) {
        return Err(err("unused opcode"));
    }
    let info = &OPCODES[opcode as usize];
    let aa = w >> 8;
    let a4 = (w >> 8) & 0xf;
    let b4 = w >> 12;
    let mut insn = Instruction {
        pc: pc as u32,
        opcode,
        regs: Regs::list(&[]),
        literal: None,
        index: None,
        index2: None,
        offset: None,
    };
    let u32at = |i: usize| -> Result<u32> { Ok(u32::from(unit(i)?) | (u32::from(unit(i + 1)?) << 16)) };
    match info.format {
        F10x => {
            if aa != 0 {
                return Err(err("nonzero high byte in format 10x"));
            }
        }
        F12x => insn.regs = Regs::list(&[a4, b4]),
        F11n => {
            insn.regs = Regs::list(&[a4]);
            insn.literal = Some(i64::from(((b4 as i8) << 4) >> 4));
        }
        F11x => insn.regs = Regs::list(&[aa]),
        F10t => insn.offset = Some(i32::from(aa as u8 as i8)),
        F20t => insn.offset = Some(i32::from(unit(1)? as i16)),
        F22x => insn.regs = Regs::list(&[aa, unit(1)?]),
        F21t => {
            insn.regs = Regs::list(&[aa]);
            insn.offset = Some(i32::from(unit(1)? as i16));
        }
        F21s => {
            insn.regs = Regs::list(&[aa]);
            insn.literal = Some(i64::from(unit(1)? as i16));
        }
        F21h => {
            insn.regs = Regs::list(&[aa]);
            let v = i64::from(unit(1)? as i16);
            insn.literal = Some(if opcode == 0x19 { v << 48 } else { v << 16 });
        }
        F21c => {
            insn.regs = Regs::list(&[aa]);
            insn.index = Some(u32::from(unit(1)?));
        }
        F23x => {
            let w1 = unit(1)?;
            insn.regs = Regs::list(&[aa, w1 & 0xff, w1 >> 8]);
        }
        F22b => {
            let w1 = unit(1)?;
            insn.regs = Regs::list(&[aa, w1 & 0xff]);
            insn.literal = Some(i64::from((w1 >> 8) as u8 as i8));
        }
        F22t => {
            insn.regs = Regs::list(&[a4, b4]);
            insn.offset = Some(i32::from(unit(1)? as i16));
        }
        F22s => {
            insn.regs = Regs::list(&[a4, b4]);
            insn.literal = Some(i64::from(unit(1)? as i16));
        }
        F22c => {
            insn.regs = Regs::list(&[a4, b4]);
            insn.index = Some(u32::from(unit(1)?));
        }
        F30t => {
            if aa != 0 {
                return Err(err("nonzero high byte in format 30t"));
            }
            insn.offset = Some(u32at(1)? as i32);
        }
        F32x => insn.regs = Regs::list(&[unit(1)?, unit(2)?]),
        F31i => {
            insn.regs = Regs::list(&[aa]);
            insn.literal = Some(i64::from(u32at(1)? as i32));
        }
        F31t => {
            insn.regs = Regs::list(&[aa]);
            insn.offset = Some(u32at(1)? as i32);
        }
        F31c => {
            insn.regs = Regs::list(&[aa]);
            insn.index = Some(u32at(1)?);
        }
        F35c | F45cc => {
            let count = b4 as usize;
            if count > 5 {
                return Err(err("more than 5 registers in format 35c"));
            }
            insn.index = Some(u32::from(unit(1)?));
            let w2 = unit(2)?;
            let all = [w2 & 0xf, (w2 >> 4) & 0xf, (w2 >> 8) & 0xf, w2 >> 12, a4];
            insn.regs = Regs::list(&all[..count]);
            if info.format == F45cc {
                insn.index2 = Some(u32::from(unit(3)?));
            }
        }
        F3rc | F4rcc => {
            insn.index = Some(u32::from(unit(1)?));
            insn.regs = Regs::Range { first: unit(2)?, count: aa };
            if info.format == F4rcc {
                insn.index2 = Some(u32::from(unit(3)?));
            }
        }
        F51l => {
            insn.regs = Regs::list(&[aa]);
            let lo = u64::from(u32at(1)?);
            let hi = u64::from(u32at(3)?);
            insn.literal = Some((lo | (hi << 32)) as i64);
        }
    }
    // Make sure the whole instruction is present even if the format didn't read every unit.
    unit(info.format.units() as usize - 1)?;
    Ok(Decoded::Insn(insn))
}

fn decode_payload(insns: &[u16], pc: usize, base: usize) -> Result<Decoded> {
    let err = |what| DexError::new(base + pc * 2, ErrorKind::Malformed(what));
    let unit = |i: usize| insns.get(pc + i).copied().ok_or_else(|| err("payload runs past end of code"));
    let int = |i: usize| -> Result<i32> { Ok((u32::from(unit(i)?) | (u32::from(unit(i + 1)?) << 16)) as i32) };
    let ident = unit(0)?;
    let (units, payload) = match ident {
        0x0100 => {
            let size = unit(1)? as usize;
            let first_key = int(2)?;
            let targets = (0..size).map(|i| int(4 + 2 * i)).collect::<Result<_>>()?;
            (4 + 2 * size, Payload::PackedSwitch { first_key, targets })
        }
        0x0200 => {
            let size = unit(1)? as usize;
            let keys = (0..size).map(|i| int(2 + 2 * i)).collect::<Result<_>>()?;
            let targets = (0..size).map(|i| int(2 + 2 * size + 2 * i)).collect::<Result<_>>()?;
            (2 + 4 * size, Payload::SparseSwitch { keys, targets })
        }
        0x0300 => {
            let element_width = unit(1)?;
            let size = u64::from(u32::from(unit(2)?) | (u32::from(unit(3)?) << 16));
            let bytes = size * u64::from(element_width);
            if bytes > (insns.len() as u64) * 2 {
                return Err(err("fill-array-data payload larger than code"));
            }
            let bytes = bytes as usize;
            let data_units = bytes.div_ceil(2);
            let mut data = Vec::with_capacity(bytes);
            for i in 0..data_units {
                data.extend_from_slice(&unit(4 + i)?.to_le_bytes());
            }
            data.truncate(bytes);
            (4 + data_units, Payload::FillArrayData { element_width, data })
        }
        _ => return Err(err("unknown payload identifier")),
    };
    Ok(Decoded::Payload { pc: pc as u32, units: units as u32, payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insn(units: &[u16]) -> Instruction {
        match decode_one(units, 0, 0).unwrap() {
            Decoded::Insn(i) => i,
            p => panic!("expected insn, got {p:?}"),
        }
    }

    #[test]
    fn table_is_complete() {
        for b in 0..=255u8 {
            let info = &OPCODES[b as usize];
            assert_eq!(info.name == "unused", is_unused(b), "opcode {b:#04x}");
        }
        // Spot checks against the spec's opcode listing.
        assert_eq!(OPCODES[0x7b].name, "neg-int");
        assert_eq!(OPCODES[0x8f].name, "int-to-short");
        assert_eq!(OPCODES[0x90].name, "add-int");
        assert_eq!(OPCODES[0xaf].name, "rem-double");
        assert_eq!(OPCODES[0xcf].name, "rem-double/2addr");
        assert_eq!(OPCODES[0xd7].name, "xor-int/lit16");
        assert_eq!(OPCODES[0xe2].name, "ushr-int/lit8");
        assert_eq!(OPCODES[0x5f].name, "iput-short");
        assert_eq!(OPCODES[0x6d].name, "sput-short");
    }

    #[test]
    fn const4_sign_extends() {
        // const/4 v1, #-1  => 12 f1
        let i = insn(&[0xf112]);
        assert_eq!(i.regs.to_vec(), vec![1]);
        assert_eq!(i.literal, Some(-1));
        let i = insn(&[0x7012]); // const/4 v0, #7
        assert_eq!(i.literal, Some(7));
    }

    #[test]
    fn high16_variants() {
        assert_eq!(insn(&[0x0015, 0x4120]).literal, Some(0x4120_0000));
        assert_eq!(insn(&[0x0019, 0x3ff0]).literal, Some(0x3ff0_0000_0000_0000));
        assert_eq!(insn(&[0x0015, 0x8000]).literal, Some(-0x8000_0000));
    }

    #[test]
    fn wide_literal() {
        let i = insn(&[0x0218, 0x4444, 0x3333, 0x2222, 0x1111]);
        assert_eq!(i.regs.to_vec(), vec![2]);
        assert_eq!(i.literal, Some(0x1111_2222_3333_4444));
    }

    #[test]
    fn invoke_35c_register_order() {
        // invoke-virtual {v1, v2, v3, v4, v5}, meth@0007  => A=5, G=5, C..F = 1,2,3,4
        let i = insn(&[0x556e, 0x0007, 0x4321]);
        assert_eq!(i.name(), "invoke-virtual");
        assert_eq!(i.index, Some(7));
        assert_eq!(i.regs.to_vec(), vec![1, 2, 3, 4, 5]);
        // invoke-static {}, meth@0001
        assert_eq!(insn(&[0x0071, 0x0001, 0x0000]).regs.to_vec(), Vec::<u16>::new());
        // count 6 is invalid
        assert!(decode_one(&[0x606e, 0, 0], 0, 0).is_err());
    }

    #[test]
    fn range_and_polymorphic() {
        let i = insn(&[0x0374, 0x0009, 0x0010]);
        assert_eq!(i.regs, Regs::Range { first: 16, count: 3 });
        let i = insn(&[0x20fa, 0x0002, 0x0010, 0x0005]);
        assert_eq!(i.name(), "invoke-polymorphic");
        assert_eq!((i.index, i.index2), (Some(2), Some(5)));
        assert_eq!(i.regs.to_vec(), vec![0, 1]);
    }

    #[test]
    fn branches() {
        assert_eq!(insn(&[0xfe28]).offset, Some(-2)); // goto -2
        assert_eq!(insn(&[0x0029, 0xff00]).offset, Some(-256));
        assert_eq!(insn(&[0x002a, 0x0000, 0x0001]).offset, Some(0x10000));
        let i = insn(&[0x1032, 0x0005]); // if-eq v0, v1, +5
        assert_eq!((i.regs.to_vec(), i.offset), (vec![0, 1], Some(5)));
    }

    #[test]
    fn lit8_and_23x() {
        let i = insn(&[0x00d8, 0xff01]); // add-int/lit8 v0, v1, #-1
        assert_eq!((i.regs.to_vec(), i.literal), (vec![0, 1], Some(-1)));
        let i = insn(&[0x0090, 0x0201]); // add-int v0, v1, v2
        assert_eq!(i.regs.to_vec(), vec![0, 1, 2]);
    }

    #[test]
    fn payloads() {
        let packed = [0x0100, 2, 10, 0, 5, 0, 7, 0];
        let d = decode_one(&packed, 0, 0).unwrap();
        assert_eq!(d.units(), 8);
        assert_eq!(d, Decoded::Payload {
            pc: 0,
            units: 8,
            payload: Payload::PackedSwitch { first_key: 10, targets: vec![5, 7] },
        });
        let sparse = [0x0200, 2, 1, 0, 0xffff, 0xffff, 3, 0, 4, 0];
        match decode_one(&sparse, 0, 0).unwrap() {
            Decoded::Payload { units: 10, payload: Payload::SparseSwitch { keys, targets }, .. } => {
                assert_eq!(keys, vec![1, -1]);
                assert_eq!(targets, vec![3, 4]);
            }
            other => panic!("{other:?}"),
        }
        // 3 one-byte elements => 2 data units, padded.
        let fill = [0x0300, 1, 3, 0, 0x0201, 0x0003];
        match decode_one(&fill, 0, 0).unwrap() {
            Decoded::Payload { units: 6, payload: Payload::FillArrayData { element_width: 1, data }, .. } => {
                assert_eq!(data, vec![1, 2, 3]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn truncated_and_unused() {
        assert!(decode_one(&[0x0014, 0x0000], 0, 0).is_err()); // const needs 3 units
        assert!(decode_one(&[0x003e], 0, 0).is_err());
        assert!(decode_one(&[0x0100, 100, 0, 0], 0, 0).is_err());
        assert!(decode_one(&[0x0300, 4, 0xffff, 0xffff], 0, 0).is_err());
    }

    #[test]
    fn linear_sweep() {
        // const/4 v0, #0 ; return v0
        let d = decode_all(&[0x0012, 0x000f], 0).unwrap();
        assert_eq!(d.iter().map(Decoded::pc).collect::<Vec<_>>(), vec![0, 1]);
    }
}
