//! Register type inference, modelled on the ART verifier's lattice but simplified: integral
//! subtypes (boolean/byte/char/short/int) collapse to `Int`, and reference joins that would
//! need the class hierarchy collapse to `RefType::Any`.
//!
//! Join semantics ("what can this register hold, merging paths"):
//! `Undefined` is the identity; `Zero` (literal 0) joins with ints, floats, and references;
//! `Narrow` (non-zero 32-bit constant) joins with ints and floats; `WideLo/WideHi` (64-bit
//! constant halves) join with long and double halves; anything else incompatible is
//! `Conflict`.

use std::fmt;

use crate::cfg::{BlockId, Cfg};
use crate::dataflow::{self, Forward};
use crate::lift::Body;
use crate::op::*;
use crate::sym::{Interner, Sym};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    /// Exactly this descriptor (a class or array type).
    Exact(Sym),
    /// Some reference type, not tracked precisely.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegType {
    Undefined,
    Conflict,
    /// The constant 0: int, float, or null.
    Zero,
    /// A non-zero 32-bit constant: int or float.
    Narrow,
    Int,
    Float,
    /// Halves of a 64-bit constant: long or double.
    WideLo,
    WideHi,
    LongLo,
    LongHi,
    DoubleLo,
    DoubleHi,
    Ref(RefType),
    /// Result of `new-instance` at instruction `at`, before its constructor runs.
    Uninit { ty: Sym, at: u32 },
    /// `this` in a constructor, before the super/this constructor call.
    UninitThis(Sym),
}

impl RegType {
    pub fn join(self, other: RegType) -> RegType {
        use RegType::*;
        if self == other {
            return self;
        }
        match (self, other) {
            (Undefined, x) | (x, Undefined) => x,
            (Conflict, _) | (_, Conflict) => Conflict,
            (Zero, Narrow | Int | Float | Ref(_)) => other,
            (Narrow | Int | Float | Ref(_), Zero) => self,
            (Narrow, Int | Float) => other,
            (Int | Float, Narrow) => self,
            (WideLo, LongLo | DoubleLo) => other,
            (LongLo | DoubleLo, WideLo) => self,
            (WideHi, LongHi | DoubleHi) => other,
            (LongHi | DoubleHi, WideHi) => self,
            (Ref(_), Ref(_)) => Ref(RefType::Any),
            _ => Conflict,
        }
    }

    pub fn is_wide_lo(self) -> bool {
        matches!(self, RegType::WideLo | RegType::LongLo | RegType::DoubleLo)
    }

    pub fn is_wide_hi(self) -> bool {
        matches!(self, RegType::WideHi | RegType::LongHi | RegType::DoubleHi)
    }

    pub fn display<'a>(&'a self, syms: &'a Interner) -> impl fmt::Display + 'a {
        struct D<'a>(&'a RegType, &'a Interner);
        impl fmt::Display for D<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.0 {
                    RegType::Ref(RefType::Exact(s)) => write!(f, "{}", self.1.get(*s)),
                    RegType::Ref(RefType::Any) => write!(f, "ref"),
                    RegType::Uninit { ty, at } => write!(f, "uninit({}@{at})", self.1.get(*ty)),
                    RegType::UninitThis(ty) => write!(f, "uninit-this({})", self.1.get(*ty)),
                    other => write!(f, "{other:?}"),
                }
            }
        }
        D(self, syms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeState {
    pub regs: Vec<RegType>,
    /// Pending result of the previous invoke / filled-new-array, as (lo, hi-or-Undefined).
    pub result: Option<(RegType, RegType)>,
}

impl TypeState {
    fn set(&mut self, r: Reg, t: RegType) {
        let r = r as usize;
        if r >= self.regs.len() {
            return;
        }
        // Writing into half of a wide pair invalidates the other half.
        if self.regs[r].is_wide_hi() && r > 0 {
            self.regs[r - 1] = RegType::Conflict;
        }
        if self.regs[r].is_wide_lo() && r + 1 < self.regs.len() {
            self.regs[r + 1] = RegType::Conflict;
        }
        self.regs[r] = t;
    }

    fn set_wide(&mut self, r: Reg, lo: RegType, hi: RegType) {
        self.set(r, RegType::Undefined);
        self.set(r.wrapping_add(1), RegType::Undefined);
        if (r as usize) + 1 < self.regs.len() {
            self.regs[r as usize] = lo;
            self.regs[r as usize + 1] = hi;
        }
    }

    fn set_value(&mut self, r: Reg, v: (RegType, RegType)) {
        if v.0.is_wide_lo() { self.set_wide(r, v.0, v.1) } else { self.set(r, v.0) }
    }

    fn get(&self, r: Reg) -> RegType {
        self.regs.get(r as usize).copied().unwrap_or(RegType::Conflict)
    }
}

/// Signature of the method whose body is analysed.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub class: Sym,
    pub name: String,
    /// Method descriptor, e.g. `(IJ)V`.
    pub proto: String,
    pub is_static: bool,
}

