//! Semantic operations. Each Dalvik opcode maps to exactly one `Op` shape; encoding details
//! that don't affect semantics (`/16`, `/range`, `/2addr`, `/jumbo`, literal widths) are
//! dropped here and chosen again by the writer.

use crate::sym::Sym;

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
    /// Resolved method handle, rendered as text (`invoke-static@Lfoo;->bar()V`).
    ConstMethodHandle { dst: Reg, handle: Sym },
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
    /// `name` and `proto` come from the call site; `bootstrap` is the rendered handle.
    InvokeCustom { name: Sym, proto: Sym, bootstrap: Sym, args: Vec<Reg> },
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
}
