//! Turning dex indices into interned references. Shared by body lifting and model loading.

use eightr_dex::value::{self as dv, EncodedValue};
use eightr_dex::{Dex, DexError, ErrorKind, Result};

use crate::op::{FieldRef, MethodRef};
use crate::sym::{Interner, Sym};
use crate::value::*;

pub struct Resolver<'a, 'd> {
    pub dex: &'a Dex<'d>,
    pub syms: &'a mut Interner,
}

impl Resolver<'_, '_> {
    pub fn string(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.string(idx)?;
        Ok(self.syms.intern(&s))
    }
    pub fn ty(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.type_descriptor(idx)?;
        Ok(self.syms.intern(&s))
    }
    pub fn proto(&mut self, idx: u32) -> Result<Sym> {
        let s = self.dex.proto_descriptor(idx)?;
        Ok(self.syms.intern(&s))
    }
    pub fn field(&mut self, idx: u32) -> Result<FieldRef> {
        let f = self.dex.field_id(idx)?;
        Ok(FieldRef { class: self.ty(f.class_idx.into())?, name: self.string(f.name_idx)?, ty: self.ty(f.type_idx.into())? })
    }
    pub fn method(&mut self, idx: u32) -> Result<MethodRef> {
        let m = self.dex.method_id(idx)?;
        Ok(MethodRef { class: self.ty(m.class_idx.into())?, name: self.string(m.name_idx)?, proto: self.proto(m.proto_idx.into())? })
    }
    pub fn method_handle(&mut self, idx: u32) -> Result<MethodHandleRef> {
        let h = self.dex.method_handle(idx)?;
        let kind = *MethodHandleKind::ALL
            .get(h.kind as usize)
            .ok_or_else(|| DexError::new(0, ErrorKind::Malformed("unknown method handle kind")))?;
        let member = if kind.is_field() {
            HandleMember::Field(self.field(h.field_or_method_idx.into())?)
        } else {
            HandleMember::Method(self.method(h.field_or_method_idx.into())?)
        };
        Ok(MethodHandleRef { kind, member })
    }
    pub fn call_site(&mut self, idx: u32) -> Result<CallSite> {
        let site = self.dex.call_site(idx)?;
        let bad = || DexError::new(0, ErrorKind::Malformed("malformed call site"));
        let (Some(EncodedValue::MethodHandle(h)), Some(EncodedValue::String(name)), Some(EncodedValue::MethodType(proto))) =
            (site.first(), site.get(1), site.get(2))
        else {
            return Err(bad());
        };
        let (h, name, proto) = (*h, *name, *proto);
        let extra = site[3..].iter().map(|v| self.value(v)).collect::<Result<_>>()?;
        Ok(CallSite { bootstrap: self.method_handle(h)?, name: self.string(name)?, proto: self.proto(proto)?, extra })
    }
    pub fn value(&mut self, v: &EncodedValue) -> Result<Value> {
        Ok(match v {
            EncodedValue::Byte(x) => Value::Byte(*x),
            EncodedValue::Short(x) => Value::Short(*x),
            EncodedValue::Char(x) => Value::Char(*x),
            EncodedValue::Int(x) => Value::Int(*x),
            EncodedValue::Long(x) => Value::Long(*x),
            EncodedValue::Float(x) => Value::Float(x.to_bits()),
            EncodedValue::Double(x) => Value::Double(x.to_bits()),
            EncodedValue::MethodType(i) => Value::MethodType(self.proto(*i)?),
            EncodedValue::MethodHandle(i) => Value::MethodHandle(self.method_handle(*i)?),
            EncodedValue::String(i) => Value::String(self.string(*i)?),
            EncodedValue::Type(i) => Value::Type(self.ty(*i)?),
            EncodedValue::Field(i) => Value::Field(self.field(*i)?),
            EncodedValue::Method(i) => Value::Method(self.method(*i)?),
            EncodedValue::Enum(i) => Value::Enum(self.field(*i)?),
            EncodedValue::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect::<Result<_>>()?),
            EncodedValue::Annotation(a) => Value::Annotation(self.encoded_annotation(a)?),
            EncodedValue::Null => Value::Null,
            EncodedValue::Boolean(b) => Value::Boolean(*b),
        })
    }
    pub fn encoded_annotation(&mut self, a: &dv::EncodedAnnotation) -> Result<EncodedAnnotation> {
        let mut elements = Vec::with_capacity(a.elements.len());
        for (name, v) in &a.elements {
            elements.push((self.string(*name)?, self.value(v)?));
        }
        Ok(EncodedAnnotation { ty: self.ty(a.type_idx)?, elements })
    }
    pub fn annotation(&mut self, a: &dv::Annotation) -> Result<Annotation> {
        let visibility = match a.visibility {
            dv::Visibility::Build => Visibility::Build,
            dv::Visibility::Runtime => Visibility::Runtime,
            dv::Visibility::System => Visibility::System,
        };
        Ok(Annotation { visibility, annotation: self.encoded_annotation(&a.annotation)? })
    }
    pub fn annotation_set(&mut self, off: u32) -> Result<Vec<Annotation>> {
        let set = self.dex.annotation_set(off)?;
        set.iter().map(|a| self.annotation(a)).collect()
    }
}