/// Splits `(ILfoo;[J)V` into (["I", "Lfoo;", "[J"], "V").
pub fn parse_proto(proto: &str) -> Option<(Vec<&str>, &str)> {
    let inner = proto.strip_prefix('(')?;
    let close = inner.find(')')?;
    let (params, ret) = (&inner[..close], &inner[close + 1..]);
    let mut out = Vec::new();
    let b = params.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        while b[i] == b'[' {
            i += 1;
            if i >= b.len() {
                return None;
            }
        }
        if b[i] == b'L' {
            i += params[i..].find(';')? + 1;
        } else {
            i += 1;
        }
        out.push(&params[start..i]);
    }
    Some((out, ret))
}

pub struct TypeInference<'a> {
    body: &'a Body,
    cfg: &'a Cfg,
    syms: &'a Interner,
    sig: &'a MethodSig,
    string: Option<Sym>,
    class: Option<Sym>,
    method_handle: Option<Sym>,
    method_type: Option<Sym>,
    throwable: Option<Sym>,
    /// Type of the exception arriving at each block (join of the catch types targeting it).
    catch_types: Vec<RegType>,
}

impl<'a> TypeInference<'a> {
    /// Interns the descriptors the analysis may need to name (array component types and a
    /// few well-known classes), so the analysis itself can run against a shared interner.
    pub fn prepare(body: &Body, sig: &MethodSig, syms: &mut Interner) {
        for d in ["Ljava/lang/String;", "Ljava/lang/Class;", "Ljava/lang/invoke/MethodHandle;", "Ljava/lang/invoke/MethodType;", "Ljava/lang/Throwable;"] {
            syms.intern(d);
        }
        let mut descs: Vec<String> = Vec::new();
        let add_proto = |p: &str, descs: &mut Vec<String>| {
            if let Some((params, ret)) = parse_proto(p) {
                descs.extend(params.iter().map(|s| s.to_string()));
                descs.push(ret.to_string());
            }
        };
        add_proto(&sig.proto, &mut descs);
        for insn in &body.insns {
            match &insn.op {
                Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } | Op::CheckCast { ty, .. } => {
                    descs.push(syms.get(*ty).to_string())
                }
                Op::InstanceGet { field, .. } | Op::StaticGet { field, .. } => descs.push(syms.get(field.ty).to_string()),
                Op::Invoke { method, .. } => add_proto(syms.get(method.proto), &mut descs),
                Op::InvokePolymorphic { proto, .. } => add_proto(syms.get(*proto), &mut descs),
                Op::InvokeCustom { call_site, .. } => add_proto(syms.get(call_site.proto), &mut descs),
                _ => {}
            }
        }
        for mut d in descs {
            loop {
                syms.intern(&d);
                match d.strip_prefix('[') {
                    Some(c) => d = c.to_string(),
                    None => break,
                }
            }
        }
    }

    /// Requires [`TypeInference::prepare`] to have been called with the same interner.
    pub fn new(body: &'a Body, cfg: &'a Cfg, sig: &'a MethodSig, syms: &'a Interner) -> Self {
        let throwable = syms.lookup("Ljava/lang/Throwable;");
        let mut catch_types = vec![RegType::Undefined; cfg.blocks.len()];
        for blk in &cfg.blocks {
            for e in &blk.succs {
                if let crate::cfg::EdgeKind::Catch(ty) = e.kind {
                    let t = RegType::Ref(ty.or(throwable).map_or(RefType::Any, RefType::Exact));
                    catch_types[e.to as usize] = catch_types[e.to as usize].join(t);
                }
            }
        }
        TypeInference {
            body,
            cfg,
            syms,
            sig,
            string: syms.lookup("Ljava/lang/String;"),
            class: syms.lookup("Ljava/lang/Class;"),
            method_handle: syms.lookup("Ljava/lang/invoke/MethodHandle;"),
            method_type: syms.lookup("Ljava/lang/invoke/MethodType;"),
            throwable,
            catch_types,
        }
    }

    fn exact(&self, s: Option<Sym>) -> RegType {
        RegType::Ref(s.map_or(RefType::Any, RefType::Exact))
    }

    /// Type of a value of descriptor `d`, as (lo, hi-or-Undefined).
    fn of_desc(&self, d: &str) -> (RegType, RegType) {
        use RegType::*;
        match d.as_bytes().first() {
            Some(b'Z' | b'B' | b'S' | b'C' | b'I') => (Int, Undefined),
            Some(b'F') => (Float, Undefined),
            Some(b'J') => (LongLo, LongHi),
            Some(b'D') => (DoubleLo, DoubleHi),
            Some(b'L' | b'[') => (self.exact(self.syms.lookup(d)), Undefined),
            _ => (Conflict, Undefined),
        }
    }

    fn of_num(t: NumType) -> (RegType, RegType) {
        use RegType::*;
        match t {
            NumType::Int => (Int, Undefined),
            NumType::Float => (Float, Undefined),
            NumType::Long => (LongLo, LongHi),
            NumType::Double => (DoubleLo, DoubleHi),
        }
    }

    pub fn run(&self) -> Vec<Option<TypeState>> {
        let entry = dataflow::solve(self, self.cfg);
        dataflow::per_instruction(self, self.body, self.cfg, &entry)
    }
}

