//! `compose/lib-key` (S) and `compose/lib-key-hint` (D): library composables named by their
//! durable group keys (`crate::compose_keys`). The method gets the function's original name (S
//! when a second key of the function corroborates the entry key, D otherwise) and
//! `@eightr.Original(name = "owner.name(descriptor)", via = "compose key K")`; its host class is
//! never renamed after the owner (R8 re-homes composables).

use eightr_ir::value::{Annotation, EncodedAnnotation, Value, Visibility};
use eightr_rules::{Attribute, Source, COMPOSE_LIB_KEY, COMPOSE_LIB_KEY_HINT};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub const ORIGINAL: &str = "Leightr/Original;";

pub struct ComposeLibKey;

impl Pass for ComposeLibKey {
    fn name(&self) -> &'static str {
        "compose-libkey"
    }

    fn source(&self) -> Source {
        Source::Compose
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let model = &cx.program.model;
        let Some(c) = crate::compose::find(model) else { return Ok(()) };
        let found = crate::compose_keys::identify(model, &c, crate::compose_keys::KeyDb::embedded());
        let s = &model.syms;
        // Only methods nothing overrides or is overridden by (static or direct): a virtual one
        // would need its whole override group renamed. Two identifications colliding on one
        // (class, name, proto) are both dropped.
        let mut targets: std::collections::BTreeMap<(usize, String, String), Vec<usize>> = std::collections::BTreeMap::new();
        for (k, f) in found.iter().enumerate() {
            let m = &model.classes[f.class].methods[f.method];
            let direct = m.access & (eightr_dex::class::access::STATIC | eightr_dex::class::access::PRIVATE) != 0;
            if direct {
                targets.entry((f.class, f.name.clone(), s.get(m.proto).to_string())).or_default().push(k);
            }
        }
        // A method with the name and proto declared up the supertype chain: a rename could make
        // an invoke resolve to this one (a private instance method) or hide it (a static).
        let declared_above = |ci: usize, name: &str, proto: eightr_ir::sym::Sym| {
            let mut stack: Vec<usize> = model.classes[ci].superclass.iter().chain(&model.classes[ci].interfaces).filter_map(|t| model.find(s.get(*t))).collect();
            let mut seen = std::collections::BTreeSet::new();
            while let Some(k) = stack.pop() {
                if !seen.insert(k) {
                    continue;
                }
                if model.classes[k].methods.iter().any(|o| s.get(o.name) == name && o.proto == proto) {
                    return true;
                }
                stack.extend(model.classes[k].superclass.iter().chain(&model.classes[k].interfaces).filter_map(|t| model.find(s.get(*t))));
            }
            false
        };
        let pins = eightr_ir::reflect::pins(model);
        let mut notes = Vec::new();
        for ((class, _, _), ks) in targets {
            let [k] = ks[..] else { continue };
            let f = &found[k];
            let m = &model.classes[class].methods[f.method];
            let item = ItemId::Method { class: ClassId(class as u32), index: f.method as u32 };
            let owner = f.owner.strip_prefix('L').and_then(|o| o.strip_suffix(';')).unwrap_or(&f.owner).replace('/', ".");
            let note = (class, f.method, format!("{owner}.{}{}", f.name, f.descriptor), format!("compose key {} ({})", f.key, f.artifact));
            let labelled = cx.labels.get(item, Attribute::MemberName).is_some();
            // Already called that (an earlier 8R run, or kept): provenance only; never over an
            // existing (S or recovered) name.
            let same = s.get(m.name) == f.name;
            if same {
                notes.push(note.clone());
                if labelled {
                    continue;
                }
            } else if labelled {
                continue;
            }
            // A name reflection pins in the class would read as pinned on the next run.
            if !same && pins.method(s.get(model.classes[class].ty), &f.name) {
                continue;
            }
            // Another method of the class, or above it, already has the name and proto.
            if model.classes[class].methods.iter().enumerate().any(|(i, o)| i != f.method && s.get(o.name) == f.name && o.proto == m.proto)
                || declared_above(class, &f.name, m.proto)
            {
                continue;
            }
            let rule = if f.corroborated { COMPOSE_LIB_KEY } else { COMPOSE_LIB_KEY_HINT };
            cx.labels.record_value(item, Attribute::MemberName, rule, None, Some(f.name.clone()))?;
            if !same {
                notes.push(note);
            }
        }
        let m = &mut cx.program.model;
        let (ty, name, via) = (m.syms.intern(ORIGINAL), m.syms.intern("name"), m.syms.intern("via"));
        for (class, method, original, how) in notes {
            let (o, h) = (m.syms.intern(&original), m.syms.intern(&how));
            let method = &mut m.classes[class].methods[method];
            // Provenance is the first pass's that named the method, an earlier run's included: a
            // re-run keeps it whichever passes match again (idempotence). One for another name is
            // stale and replaced.
            let syms = &m.syms;
            if method.annotations.iter().any(|a| a.annotation.ty == ty && a.annotation.elements.iter().any(|(k, v)| *k == name && matches!(v, Value::String(x) if method_name(syms.get(*x)) == method_name(&original)))) {
                continue;
            }
            method.annotations.retain(|a| a.annotation.ty != ty);
            method.annotations.push(Annotation {
                visibility: Visibility::Build,
                annotation: EncodedAnnotation { ty, elements: vec![(name, Value::String(o)), (via, Value::String(h))] },
            });
        }
        Ok(())
    }
}

/// The method name of an `@eightr.Original` name (`owner.name(descriptor)`): which method the
/// provenance is for. Passes may disagree on the owner of one method.
pub(super) fn method_name(original: &str) -> &str {
    let head = original.split('(').next().unwrap_or(original);
    head.rsplit('.').next().unwrap_or(head)
}
