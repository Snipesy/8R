//! Constant pools: every string, type, proto, field, method, method handle, and call site a
//! set of classes references, sorted in the orders the dex format requires.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::model::{Class, Program};
use eightr_ir::op::{FieldRef, MethodRef, Op};
use eightr_ir::types::parse_proto;
use eightr_ir::value::{Annotation, CallSite, EncodedAnnotation, HandleMember, MethodHandleRef, Value};
use eightr_ir::Interner;

use crate::WriteError;

/// Dex string order: by UTF-16 code units.
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtoKey {
    pub ret: u32,
    pub params: Vec<u32>,
}

#[derive(Default)]
struct Collect {
    strings: BTreeSet<String>,
    types: BTreeSet<String>,
    protos: BTreeSet<String>,
    fields: BTreeSet<(String, String, String)>,
    methods: BTreeSet<(String, String, String)>,
    handles: Vec<MethodHandleRef>,
    call_sites: Vec<CallSite>,
}

impl Collect {
    fn string(&mut self, s: &str) {
        if !self.strings.contains(s) {
            self.strings.insert(s.to_string());
        }
    }
    fn ty(&mut self, t: &str) {
        self.string(t);
        if !self.types.contains(t) {
            self.types.insert(t.to_string());
        }
    }
    fn proto(&mut self, p: &str) -> Result<(), WriteError> {
        let (params, ret) = parse_proto(p).ok_or_else(|| WriteError(format!("bad method descriptor {p}")))?;
        self.string(&shorty(p)?);
        self.ty(ret);
        for t in params {
            self.ty(t);
        }
        self.protos.insert(p.to_string());
        Ok(())
    }
    fn field(&mut self, f: &FieldRef, s: &Interner) {
        self.ty(s.get(f.class));
        self.string(s.get(f.name));
        self.ty(s.get(f.ty));
        self.fields.insert((s.get(f.class).into(), s.get(f.name).into(), s.get(f.ty).into()));
    }
    fn method(&mut self, m: &MethodRef, s: &Interner) -> Result<(), WriteError> {
        self.ty(s.get(m.class));
        self.string(s.get(m.name));
        self.proto(s.get(m.proto))?;
        self.methods.insert((s.get(m.class).into(), s.get(m.name).into(), s.get(m.proto).into()));
        Ok(())
    }
    fn handle(&mut self, h: &MethodHandleRef, s: &Interner) -> Result<(), WriteError> {
        match &h.member {
            HandleMember::Field(f) => self.field(f, s),
            HandleMember::Method(m) => self.method(m, s)?,
        }
        if !self.handles.contains(h) {
            self.handles.push(*h);
        }
        Ok(())
    }
    fn value(&mut self, v: &Value, s: &Interner) -> Result<(), WriteError> {
        match v {
            Value::MethodType(p) => self.proto(s.get(*p))?,
            Value::MethodHandle(h) => self.handle(h, s)?,
            Value::String(x) => self.string(s.get(*x)),
            Value::Type(t) => self.ty(s.get(*t)),
            Value::Field(f) | Value::Enum(f) => self.field(f, s),
            Value::Method(m) => self.method(m, s)?,
            Value::Array(a) => {
                for x in a {
                    self.value(x, s)?;
                }
            }
            Value::Annotation(a) => self.encoded_annotation(a, s)?,
            _ => {}
        }
        Ok(())
    }
    fn encoded_annotation(&mut self, a: &EncodedAnnotation, s: &Interner) -> Result<(), WriteError> {
        self.ty(s.get(a.ty));
        for (n, v) in &a.elements {
            self.string(s.get(*n));
            self.value(v, s)?;
        }
        Ok(())
    }
    fn annotations(&mut self, anns: &[Annotation], s: &Interner) -> Result<(), WriteError> {
        for a in anns {
            self.encoded_annotation(&a.annotation, s)?;
        }
        Ok(())
    }
    fn call_site(&mut self, c: &CallSite, s: &Interner) -> Result<(), WriteError> {
        self.handle(&c.bootstrap, s)?;
        self.string(s.get(c.name));
        self.proto(s.get(c.proto))?;
        for v in &c.extra {
            self.value(v, s)?;
        }
        if !self.call_sites.contains(c) {
            self.call_sites.push(c.clone());
        }
        Ok(())
    }
    fn op(&mut self, op: &Op, s: &Interner) -> Result<(), WriteError> {
        match op {
            Op::ConstString { value, .. } => self.string(s.get(*value)),
            Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. }
            | Op::NewInstance { ty, .. } | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => self.ty(s.get(*ty)),
            Op::ConstMethodHandle { handle, .. } => self.handle(handle, s)?,
            Op::ConstMethodType { proto, .. } => self.proto(s.get(*proto))?,
            Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. }
            | Op::StaticPut { field, .. } => self.field(field, s),
            Op::Invoke { method, .. } => self.method(method, s)?,
            Op::InvokePolymorphic { method, proto, .. } => {
                self.method(method, s)?;
                self.proto(s.get(*proto))?;
            }
            Op::InvokeCustom { call_site, .. } => self.call_site(call_site, s)?,
            Op::Switch { .. } | Op::FillArrayData { .. } | Op::Nop | Op::Move { .. } | Op::MoveResult { .. }
            | Op::MoveException { .. } | Op::ReturnVoid | Op::Return { .. } | Op::Const { .. }
            | Op::MonitorEnter { .. } | Op::MonitorExit { .. } | Op::ArrayLength { .. } | Op::Throw { .. }
            | Op::Goto { .. } | Op::Cmp { .. } | Op::If { .. } | Op::IfZ { .. } | Op::ArrayGet { .. }
            | Op::ArrayPut { .. } | Op::Unop { .. } | Op::Binop { .. } => {}
        }
        Ok(())
    }
    fn class(&mut self, c: &Class, s: &Interner) -> Result<(), WriteError> {
        self.ty(s.get(c.ty));
        if let Some(t) = c.superclass {
            self.ty(s.get(t));
        }
        for t in &c.interfaces {
            self.ty(s.get(*t));
        }
        if let Some(f) = c.source_file {
            self.string(s.get(f));
        }
        self.annotations(&c.annotations, s)?;
        for f in &c.fields {
            self.field(&FieldRef { class: c.ty, name: f.name, ty: f.ty }, s);
            if let Some(v) = &f.static_value {
                self.value(v, s)?;
            }
            self.annotations(&f.annotations, s)?;
        }
        for m in &c.methods {
            self.method(&MethodRef { class: c.ty, name: m.name, proto: m.proto }, s)?;
            self.annotations(&m.annotations, s)?;
            for set in m.parameter_annotations.iter().flatten() {
                self.annotations(set, s)?;
            }
            if let Some(b) = &m.code {
                for i in &b.insns {
                    self.op(&i.op, s)?;
                }
                for t in &b.tries {
                    for h in t.handlers.iter().filter_map(|h| h.ty) {
                        self.ty(s.get(h));
                    }
                }
                for l in &b.locals {
                    if let Some(n) = l.name {
                        self.string(s.get(n));
                    }
                    if let Some(t) = l.ty {
                        self.ty(s.get(t));
                    }
                    if let Some(g) = l.signature {
                        self.string(s.get(g));
                    }
                }
                for n in b.parameter_names.iter().flatten() {
                    self.string(s.get(*n));
                }
            }
        }
        Ok(())
    }
}

