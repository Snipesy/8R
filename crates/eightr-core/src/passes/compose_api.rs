//! `compose/runtime-api` (S): names of the Compose runtime's `Composer` members, from the roles
//! the compiler plugin's calls give them (`crate::compose::roles`): `startRestartGroup`,
//! `endRestartGroup`, `shouldExecute` / `getSkipping`, `skipToGroupEnd`, `rememberedValue`,
//! `updateRememberedValue`, `changed`, `changedInstance`, and the static `updateChangedFlags`.
//! The plugin emits exactly these calls in these shapes, so a member playing the role in (almost)
//! every composable is that member.

use eightr_rules::{Attribute, Source, COMPOSE_RUNTIME_API, COMPOSE_SINGLETONS};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub struct ComposeApi;

impl Pass for ComposeApi {
    fn name(&self) -> &'static str {
        "compose-api"
    }

    fn source(&self) -> Source {
        Source::Compose
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let p = &*cx.program;
        let Some(c) = crate::compose::find(&p.model) else { return Ok(()) };
        let roles = crate::compose::roles(&p.model, &c);
        let Some(ci) = p.find(&c.class) else { return Ok(()) };
        // The override group renames together: the composer, its program supertypes, and every
        // subtype of those (siblings implementing the same interface included).
        let roots: Vec<ClassId> = p.class_ids().filter(|&id| id == ci || related(p, ci, id)).collect();
        let related: Vec<ClassId> = p.class_ids().filter(|&id| roots.contains(&id) || roots.iter().any(|&r| related(p, id, r))).collect();
        let mut labels = Vec::new();
        for role in &roles.composer {
            for &id in &related {
                let cls = p.class(id);
                if let Some(index) = cls.methods.iter().position(|m| p.str(m.name) == role.method.0 && p.str(m.proto) == role.method.1) {
                    labels.push((ItemId::Method { class: id, index: index as u32 }, role.name));
                }
            }
        }
        if let Some((class, name, proto)) = &roles.update_changed_flags {
            if let Some(id) = p.find(class) {
                if let Some(index) = p.class(id).methods.iter().position(|m| p.str(m.name) == name && p.str(m.proto) == proto) {
                    labels.push((ItemId::Method { class: id, index: index as u32 }, "updateChangedFlags"));
                }
            }
        }
        for (item, name) in labels {
            cx.labels.record_value(item, Attribute::MemberName, COMPOSE_RUNTIME_API, None, Some(name.to_string()))?;
        }
        // ComposableSingletons fields: `lambda$K` from Kotlin 2.1.20, `lambda-N` before. No
        // program-wide fact proves the compiler of each module (the runtime's own singletons and
        // the app's may differ, as in Gretio), so the name is D: applied as a readable name, with
        // the field annotated with its key (`@eightr.ComposableSingleton(key = K)`).
        let singletons = crate::compose::singletons(&p.model, &c);
        for &(class, index, key) in &singletons {
            let item = ItemId::Field { class: ClassId(class as u32), index: index as u32 };
            cx.labels.record_value(item, Attribute::MemberName, COMPOSE_SINGLETONS, None, Some(format!("lambda${key}")))?;
        }
        let m = &mut cx.program.model;
        let ty = m.syms.intern(super::compose_params::COMPOSABLE_SINGLETON);
        let key_name = m.syms.intern("key");
        for (class, index, key) in singletons {
            let f = &mut m.classes[class].fields[index];
            f.annotations.retain(|a| a.annotation.ty != ty);
            f.annotations.push(eightr_ir::value::Annotation {
                visibility: eightr_ir::value::Visibility::Build,
                annotation: eightr_ir::value::EncodedAnnotation { ty, elements: vec![(key_name, eightr_ir::value::Value::Int(key))] },
            });
        }
        Ok(())
    }
}

/// Is `sup` a (transitive) program supertype of `sub`?
fn related(p: &crate::program::Program, sub: ClassId, sup: ClassId) -> bool {
    let mut stack = vec![sub];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let c = p.class(id);
        for t in c.superclass.iter().chain(&c.interfaces) {
            if let Some(tid) = p.find(p.str(*t)) {
                if tid == sup {
                    return true;
                }
                stack.push(tid);
            }
        }
    }
    false
}
