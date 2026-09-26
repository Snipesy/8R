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
        let mut notes = Vec::new();
        for ((class, _, _), ks) in targets {
            let [k] = ks[..] else { continue };
            let f = &found[k];
            // Another method of the class already has the name and proto.
            let m = &model.classes[class].methods[f.method];
            if model.classes[class].methods.iter().enumerate().any(|(i, o)| i != f.method && s.get(o.name) == f.name && o.proto == m.proto) {
                continue;
            }
            let item = ItemId::Method { class: ClassId(class as u32), index: f.method as u32 };
            let rule = if f.corroborated { COMPOSE_LIB_KEY } else { COMPOSE_LIB_KEY_HINT };
            cx.labels.record_value(item, Attribute::MemberName, rule, None, Some(f.name.clone()))?;
            let owner = f.owner.strip_prefix('L').and_then(|o| o.strip_suffix(';')).unwrap_or(&f.owner).replace('/', ".");
            notes.push((class, f.method, format!("{owner}.{}{}", f.name, f.descriptor), format!("compose key {} ({})", f.key, f.artifact)));
        }
        let m = &mut cx.program.model;
        let (ty, name, via) = (m.syms.intern(ORIGINAL), m.syms.intern("name"), m.syms.intern("via"));
        for (class, method, original, how) in notes {
            let (o, h) = (m.syms.intern(&original), m.syms.intern(&how));
            let method = &mut m.classes[class].methods[method];
            method.annotations.retain(|a| a.annotation.ty != ty);
            method.annotations.push(Annotation {
                visibility: Visibility::Build,
                annotation: EncodedAnnotation { ty, elements: vec![(name, Value::String(o)), (via, Value::String(h))] },
            });
        }
        Ok(())
    }
}