/// Shorty descriptor: return type then parameters, references collapsed to `L`.
pub fn shorty(proto: &str) -> Result<String, WriteError> {
    let (params, ret) = parse_proto(proto).ok_or_else(|| WriteError(format!("bad method descriptor {proto}")))?;
    let short = |t: &str| if t.starts_with(['L', '[']) { 'L' } else { t.chars().next().unwrap_or('V') };
    Ok(std::iter::once(short(ret)).chain(params.iter().map(|t| short(t))).collect())
}

/// Sorted pools with index lookups.
pub struct Pools {
    pub strings: Vec<String>,
    pub string_idx: BTreeMap<String, u32>,
    pub types: Vec<String>,
    pub type_idx: BTreeMap<String, u32>,
    /// (descriptor, key) sorted by key.
    pub protos: Vec<(String, ProtoKey)>,
    pub proto_idx: BTreeMap<String, u32>,
    /// (class, name, type) sorted by indices.
    pub fields: Vec<(String, String, String)>,
    pub field_idx: BTreeMap<(String, String, String), u32>,
    pub methods: Vec<(String, String, String)>,
    pub method_idx: BTreeMap<(String, String, String), u32>,
    pub handles: Vec<MethodHandleRef>,
    pub call_sites: Vec<CallSite>,
}

