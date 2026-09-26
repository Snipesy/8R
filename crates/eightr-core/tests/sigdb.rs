//! The signature DB (crates/eightr-core/src/sigdb, `cargo xtask sigdb`): files round-trip, and
//! fingerprints don't depend on program names.

use std::fs;
use std::path::{Path, PathBuf};

use eightr_core::sigdb::db::SigDb;
use eightr_core::sigdb::print::{class_print, method_print, reflective_strings};
use eightr_dex::Dex;
use eightr_ir::model::Program as Model;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load(p: &Path) -> Model {
    let bytes = fs::read(p).unwrap();
    let dex = Dex::parse(&bytes).unwrap();
    Model::load(&[&dex]).unwrap()
}

#[test]
fn db_files_round_trip() {
    let mut n = 0;
    for e in fs::read_dir(root().join("sigdb")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "sigdb") {
            continue;
        }
        let bytes = fs::read(&p).unwrap();
        let db = SigDb::decode(&bytes).unwrap();
        assert_eq!(db.encode(), bytes, "{}", p.display());
        assert!(!db.versions.is_empty() && db.records.len() >= db.methods.len() / 2, "{}", p.display());
        assert!(db.records.iter().all(|r| r.versions != 0 && (r.method as usize) < db.methods.len()));
        n += 1;
    }
    assert!(n >= 5, "only {n} DB files");
}

/// Fingerprints (hashes, sketches, callee tokens, class shapes) are the same after renaming every
/// program class and member.
#[test]
fn fingerprints_ignore_program_names() {
    let path = root().join("fixtures/out/sigdb_app/r8/classes.dex");
    let a = load(&path);
    let mut b = load(&path);
    let mut r = eightr_ir::rename::Renaming::default();
    for (i, c) in a.classes.iter().enumerate() {
        r.classes.insert(a.syms.get(c.ty).to_string(), format!("Lzz/Renamed{i};"));
        for m in &c.methods {
            let n = a.syms.get(m.name);
            if !n.starts_with('<') {
                r.members.insert(n.to_string(), format!("m{}", n.len() * 7919 % 1000 + n.bytes().map(u64::from).sum::<u64>() as usize));
            }
        }
        for f in &c.fields {
            let n = a.syms.get(f.name);
            r.members.insert(n.to_string(), format!("f_{n}"));
        }
    }
    r.apply(&mut b);
    let prints = |m: &Model| {
        let stable = |d: &str| eightr_core::sigdb::print::platform_stable(d);
        let reflective = reflective_strings(m);
        let mut v = Vec::new();
        for ci in 0..m.classes.len() {
            let cp = class_print(m, ci, &stable);
            v.push((cp.c2, cp.c3, 0u64, [0u32; 16], Vec::new()));
            for mi in 0..m.classes[ci].methods.len() {
                if let Some(p) = method_print(m, ci, mi, &stable, &reflective) {
                    v.push((p.all, p.strings, p.proto, p.sketch, p.callees.iter().map(|c| c.0).collect()));
                }
            }
        }
        // The renamer may reorder classes and methods.
        v.sort();
        v
    };
    let (pa, pb) = (prints(&a), prints(&b));
    assert!(pa.len() > 1000);
    let only_a: Vec<_> = pa.iter().filter(|x| !pb.contains(x)).collect();
    assert!(only_a.is_empty(), "{} fingerprints differ after renaming, e.g. {:?}", only_a.len(), &only_a[..only_a.len().min(2)]);
    assert_eq!(pa, pb);
}

/// The matcher (crates/eightr-core/src/sigdb/matcher.rs) on sigdb_app, graded by its mapping:
/// precision per stage, recall over the library methods the DB knows. Names are D, so the bar is
/// a ratchet, not 100%.
#[test]
fn matcher_precision_recall_on_sigdb_app() {
    use eightr_mapping::{Mapping, MemberKind, Metadata};
    let dir = root().join("fixtures/out/sigdb_app/r8");
    let mut model = load(&dir.join("classes.dex"));
    let program: std::collections::BTreeSet<String> = model.classes.iter().map(|c| model.syms.get(c.ty).to_string()).collect();
    eightr_core::rewrites::run_all(&mut model).unwrap();
    let mapping = Mapping::parse_normalized(&fs::read_to_string(dir.join("mapping.txt")).unwrap()).unwrap();
    let dbs = eightr_core::sigdb::matcher::embedded();
    let matches = eightr_core::sigdb::matcher::match_program(&model, dbs);
    let desc = |dotted: &str| format!("L{};", dotted.replace('.', "/"));
    // (residual class, residual name) → original (owner, name), when unambiguous.
    let truth = |class: &str, name: &str| -> Option<(String, String)> {
        let cm = mapping.classes.iter().find(|c| desc(&c.obfuscated) == class)?;
        let cands: std::collections::BTreeSet<(String, String)> = cm
            .outermost_methods()
            .into_iter()
            .filter(|(m, md)| m.obfuscated == name && !md.iter().any(|x| x.parsed == Metadata::Synthesized))
            .map(|(m, _)| (desc(m.original_owner.as_deref().unwrap_or(&cm.original)), m.original_name.clone()))
            .collect();
        let _ = MemberKind::Field;
        (cands.len() == 1).then(|| cands.into_iter().next().unwrap())
    };
    let db_classes: std::collections::BTreeSet<&str> = dbs.iter().flat_map(|d| d.classes.iter().map(String::as_str)).collect();
    let mut per: std::collections::BTreeMap<&str, (usize, usize)> = std::collections::BTreeMap::new();
    let mut matched_truth = 0;
    for m in &matches.methods {
        let c = &model.classes[m.class];
        let (cd, mn) = (model.syms.get(c.ty), model.syms.get(c.methods[m.method].name));
        let db = &dbs[m.key.0 as usize];
        let (kc, kn, _) = &db.methods[m.key.1 as usize];
        let got = (db.classes[*kc as usize].clone(), kn.clone());
        let Some(t) = truth(cd, mn) else { continue };
        let e = per.entry(m.via).or_default();
        e.1 += 1;
        if t == got {
            e.0 += 1;
            matched_truth += 1;
        }
    }
    // Recall: library methods of the app (original owner known to the DB), in original classes.
    let mut total = 0;
    for c in &model.classes {
        let cd = model.syms.get(c.ty);
        if !program.contains(cd) {
            continue;
        }
        for m in c.methods.iter().filter(|m| m.code.is_some()) {
            let n = model.syms.get(m.name);
            if n == "<init>" || n == "<clinit>" {
                continue;
            }
            if truth(cd, n).is_some_and(|(o, _)| db_classes.contains(o.as_str())) {
                total += 1;
            }
        }
    }
    let (ok, all): (usize, usize) = per.values().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    eprintln!("sigdb matcher: {per:?}; precision {ok}/{all}, recall {matched_truth}/{total}; {} class pairs", matches.classes.len());
    assert!(all > 0 && ok * 100 >= all * 92, "precision {ok}/{all}");
    assert!(matched_truth * 100 >= total * 68, "recall {matched_truth}/{total}");
}
