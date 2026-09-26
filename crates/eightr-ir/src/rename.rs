//! Consistent renaming of classes and members across a whole [`Program`]: every reference in
//! code, signatures, annotations, values, and debug info follows. Used by the α-invariance
//! tests (to produce equally valid alternative R8 outputs) and by the naming stage.

use std::collections::BTreeMap;

use crate::lift::Body;
use crate::model::Program;
use crate::op::{FieldRef, MethodRef, Op};
use crate::sym::{Interner, Sym};
use crate::types::parse_proto;
use crate::value::{Annotation, CallSite, EncodedAnnotation, HandleMember, MethodHandleRef, Value};

#[derive(Debug, Clone, Default)]
pub struct Renaming {
    /// Class descriptor → new class descriptor (`La/b;` → `La/c;`).
    pub classes: BTreeMap<String, String>,
    /// Member name → new name. Applies to every field and method of a **program** class
    /// with that name, and to every reference to such a member (references whose owner is a
    /// program class). Renaming by name keeps overrides and overloads consistent.
    pub members: BTreeMap<String, String>,
}

struct Ctx<'a> {
    r: &'a Renaming,
    /// Descriptors of program classes, before renaming.
    program: std::collections::BTreeSet<String>,
    syms: &'a mut Interner,
}

impl Ctx<'_> {
    fn desc(&self, d: &str) -> String {
        let dims = d.bytes().take_while(|&b| b == b'[').count();
        match self.r.classes.get(&d[dims..]) {
            Some(n) => format!("{}{}", &d[..dims], n),
            None => d.to_string(),
        }
    }
    fn ty(&mut self, t: Sym) -> Sym {
        let new = self.desc(self.syms.get(t));
        self.syms.intern(&new)
    }
    fn proto(&mut self, p: Sym) -> Sym {
        let old = self.syms.get(p).to_string();
        let Some((params, ret)) = parse_proto(&old) else { return p };
        let new = format!("({}){}", params.iter().map(|x| self.desc(x)).collect::<String>(), self.desc(ret));
        self.syms.intern(&new)
    }
    /// Rewrites class names inside a JVM generic signature (`Ljava/util/List<La/b;>;`).
    fn signature(&self, sig: &str) -> String {
        let mut out = String::with_capacity(sig.len());
        let b = sig.as_bytes();
        let mut i = 0;
        while i < b.len() {
            // A class type starts with 'L' right after a delimiter (or at the start).
            let at_start = i == 0 || matches!(b[i - 1], b'(' | b')' | b'<' | b'>' | b';' | b'[' | b':' | b'+' | b'-' | b'^');
            if b[i] == b'L' && at_start {
                let end = sig[i..].find([';', '<']).map_or(b.len(), |e| i + e);
                let key = format!("{};", &sig[i..end]);
                match self.r.classes.get(&key) {
                    Some(n) => out.push_str(n.trim_end_matches(';')),
                    None => out.push_str(&sig[i..end]),
                }
                i = end;
            } else {
                out.push(b[i] as char);
                i += 1;
            }
        }
        out
    }
    fn member_name(&mut self, owner_before: Sym, name: Sym) -> Sym {
        if !self.program.contains(self.syms.get(owner_before)) {
            return name;
        }
        match self.r.members.get(self.syms.get(name)) {
            Some(n) => {
                let n = n.clone();
                self.syms.intern(&n)
            }
            None => name,
        }
    }
    fn field(&mut self, f: FieldRef) -> FieldRef {
        FieldRef { name: self.member_name(f.class, f.name), class: self.ty(f.class), ty: self.ty(f.ty) }
    }
    fn method(&mut self, m: MethodRef) -> MethodRef {
        MethodRef { name: self.member_name(m.class, m.name), class: self.ty(m.class), proto: self.proto(m.proto) }
    }
    fn handle(&mut self, h: MethodHandleRef) -> MethodHandleRef {
        let member = match h.member {
            HandleMember::Field(f) => HandleMember::Field(self.field(f)),
            HandleMember::Method(m) => HandleMember::Method(self.method(m)),
        };
        MethodHandleRef { kind: h.kind, member }
    }
    fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::MethodType(p) => Value::MethodType(self.proto(*p)),
            Value::MethodHandle(h) => Value::MethodHandle(self.handle(*h)),
            Value::Type(t) => Value::Type(self.ty(*t)),
            Value::Field(f) => Value::Field(self.field(*f)),
            Value::Method(m) => Value::Method(self.method(*m)),
            Value::Enum(f) => Value::Enum(self.field(*f)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Annotation(a) => Value::Annotation(self.encoded(a, None)),
            other => other.clone(),
        }
    }
    /// `owner_before`: the annotated class's descriptor before renaming, for InnerClass names.
    fn encoded(&mut self, a: &EncodedAnnotation, owner_before: Option<&str>) -> EncodedAnnotation {
        let ann_ty = self.syms.get(a.ty).to_string();
        let mut elements = Vec::with_capacity(a.elements.len());
        for (name, v) in &a.elements {
            let ename = self.syms.get(*name).to_string();
            let nv = match (ann_ty.as_str(), ename.as_str(), v) {
                // The inner simple name: follows the renamed class's last '$' segment.
                ("Ldalvik/annotation/InnerClass;", "name", Value::String(s)) => {
                    let new_owner = owner_before.and_then(|o| self.r.classes.get(o));
                    match new_owner {
                        Some(n) => {
                            let simple = n.trim_end_matches(';').rsplit(['/', '$']).next().unwrap_or("").to_string();
                            Value::String(self.syms.intern(&simple))
                        }
                        None => Value::String(*s),
                    }
                }
                // Generic signatures are split into arbitrary string pieces; rejoin, rewrite,
                // and store as one piece (semantically identical).
                ("Ldalvik/annotation/Signature;", "value", Value::Array(parts)) => {
                    let joined: String = parts
                        .iter()
                        .map(|p| match p {
                            Value::String(s) => self.syms.get(*s).to_string(),
                            _ => String::new(),
                        })
                        .collect();
                    let rewritten = self.signature(&joined);
                    if rewritten == joined {
                        v.clone()
                    } else {
                        Value::Array(vec![Value::String(self.syms.intern(&rewritten))])
                    }
                }
                _ => self.value(v),
            };
            elements.push((*name, nv));
        }
        EncodedAnnotation { ty: self.ty(a.ty), elements }
    }
    fn annotations(&mut self, anns: &[Annotation], owner_before: Option<&str>) -> Vec<Annotation> {
        anns.iter().map(|a| Annotation { visibility: a.visibility, annotation: self.encoded(&a.annotation, owner_before) }).collect()
    }
    fn call_site(&mut self, c: &CallSite) -> CallSite {
        CallSite {
            bootstrap: self.handle(c.bootstrap),
            name: c.name,
            proto: self.proto(c.proto),
            extra: c.extra.iter().map(|v| self.value(v)).collect(),
        }
    }
    fn op(&mut self, op: &Op) -> Op {
        let mut op = op.clone();
        match &mut op {
            Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. } | Op::NewInstance { ty, .. }
            | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => *ty = self.ty(*ty),
            Op::ConstMethodHandle { handle, .. } => *handle = self.handle(*handle),
            Op::ConstMethodType { proto, .. } => *proto = self.proto(*proto),
            Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. }
            | Op::StaticPut { field, .. } => *field = self.field(*field),
            Op::Invoke { method, .. } => *method = self.method(*method),
            Op::InvokePolymorphic { method, proto, .. } => {
                *method = self.method(*method);
                *proto = self.proto(*proto);
            }
            Op::InvokeCustom { call_site, .. } => **call_site = self.call_site(call_site),
            _ => {}
        }
        op
    }
    fn body(&mut self, b: &Body) -> Body {
        let mut b = b.clone();
        for i in &mut b.insns {
            i.op = self.op(&i.op);
        }
        for t in &mut b.tries {
            for h in &mut t.handlers {
                h.ty = h.ty.map(|t| self.ty(t));
            }
        }
        for l in &mut b.locals {
            l.ty = l.ty.map(|t| self.ty(t));
            if let Some(sig) = l.signature {
                let new = self.signature(self.syms.get(sig));
                l.signature = Some(self.syms.intern(&new));
            }
        }
        b
    }
}

