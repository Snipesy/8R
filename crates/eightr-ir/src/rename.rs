//! Consistent renaming of classes and members across a whole [`Program`]: every reference in
//! code, signatures, annotations, values, and debug info follows. Used by the α-invariance
//! tests (to produce equally valid alternative R8 outputs) and by the naming stage.

use std::collections::{BTreeMap, BTreeSet};

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
    /// Precise field renames: (declaring class, name, type) → new name, all in pre-rename
    /// terms. References are resolved to their declaring class the way the JVM does.
    pub fields: BTreeMap<(String, String, String), String>,
    /// Precise method renames: (declaring class, name, descriptor) → new name. The caller
    /// must give every method of an override group the same new name.
    pub methods: BTreeMap<(String, String, String), String>,
}

/// (superclass, interfaces, fields (name, type), methods (name, proto))
type ClassShape = (Option<String>, Vec<String>, BTreeSet<(String, String)>, BTreeSet<(String, String)>);

/// Pre-rename class hierarchy, for resolving member references to declarations.
struct Hierarchy {
    classes: BTreeMap<String, ClassShape>,
}

impl Hierarchy {
    fn of(p: &Program) -> Hierarchy {
        let s = &p.syms;
        let classes = p
            .classes
            .iter()
            .map(|c| {
                (
                    s.get(c.ty).to_string(),
                    (
                        c.superclass.map(|t| s.get(t).to_string()),
                        c.interfaces.iter().map(|t| s.get(*t).to_string()).collect(),
                        c.fields.iter().map(|f| (s.get(f.name).to_string(), s.get(f.ty).to_string())).collect(),
                        c.methods.iter().map(|m| (s.get(m.name).to_string(), s.get(m.proto).to_string())).collect(),
                    ),
                )
            })
            .collect();
        Hierarchy { classes }
    }

    /// JVM field resolution: the class, then its superinterfaces (recursively), then its
    /// superclass. Returns the declaring program class.
    fn field(&self, class: &str, name: &str, ty: &str, depth: u32) -> Option<String> {
        let (sup, ifaces, fields, _) = self.classes.get(class)?;
        if depth > 64 {
            return None;
        }
        if fields.contains(&(name.to_string(), ty.to_string())) {
            return Some(class.to_string());
        }
        for i in ifaces {
            if let Some(d) = self.field(i, name, ty, depth + 1) {
                return Some(d);
            }
        }
        sup.as_deref().and_then(|s| self.field(s, name, ty, depth + 1))
    }

    /// JVM method resolution: the class and its superclasses, then superinterfaces.
    fn method(&self, class: &str, name: &str, proto: &str) -> Option<String> {
        let key = (name.to_string(), proto.to_string());
        let mut c = Some(class.to_string());
        let mut depth = 0;
        while let Some(cur) = c {
            let Some((sup, _, _, methods)) = self.classes.get(&cur) else { break };
            if methods.contains(&key) {
                return Some(cur);
            }
            c = sup.clone();
            depth += 1;
            if depth > 64 {
                return None;
            }
        }
        // Interfaces, breadth-first from the class and its superclasses.
        let mut queue: Vec<String> = Vec::new();
        let mut c = Some(class.to_string());
        while let Some(cur) = c {
            let Some((sup, ifaces, _, _)) = self.classes.get(&cur) else { break };
            queue.extend(ifaces.iter().cloned());
            c = sup.clone();
            if queue.len() > 4096 {
                break;
            }
        }
        let mut seen = BTreeSet::new();
        while let Some(i) = queue.first().cloned() {
            queue.remove(0);
            if !seen.insert(i.clone()) {
                continue;
            }
            if let Some((_, ifaces, _, methods)) = self.classes.get(&i) {
                if methods.contains(&key) {
                    return Some(i);
                }
                queue.extend(ifaces.iter().cloned());
            }
        }
        None
    }
}