impl Forward for TypeInference<'_> {
    type State = TypeState;

    fn entry_state(&self) -> TypeState {
        let mut s = TypeState { regs: vec![RegType::Undefined; self.body.registers as usize], result: None };
        let mut r = self.body.registers.saturating_sub(self.body.ins);
        if !self.sig.is_static {
            let this = if self.sig.name == "<init>" {
                RegType::UninitThis(self.sig.class)
            } else {
                RegType::Ref(RefType::Exact(self.sig.class))
            };
            s.set(r, this);
            r += 1;
        }
        if let Some((params, _)) = parse_proto(&self.sig.proto) {
            for p in params {
                let v = self.of_desc(p);
                s.set_value(r, v);
                r += if v.0.is_wide_lo() { 2 } else { 1 };
            }
        }
        s
    }

    fn join(&self, into: &mut Option<TypeState>, incoming: &TypeState) -> bool {
        match into {
            None => {
                *into = Some(incoming.clone());
                true
            }
            Some(cur) => {
                let mut changed = false;
                for (a, &b) in cur.regs.iter_mut().zip(&incoming.regs) {
                    let j = a.join(b);
                    if j != *a {
                        *a = j;
                        changed = true;
                    }
                }
                let result = match (cur.result, incoming.result) {
                    (Some(a), Some(b)) => Some((a.0.join(b.0), a.1.join(b.1))),
                    (a, b) => a.or(b),
                };
                if result != cur.result {
                    cur.result = result;
                    changed = true;
                }
                changed
            }
        }
    }

    fn transfer(&self, block: BlockId, idx: u32, s: &mut TypeState) {
        use RegType::*;
        let op = &self.body.insns[idx as usize].op;
        let pending = s.result.take();
        match op {
            Op::Nop | Op::ReturnVoid | Op::Return { .. } | Op::MonitorEnter { .. } | Op::MonitorExit { .. }
            | Op::FillArrayData { .. } | Op::Throw { .. } | Op::Goto { .. } | Op::Switch { .. } | Op::If { .. }
            | Op::IfZ { .. } | Op::ArrayPut { .. } | Op::InstancePut { .. } | Op::StaticPut { .. } => {}
            Op::Move { width: Width::Wide, dst, src } => {
                let v = (s.get(*src), s.get(src.wrapping_add(1)));
                s.set_wide(*dst, v.0, v.1);
            }
            Op::Move { dst, src, .. } => {
                let v = s.get(*src);
                s.set(*dst, v);
            }
            Op::MoveResult { dst, .. } => s.set_value(*dst, pending.unwrap_or((Conflict, Undefined))),
            Op::MoveException { dst } => {
                let t = match self.catch_types[block as usize] {
                    RegType::Undefined => self.exact(self.throwable),
                    t => t,
                };
                s.set(*dst, t);
            }
            Op::Const { dst, value: Const::Narrow(v) } => s.set(*dst, if *v == 0 { Zero } else { Narrow }),
            Op::Const { dst, value: Const::Wide(_) } => s.set_wide(*dst, WideLo, WideHi),
            Op::ConstString { dst, .. } => s.set(*dst, self.exact(self.string)),
            Op::ConstClass { dst, .. } => s.set(*dst, self.exact(self.class)),
            Op::ConstMethodHandle { dst, .. } => s.set(*dst, self.exact(self.method_handle)),
            Op::ConstMethodType { dst, .. } => s.set(*dst, self.exact(self.method_type)),
            Op::CheckCast { reg, ty } => s.set(*reg, Ref(RefType::Exact(*ty))),
            Op::InstanceOf { dst, .. } | Op::ArrayLength { dst, .. } | Op::Cmp { dst, .. } => s.set(*dst, Int),
            Op::NewInstance { dst, ty } => s.set(*dst, Uninit { ty: *ty, at: idx }),
            Op::NewArray { dst, ty, .. } => s.set(*dst, Ref(RefType::Exact(*ty))),
            Op::FilledNewArray { ty, .. } => s.result = Some((Ref(RefType::Exact(*ty)), Undefined)),
            Op::ArrayGet { kind, dst, array, .. } => {
                let component = match s.get(*array) {
                    Ref(RefType::Exact(a)) => self.syms.get(a).strip_prefix('[').map(str::to_string),
                    _ => None,
                };
                let v = match (kind, component.as_deref()) {
                    (MemKind::Narrow, Some(c @ ("I" | "F"))) => self.of_desc(c),
                    (MemKind::Narrow, _) => (Narrow, Undefined),
                    (MemKind::Wide, Some(c @ ("J" | "D"))) => self.of_desc(c),
                    (MemKind::Wide, _) => (WideLo, WideHi),
                    (MemKind::Object, Some(c)) if c.starts_with(['L', '[']) => self.of_desc(c),
                    (MemKind::Object, _) => (Ref(RefType::Any), Undefined),
                    _ => (Int, Undefined),
                };
                s.set_value(*dst, v);
            }
            Op::InstanceGet { dst, field, .. } | Op::StaticGet { dst, field, .. } => {
                let v = self.of_desc(self.syms.get(field.ty));
                s.set_value(*dst, v);
            }
            Op::Invoke { kind, method, args } => {
                if *kind == InvokeKind::Direct && self.syms.get(method.name) == "<init>" {
                    if let Some(&this) = args.first() {
                        let init = match s.get(this) {
                            Uninit { ty, .. } | UninitThis(ty) => Some((s.get(this), ty)),
                            _ => None,
                        };
                        if let Some((uninit, ty)) = init {
                            for r in s.regs.iter_mut() {
                                if *r == uninit {
                                    *r = Ref(RefType::Exact(ty));
                                }
                            }
                        }
                    }
                }
                s.result = self.return_type(self.syms.get(method.proto));
            }
            Op::InvokePolymorphic { proto, .. } => s.result = self.return_type(self.syms.get(*proto)),
            Op::InvokeCustom { call_site, .. } => s.result = self.return_type(self.syms.get(call_site.proto)),
            Op::Unop { op, dst, .. } => s.set_value(*dst, Self::of_num(op.types().1)),
            Op::Binop { ty, dst, .. } => s.set_value(*dst, Self::of_num(*ty)),
        }
    }
}

