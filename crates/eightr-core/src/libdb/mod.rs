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
}

/// The packs forged for the app's own build profile and this 8R's fingerprint code (others are
/// reported and ignored). The result doesn't depend on the order packs are given in: they're
/// ordered by (profile, catalog, tools, lock), and of packs for the same profile the first in that
/// order is used, with a warning.
pub fn select(packs: &[Pack], app: Option<&Profile>, findings: &mut Vec<Finding>) -> LibDbs {
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
    let order = |p: &Pack| (p.profile.key(), p.catalog.clone(), p.tools.clone(), p.lock.clone(), p.records.len());
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
    }
    notes.sort_by(|a, b| a.message.cmp(&b.message));
    findings.extend(notes);
    out
}
