//! App-derived pins (docs/research/libdb.md §9): what the app's own build kept of its libraries,
//! read from its dex, as keep rules for every scenario.
//!
//! An app build keeps library classes and members for reasons the forge can't see: AGP's rules from
//! the manifest and layouts (a `ComposeView` in an XML layout keeps its constructors), the app's own
//! `-keep` rules, reflection. Those pins change how R8 compiles the library code around them (a
//! pinned class isn't merged, its kept members aren't inlined or specialized), so a scenario
//! without them emits different bodies. The pins are facts of the app's dex: a library class whose
//! exact descriptor is in the app kept its name; so did a member whose name (and descriptor, or a
//! name too long to be a minified one) is the library's.

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::model::Program as Model;

use crate::api::Api;
use crate::gen::java_name;
use crate::tools::Result;

/// The app side: its classes' members, and its identity (a hash of its dex files).
#[derive(Debug, Clone, Default)]
pub struct AppShape {
    /// Class descriptor → its members.
    pub classes: BTreeMap<String, crate::api::Members>,
    /// sha256 over the dex files' own sha256s, in input order (`app_id`).
    pub id: String,
}

pub use eightr_core::libdb::app_id;

pub fn app_shape(inputs: &[eightr_core::input::DexInput]) -> Result<AppShape> {
    let dexes: Vec<eightr_dex::Dex> = inputs.iter().map(|i| eightr_dex::Dex::parse(&i.bytes).map_err(|e| format!("{}: {e:?}", i.name))).collect::<Result<_>>()?;
    let refs: Vec<&eightr_dex::Dex> = dexes.iter().collect();
    let model = Model::load(&refs).map_err(|e| format!("{e:?}"))?;
    let s = &model.syms;
    let mut classes = BTreeMap::new();
    for c in &model.classes {
        let methods = c.methods.iter().map(|m| (s.get(m.name).to_string(), s.get(m.proto).to_string())).collect();
        let fields = c.fields.iter().map(|f| (s.get(f.name).to_string(), s.get(f.ty).to_string())).collect();
        classes.insert(s.get(c.ty).to_string(), (methods, fields));
    }
    let shas: Vec<String> = inputs.iter().map(|i| crate::tools::sha256_hex(&i.bytes)).collect();
    Ok(AppShape { classes, id: app_id(&shas) })
}

/// Shortest member name that can't be one R8 made up (it uses a…z, aa…zz, aaa…: a class would
/// need thousands of members for four letters).
const MIN_NAME: usize = 4;

/// A library member the app member `(name, desc)` is: the same descriptor, or the only library
/// member of that name and arity when the name can't be minified.
fn pinned<'a>(name: &str, desc: &str, library: &'a [(String, String)], method: bool) -> Option<&'a (String, String)> {
    // R8's own names are one or two letters in practice (three only past 700 members): those
    // could coincide with a library name.
    if name.len() >= 3 || name.starts_with('<') {
        if let Some(m) = library.iter().find(|m| m.0 == name && m.1 == desc) {
            return Some(m);
        }
    }
    if name.len() < MIN_NAME || name.starts_with('<') {
        return None;
    }
    // Types agree where they can: primitives exactly, references by kind (the app may have renamed
    // them).
    let kind = |t: &str| if t.starts_with('L') || t.starts_with('[') { "L".to_string() } else { t.to_string() };
    let shape = |d: &str| -> Option<Vec<String>> {
        if !method {
            return Some(vec![kind(d)]);
        }
        let (ps, r) = eightr_ir::types::parse_proto(d)?;
        Some(ps.iter().chain(std::iter::once(&r)).map(|t| kind(t)).collect())
    };
    let want = shape(desc);
    let same: Vec<&(String, String)> = library.iter().filter(|m| m.0 == name && shape(&m.1) == want).collect();
    match same[..] {
        [one] => Some(one),
        _ => None,
    }
}

fn member_spec(name: &str, desc: &str, method: bool) -> Option<String> {
    if !method {
        return Some(format!("{} {name};", java_name(desc)));
    }
    let (ps, r) = eightr_ir::types::parse_proto(desc)?;
    let ps: Vec<String> = ps.iter().map(|t| java_name(t)).collect();
    Some(if name == "<init>" { format!("<init>({});", ps.join(",")) } else { format!("{} {name}({});", java_name(r), ps.join(",")) })
}

/// Keep rules for the library classes and members the app kept (sorted, deterministic).
pub fn rules(api: &Api, app: &AppShape) -> String {
    let mut out = String::new();
    for (desc, (app_methods, app_fields)) in &app.classes {
        let Some((lib_methods, lib_fields)) = api.members.get(desc) else { continue };
        let mut specs: BTreeSet<String> = BTreeSet::new();
        let mut ctors: BTreeSet<String> = BTreeSet::new();
        for (n, d) in app_methods {
            // Names every build keeps anyway aren't evidence of a keep: constructors, and
            // overrides of Object's methods.
            let object_override = matches!((n.as_str(), d.as_str()), ("equals", "(Ljava/lang/Object;)Z") | ("hashCode", "()I") | ("toString", "()Ljava/lang/String;"));
            if n == "<clinit>" || object_override {
                continue;
            }
            if let Some(m) = pinned(n, d, lib_methods, true).and_then(|m| member_spec(&m.0, &m.1, true)) {
                if n == "<init>" {
                    ctors.insert(m);
                } else {
                    specs.insert(m);
                }
            }
        }
        for (n, t) in app_fields {
            if let Some(f) = pinned(n, t, lib_fields, false).and_then(|f| member_spec(&f.0, &f.1, false)) {
                specs.insert(f);
            }
        }
        // Constructors count only for a class the app kept members of (e.g. a view kept for its
        // layout: its constructors are what the layout inflates).
        if !specs.is_empty() {
            specs.extend(ctors);
        }
        let body: String = specs.iter().map(|x| format!("\n    {x}")).collect();
        out.push_str(&format!("-keep class {} {{{body}\n}}\n", java_name(desc)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_from_names_the_app_kept() {
        let mut api = Api::default();
        api.members.insert(
            "La/View;".into(),
            (
                vec![("<init>".into(), "(Landroid/content/Context;)V".into()), ("setContent".into(), "(La/Fn;)V".into()), ("a".into(), "()V".into()), ("run".into(), "()V".into()), ("run".into(), "(I)V".into())],
                vec![("state".into(), "La/State;".into())],
            ),
        );
        let mut app = AppShape::default();
        app.classes.insert(
            "La/View;".into(),
            (
                // Exact ctor; renamed parameter type but a long unique name; a minified `a` (even
                // with the library's descriptor) and a reference/primitive mismatch don't pin.
                vec![("<init>".into(), "(Landroid/content/Context;)V".into()), ("setContent".into(), "(Lzz;)V".into()), ("a".into(), "()V".into()), ("b".into(), "()V".into()), ("run".into(), "(Lq;)V".into())],
                vec![("state".into(), "Lx;".into())],
            ),
        );
        app.classes.insert("Lq;".into(), (vec![], vec![]));
        let r = rules(&api, &app);
        assert_eq!(r, "-keep class a.View {\n    <init>(android.content.Context);\n    a.State state;\n    void setContent(a.Fn);\n}\n");
    }
}
