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