struct Ctx<'a> {
    r: &'a Renaming,
    /// Descriptors of program classes, before renaming.
    program: std::collections::BTreeSet<String>,
    hierarchy: Option<Hierarchy>,
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
        if let Some(ranges) = crate::types::signature_class_ranges(sig) {
            let mut out = String::with_capacity(sig.len());
            let mut last = 0;
            for r in ranges {
                out.push_str(&sig[last..r.start]);
                let key = format!("{};", &sig[r.clone()]);
                match self.r.classes.get(&key) {
                    Some(n) => out.push_str(n.trim_end_matches(';')),
                    None => out.push_str(&sig[r.clone()]),
                }
                last = r.end;
            }
            out.push_str(&sig[last..]);
            return out;
        }
        // Unparseable (malformed or non-standard): rename class types after delimiters.
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
    fn global_name(&mut self, owner_before: Sym, name: Sym) -> Option<Sym> {
        if !self.program.contains(self.syms.get(owner_before)) {
            return None;
        }
        let n = self.r.members.get(self.syms.get(name))?.clone();
        Some(self.syms.intern(&n))
    }
    /// New name for a field declared or referenced as (owner, name, type), pre-rename.
    fn field_name(&mut self, owner: Sym, name: Sym, ty: Sym) -> Sym {
        if let Some(n) = self.global_name(owner, name) {
            return n;
        }
        if self.r.fields.is_empty() {
            return name;
        }
        let (o, n, t) = (self.syms.get(owner).to_string(), self.syms.get(name).to_string(), self.syms.get(ty).to_string());
        let decl = self.hierarchy.as_ref().and_then(|h| h.field(&o, &n, &t, 0));
        match decl.and_then(|d| self.r.fields.get(&(d, n, t)).cloned()) {
            Some(new) => self.syms.intern(&new),
            None => name,
        }
    }
    /// New name for a method declared or referenced as (owner, name, proto), pre-rename.
    fn method_name(&mut self, owner: Sym, name: Sym, proto: Sym) -> Sym {
        if let Some(n) = self.global_name(owner, name) {
            return n;
        }
        if self.r.methods.is_empty() {
            return name;
        }
        let (o, n, pr) = (self.syms.get(owner).to_string(), self.syms.get(name).to_string(), self.syms.get(proto).to_string());
        let decl = self.hierarchy.as_ref().and_then(|h| h.method(&o, &n, &pr));
        match decl.and_then(|d| self.r.methods.get(&(d, n, pr)).cloned()) {
            Some(new) => self.syms.intern(&new),
            None => name,
        }
    }
    fn field(&mut self, f: FieldRef) -> FieldRef {
        FieldRef { name: self.field_name(f.class, f.name, f.ty), class: self.ty(f.class), ty: self.ty(f.ty) }
    }
    fn method(&mut self, m: MethodRef) -> MethodRef {
        MethodRef { name: self.method_name(m.class, m.name, m.proto), class: self.ty(m.class), proto: self.proto(m.proto) }
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
        let hierarchy = (!self.fields.is_empty() || !self.methods.is_empty()).then(|| Hierarchy::of(p));
        // Reflective name strings that R8 rewrote with its renames; keep them in sync.
        let sites = crate::reflect::sites(p);
        let mut string_rewrites: Vec<(usize, usize, u32, String)> = Vec::new();
        for x in &sites {
            let Some(insn) = x.string_insn else { continue };
            let new = match x.kind {
                crate::reflect::Kind::Class => {
                    let desc = format!("L{};", x.name.replace('.', "/"));
                    self.classes.get(&desc).map(|n| n.strip_prefix('L').and_then(|n| n.strip_suffix(';')).unwrap_or(n).replace('/', "."))
                }
                crate::reflect::Kind::UpdaterField | crate::reflect::Kind::DeclaredField => {
                    let Some(owner) = &x.owner else { continue };
                    let Some(ci) = p.classes.iter().position(|c| p.syms.get(c.ty) == owner) else { continue };
                    let mut matching = p.classes[ci].fields.iter().filter(|f| p.syms.get(f.name) == x.name);
                    let (Some(f), None) = (matching.next(), matching.next()) else { continue };
                    let key = (owner.clone(), x.name.clone(), p.syms.get(f.ty).to_string());
                    self.fields.get(&key).cloned().or_else(|| self.members.get(&x.name).cloned())
                }
                _ => None,
            };
            if let Some(new) = new.filter(|n| *n != x.name) {
                string_rewrites.push((x.class, x.method, insn, new));
            }
        }
        // A string shared by lookups that would need different names can't be rewritten
        // (callers pin such targets; this keeps an inconsistent renaming from half-applying).
        let mut wanted: BTreeMap<(usize, usize, u32), BTreeSet<String>> = BTreeMap::new();
        for (ci, mi, insn, new) in &string_rewrites {
            wanted.entry((*ci, *mi, *insn)).or_default().insert(new.clone());
        }
        string_rewrites.retain(|(ci, mi, insn, _)| wanted[&(*ci, *mi, *insn)].len() == 1);
        let mut syms = std::mem::take(&mut p.syms);
        for (ci, mi, insn, new) in &string_rewrites {
            let sym = syms.intern(new);
            if let Some(b) = p.classes[*ci].methods[*mi].code.as_mut() {
                if let Op::ConstString { value, .. } = &mut b.insns[*insn as usize].op {
                    *value = sym;
                }
            }
        }
        let mut cx = Ctx { r: self, program, hierarchy, syms: &mut syms };
        for c in &mut p.classes {
            let owner = cx.syms.get(c.ty).to_string();
            c.annotations = cx.annotations(&c.annotations, Some(&owner));
            for f in &mut c.fields {
                f.name = cx.field_name(c.ty, f.name, f.ty);
                f.ty = cx.ty(f.ty);
                f.static_value = f.static_value.as_ref().map(|v| cx.value(v));
                f.annotations = cx.annotations(&f.annotations, None);
            }
            for m in &mut c.methods {
                m.name = cx.method_name(c.ty, m.name, m.proto);
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
            ..Default::default()
        };
        let mut syms = Interner::default();
        let cx = Ctx { r: &r, program: Default::default(), hierarchy: None, syms: &mut syms };
        assert_eq!(cx.signature("Ljava/util/List<La/b;>;"), "Ljava/util/List<La/z;>;");
        assert_eq!(cx.signature("(La/b;[Lq;)La/b;"), "(La/z;[Lr;)La/z;");
        assert_eq!(cx.signature("<T:La/b;>Ljava/lang/Object;"), "<T:La/z;>Ljava/lang/Object;");
        assert_eq!(cx.signature("La/b<TT;>;"), "La/z<TT;>;");
        assert_eq!(cx.desc("[[La/b;"), "[[La/z;");
        assert_eq!(cx.desc("I"), "I");
    }
}
