//! `cargo xtask sigdb`: builds the embedded fallback DBs `sigdb/<library>.sigdb` from the library
//! versions in `fixtures/sigdb.conf`. Each version is forged by `8r-forge` (docs/research/libdb.md)
//! for the profile "that library alone" (pinned R8, min-api 24): scenario builds of its closure,
//! normalized by 8R's rewrites, keyed by the scenario mappings. Only the library's own (declared)
//! classes go into its DB; a version's records are the union over the scenarios.

use std::collections::BTreeMap;

use eightr_core::libdb::pack::NO_METHOD;
use eightr_core::libdb::{Coord, Pack, Profile};
use eightr_core::sigdb::db::{ClassRecord, Record, SigDb};

/// The pinned R8 and min-api of the embedded DBs.
const R8: &str = "9.4.24";
const MIN_API: u32 = 24;

fn intern_class(db: &mut SigDb, index: &mut BTreeMap<String, u32>, name: &str) -> u32 {
    *index.entry(name.to_string()).or_insert_with(|| {
        db.classes.push(name.to_string());
        (db.classes.len() - 1) as u32
    })
}

/// Adds one version, forged as `pack`, to a library's DB.
pub fn add_pack(db: &mut SigDb, version: &str, pack: &Pack) -> Result<(), String> {
    let bit = match db.versions.iter().position(|v| v == version) {
        Some(i) => 1u64 << i,
        None => {
            if db.versions.len() >= 64 {
                return Err(format!("{}: more than 64 versions", db.library));
            }
            db.versions.push(version.to_string());
            1u64 << (db.versions.len() - 1)
        }
    };
    let view = pack.to_sigdb(true);
    let mut classes: BTreeMap<String, u32> = db.classes.iter().enumerate().map(|(i, c)| (c.clone(), i as u32)).collect();
    let mut methods: BTreeMap<(u32, String, String), u32> = db.methods.iter().enumerate().map(|(i, (c, n, p))| ((*c, n.clone(), p.clone()), i as u32)).collect();
    let method = |db: &mut SigDb, classes: &mut BTreeMap<String, u32>, methods: &mut BTreeMap<(u32, String, String), u32>, k: u32| -> u32 {
        let (c, n, p) = &view.methods[k as usize];
        let c = intern_class(db, classes, &view.classes[*c as usize]);
        *methods.entry((c, n.clone(), p.clone())).or_insert_with(|| {
            db.methods.push((c, n.clone(), p.clone()));
            (db.methods.len() - 1) as u32
        })
    };
    let mut records: BTreeMap<Record, u64> = db.records.drain(..).map(|r| (Record { versions: 0, ..r.clone() }, r.versions)).collect();
    let mut class_records: BTreeMap<ClassRecord, u64> = db.class_records.drain(..).map(|r| (ClassRecord { versions: 0, ..r.clone() }, r.versions)).collect();
    for r in &view.records {
        let m = method(db, &mut classes, &mut methods, r.method);
        let callees = r.callees.iter().map(|&(t, k)| (t, if k == NO_METHOD { u32::MAX } else { method(db, &mut classes, &mut methods, k) })).collect();
        *records.entry(Record { method: m, versions: 0, callees, ..r.clone() }).or_default() |= bit;
    }
    for r in &view.class_records {
        let c = intern_class(db, &mut classes, &view.classes[r.class as usize]);
        *class_records.entry(ClassRecord { class: c, versions: 0, ..r.clone() }).or_default() |= bit;
    }
    db.records = records.into_iter().map(|(r, v)| Record { versions: v, ..r }).collect();
    db.class_records = class_records.into_iter().map(|(r, v)| ClassRecord { versions: v, ..r }).collect();
    Ok(())
}

/// One `fixtures/sigdb.conf` line: `library version group:artifact:version`.
struct Entry {
    library: String,
    version: String,
    coord: Coord,
}

fn parse_conf(text: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        let [library, version, coord] = w[..] else { return Err(format!("sigdb.conf:{}: expected `library version group:artifact:version`", n + 1)) };
        let coord = Coord::parse(coord).ok_or_else(|| format!("sigdb.conf:{}: bad coordinate {coord}", n + 1))?;
        out.push(Entry { library: library.into(), version: version.into(), coord });
    }
    Ok(out)
}

pub fn sigdb(only: &[String]) -> Result<(), String> {
    use std::fs;
    let root = crate::root();
    let entries = parse_conf(&fs::read_to_string(root.join("fixtures/sigdb.conf")).map_err(|e| e.to_string())?)?;
    let mut dbs: BTreeMap<String, SigDb> = BTreeMap::new();
    for e in &entries {
        if !only.is_empty() && !only.contains(&e.library) {
            continue;
        }
        eprintln!("sigdb {} {} ({})", e.library, e.version, e.coord);
        let profile = Profile { r8: R8.into(), min_api: MIN_API, mode: "full".into(), libraries: vec![e.coord.clone()] };
        let opts = eightr_forge::build::Options { catalog: eightr_forge::catalog::DEFAULT.to_string(), pins: Vec::new(), jobs: 4, log: false, app: None };
        let (_, pack) = eightr_forge::build::build(&profile, &opts)?;
        let r8 = pack.tools.iter().find(|t| t.0 == "r8").map(|t| t.1.clone()).unwrap_or_default();
        let db = dbs.entry(e.library.clone()).or_insert_with(|| SigDb::new(&e.library, &r8));
        add_pack(db, &e.version, &pack)?;
    }
    let out = root.join("sigdb");
    fs::create_dir_all(&out).map_err(|x| x.to_string())?;
    for (name, db) in &dbs {
        let bytes = db.encode();
        eprintln!("  {name}: {} versions, {} methods, {} records, {} KB", db.versions.len(), db.methods.len(), db.records.len(), bytes.len() / 1024);
        fs::write(out.join(format!("{name}.sigdb")), bytes).map_err(|x| x.to_string())?;
    }
    Ok(())
}
