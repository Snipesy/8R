//! `r8/sigdb-method-name` (D): library methods named from the signature DB
//! (`crate::sigdb::matcher`), with `@eightr.Original(name, via)`; classes the matched members vote
//! for get the library class's simple name as their structural-name hint.

use std::collections::BTreeMap;

use eightr_dex::class::access;
use eightr_ir::value::{Annotation, EncodedAnnotation, Value, Visibility};
use eightr_rules::{Attribute, Source, LIBDB_METHOD_NAME, SIGDB_METHOD_NAME};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub struct Sigdb;

impl Pass for Sigdb {
    fn name(&self) -> &'static str {
        "sigdb"
    }

    fn source(&self) -> Source {
        Source::R8
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let dbs = crate::sigdb::matcher::with_packs(&cx.evidence.libdb);
        let pins = eightr_ir::reflect::pins(&cx.program.model);
        let matches = crate::sigdb::matcher::match_program(&cx.program.model, &dbs);
        let p = &cx.program.model;
        let s = &p.syms;
        let subclassed: std::collections::BTreeSet<&str> = p.classes.iter().filter_map(|c| c.superclass).map(|t| s.get(t)).collect();
        // Declared by a program supertype (an override group member).
        let overrides = |ci: usize, name: &str, proto: &str| {
            let mut stack: Vec<usize> = p.classes[ci].superclass.iter().chain(&p.classes[ci].interfaces).filter_map(|t| p.find(s.get(*t))).collect();
            let mut seen = std::collections::BTreeSet::new();
            while let Some(k) = stack.pop() {
                if !seen.insert(k) {
                    continue;
                }
                if p.classes[k].methods.iter().any(|m| s.get(m.name) == name && s.get(m.proto) == proto) {
                    return true;
                }
                stack.extend(p.classes[k].superclass.iter().chain(&p.classes[k].interfaces).filter_map(|t| p.find(s.get(*t))));
            }
            false
        };
        // Every supertype outside the program is java/lang/Object.
        let only_object_outside = |ci: usize| {
            let mut stack = vec![ci];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(k) = stack.pop() {
                if !seen.insert(k) {
                    continue;
                }
                for t in p.classes[k].superclass.iter().chain(&p.classes[k].interfaces) {
                    match p.find(s.get(*t)) {
                        Some(sup) => stack.push(sup),
                        None if s.get(*t) == "Ljava/lang/Object;" => {}
                        None => return false,
                    }
                }
            }
            true
        };
        // LibDB S eligibility (r8/libdb-method-name): an exact match of a strict pack record (see
        // `libdb::PackFacts::strict`), with another such match of the same app class to the same
        // pack class.
        let facts = &cx.evidence.libdb.packs;
        let stable = |d: &str| crate::sigdb::print::platform_stable(d);
        let reflective = crate::sigdb::print::reflective_strings(p);
        let mut prints: BTreeMap<usize, crate::sigdb::print::MethodPrint> = BTreeMap::new();
        for (k, m) in matches.methods.iter().enumerate() {
            if (m.key.0 as usize) < facts.len() {
                if let Some(mp) = crate::sigdb::print::method_print(p, m.class, m.method, &stable, &reflective) {
                    prints.insert(k, mp);
                }
            }
        }
        let pack_class = |m: &crate::sigdb::matcher::Match| dbs[m.key.0 as usize].methods[m.key.1 as usize].0;
        let strict = |k: usize, m: &crate::sigdb::matcher::Match| {
            m.via == "exact:all" && prints.get(&k).is_some_and(|mp| facts[m.key.0 as usize].strict.contains(&(m.key.1, mp.all)))
        };
        let mut per_class: BTreeMap<(usize, u16, u32), Vec<usize>> = BTreeMap::new();
        for (k, m) in matches.methods.iter().enumerate() {
            if strict(k, m) {
                per_class.entry((m.class, m.key.0, pack_class(m))).or_default().push(k);
            }
        }
        let s_ok: std::collections::BTreeSet<usize> = per_class.values().filter(|v| v.len() >= 2).flatten().copied().collect();
        // (class, new name, proto) → matches; names that collide are dropped.
        let mut targets: BTreeMap<(usize, String, String), Vec<usize>> = BTreeMap::new();
        let mut notes = Vec::new();
        let mut same: Vec<(ItemId, String, usize)> = Vec::new();
        // Matches of methods already called that and labelled (kept by R8, or named by an earlier
        // 8R run): no label, but evidence for class and field names like any named match.
        let mut confirmed: Vec<usize> = Vec::new();
        let note = |m: &crate::sigdb::matcher::Match| {
            let db = &dbs[m.key.0 as usize];
            let (dc, dn, dp) = &db.methods[m.key.1 as usize];
            let owner = db.classes[*dc as usize].trim_start_matches('L').trim_end_matches(';').replace('/', ".");
            (m.class, m.method, format!("{owner}.{dn}{dp}"), format!("sigdb {} ({})", m.via, db.library))
        };
        for (k, m) in matches.methods.iter().enumerate() {
            let method = &p.classes[m.class].methods[m.method];
            let (name, proto) = (s.get(method.name), s.get(method.proto));
            if name == "<init>" || name == "<clinit>" {
                continue;
            }
            let db = &dbs[m.key.0 as usize];
            let new = db.methods[m.key.1 as usize].1.clone();
            if new.starts_with('<') {
                continue;
            }
            let direct = method.access & (access::STATIC | access::PRIVATE) != 0;
            // A virtual method may take a new name only if nothing can override it or be
            // overridden by it: not an interface's, no program subclass, no program supertype
            // declaring it, no library supertype but Object, and not an Object method's name.
            let object_name = matches!((new.as_str(), proto), ("equals", "(Ljava/lang/Object;)Z") | ("hashCode", "()I") | ("toString", "()Ljava/lang/String;") | ("finalize", "()V") | ("clone", "()Ljava/lang/Object;"));
            let lone_virtual = p.classes[m.class].access & access::INTERFACE == 0
                && !subclassed.contains(s.get(p.classes[m.class].ty))
                && !overrides(m.class, name, proto)
                && only_object_outside(m.class)
                && !object_name;
            if !(direct || lone_virtual) {
                continue;
            }
            let owner = s.get(p.classes[m.class].ty);
            if pins.method(owner, name) || crate::naming::is_platform_class(owner) {
                continue;
            }
            let item = ItemId::Method { class: ClassId(m.class as u32), index: m.method as u32 };
            let labelled = cx.labels.get(item, Attribute::MemberName).is_some();
            // Already called that (kept by R8, or named by an earlier 8R run): provenance, and
            // the D name so structural naming leaves it (re-runs stay idempotent).
            if name == new {
                notes.push(note(m));
                if labelled {
                    confirmed.push(k);
                } else {
                    same.push((item, new, k));
                }
                continue;
            }
            if labelled {
                continue; // S or recovered already
            }

            targets.entry((m.class, new, proto.to_string())).or_default().push(k);
        }
        // Renames that stand alone: the new name and proto nowhere in the class or up its
        // supertypes, under their *final* names (another rename here may give a supertype's
        // method the same name: two renames must not create an override). Conflicts are dropped
        // until none remain.
        // A name that occurs as a string constant in the class would read as a reflective lookup
        // of the member next time (pinned): not a D name to give.
        // (Code strings and static string values, as `reflect::pins` reads them.)
        let class_strings = |ci: usize| -> std::collections::BTreeSet<String> {
            let mut out: std::collections::BTreeSet<String> = p.classes[ci]
                .methods
                .iter()
                .flat_map(|m| m.code.iter().flat_map(|b| &b.insns))
                .filter_map(|x| match &x.op {
                    eightr_ir::op::Op::ConstString { value, .. } => Some(s.get(*value).to_string()),
                    _ => None,
                })
                .collect();
            fn values(v: &eightr_ir::value::Value, s: &eightr_ir::sym::Interner, out: &mut std::collections::BTreeSet<String>) {
                match v {
                    eightr_ir::value::Value::String(x) => {
                        out.insert(s.get(*x).to_string());
                    }
                    eightr_ir::value::Value::Array(a) => a.iter().for_each(|x| values(x, s, out)),
                    _ => {}
                }
            }
            for f in &p.classes[ci].fields {
                if let Some(v) = &f.static_value {
                    values(v, s, &mut out);
                }
            }
            out
        };
        // Method references by (class, name, proto): a static or private method renamed to a
        // name invoked on its class or a subclass would capture those calls (R8 may have made a
        // library method static with an inherited framework method's shape).
        let refs: std::collections::BTreeSet<(String, String, String)> = p
            .classes
            .iter()
            .flat_map(|c| &c.methods)
            .filter_map(|m| m.code.as_ref())
            .flat_map(|b| &b.insns)
            .filter_map(|x| match &x.op {
                eightr_ir::op::Op::Invoke { method, .. } => Some((s.get(method.class).to_string(), s.get(method.name).to_string(), s.get(method.proto).to_string())),
                _ => None,
            })
            .collect();
        // Direct program subclasses of each class (by superclass descriptor).
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); p.classes.len()];
        for (k, c) in p.classes.iter().enumerate() {
            if let Some(sup) = c.superclass.and_then(|t| p.find(s.get(t))) {
                if sup != k {
                    children[sup].push(k);
                }
            }
        }
        let subclasses_or_self = |ci: usize| -> Vec<String> {
            let mut seen = std::collections::BTreeSet::from([ci]);
            let mut stack = vec![ci];
            while let Some(k) = stack.pop() {
                for &c in &children[k] {
                    if seen.insert(c) {
                        stack.push(c);
                    }
                }
            }
            seen.into_iter().map(|k| s.get(p.classes[k].ty).to_string()).collect()
        };
        let mut accepted: BTreeMap<(usize, usize), (String, String, usize)> = BTreeMap::new();
        for ((class, new, proto), ks) in targets {
            if let [k] = ks[..] {
                // Nor a name reflection pins in the class (e.g. a lookup of "get" on an unknown
                // owner): the next run would take it for pinned.
                let called = subclasses_or_self(class).iter().any(|d| refs.contains(&(d.clone(), new.clone(), proto.clone())));
                if !called && !class_strings(class).contains(&new) && !pins.method(s.get(p.classes[class].ty), &new) {
                    accepted.insert((class, matches.methods[k].method), (new, proto, k));
                }
            }
        }
        loop {
            let final_name = |ci: usize, mi: usize| -> String { accepted.get(&(ci, mi)).map_or_else(|| s.get(p.classes[ci].methods[mi].name).to_string(), |x| x.0.clone()) };
            let supers = |ci: usize| -> Vec<usize> {
                let mut out = Vec::new();
                let mut stack: Vec<usize> = p.classes[ci].superclass.iter().chain(&p.classes[ci].interfaces).filter_map(|t| p.find(s.get(*t))).collect();
                while let Some(k) = stack.pop() {
                    if !out.contains(&k) {
                        out.push(k);
                        stack.extend(p.classes[k].superclass.iter().chain(&p.classes[k].interfaces).filter_map(|t| p.find(s.get(*t))));
                    }
                }
                out
            };
            let conflicts: Vec<(usize, usize)> = accepted
                .iter()
                .filter(|(&(ci, mi), (new, proto, _))| {
                    let same = |k: usize, j: usize| (k, j) != (ci, mi) && final_name(k, j) == *new && s.get(p.classes[k].methods[j].proto) == proto;
                    (0..p.classes[ci].methods.len()).any(|j| same(ci, j)) || supers(ci).into_iter().any(|k| (0..p.classes[k].methods.len()).any(|j| same(k, j)))
                })
                .map(|(&key, _)| key)
                .collect();
            if conflicts.is_empty() {
                break;
            }
            for key in conflicts {
                accepted.remove(&key);
            }
        }
        let mut labels = Vec::new();
        for ((class, method), (new, _, k)) in accepted {
            labels.push((ItemId::Method { class: ClassId(class as u32), index: method as u32 }, new, k));
            notes.push(note(&matches.methods[k]));
        }
        // Matches named (renamed or confirmed), by match index: S or D.
        let mut named: BTreeMap<usize, bool> = BTreeMap::new();
        for (item, name, k) in labels.into_iter().chain(same) {
            let s_tier = s_ok.contains(&k);
            cx.labels.record_value(item, Attribute::MemberName, if s_tier { LIBDB_METHOD_NAME } else { SIGDB_METHOD_NAME }, None, Some(name))?;
            named.insert(k, s_tier);
        }
        for k in confirmed {
            named.insert(k, s_ok.contains(&k));
        }
        let library = super::libdb_names::name_classes_and_fields(p, cx.evidence, cx.labels, &dbs, &matches, &prints, &named)?;
        // Class hints.
        for (&ci, &(li, dc)) in &matches.classes {
            let d = &dbs[li as usize].classes[dc as usize];
            let simple = d.trim_end_matches(';').rsplit('/').next().unwrap_or(d);
            let tail = simple.rsplit('$').find(|t| !t.is_empty() && !t.bytes().all(|b| b.is_ascii_digit())).unwrap_or(simple);
            cx.program.class_hints.entry(ClassId(ci as u32)).or_insert_with(|| tail.to_string());
        }
        // The library of every method named from a pack (and of the matched notes).
        let lib_of: BTreeMap<(usize, usize), (String, String)> = matches
            .methods
            .iter()
            .filter(|m| (m.key.0 as usize) < facts.len())
            .filter_map(|m| {
                let f = &facts[m.key.0 as usize];
                let coord = f.coord.get(pack_class(m) as usize).cloned().flatten()?;
                Some(((m.class, m.method), (coord, f.app.clone())))
            })
            .collect();
        let m = &mut cx.program.model;
        let ty = m.syms.intern(super::compose_libkey::ORIGINAL);
        let lib_ty = m.syms.intern(super::libdb_names::LIBRARY);
        let (name_k, via_k, value_k, app_k) = (m.syms.intern("name"), m.syms.intern("via"), m.syms.intern("value"), m.syms.intern("app"));
        let tag = |m: &mut eightr_ir::model::Program, coord: &str, app: &str| -> Annotation {
            let (c, a) = (m.syms.intern(coord), m.syms.intern(app));
            Annotation { visibility: Visibility::Build, annotation: EncodedAnnotation { ty: lib_ty, elements: vec![(app_k, Value::String(a)), (value_k, Value::String(c))] } }
        };
        for (class, method, original, via) in notes {
            let (o, v) = (m.syms.intern(&original), m.syms.intern(&via));
            let lib = lib_of.get(&(class, method)).map(|(c, a)| tag(m, c, a));
            let method = &mut m.classes[class].methods[method];
            // Provenance: the first pass's that named the method (an earlier run's included),
            // unless it is for another name (stale).
            let syms = &m.syms;
            let current = |a: &Annotation| {
                a.annotation.ty == ty && a.annotation.elements.iter().any(|(k, v)| *k == name_k && matches!(v, Value::String(x) if super::compose_libkey::method_name(syms.get(*x)) == super::compose_libkey::method_name(&original)))
            };
            if !method.annotations.iter().any(current) {
                method.annotations.retain(|a| a.annotation.ty != ty);
                method.annotations.push(Annotation {
                    visibility: Visibility::Build,
                    annotation: EncodedAnnotation { ty, elements: vec![(name_k, Value::String(o)), (via_k, Value::String(v))] },
                });
            }
            if let Some(lib) = lib {
                method.annotations.retain(|a| a.annotation.ty != lib_ty);
                method.annotations.push(lib);
            }
        }
        // Classes and fields named from a pack carry the tag too.
        for (item, coord, app) in library {
            let t = tag(m, &coord, &app);
            let anns = match item {
                ItemId::Class { class } => &mut m.classes[class.0 as usize].annotations,
                ItemId::Field { class, index } => &mut m.classes[class.0 as usize].fields[index as usize].annotations,
                ItemId::Method { class, index } => &mut m.classes[class.0 as usize].methods[index as usize].annotations,
            };
            anns.retain(|a| a.annotation.ty != lib_ty);
            anns.push(t);
        }
        Ok(())
    }
}
