//! Every type a class mentions, found structurally: the header, member signatures, code,
//! catch types, annotations (including generic signatures) and static values.

use crate::model::{Class, Program};
use crate::op::{FieldRef, MethodRef, Op};
use crate::sym::{Interner, Sym};
use crate::types::{parse_proto, signature_class_ranges};
use crate::value::{Annotation, EncodedAnnotation, HandleMember, MethodHandleRef, Value};

struct V<'a, F: FnMut(&str)> {
    s: &'a Interner,
    f: F,
}

impl<F: FnMut(&str)> V<'_, F> {
    fn desc(&mut self, d: &str) {
        let base = d.trim_start_matches('[');
        if base.starts_with('L') {
            (self.f)(base);
        }
    }
    fn ty(&mut self, t: Sym) {
        let s = self.s;
        self.desc(s.get(t));
    }
    fn proto(&mut self, p: Sym) {
        let s = self.s;
        if let Some((params, ret)) = parse_proto(s.get(p)) {
            for x in params {
                self.desc(x);
            }
            self.desc(ret);
        }
    }
    fn field(&mut self, f: &FieldRef) {
        self.ty(f.class);
        self.ty(f.ty);
    }
    fn method(&mut self, m: &MethodRef) {
        self.ty(m.class);
        self.proto(m.proto);
    }
    fn handle(&mut self, h: &MethodHandleRef) {
        match &h.member {
            HandleMember::Field(f) => self.field(f),
            HandleMember::Method(m) => self.method(m),
        }
    }
    fn value(&mut self, v: &Value) {
        match v {
            Value::MethodType(p) => self.proto(*p),
            Value::MethodHandle(h) => self.handle(h),
            Value::Type(t) => self.ty(*t),
            Value::Field(f) | Value::Enum(f) => self.field(f),
            Value::Method(m) => self.method(m),
            Value::Array(a) => a.iter().for_each(|x| self.value(x)),
            Value::Annotation(a) => self.encoded(a),
            _ => {}
        }
    }
    fn encoded(&mut self, a: &EncodedAnnotation) {
        self.ty(a.ty);
        let s = self.s;
        let signature = s.get(a.ty) == "Ldalvik/annotation/Signature;";
        for (_, v) in &a.elements {
            match v {
                // Generic signatures are split into string pieces; rejoin and parse.
                Value::Array(parts) if signature => {
                    let joined: String = parts.iter().filter_map(|p| if let Value::String(x) = p { Some(s.get(*x)) } else { None }).collect();
                    for r in signature_class_ranges(&joined).unwrap_or_default() {
                        (self.f)(&format!("{};", &joined[r]));
                    }
                }
                _ => self.value(v),
            }
        }
    }
    fn annotations(&mut self, anns: &[Annotation]) {
        anns.iter().for_each(|a| self.encoded(&a.annotation));
    }
    fn op(&mut self, op: &Op) {
        match op {
            Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. } | Op::NewInstance { ty, .. }
            | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => self.ty(*ty),
            Op::ConstMethodHandle { handle, .. } => self.handle(handle),
            Op::ConstMethodType { proto, .. } => self.proto(*proto),
            Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. }
            | Op::StaticPut { field, .. } => self.field(field),
            Op::Invoke { method, .. } => self.method(method),
            Op::InvokePolymorphic { method, proto, .. } => {
                self.method(method);
                self.proto(*proto);
            }
            Op::InvokeCustom { call_site, .. } => {
                self.handle(&call_site.bootstrap);
                self.proto(call_site.proto);
                call_site.extra.iter().for_each(|v| self.value(v));
            }
            _ => {}
        }
    }
}

/// Calls `f` with every class descriptor (`Lpkg/Name;`, array element types included) that
/// `c` mentions, its own type included. May repeat.
pub fn class_types(p: &Program, c: &Class, f: impl FnMut(&str)) {
    let mut v = V { s: &p.syms, f };
    v.ty(c.ty);
    if let Some(s) = c.superclass {
        v.ty(s);
    }
    c.interfaces.iter().for_each(|&i| v.ty(i));
    v.annotations(&c.annotations);
    for fd in &c.fields {
        v.ty(fd.ty);
        v.annotations(&fd.annotations);
        if let Some(x) = &fd.static_value {
            v.value(x);
        }
    }
    for m in &c.methods {
        v.proto(m.proto);
        v.annotations(&m.annotations);
        for set in m.parameter_annotations.iter().flatten() {
            v.annotations(set);
        }
        if let Some(b) = &m.code {
            b.insns.iter().for_each(|i| v.op(&i.op));
            for h in b.tries.iter().flat_map(|t| &t.handlers) {
                if let Some(t) = h.ty {
                    v.ty(t);
                }
            }
            for l in &b.locals {
                if let Some(t) = l.ty {
                    v.ty(t);
                }
            }
        }
    }
}

/// Calls `f` with every class descriptor one instruction mentions.
pub fn op_types(p: &Program, op: &Op, f: impl FnMut(&str)) {
    V { s: &p.syms, f }.op(op);
}

/// Calls `f` with every class descriptor an encoded value mentions.
pub fn value_types(p: &Program, v: &Value, f: impl FnMut(&str)) {
    V { s: &p.syms, f }.value(v);
}
