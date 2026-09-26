//! `r8/sigdb-method-name` (D): library methods named from the signature DB
//! (`crate::sigdb::matcher`), with `@eightr.Original(name, via)`; classes the matched members vote
//! for get the library class's simple name as their structural-name hint.

use std::collections::BTreeMap;

use eightr_dex::class::access;
use eightr_ir::value::{Annotation, EncodedAnnotation, Value, Visibility};
use eightr_rules::{Attribute, Source, SIGDB_METHOD_NAME};

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
        let dbs = crate::sigdb::matcher::embedded();
        let matches = crate::sigdb::matcher::match_program(&cx.program.model, dbs);
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
        // (class, new name, proto) → matches; names that collide are dropped.
        let mut targets: BTreeMap<(usize, String, String), Vec<usize>> = BTreeMap::new();
        let mut notes = Vec::new();
        let mut same: Vec<(ItemId, String)> = Vec::new();
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
            let item = ItemId::Method { class: ClassId(m.class as u32), index: m.method as u32 };
            let labelled = cx.labels.get(item, Attribute::MemberName).is_some();
            // Already called that (kept by R8, or named by an earlier 8R run): provenance, and
            // the D name so structural naming leaves it (re-runs stay idempotent).
            if name == new {
                notes.push(note(m));
                if !labelled {
                    same.push((item, new));
                }
                continue;
            }
            if labelled {
                continue; // S or recovered already
            }

            targets.entry((m.class, new, proto.to_string())).or_default().push(k);
        }
        let mut labels = Vec::new();
        for ((class, new, proto), ks) in targets {
            let [k] = ks[..] else { continue };
            let m = &matches.methods[k];
            if p.classes[class].methods.iter().enumerate().any(|(i, o)| i != m.method && s.get(o.name) == new && s.get(o.proto) == proto) {
                continue;
            }
            labels.push((ItemId::Method { class: ClassId(class as u32), index: m.method as u32 }, new));
            notes.push(note(m));
        }
        for (item, name) in labels.into_iter().chain(same) {
            cx.labels.record_value(item, Attribute::MemberName, SIGDB_METHOD_NAME, None, Some(name))?;
        }
        // Class hints.
        for (&ci, &(li, dc)) in &matches.classes {
            let d = &dbs[li as usize].classes[dc as usize];
            let simple = d.trim_end_matches(';').rsplit('/').next().unwrap_or(d);
            let tail = simple.rsplit('$').find(|t| !t.is_empty() && !t.bytes().all(|b| b.is_ascii_digit())).unwrap_or(simple);
            cx.program.class_hints.insert(ClassId(ci as u32), tail.to_string());
        }
        let m = &mut cx.program.model;
        let ty = m.syms.intern(super::compose_libkey::ORIGINAL);
        let (name_k, via_k) = (m.syms.intern("name"), m.syms.intern("via"));
        for (class, method, original, via) in notes {
            let (o, v) = (m.syms.intern(&original), m.syms.intern(&via));
            let method = &mut m.classes[class].methods[method];
            method.annotations.retain(|a| a.annotation.ty != ty);
            method.annotations.push(Annotation {
                visibility: Visibility::Build,
                annotation: EncodedAnnotation { ty, elements: vec![(name_k, Value::String(o)), (via_k, Value::String(v))] },
            });
        }
        Ok(())
    }
}
