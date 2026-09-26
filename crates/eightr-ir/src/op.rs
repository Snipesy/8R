//! Semantic operations. Each Dalvik opcode maps to exactly one `Op` shape; encoding details
//! that don't affect semantics (`/16`, `/range`, `/2addr`, `/jumbo`, literal widths) are
//! dropped here and chosen again by the writer.

use crate::sym::Sym;
use crate::value::{CallSite, MethodHandleRef};

pub type Reg = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Single,
    Wide,
    Object,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Const {
    /// 32-bit constant (int or float bits).
    Narrow(i32),
    /// 64-bit constant (long or double bits).
    Wide(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpKind {
    LFloat,
    GFloat,
    LDouble,
    GDouble,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Ge,
    Gt,
    Le,
}

/// Element/field access kind, shared by `aget*`/`iget*`/`sget*` families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemKind {
    /// 32-bit: int or float.
    Narrow,
    Wide,
    Object,
    Boolean,
    Byte,
    Char,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeKind {
    Virtual,
    Super,
    Direct,
    Static,
    Interface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumType {
    Int,
    Long,
    Float,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Ushr,
    /// Reverse subtract (`rsub-int`): `lit - reg`.
    Rsub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    NegInt,
    NotInt,
    NegLong,
    NotLong,
    NegFloat,
    NegDouble,
    IntToLong,
    IntToFloat,
    IntToDouble,
    LongToInt,
    LongToFloat,
    LongToDouble,
    FloatToInt,
    FloatToLong,
    FloatToDouble,
    DoubleToInt,
    DoubleToLong,
    DoubleToFloat,
    IntToByte,
    IntToChar,
    IntToShort,
}

impl UnOp {
    pub const ALL: [UnOp; 21] = [
        UnOp::NegInt, UnOp::NotInt, UnOp::NegLong, UnOp::NotLong, UnOp::NegFloat, UnOp::NegDouble,
        UnOp::IntToLong, UnOp::IntToFloat, UnOp::IntToDouble, UnOp::LongToInt, UnOp::LongToFloat,
        UnOp::LongToDouble, UnOp::FloatToInt, UnOp::FloatToLong, UnOp::FloatToDouble,
        UnOp::DoubleToInt, UnOp::DoubleToLong, UnOp::DoubleToFloat, UnOp::IntToByte,
        UnOp::IntToChar, UnOp::IntToShort,
    ];

    /// (source type, result type)
    pub fn types(self) -> (NumType, NumType) {
        use NumType::*;
        use UnOp::*;
        match self {
            NegInt | NotInt | IntToByte | IntToChar | IntToShort => (Int, Int),
            NegLong | NotLong => (Long, Long),
            NegFloat => (Float, Float),
            NegDouble => (Double, Double),
            IntToLong => (Int, Long),
            IntToFloat => (Int, Float),
            IntToDouble => (Int, Double),
            LongToInt => (Long, Int),
            LongToFloat => (Long, Float),
            LongToDouble => (Long, Double),
            FloatToInt => (Float, Int),
            FloatToLong => (Float, Long),
            FloatToDouble => (Float, Double),
            DoubleToInt => (Double, Int),
            DoubleToLong => (Double, Long),
            DoubleToFloat => (Double, Float),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Reg(Reg),
    Lit(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRef {
    pub class: Sym,
    pub name: Sym,
    /// Field type descriptor.
    pub ty: Sym,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodRef {
    pub class: Sym,
    pub name: Sym,
    /// Method descriptor, e.g. `(ILjava/lang/String;)V`.
    pub proto: Sym,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Nop,
    Move { width: Width, dst: Reg, src: Reg },
    MoveResult { width: Width, dst: Reg },
    MoveException { dst: Reg },
    ReturnVoid,
    Return { width: Width, src: Reg },
    Const { dst: Reg, value: Const },
    ConstString { dst: Reg, value: Sym },
    ConstClass { dst: Reg, ty: Sym },
    ConstMethodHandle { dst: Reg, handle: MethodHandleRef },
    ConstMethodType { dst: Reg, proto: Sym },
    MonitorEnter { obj: Reg },
    MonitorExit { obj: Reg },
    CheckCast { reg: Reg, ty: Sym },
    InstanceOf { dst: Reg, obj: Reg, ty: Sym },
    ArrayLength { dst: Reg, array: Reg },
    NewInstance { dst: Reg, ty: Sym },
    NewArray { dst: Reg, size: Reg, ty: Sym },
    FilledNewArray { ty: Sym, args: Vec<Reg> },
    FillArrayData { array: Reg, element_width: u16, data: Vec<u8> },
    Throw { src: Reg },
    /// Targets are instruction indices into `Body::insns`, not pcs.
    Goto { target: u32 },
    Switch { src: Reg, packed: bool, cases: Vec<(i32, u32)> },
    Cmp { kind: CmpKind, dst: Reg, a: Reg, b: Reg },
    If { cond: Cond, a: Reg, b: Reg, target: u32 },
    IfZ { cond: Cond, a: Reg, target: u32 },
    ArrayGet { kind: MemKind, dst: Reg, array: Reg, index: Reg },
    ArrayPut { kind: MemKind, src: Reg, array: Reg, index: Reg },
    InstanceGet { kind: MemKind, dst: Reg, obj: Reg, field: FieldRef },
    InstancePut { kind: MemKind, src: Reg, obj: Reg, field: FieldRef },
    StaticGet { kind: MemKind, dst: Reg, field: FieldRef },
    StaticPut { kind: MemKind, src: Reg, field: FieldRef },
    Invoke { kind: InvokeKind, method: MethodRef, args: Vec<Reg> },
    InvokePolymorphic { method: MethodRef, proto: Sym, args: Vec<Reg> },
    InvokeCustom { call_site: Box<CallSite>, args: Vec<Reg> },
    Unop { op: UnOp, dst: Reg, src: Reg },
    Binop { op: BinOp, ty: NumType, dst: Reg, a: Reg, b: Operand },
}

impl Op {
    /// Whether executing this op may throw (the Dalvik spec's "can throw" set).
    pub fn can_throw(&self) -> bool {
        use Op::*;
        matches!(
            self,
            ConstString { .. } | ConstClass { .. } | ConstMethodHandle { .. } | ConstMethodType { .. }
                | MonitorEnter { .. } | MonitorExit { .. } | CheckCast { .. } | InstanceOf { .. }
                | ArrayLength { .. } | NewInstance { .. } | NewArray { .. } | FilledNewArray { .. }
                | FillArrayData { .. } | Throw { .. } | ArrayGet { .. } | ArrayPut { .. }
                | InstanceGet { .. } | InstancePut { .. } | StaticGet { .. } | StaticPut { .. }
                | Invoke { .. } | InvokePolymorphic { .. } | InvokeCustom { .. }
                | Binop { op: BinOp::Div | BinOp::Rem, ty: NumType::Int | NumType::Long, .. }
        )
    }

    /// Control never falls through to the next instruction.
    pub fn ends_flow(&self) -> bool {
        matches!(self, Op::Goto { .. } | Op::ReturnVoid | Op::Return { .. } | Op::Throw { .. })
    }

    pub fn is_branch(&self) -> bool {
        matches!(self, Op::Goto { .. } | Op::If { .. } | Op::IfZ { .. } | Op::Switch { .. })
    }

    /// Instruction-index targets of branches (excluding fallthrough).
    pub fn targets(&self) -> Vec<u32> {
        match self {
            Op::Goto { target } | Op::If { target, .. } | Op::IfZ { target, .. } => vec![*target],
            Op::Switch { cases, .. } => cases.iter().map(|&(_, t)| t).collect(),
            _ => Vec::new(),
        }
    }

    /// Registers read. Wide operands contribute both halves.
    pub fn uses(&self) -> Vec<Reg> {
        use Op::*;
        let pair = |r: Reg| vec![r, r.wrapping_add(1)];
        let mem = |kind: MemKind, r: Reg| if kind == MemKind::Wide { pair(r) } else { vec![r] };
        match self {
            Nop | MoveResult { .. } | MoveException { .. } | ReturnVoid | Const { .. } | ConstString { .. }
            | ConstClass { .. } | ConstMethodHandle { .. } | ConstMethodType { .. } | NewInstance { .. }
            | Goto { .. } | StaticGet { .. } => vec![],
            Move { width, src, .. } | Return { width, src } => {
                if *width == Width::Wide { pair(*src) } else { vec![*src] }
            }
            MonitorEnter { obj } | MonitorExit { obj } => vec![*obj],
            CheckCast { reg, .. } => vec![*reg],
            InstanceOf { obj, .. } => vec![*obj],
            ArrayLength { array, .. } => vec![*array],
            NewArray { size, .. } => vec![*size],
            FilledNewArray { args, .. } | Invoke { args, .. } | InvokePolymorphic { args, .. }
            | InvokeCustom { args, .. } => args.clone(),
            FillArrayData { array, .. } => vec![*array],
            Throw { src } => vec![*src],
            Switch { src, .. } => vec![*src],
            Cmp { kind, a, b, .. } => match kind {
                CmpKind::LFloat | CmpKind::GFloat => vec![*a, *b],
                _ => [pair(*a), pair(*b)].concat(),
            },
            If { a, b, .. } => vec![*a, *b],
            IfZ { a, .. } => vec![*a],
            ArrayGet { array, index, .. } => vec![*array, *index],
            ArrayPut { kind, src, array, index } => [mem(*kind, *src), vec![*array, *index]].concat(),
            InstanceGet { obj, .. } => vec![*obj],
            InstancePut { kind, src, obj, .. } => [mem(*kind, *src), vec![*obj]].concat(),
            StaticPut { kind, src, .. } => mem(*kind, *src),
            Unop { op, src, .. } => {
                let (from, _) = op.types();
                if matches!(from, NumType::Long | NumType::Double) { pair(*src) } else { vec![*src] }
            }
            Binop { ty, a, b, op, .. } => {
                let wide = matches!(ty, NumType::Long | NumType::Double);
                // Long shifts take an int shift amount.
                let shift = matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr);
                let mut v = if wide { pair(*a) } else { vec![*a] };
                if let Operand::Reg(b) = b {
                    v.extend(if wide && !shift { pair(*b) } else { vec![*b] });
                }
                v
            }
        }
    }

    /// First registers of the wide (long/double) pairs this op reads or writes. Exact except for
    /// invokes and `filled-new-array`, whose argument widths live in the (uninterned) proto:
    /// there every pair of consecutive argument registers is reported, an over-approximation.
    pub fn wide_starts(&self) -> Vec<Reg> {
        use Op::*;
        let mut v = Vec::new();
        if let Some((r, true)) = self.def() {
            v.push(r);
        }
        match self {
            Move { width: Width::Wide, src, .. } | Return { width: Width::Wide, src } => v.push(*src),
            Cmp { kind, a, b, .. } if !matches!(kind, CmpKind::LFloat | CmpKind::GFloat) => v.extend([*a, *b]),
            ArrayPut { kind: MemKind::Wide, src, .. }
            | InstancePut { kind: MemKind::Wide, src, .. }
            | StaticPut { kind: MemKind::Wide, src, .. } => v.push(*src),
            Unop { op, src, .. } if matches!(op.types().0, NumType::Long | NumType::Double) => v.push(*src),
            Binop { ty: NumType::Long | NumType::Double, a, b, op, .. } => {
                v.push(*a);
                if let Operand::Reg(b) = b {
                    if !matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr) {
                        v.push(*b);
                    }
                }
            }
            FilledNewArray { args, .. } | Invoke { args, .. } | InvokePolymorphic { args, .. } | InvokeCustom { args, .. } => {
                v.extend(args.windows(2).filter(|w| w[1] == w[0].wrapping_add(1)).map(|w| w[0]));
            }
            _ => {}
        }
        v
    }

    /// The value this op defines, as (reg, wide). A wide def writes `reg` and `reg + 1`.
    /// `check-cast` is not a def: it refines the type of an existing value.
    pub fn def(&self) -> Option<(Reg, bool)> {
        use Op::*;
        match self {
            Move { width, dst, .. } | MoveResult { width, dst } => Some((*dst, *width == Width::Wide)),
            MoveException { dst } | ConstString { dst, .. } | ConstClass { dst, .. }
            | ConstMethodHandle { dst, .. } | ConstMethodType { dst, .. } | InstanceOf { dst, .. }
            | ArrayLength { dst, .. } | NewInstance { dst, .. } | NewArray { dst, .. } | Cmp { dst, .. } => {
                Some((*dst, false))
            }
            Const { dst, value } => Some((*dst, matches!(value, self::Const::Wide(_)))),
            ArrayGet { kind, dst, .. } | InstanceGet { kind, dst, .. } | StaticGet { kind, dst, .. } => {
                Some((*dst, *kind == MemKind::Wide))
            }
            Unop { op, dst, .. } => Some((*dst, matches!(op.types().1, NumType::Long | NumType::Double))),
            Binop { ty, dst, .. } => Some((*dst, matches!(ty, NumType::Long | NumType::Double))),
            _ => None,
        }
    }

    /// Rewrites every register operand through `f` (uses and defs alike).
    pub fn map_regs(&mut self, f: &mut impl FnMut(Reg) -> Reg) {
        use Op::*;
        match self {
            Nop | ReturnVoid | Goto { .. } => {}
            Move { dst, src, .. } | ArrayLength { dst, array: src } | Unop { dst, src, .. } => {
                *dst = f(*dst);
                *src = f(*src);
            }
            MoveResult { dst, .. } | MoveException { dst } | Const { dst, .. } | ConstString { dst, .. }
            | ConstClass { dst, .. } | ConstMethodHandle { dst, .. } | ConstMethodType { dst, .. }
            | NewInstance { dst, .. } | StaticGet { dst, .. } => *dst = f(*dst),
            Return { src, .. } | Throw { src } | Switch { src, .. } | StaticPut { src, .. } => *src = f(*src),
            MonitorEnter { obj } | MonitorExit { obj } => *obj = f(*obj),
            CheckCast { reg, .. } => *reg = f(*reg),
            InstanceOf { dst, obj, .. } | InstanceGet { dst, obj, .. } => {
                *dst = f(*dst);
                *obj = f(*obj);
            }
            InstancePut { src, obj, .. } => {
                *src = f(*src);
                *obj = f(*obj);
            }
            NewArray { dst, size, .. } => {
                *dst = f(*dst);
                *size = f(*size);
            }
            FilledNewArray { args, .. } | Invoke { args, .. } | InvokePolymorphic { args, .. } | InvokeCustom { args, .. } => {
                for a in args.iter_mut() {
                    *a = f(*a);
                }
            }
            FillArrayData { array, .. } => *array = f(*array),
            Cmp { dst, a, b, .. } => {
                *dst = f(*dst);
                *a = f(*a);
                *b = f(*b);
            }
            If { a, b, .. } => {
                *a = f(*a);
                *b = f(*b);
            }
            IfZ { a, .. } => *a = f(*a),
            ArrayGet { dst, array, index, .. } => {
                *dst = f(*dst);
                *array = f(*array);
                *index = f(*index);
            }
            ArrayPut { src, array, index, .. } => {
                *src = f(*src);
                *array = f(*array);
                *index = f(*index);
            }
            Binop { dst, a, b, .. } => {
                *dst = f(*dst);
                *a = f(*a);
                if let Operand::Reg(r) = b {
                    *r = f(*r);
                }
            }
        }
    }

    /// Rewrites every branch target (instruction index) through `f`.
    pub fn map_targets(&mut self, f: &mut impl FnMut(u32) -> u32) {
        match self {
            Op::Goto { target } | Op::If { target, .. } | Op::IfZ { target, .. } => *target = f(*target),
            Op::Switch { cases, .. } => {
                for (_, t) in cases.iter_mut() {
                    *t = f(*t);
                }
            }
            _ => {}
        }
    }

    /// Whether some Dalvik encoding accepts this op's registers (mirrors the writer's format
    /// choices). Rewrites that move registers must keep every op encodable or refuse.
    pub fn encodable(&self) -> bool {
        use Op::*;
        let f4 = |r: &Reg| *r < 16;
        let f8 = |r: &Reg| *r < 256;
        let invoke_ok = |args: &[Reg]| {
            (args.len() <= 5 && args.iter().all(f4))
                || (args.len() <= 255 && args.windows(2).all(|w| w[1] == w[0].wrapping_add(1)))
        };
        match self {
            Nop | ReturnVoid | Goto { .. } | Move { .. } => true,
            MoveResult { dst, .. } | MoveException { dst } | Const { dst, .. } | ConstString { dst, .. }
            | ConstClass { dst, .. } | ConstMethodHandle { dst, .. } | ConstMethodType { dst, .. }
            | NewInstance { dst, .. } | StaticGet { dst, .. } => f8(dst),
            Return { src, .. } | Throw { src } | Switch { src, .. } | StaticPut { src, .. } | FillArrayData { array: src, .. } => f8(src),
            MonitorEnter { obj } | MonitorExit { obj } => f8(obj),
            CheckCast { reg, .. } => f8(reg),
            IfZ { a, .. } => f8(a),
            InstanceOf { dst, obj, .. } | InstanceGet { dst, obj, .. } => f4(dst) && f4(obj),
            InstancePut { src, obj, .. } => f4(src) && f4(obj),
            ArrayLength { dst, array } => f4(dst) && f4(array),
            NewArray { dst, size, .. } => f4(dst) && f4(size),
            If { a, b, .. } => f4(a) && f4(b),
            Unop { dst, src, .. } => f4(dst) && f4(src),
            Cmp { dst, a, b, .. } => f8(dst) && f8(a) && f8(b),
            ArrayGet { dst, array, index, .. } => f8(dst) && f8(array) && f8(index),
            ArrayPut { src, array, index, .. } => f8(src) && f8(array) && f8(index),
            FilledNewArray { args, .. } | Invoke { args, .. } | InvokePolymorphic { args, .. } | InvokeCustom { args, .. } => {
                invoke_ok(args)
            }
            Binop { dst, a, b: Operand::Reg(b), .. } => (dst == a && f4(dst) && f4(b)) || (f8(dst) && f8(a) && f8(b)),
            Binop { op, dst, a, b: Operand::Lit(l), .. } => {
                let lit8 = (-128..=127).contains(l) && f8(dst) && f8(a);
                let lit16_op = !matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr);
                let lit16 = lit16_op && (-32768..=32767).contains(l) && f4(dst) && f4(a);
                lit8 || lit16
            }
        }
    }
}
