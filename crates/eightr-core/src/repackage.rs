//! Package restoration (`r8/libdb-class-name`'s Package): move library classes back to their
//! original packages (R8 repackages them, usually into one flat package) wherever access allows.
//!
//! Moving a class across packages can change what verifies or how calls dispatch. Every move is
//! checked over the whole program after all moves, and the moves involved in a violation are
//! cancelled, to a fixpoint:
//! * a reference to a non-public program class (in code, a catch, a supertype) from another
//!   package;
//! * an access to a package-private program member from another package, or to a protected one
//!   from another package outside its subclasses;
//! * a method with the name and proto of a package-private method of a program superclass: whether
//!   it overrides it depends on the two sharing a package, so that must stay as it is;
//! * method handles and call sites (`const-method-handle`, `invoke-custom`, …): every program
//!   class they mention stays in the caller's package.
//!
//! Collisions (the new descriptor is another class's) refuse the move. Deterministic: classes,
//! references and cancellations are iterated in order.

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::class::access;
use eightr_ir::op::Op;
use eightr_ir::value::{HandleMember, MethodHandleRef, Value};

use crate::program::{package_of, simple_name_of, ClassId, Program};

const PUBLIC: u32 = access::PUBLIC;
const PRIVATE: u32 = access::PRIVATE;
const PROTECTED: u32 = access::PROTECTED;

/// Where a member reference resolves in the program: (declaring class, access flags). Fields:
/// the class, its interfaces, then its superclass; methods: the superclass chain, then interfaces.
fn resolve(p: &Program, class: &str, name: &str, desc: &str, method: bool) -> Option<(ClassId, u32)> {
    let declared = |id: ClassId| -> Option<u32> {
        let c = p.class(id);
        if method {
            c.methods.iter().find(|m| p.str(m.name) == name && p.str(m.proto) == desc).map(|m| m.access)
        } else {
            c.fields.iter().find(|f| p.str(f.name) == name && p.str(f.ty) == desc).map(|f| f.access)
        }
    };
    let start = p.find(class)?;
    let mut seen = BTreeSet::new();
    if method {
        let mut chain = vec![start];
        let mut cur = p.class(start).superclass.and_then(|t| p.find(p.str(t)));
        while let Some(k) = cur.filter(|k| !chain.contains(k) && chain.len() < 256) {
            chain.push(k);
            cur = p.class(k).superclass.and_then(|t| p.find(p.str(t)));
        }
        if let Some(r) = chain.iter().find_map(|&k| declared(k).map(|a| (k, a))) {
            return Some(r);
        }
        let mut queue: Vec<ClassId> = chain.iter().flat_map(|&k| p.class(k).interfaces.iter().filter_map(|t| p.find(p.str(*t)))).collect();
        while let Some(k) = queue.pop() {
            if !seen.insert(k) || seen.len() > 256 {
                continue;
            }
            if let Some(a) = declared(k) {
                return Some((k, a));
            }
            queue.extend(p.class(k).interfaces.iter().filter_map(|t| p.find(p.str(*t))));
        }
        return None;
    }
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > 256 {
            continue;
        }
        if let Some(a) = declared(id) {
            return Some((id, a));
        }
        // Superclass searched last: push it first.
        let c = p.class(id);
        stack.extend(c.superclass.iter().chain(c.interfaces.iter().rev()).filter_map(|t| p.find(p.str(*t))));
    }
    None
}

/// Program classes a type descriptor or method descriptor mentions.
fn classes_in(p: &Program, desc: &str, out: &mut Vec<ClassId>) {
    let mut push = |t: &str| {
        if let Some(k) = p.find(t.trim_start_matches('[')) {
            out.push(k);
        }
    };
    match eightr_ir::types::parse_proto(desc) {
        Some((ps, r)) => {
            ps.iter().for_each(|t| push(t));
            push(r);
        }
        None => push(desc),
    }
}