impl Pools {
    pub fn build(p: &Program, classes: &[usize]) -> Result<Pools, WriteError> {
        let s = &p.syms;
        let mut c = Collect::default();
        for &i in classes {
            c.class(&p.classes[i], s)?;
        }
        for r in &p.retained_strings {
            c.string(r);
        }
        let mut strings: Vec<String> = c.strings.into_iter().collect();
        strings.sort_by(|a, b| utf16_cmp(a, b));
        let string_idx: BTreeMap<String, u32> = strings.iter().enumerate().map(|(i, x)| (x.clone(), i as u32)).collect();
        let mut types: Vec<String> = c.types.into_iter().collect();
        types.sort_by_key(|t| string_idx[t]);
        let type_idx: BTreeMap<String, u32> = types.iter().enumerate().map(|(i, x)| (x.clone(), i as u32)).collect();

        let mut protos: Vec<(String, ProtoKey)> = c
            .protos
            .into_iter()
            .map(|d| {
                let (params, ret) = parse_proto(&d).expect("validated when collected");
                let key = ProtoKey { ret: type_idx[ret], params: params.iter().map(|t| type_idx[*t]).collect() };
                (d, key)
            })
            .collect();
        protos.sort_by(|a, b| a.1.cmp(&b.1));
        let proto_idx: BTreeMap<String, u32> = protos.iter().enumerate().map(|(i, x)| (x.0.clone(), i as u32)).collect();

        let mut fields: Vec<(String, String, String)> = c.fields.into_iter().collect();
        fields.sort_by_key(|(cl, n, t)| (type_idx[cl], string_idx[n], type_idx[t]));
        let field_idx = fields.iter().enumerate().map(|(i, x)| (x.clone(), i as u32)).collect();
        let mut methods: Vec<(String, String, String)> = c.methods.into_iter().collect();
        methods.sort_by_key(|(cl, n, pr)| (type_idx[cl], string_idx[n], proto_idx[pr]));
        let method_idx = methods.iter().enumerate().map(|(i, x)| (x.clone(), i as u32)).collect();

        let mut pools = Pools {
            strings,
            string_idx,
            types,
            type_idx,
            protos,
            proto_idx,
            fields,
            field_idx,
            methods,
            method_idx,
            handles: c.handles,
            call_sites: c.call_sites,
        };
        // Canonical orders for handles and call sites (no spec order; must be deterministic).
        let mut hs = std::mem::take(&mut pools.handles);
        hs.sort_by_key(|h| pools.handle_key(h, s));
        pools.handles = hs;
        let mut cs = std::mem::take(&mut pools.call_sites);
        cs.sort_by_cached_key(|c| pools.call_site_key(c, s));
        pools.call_sites = cs;
        Ok(pools)
    }

    fn handle_key(&self, h: &MethodHandleRef, s: &Interner) -> (u16, u32) {
        let member = match &h.member {
            HandleMember::Field(f) => self.field(f, s),
            HandleMember::Method(m) => self.method(m, s),
        };
        (h.kind.code(), member)
    }

    fn call_site_key(&self, c: &CallSite, s: &Interner) -> (u32, u32, u32, String) {
        let extra = c.extra.iter().map(|v| eightr_ir::print::value(v, s)).collect::<Vec<_>>().join(",");
        (self.handle(&c.bootstrap, s), self.string(s.get(c.name)), self.proto(s.get(c.proto)), extra)
    }

    pub fn string(&self, x: &str) -> u32 {
        self.string_idx[x]
    }
    pub fn ty(&self, x: &str) -> u32 {
        self.type_idx[x]
    }
    pub fn proto(&self, x: &str) -> u32 {
        self.proto_idx[x]
    }
    pub fn field(&self, f: &FieldRef, s: &Interner) -> u32 {
        self.field_idx[&(s.get(f.class).to_string(), s.get(f.name).to_string(), s.get(f.ty).to_string())]
    }
    pub fn method(&self, m: &MethodRef, s: &Interner) -> u32 {
        self.method_idx[&(s.get(m.class).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string())]
    }
    pub fn handle(&self, h: &MethodHandleRef, _s: &Interner) -> u32 {
        self.handles.iter().position(|x| x == h).expect("collected") as u32
    }
    pub fn call_site(&self, c: &CallSite) -> u32 {
        self.call_sites.iter().position(|x| x == c).expect("collected") as u32
    }
}
