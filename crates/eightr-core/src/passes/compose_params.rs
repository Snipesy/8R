//! `compose/synthetic-params` (D; the `$composer` role S): restartable composables' synthetic
//! parameters named in debug info, and what the plugin's lowering proves about the original
//! signature recorded in a build annotation (`crate::composables`):
//!
//! ```text
//! @eightr.Composable(key = K, composer = 2, changed = {3}, defaults = {4},
//!                    bindings = {"p0: slot 1 of $changed", "p1: default #2"},
//!                    slotsAtLeast = 5, removedAtLeast = 1)
//! ```
//!
//! Parameter indices count the method's own parameters from 0 (the receiver excluded). The restart
//! lambda's method gets `@eightr.RestartScope(key = K)`. Nothing changes code.

use eightr_ir::value::{Annotation, EncodedAnnotation, Value, Visibility};
use eightr_rules::{Attribute, Source, COMPOSE_SYNTHETIC_PARAMS};

use super::{Context, Pass};
use crate::error::Result;
use crate::program::{ClassId, ItemId};

pub const COMPOSABLE: &str = "Leightr/Composable;";
pub const RESTART_SCOPE: &str = "Leightr/RestartScope;";

pub struct ComposeParams;

impl Pass for ComposeParams {
    fn name(&self) -> &'static str {
        "compose-params"
    }

    fn source(&self) -> Source {
        Source::Compose
    }

    fn run(&self, cx: &mut Context) -> Result<()> {
        let model = &cx.program.model;
        let Some(c) = crate::compose::find(model) else { return Ok(()) };
        let roles = crate::compose::roles(model, &c);
        let all = crate::composables::composables(model, &c, &roles);
        let m = &mut cx.program.model;
        let int = |v: i64| Value::Int(v as i32);
        for comp in &all {
            let mut names: Vec<(usize, String)> = Vec::new();
            if let Some(j) = comp.composer {
                names.push((j, "$composer".into()));
            }
            for (k, &j) in comp.changed.iter().enumerate() {
                names.push((j, if k == 0 { "$changed".into() } else { format!("$changed{k}") }));
            }
            for (k, &j) in comp.defaults.iter().enumerate() {
                names.push((j, if k == 0 { "$default".into() } else { format!("$default{k}") }));
            }
            let mut elements: Vec<(&str, Value)> = vec![("key", Value::Int(comp.key))];
            if let Some(j) = comp.composer {
                elements.push(("composer", int(j as i64)));
            }
            elements.push(("changed", Value::Array(comp.changed.iter().map(|&j| int(j as i64)).collect())));
            if !comp.defaults.is_empty() {
                elements.push(("defaults", Value::Array(comp.defaults.iter().map(|&j| int(j as i64)).collect())));
            }
            let changed_name = |q: usize| names.iter().find(|n| n.0 == q).map_or(String::new(), |n| n.1.clone());
            let mut bindings: Vec<String> = comp.slots.iter().map(|&(j, q, s)| format!("p{j}: slot {s} of {}", changed_name(q))).collect();
            bindings.extend(comp.default_bits.iter().map(|&(j, i)| format!("p{j}: default #{i}")));
            if !bindings.is_empty() {
                elements.push(("bindings", Value::Array(bindings.iter().map(|b| Value::String(m.syms.intern(b))).collect())));
            }
            if comp.slots_at_least > 0 {
                elements.push(("slotsAtLeast", int(comp.slots_at_least.into())));
            }
            if comp.removed_at_least > 0 {
                elements.push(("removedAtLeast", int(comp.removed_at_least.into())));
            }
            annotate(m, comp.class, comp.method, COMPOSABLE, elements);
            for &(ci, mi) in &comp.restart_methods {
                annotate(m, ci, mi, RESTART_SCOPE, vec![("key", Value::Int(comp.key))]);
            }

            // Debug-info names, where the input has none.
            let syms: Vec<(usize, eightr_ir::sym::Sym)> = names.iter().map(|(j, n)| (*j, m.syms.intern(n))).collect();
            let method = &mut m.classes[comp.class].methods[comp.method];
            let count = eightr_ir::types::parse_proto(m.syms.get(method.proto)).map_or(0, |(p, _)| p.len());
            let body = method.code.as_mut().unwrap();
            if body.parameter_names.len() < count {
                body.parameter_names.resize(count, None);
            }
            let mut named = false;
            for &(j, n) in &syms {
                if body.parameter_names[j].is_none() {
                    body.parameter_names[j] = Some(n);
                    named = true;
                }
            }
            if named {
                let item = ItemId::Method { class: ClassId(comp.class as u32), index: comp.method as u32 };
                let value = names.iter().map(|(j, n)| format!("p{j}={n}")).collect::<Vec<_>>().join(",");
                cx.labels.record_value(item, Attribute::ParamNames, COMPOSE_SYNTHETIC_PARAMS, None, Some(value))?;
            }
        }
        Ok(())
    }
}

fn annotate(m: &mut eightr_ir::model::Program, ci: usize, mi: usize, ty: &str, elements: Vec<(&str, Value)>) {
    let ty = m.syms.intern(ty);
    let elements = elements.into_iter().map(|(k, v)| (m.syms.intern(k), v)).collect();
    let method = &mut m.classes[ci].methods[mi];
    method.annotations.retain(|a| a.annotation.ty != ty);
    method.annotations.push(Annotation { visibility: Visibility::Build, annotation: EncodedAnnotation { ty, elements } });
}