fn handle_classes(p: &Program, h: &MethodHandleRef, out: &mut Vec<ClassId>) {
    match &h.member {
        HandleMember::Field(f) => {
            classes_in(p, p.str(f.class), out);
            classes_in(p, p.str(f.ty), out);
        }
        HandleMember::Method(m) => {
            classes_in(p, p.str(m.class), out);
            classes_in(p, p.str(m.proto), out);
        }
    }
}

fn value_classes(p: &Program, v: &Value, out: &mut Vec<ClassId>) {
    match v {
        Value::Type(t) | Value::MethodType(t) => classes_in(p, p.str(*t), out),
        Value::MethodHandle(h) => handle_classes(p, h, out),
        Value::Field(f) | Value::Enum(f) => classes_in(p, p.str(f.class), out),
        Value::Method(m) => classes_in(p, p.str(m.class), out),
        Value::Array(vs) => vs.iter().for_each(|x| value_classes(p, x, out)),
        _ => {}
    }
}

/// Program superclasses of a class (transitively).
fn supers(p: &Program, id: ClassId) -> Vec<ClassId> {
    let mut out = Vec::new();
    let mut cur = p.class(id).superclass.and_then(|t| p.find(p.str(t)));
    while let Some(k) = cur {
        if out.contains(&k) || out.len() > 256 {
            break;
        }
        out.push(k);
        cur = p.class(k).superclass.and_then(|t| p.find(p.str(t)));
    }
    out
}

/// Pairs of classes.
type Pairs = BTreeSet<(ClassId, ClassId)>;

/// Pairs of classes that must share a package for the program to keep verifying and dispatching
/// as it does: (class, other); and pairs whose sharing a package must not change (overrides of
/// package-private methods).
fn constraints(p: &Program) -> (Pairs, Pairs) {
    let mut out = BTreeSet::new();
    let mut overrides = BTreeSet::new();
    let non_public_class = |d: &str| p.find(d.trim_start_matches('[')).filter(|&k| p.class(k).access & PUBLIC == 0);
    for id in p.class_ids() {
        let c = p.class(id);
        let mut need = |k: ClassId| {
            if k != id {
                out.insert((id, k));
            }
        };
        for t in c.superclass.iter().chain(&c.interfaces) {
            if let Some(k) = non_public_class(p.str(*t)) {
                need(k);
            }
        }
        // Overrides of package-private methods.
        let sup = supers(p, id);
        for m in &c.methods {
            let n = p.str(m.name);
            if m.access & (PRIVATE | access::STATIC) != 0 || n.starts_with('<') {
                continue;
            }
            for &k in &sup {
                if p.class(k).methods.iter().any(|x| p.str(x.name) == n && x.proto == m.proto && x.access & (PUBLIC | PROTECTED | PRIVATE | access::STATIC) == 0) {
                    overrides.insert((id, k));
                }
            }
        }
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            for tr in &b.tries {
                for h in &tr.handlers {
                    if let Some(k) = h.ty.and_then(|t| non_public_class(p.str(t))) {
                        need(k);
                    }
                }
            }
            for x in &b.insns {
                let (owner, member): (Option<&str>, Option<(&str, &str, bool)>) = match &x.op {
                    Op::NewInstance { ty, .. } | Op::ConstClass { ty, .. } | Op::CheckCast { ty, .. } | Op::InstanceOf { ty, .. } | Op::NewArray { ty, .. } | Op::FilledNewArray { ty, .. } => {
                        (Some(p.str(*ty)), None)
                    }
                    Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. } | Op::StaticPut { field, .. } => {
                        (Some(p.str(field.class)), Some((p.str(field.name), p.str(field.ty), false)))
                    }
                    Op::Invoke { method, .. } => (Some(p.str(method.class)), Some((p.str(method.name), p.str(method.proto), true))),
                    _ => (None, None),
                };
                if let Some(k) = owner.and_then(non_public_class) {
                    need(k);
                }
                let mut mentioned = Vec::new();
                match &x.op {
                    Op::ConstMethodHandle { handle, .. } => handle_classes(p, handle, &mut mentioned),
                    Op::ConstMethodType { proto, .. } => classes_in(p, p.str(*proto), &mut mentioned),
                    Op::InvokePolymorphic { method, proto, .. } => {
                        classes_in(p, p.str(method.class), &mut mentioned);
                        classes_in(p, p.str(*proto), &mut mentioned);
                    }
                    Op::InvokeCustom { call_site, .. } => {
                        handle_classes(p, &call_site.bootstrap, &mut mentioned);
                        classes_in(p, p.str(call_site.proto), &mut mentioned);
                        call_site.extra.iter().for_each(|v| value_classes(p, v, &mut mentioned));
                    }
                    _ => {}
                }
                mentioned.into_iter().for_each(&mut need);
                if let (Some(o), Some((n, d, is_method))) = (owner, member) {
                    if let Some((decl, flags)) = resolve(p, o.trim_start_matches('['), n, d, is_method) {
                        let package_private = flags & (PUBLIC | PROTECTED | PRIVATE) == 0;
                        let protected_outside = flags & PROTECTED != 0 && !sup.contains(&decl) && decl != id;
                        if package_private || protected_outside {
                            need(decl);
                        }
                    }
                }
            }
        }
    }
    (out, overrides)
}

