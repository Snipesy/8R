//! Resolved constant values and annotations: the dex `encoded_value` family with every index
//! replaced by an interned reference, so they can be renamed and re-written.

use crate::op::{FieldRef, MethodRef};
use crate::sym::Sym;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodHandleKind {
    StaticPut,
    StaticGet,
    InstancePut,
    InstanceGet,
    InvokeStatic,
    InvokeInstance,
    InvokeConstructor,
    InvokeDirect,
    InvokeInterface,
}

impl MethodHandleKind {
    pub const ALL: [MethodHandleKind; 9] = [
        Self::StaticPut, Self::StaticGet, Self::InstancePut, Self::InstanceGet, Self::InvokeStatic,
        Self::InvokeInstance, Self::InvokeConstructor, Self::InvokeDirect, Self::InvokeInterface,
    ];

    pub fn code(self) -> u16 {
        Self::ALL.iter().position(|&k| k == self).expect("in ALL") as u16
    }

    pub fn is_field(self) -> bool {
        (self.code()) <= 3
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleMember {
    Field(FieldRef),
    Method(MethodRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodHandleRef {
    pub kind: MethodHandleKind,
    pub member: HandleMember,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    pub bootstrap: MethodHandleRef,
    pub name: Sym,
    /// Method descriptor.
    pub proto: Sym,
    /// Extra static arguments to the bootstrap method.
    pub extra: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    /// Raw bits, so NaN payloads and -0.0 round-trip and equality is exact.
    Float(u32),
    Double(u64),
    MethodType(Sym),
    MethodHandle(MethodHandleRef),
    String(Sym),
    Type(Sym),
    Field(FieldRef),
    Method(MethodRef),
    Enum(FieldRef),
    Array(Vec<Value>),
    Annotation(EncodedAnnotation),
    Null,
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncodedAnnotation {
    pub ty: Sym,
    /// (element name, value), in file order.
    pub elements: Vec<(Sym, Value)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Build,
    Runtime,
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub visibility: Visibility,
    pub annotation: EncodedAnnotation,
}

/// Whether `v` equals the implicit initial value of a field (zero, false, or null).
pub fn is_default(v: &Value) -> bool {
    matches!(
        v,
        Value::Byte(0) | Value::Short(0) | Value::Char(0) | Value::Int(0) | Value::Long(0) | Value::Float(0)
            | Value::Double(0) | Value::Null | Value::Boolean(false)
    )
}

/// The implicit initial value for a field of descriptor `ty`.
pub fn default_for(ty: &str) -> Value {
    match ty.as_bytes().first() {
        Some(b'Z') => Value::Boolean(false),
        Some(b'B') => Value::Byte(0),
        Some(b'S') => Value::Short(0),
        Some(b'C') => Value::Char(0),
        Some(b'I') => Value::Int(0),
        Some(b'J') => Value::Long(0),
        Some(b'F') => Value::Float(0),
        Some(b'D') => Value::Double(0),
        _ => Value::Null,
    }
}