impl Renaming {
    /// Applies the renaming to `p` in place and re-sorts classes.
    pub fn apply(&self, p: &mut Program) {
        let program = p.classes.iter().map(|c| p.syms.get(c.ty).to_string()).collect();
        let mut syms = std::mem::take(&mut p.syms);
        let mut cx = Ctx { r: self, program, syms: &mut syms };
        for c in &mut p.classes {
            let owner = cx.syms.get(c.ty).to_string();
            c.annotations = cx.annotations(&c.annotations, Some(&owner));
            for f in &mut c.fields {
                f.name = cx.member_name(c.ty, f.name);
                f.ty = cx.ty(f.ty);
                f.static_value = f.static_value.as_ref().map(|v| cx.value(v));
                f.annotations = cx.annotations(&f.annotations, None);
            }
            for m in &mut c.methods {
                m.name = cx.member_name(c.ty, m.name);
                m.proto = cx.proto(m.proto);
                m.annotations = cx.annotations(&m.annotations, None);
                if let Some(ps) = &mut m.parameter_annotations {
                    for set in ps.iter_mut() {
                        *set = cx.annotations(set, None);
                    }
                }
                if let Some(b) = &m.code {
                    m.code = Some(cx.body(b));
                }
            }
            c.superclass = c.superclass.map(|t| cx.ty(t));
            c.interfaces = c.interfaces.iter().map(|t| cx.ty(*t)).collect();
            c.ty = cx.ty(c.ty);
        }
        p.syms = syms;
        p.sort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_rewriting() {
        let r = Renaming {
            classes: [("La/b;".to_string(), "La/z;".to_string()), ("Lq;".to_string(), "Lr;".to_string())].into(),
            members: BTreeMap::new(),
        };
        let mut syms = Interner::default();
        let cx = Ctx { r: &r, program: Default::default(), syms: &mut syms };
        assert_eq!(cx.signature("Ljava/util/List<La/b;>;"), "Ljava/util/List<La/z;>;");
        assert_eq!(cx.signature("(La/b;[Lq;)La/b;"), "(La/z;[Lr;)La/z;");
        assert_eq!(cx.signature("<T:La/b;>Ljava/lang/Object;"), "<T:La/z;>Ljava/lang/Object;");
        assert_eq!(cx.signature("La/b<TT;>;"), "La/z<TT;>;");
        assert_eq!(cx.desc("[[La/b;"), "[[La/z;");
        assert_eq!(cx.desc("I"), "I");
    }
}
