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

/// The packs forged for the app's own build profile (others are reported and ignored), as
/// matcher DBs, sorted by profile key.
pub fn select(packs: &[Pack], app: Option<&Profile>, findings: &mut Vec<Finding>) -> Vec<SigDb> {
    let mut out: Vec<(String, SigDb)> = Vec::new();
    let code = code_id();
    for p in packs {
        let theirs = p.tools.iter().find(|t| t.0 == "fingerprint").map(|t| t.1.as_str());
        if theirs != Some(code.as_str()) {
            findings.push(Finding {
                severity: Severity::Warning,
                message: format!("libdb: pack {} was forged by other fingerprint code ({}, this 8R: {code}); ignored, forge it again", p.profile.key(), theirs.unwrap_or("unknown")),
            });
            continue;
        }
        match app {
            Some(a) if a.canonical() == p.profile.canonical() => {
                findings.push(Finding {
                    severity: Severity::Info,
                    message: format!("libdb: pack {} ({} scenarios, {} records) matches the build profile", p.profile.key(), p.scenarios.len(), p.records.len()),
                });
                out.push((p.profile.key(), p.to_sigdb(false)));
            }
            _ => findings.push(Finding {
                severity: Severity::Warning,
                message: format!(
                    "libdb: pack {} is for another build profile than this app's ({}); ignored",
                    p.profile.key(),
                    app.map_or("none: no R8 marker".to_string(), |a| a.key())
                ),
            }),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out.into_iter().map(|x| x.1).collect()
}