/// Restores packages: `wanted` maps classes to their original package. Updates `classes` (the
/// descriptor renaming) and returns (restored, refused) counts.
pub fn restore(p: &Program, wanted: &BTreeMap<ClassId, String>, classes: &mut BTreeMap<String, String>) -> (usize, usize) {
    let final_desc = |id: ClassId, classes: &BTreeMap<String, String>| -> String {
        let d = p.descriptor(id);
        classes.get(d).cloned().unwrap_or_else(|| d.to_string())
    };
    let mut moves: BTreeMap<ClassId, String> = BTreeMap::new();
    for (&id, pkg) in wanted {
        let current = final_desc(id, classes);
        if package_of(&current) == pkg {
            continue;
        }
        let simple = simple_name_of(&current);
        let new = if pkg.is_empty() { format!("L{simple};") } else { format!("L{pkg}/{simple};") };
        moves.insert(id, new);
    }
    let requested = moves.len();
    // Collisions: two moves to one descriptor, or onto another class's final descriptor.
    let mut finals: BTreeMap<String, usize> = BTreeMap::new();
    for id in p.class_ids() {
        *finals.entry(moves.get(&id).cloned().unwrap_or_else(|| final_desc(id, classes))).or_default() += 1;
    }
    moves.retain(|_, new| finals[new.as_str()] == 1 && p.find(new).is_none());
    let (pairs, overrides) = constraints(p);
    let before = |id: ClassId| package_of(&final_desc(id, classes)).to_string();
    loop {
        let pkg = |id: ClassId, moves: &BTreeMap<ClassId, String>| -> String {
            package_of(&moves.get(&id).cloned().unwrap_or_else(|| final_desc(id, classes))).to_string()
        };
        let mut cancel: BTreeSet<ClassId> = BTreeSet::new();
        for &(a, b) in &pairs {
            if (moves.contains_key(&a) || moves.contains_key(&b)) && pkg(a, &moves) != pkg(b, &moves) {
                cancel.extend([a, b].into_iter().filter(|x| moves.contains_key(x)));
            }
        }
        for &(a, b) in &overrides {
            if (moves.contains_key(&a) || moves.contains_key(&b)) && (pkg(a, &moves) == pkg(b, &moves)) != (before(a) == before(b)) {
                cancel.extend([a, b].into_iter().filter(|x| moves.contains_key(x)));
            }
        }
        if cancel.is_empty() {
            break;
        }
        for id in cancel {
            moves.remove(&id);
        }
    }
    let restored = moves.len();
    for (id, new) in moves {
        classes.insert(p.descriptor(id).to_string(), new);
    }
    (restored, requested - restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eightr_ir::lift::{Body, Insn};
    use eightr_ir::model::{Class, Method, Program as Model};

    /// A class: `(descriptor, access, superclass, [(method name, access, ops)])`.
    type Spec<'a> = (&'a str, u32, &'a str, Vec<(&'a str, u32, Vec<Op>)>);

    fn program(classes: &[Spec]) -> Program {
        let mut m = Model::default();
        let mut out = Vec::new();
        for (d, acc, sup, methods) in classes {
            let ty = m.syms.intern(d);
            let superclass = Some(m.syms.intern(sup));
            let methods = methods
                .iter()
                .map(|(n, a, ops)| Method {
                    name: m.syms.intern(n),
                    proto: m.syms.intern("()V"),
                    access: *a,
                    code: Some(Body {
                        registers: 1,
                        ins: 1,
                        outs: 0,
                        insns: ops.iter().cloned().chain([Op::ReturnVoid]).enumerate().map(|(i, op)| Insn { pc: i as u32, op }).collect(),
                        tries: vec![],
                        positions: vec![],
                        locals: vec![],
                        parameter_names: vec![],
                    }),
                    annotations: vec![],
                    parameter_annotations: None,
                })
                .collect();
            out.push(Class { ty, access: *acc, superclass, interfaces: vec![], source_file: None, annotations: vec![], fields: vec![], methods, origin: 0 });
        }
        out.sort_by(|a, b| m.syms.get(a.ty).cmp(m.syms.get(b.ty)));
        m.classes = out;
        Program { model: m, class_hints: Default::default() }
    }

    fn moved(p: &Program, wanted: &[(&str, &str)]) -> BTreeMap<String, String> {
        let w = wanted.iter().map(|(d, pkg)| (p.find(d).unwrap(), pkg.to_string())).collect();
        let mut classes = BTreeMap::new();
        restore(p, &w, &mut classes);
        classes
    }

    const OBJ: &str = "Ljava/lang/Object;";

    /// `B.a()` doesn't override package-private `A.a()` across packages; moving B next to A would
    /// make it, so B stays. An unrelated name moves.
    #[test]
    fn overriding_of_package_private_methods_is_kept() {
        let a = ("Lx/A;", PUBLIC, OBJ, vec![("a", 0, vec![])]);
        let p = program(&[a.clone(), ("Lo/B;", PUBLIC, "Lx/A;", vec![("a", PUBLIC, vec![])])]);
        assert!(moved(&p, &[("Lo/B;", "x")]).is_empty());
        let p = program(&[a, ("Lo/B;", PUBLIC, "Lx/A;", vec![("b", PUBLIC, vec![])])]);
        assert_eq!(moved(&p, &[("Lo/B;", "x")]).get("Lo/B;").map(String::as_str), Some("Lx/B;"));
    }

    /// A non-public class and its user move together or not at all; a taken name never.
    #[test]
    fn non_public_classes_move_with_their_users() {
        let mut p = program(&[("Lo/C;", 0, OBJ, vec![]), ("Lo/D;", PUBLIC, OBJ, vec![("f", PUBLIC | access::STATIC, vec![Op::Nop])])]);
        // D creates a C.
        let c = p.model.syms.intern("Lo/C;");
        let d = p.find("Lo/D;").unwrap();
        p.model.classes[d.0 as usize].methods[0].code.as_mut().unwrap().insns[0].op = Op::NewInstance { dst: 0, ty: c };
        assert!(moved(&p, &[("Lo/C;", "x")]).is_empty());
        assert!(moved(&p, &[("Lo/D;", "x")]).is_empty());
        assert_eq!(moved(&p, &[("Lo/C;", "x"), ("Lo/D;", "x")]).len(), 2);
        let p2 = program(&[("Lo/E;", PUBLIC, OBJ, vec![]), ("Lx/E;", PUBLIC, OBJ, vec![])]);
        assert!(moved(&p2, &[("Lo/E;", "x")]).is_empty());
    }
}
