//! LibDB: library fingerprints forged from scenario builds of an app's exact build profile
//! (docs/research/libdb.md). `8r-forge` writes packs; 8R reads them.

pub mod pack;
pub mod profile;

pub use pack::Pack;

/// The identity of the code that decides a pack's fingerprints: 8R's lifting, rewrites and
/// fingerprint functions, by source text. A pack forged by other code is stale (its hashes
/// wouldn't agree with this 8R's), so 8R ignores it and the forge's cache key changes with it.
pub fn code_id() -> String {
    const SOURCES: &[&str] = &[
        include_str!("../sigdb/print.rs"),
        include_str!("../rewrites/mod.rs"),
        include_str!("../rewrites/outlines.rs"),
        include_str!("../rewrites/merged.rs"),
        include_str!("../rewrites/rebox.rs"),
        include_str!("../passes/enum_unboxing.rs"),
        include_str!("../../data/platform-api.txt"),
        include_str!("../../../eightr-dex/src/class.rs"),
        include_str!("../../../eightr-dex/src/code.rs"),
        include_str!("../../../eightr-dex/src/debug.rs"),
        include_str!("../../../eightr-dex/src/dex.rs"),
        include_str!("../../../eightr-dex/src/insn.rs"),
        include_str!("../../../eightr-dex/src/mutf8.rs"),
        include_str!("../../../eightr-dex/src/value.rs"),
        include_str!("../../../eightr-ir/src/sym.rs"),
        include_str!("../../../eightr-ir/src/cfg.rs"),
        include_str!("../../../eightr-ir/src/dataflow.rs"),
        include_str!("../../../eightr-ir/src/defs.rs"),
        include_str!("../../../eightr-ir/src/edit.rs"),
        include_str!("../../../eightr-ir/src/inline.rs"),
        include_str!("../../../eightr-ir/src/lift.rs"),
        include_str!("../../../eightr-ir/src/liveness.rs"),
        include_str!("../../../eightr-ir/src/model.rs"),
        include_str!("../../../eightr-ir/src/op.rs"),
        include_str!("../../../eightr-ir/src/reflect.rs"),
        include_str!("../../../eightr-ir/src/resolve.rs"),
        include_str!("../../../eightr-ir/src/types.rs"),
        include_str!("../../../eightr-ir/src/value.rs"),
    ];
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for s in SOURCES {
        h.update((s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    }
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}
pub use profile::{Coord, Profile};

use crate::report::{Finding, Severity};
use crate::sigdb::db::{ClassRecord as DbClass, Record as DbRecord, SigDb};

impl Pack {
    /// The pack as a signature DB for the matcher (`crate::sigdb::matcher`): records of classes
    /// the closure owns (`declared_only`: only the app's declared artifacts), scenarios as the
    /// version labels.
    pub fn to_sigdb(&self, declared_only: bool) -> SigDb {
        let keep = |class: u32| {
            let a = self.classes[class as usize].1;
            a != pack::NO_ARTIFACT && (!declared_only || self.artifacts.get(a as usize).is_some_and(|x| x.1))
        };
        SigDb {
            library: format!("libdb:{}", self.profile.key()),
            versions: self.scenarios.clone(),
            r8: self.profile.r8.clone(),
            classes: self.classes.iter().map(|c| c.0.clone()).collect(),
            methods: self.methods.clone(),
            records: self
                .records
                .iter()
                .filter(|r| keep(self.methods[r.method as usize].0))
                .map(|r| DbRecord { method: r.method, versions: r.scenarios, informative: r.informative, all: r.all, strings: r.strings, proto: r.proto, sketch: r.sketch, callees: r.callees.clone() })
                .collect(),
            class_records: self.class_records.iter().filter(|r| keep(r.class)).map(|r| DbClass { class: r.class, versions: r.scenarios, c2: r.c2, c3: r.c3 }).collect(),
        }
    }
}

/// The packs 8R uses for an app, as matcher DBs, and the classes they speak for (the classes of
/// the app's declared artifacts: the pack's versions of those are facts; undeclared ones, e.g. a
/// stdlib the resolver chose, are a guess and don't displace the embedded DBs).
#[derive(Debug, Clone, Default)]
pub struct LibDbs {
    pub dbs: Vec<SigDb>,
    pub covered: std::collections::BTreeSet<String>,
    /// Facts of the packs behind `dbs[i]` for `i < packs.len()` (the embedded DBs follow).
    pub packs: Vec<PackFacts>,
}

/// What 8R needs from a pack beyond its matcher DB.
#[derive(Debug, Clone, Default)]
pub struct PackFacts {
    /// (method, body hash) of records that may name S: informative, unique in the pack, not a
    /// re-keyed default-argument bridge, not in a synthetic-looking class, of a declared artifact.
    pub strict: std::collections::BTreeSet<(u32, u64)>,
    /// Per pack class: the owning artifact's coordinate (`group:artifact:version`), if any.
    pub coord: Vec<Option<String>>,
    /// (method, body hash) → the record's program field accesses: (token, pack field index).
    pub fields: std::collections::BTreeMap<(u32, u64), Vec<(u64, u32)>>,
    /// Pack fields: (class index, original name, original type).
    pub field_keys: Vec<(u32, String, String)>,
    /// Pack class → its class records' shapes (C2, C3).
    pub shapes: std::collections::BTreeMap<u32, Vec<(u64, u64)>>,
    /// Exact shape (C2) → the pack classes it was seen for.
    pub shape_classes: std::collections::BTreeMap<u64, std::collections::BTreeSet<u32>>,
    /// Pack class → its original superclasses seen in the builds (program classes only).
    pub parents: std::collections::BTreeMap<u32, std::collections::BTreeSet<u32>>,
    /// The app the pack was forged for (empty: a profile pack).
    pub app: String,
}

impl PackFacts {
    /// Whether `a` is `b` or one of its (transitive) superclasses, by the builds' hierarchy.
    pub fn is_ancestor(&self, a: u32, b: u32) -> bool {
        let mut stack = vec![b];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(k) = stack.pop() {
            if k == a {
                return true;
            }
            if seen.insert(k) {
                stack.extend(self.parents.get(&k).into_iter().flatten().copied());
            }
        }
        false
    }

    pub fn of(p: &Pack) -> PackFacts {
        let declared = |c: u32| p.artifacts.get(p.classes[c as usize].1 as usize).is_some_and(|a| a.1);
        let mut f = PackFacts {
            coord: p.classes.iter().map(|(_, a)| p.artifacts.get(*a as usize).map(|x| x.0.clone())).collect(),
            field_keys: p.fields.clone(),
            app: p.app.clone(),
            ..Default::default()
        };
        // A body that is mostly one other method inlined into it is as much that method's: the
        // name is ambiguous between them.
        let mostly_inlined = |r: &pack::Record| {
            let mut by_callee: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
            for &(a, b, st) in &r.frames {
                if let Some(&(outer, _)) = p.stacks.get(st as usize).and_then(|s| s.last()) {
                    if outer != r.method {
                        *by_callee.entry(outer).or_default() += b - a;
                    }
                }
            }
            by_callee.values().any(|&n| r.insns > 0 && n * 2 >= r.insns)
        };
        // Sibling methods of one class with the same erased proto and near-identical bodies (they
        // differ only in which program method or field they use, which fingerprints erase): a
        // body matching one could be the other's. Every variant counts, informative or not
        // (`selectCharsIn` and `selectUntransformedCharsIn`).
        let mut siblings: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        let mut groups: std::collections::BTreeMap<(u32, u64), Vec<&pack::Record>> = std::collections::BTreeMap::new();
        for r in &p.records {
            groups.entry((p.methods[r.method as usize].0, r.proto)).or_default().push(r);
        }
        for rs in groups.values() {
            for (i, a) in rs.iter().enumerate() {
                for b in &rs[i + 1..] {
                    if a.method != b.method && crate::sigdb::print::similarity(&a.sketch, &b.sketch) >= 0.75 {
                        siblings.insert(a.method);
                        siblings.insert(b.method);
                    }
                }
            }
        }
        // A thin wrapper (a body that says nothing beyond its calls) of a method of its class with
        // the same proto: R8 inlining the callee makes their bodies one (`selectCharsIn` over
        // `selectUntransformedCharsIn`).
        for r in p.records.iter().filter(|r| !r.informative) {
            let (class, _, proto) = &p.methods[r.method as usize];
            for &(_, k) in &r.callees {
                if k != pack::NO_METHOD && k != r.method {
                    if let Some((kc, _, kp)) = p.methods.get(k as usize) {
                        if kc == class && crate::sigdb::print::erase_proto(kp, &crate::sigdb::print::platform_stable) == crate::sigdb::print::erase_proto(proto, &crate::sigdb::print::platform_stable) {
                            siblings.insert(r.method);
                            siblings.insert(k);
                        }
                    }
                }
            }
        }
        // Code clones: methods that ever had the same body (in any build, any variant), such as
        // the copies of one class in several artifacts (`AnchoredDraggableState` in foundation,
        // material, material3) or two functions doing the same. A body matching one variant of
        // either may be either's: R8 output varies per build.
        let mut by_body: std::collections::BTreeMap<u64, std::collections::BTreeSet<u32>> = std::collections::BTreeMap::new();
        for r in p.records.iter().filter(|r| r.informative) {
            by_body.entry(r.all).or_default().insert(r.method);
        }
        let clones: std::collections::BTreeSet<u32> = by_body.into_values().filter(|m| m.len() > 1).flatten().collect();
        // A function with a default-argument bridge: R8 may keep either `f` or `f$default` for the
        // same specialized body, so the exact name isn't provable.
        let bridged: std::collections::BTreeSet<(u32, &str)> =
            p.methods.iter().filter_map(|(c, n, _)| n.strip_suffix("$default").map(|f| (*c, f))).collect();
        for r in &p.records {
            let class = p.methods[r.method as usize].0;
            let has_bridge = bridged.contains(&(class, p.methods[r.method as usize].1.as_str()));
            if r.informative && r.unique && !r.bridge && !r.synthetic_owner && !has_bridge && declared(class) && !mostly_inlined(r) && !siblings.contains(&r.method) && !clones.contains(&r.method) {
                f.strict.insert((r.method, r.all));
            }
            if !r.fields.is_empty() {
                f.fields.entry((r.method, r.all)).or_insert_with(|| r.fields.clone());
            }
        }
        for r in &p.class_records {
            f.shapes.entry(r.class).or_default().push((r.c2, r.c3));
            f.shape_classes.entry(r.c2).or_default().insert(r.class);
            if r.sup != pack::NO_CLASS {
                f.parents.entry(r.class).or_default().insert(r.sup);
            }
        }
        f
    }
}

/// The packs forged for the app's own build profile and this 8R's fingerprint code (others are
/// reported and ignored). The result doesn't depend on the order packs are given in: they're
/// ordered by (profile, catalog, tools, lock), and of packs for the same profile the first in that
/// order is used, with a warning.
/// The identity of an app's code: sha256 over its dex files' sha256s (hex, input order). A pack
/// forged for one app (`Pack::app`) is used for that app only.
pub fn app_id(dex_sha256s: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for s in dex_sha256s {
        h.update(s.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The apps whose packs named items of `model`: the `app` of its `@eightr.Library` tags. Input
/// carrying them is 8R's own output for those apps.
pub fn tagged_apps(model: &eightr_ir::model::Program) -> std::collections::BTreeSet<String> {
    let s = &model.syms;
    let mut out = std::collections::BTreeSet::new();
    for c in &model.classes {
        let anns = c.annotations.iter().chain(c.fields.iter().flat_map(|f| &f.annotations)).chain(c.methods.iter().flat_map(|m| &m.annotations));
        for a in anns.filter(|a| s.get(a.annotation.ty) == crate::passes::libdb_names::LIBRARY) {
            for (k, v) in &a.annotation.elements {
                if let (true, eightr_ir::value::Value::String(app)) = (s.get(*k) == "app", v) {
                    if !s.get(*app).is_empty() {
                        out.insert(s.get(*app).to_string());
                    }
                }
            }
        }
    }
    out
}

/// `tagged`: [`tagged_apps`] of the input. A pack forged for one app also serves 8R's output for
/// it (re-runs see the evidence the first run had: idempotence).
pub fn select(packs: &[Pack], app: Option<&Profile>, app_code: &str, tagged: &std::collections::BTreeSet<String>, trust: bool, findings: &mut Vec<Finding>) -> LibDbs {
    let code = code_id();
    let mut notes: Vec<Finding> = Vec::new();
    let mut ok: Vec<&Pack> = Vec::new();
    for p in packs {
        let theirs = p.tools.iter().find(|t| t.0 == "fingerprint").map(|t| t.1.as_str());
        if theirs != Some(code.as_str()) {
            notes.push(Finding {
                severity: Severity::Warning,
                message: format!("libdb: pack {} was forged by other fingerprint code ({}, this 8R: {code}); ignored, forge it again", p.profile.key(), theirs.unwrap_or("unknown")),
            });
            continue;
        }
        if trust {
            ok.push(p);
            continue;
        }
        if !p.app.is_empty() && p.app != app_code {
            if tagged.contains(&p.app) {
                ok.push(p);
            } else {
                notes.push(Finding { severity: Severity::Warning, message: format!("libdb: pack {} was forged for another app; ignored", p.profile.key()) });
            }
            continue;
        }
        match app {
            Some(a) if a.canonical() == p.profile.canonical() => ok.push(p),
            _ => notes.push(Finding {
                severity: Severity::Warning,
                message: format!(
                    "libdb: pack {} is for another build profile than this app's ({}); ignored",
                    p.profile.key(),
                    app.map_or("none: no R8 marker".to_string(), |a| a.key())
                ),
            }),
        }
    }
    // An app's own pack before a profile pack.
    let order = |p: &Pack| (p.profile.key(), p.app.is_empty(), p.catalog.clone(), p.tools.clone(), p.lock.clone(), p.records.len());
    ok.sort_by_key(|p| order(p));
    let mut out = LibDbs::default();
    let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in ok {
        if !used.insert(p.profile.key()) {
            notes.push(Finding { severity: Severity::Warning, message: format!("libdb: another pack for profile {} was given; only one is used", p.profile.key()) });
            continue;
        }
        notes.push(Finding {
            severity: Severity::Info,
            message: format!("libdb: pack {} ({} scenarios, {} records) matches the build profile", p.profile.key(), p.scenarios.len(), p.records.len()),
        });
        for (c, a) in &p.classes {
            if p.artifacts.get(*a as usize).is_some_and(|x| x.1) {
                out.covered.insert(c.clone());
            }
        }
        out.dbs.push(p.to_sigdb(false));
        out.packs.push(PackFacts::of(p));
    }
    notes.sort_by(|a, b| a.message.cmp(&b.message));
    findings.extend(notes);
    out
}