impl TypeInference<'_> {
    fn return_type(&self, proto: &str) -> Option<(RegType, RegType)> {
        let (_, ret) = parse_proto(proto)?;
        (ret != "V").then(|| self.of_desc(ret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RegType::*;

    #[test]
    fn proto_parsing() {
        assert_eq!(parse_proto("()V"), Some((vec![], "V")));
        assert_eq!(
            parse_proto("(IJLjava/lang/String;[[D[Lfoo;)Z"),
            Some((vec!["I", "J", "Ljava/lang/String;", "[[D", "[Lfoo;"], "Z"))
        );
        assert_eq!(parse_proto("(Lfoo)V"), None);
        assert_eq!(parse_proto("([)V"), None);
    }

    #[test]
    fn lattice_joins() {
        assert_eq!(Undefined.join(Int), Int);
        assert_eq!(Zero.join(Int), Int);
        assert_eq!(Zero.join(Ref(RefType::Any)), Ref(RefType::Any));
        assert_eq!(Narrow.join(Float), Float);
        assert_eq!(Narrow.join(Zero), Narrow);
        assert_eq!(Int.join(Float), Conflict);
        assert_eq!(Narrow.join(Ref(RefType::Any)), Conflict);
        assert_eq!(WideLo.join(DoubleLo), DoubleLo);
        assert_eq!(LongLo.join(DoubleLo), Conflict);
        assert_eq!(Conflict.join(Undefined), Conflict);
    }

    #[test]
    fn join_is_commutative_and_idempotent() {
        let mut syms = Interner::default();
        let a = syms.intern("La;");
        let b = syms.intern("Lb;");
        let all = [
            Undefined, Conflict, Zero, Narrow, Int, Float, WideLo, WideHi, LongLo, LongHi, DoubleLo, DoubleHi,
            Ref(RefType::Exact(a)), Ref(RefType::Exact(b)), Ref(RefType::Any), Uninit { ty: a, at: 1 }, UninitThis(a),
        ];
        for &x in &all {
            assert_eq!(x.join(x), x);
            for &y in &all {
                assert_eq!(x.join(y), y.join(x), "{x:?} {y:?}");
                // Associativity, which the solver's termination and order-independence rely on.
                for &z in &all {
                    assert_eq!(x.join(y).join(z), x.join(y.join(z)), "{x:?} {y:?} {z:?}");
                }
            }
        }
    }
}
