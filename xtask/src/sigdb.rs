//! `cargo xtask sigdb`: builds `sigdb/<library>.sigdb` from the libraries in `fixtures/sigdb.conf`
//! (docs/research/sigdb.md §5): each version run through the pinned R8 alone (its consumer rules and
//! `-keep public class * { public protected *; }`), normalized by 8R's rewrites, fingerprinted by
//! 8R, keyed by the originals the build's mapping gives.

use std::collections::BTreeMap;

use eightr_core::sigdb::db::{ClassRecord, Record, SigDb};
use eightr_core::sigdb::print::{class_print, method_print, reflective_strings};
use eightr_ir::model::Program as Model;
use eightr_forge::names::Names;
use eightr_mapping::Mapping;

fn intern_class(db: &mut SigDb, index: &mut BTreeMap<String, u32>, name: String) -> u32 {
    *index.entry(name.clone()).or_insert_with(|| {
        db.classes.push(name);
        (db.classes.len() - 1) as u32
    })
}

/// Interning of DB keys, carried across the versions of one library.
struct Keys {
    classes: BTreeMap<String, u32>,
    methods: BTreeMap<(u32, String, String), u32>,
}

impl Keys {
    fn of(db: &SigDb) -> Keys {
        Keys {
            classes: db.classes.iter().enumerate().map(|(i, c)| (c.clone(), i as u32)).collect(),
            methods: db.methods.iter().enumerate().map(|(i, (c, n, p))| ((*c, n.clone(), p.clone()), i as u32)).collect(),
        }
    }

    /// The key of a residual method, `None` for R8's own methods.
    fn method(&mut self, db: &mut SigDb, names: &Names, rc: &str, rn: &str, rp: &str) -> Option<u32> {
        let (owner, n, p) = names.method(rc, rn, rp)?;
        let class = intern_class(db, &mut self.classes, owner);
        Some(*self.methods.entry((class, n.clone(), p.clone())).or_insert_with(|| {
            db.methods.push((class, n, p));
            (db.methods.len() - 1) as u32
        }))
    }
}

/// Adds one version's residual program. The model is normalized by 8R's own rewrites first
/// (outlines inlined back, merged classes split), as an app's is before matching; classes 8R
/// created and the bases it split have no single original and get no records. A reference is
/// stable (keeps its name) only if it names a platform class, the rule the app side uses.
pub fn add_version(db: &mut SigDb, version: &str, model: &mut Model, mapping: &Mapping) -> Result<(), String> {
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
    let names = Names::new(mapping);
    let program: std::collections::BTreeSet<String> = model.classes.iter().map(|c| model.syms.get(c.ty).to_string()).collect();
    let rewrites = eightr_core::rewrites::run_all(model).map_err(|e| e.to_string())?;
    let split: std::collections::BTreeSet<String> = rewrites.iter().filter(|r| r.rule == eightr_rules::SPLIT_MERGED_CLASS).map(|r| r.item.clone()).collect();
    let model = &*model;
    let reflective = reflective_strings(model);
    let s = &model.syms;
    let stable = |d: &str| eightr_core::sigdb::print::platform_stable(d);
    let mut keys = Keys::of(db);
    let mut records: BTreeMap<Record, u64> = db.records.drain(..).map(|r| (Record { versions: 0, ..r.clone() }, r.versions)).collect();
    let mut class_records: BTreeMap<ClassRecord, u64> = db.class_records.drain(..).map(|r| (ClassRecord { versions: 0, ..r.clone() }, r.versions)).collect();
    for (ci, c) in model.classes.iter().enumerate() {
        let rc = s.get(c.ty);
        if !program.contains(rc) || split.contains(rc) || names.synthesized_classes.contains(rc) {
            continue;
        }
        let class = intern_class(db, &mut keys.classes, names.class(rc));
        let cp = class_print(model, ci, &stable);
        *class_records.entry(ClassRecord { class, versions: 0, c2: cp.c2, c3: cp.c3 }).or_default() |= bit;
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(mp) = method_print(model, ci, mi, &stable, &reflective) else { continue };
            let Some(method) = keys.method(db, &names, rc, s.get(m.name), s.get(m.proto)) else { continue };
            let callees = mp
                .callees
                .iter()
                .map(|(tok, (cc, cn, cpr))| (*tok, if program.contains(cc.as_str()) { keys.method(db, &names, cc, cn, cpr).unwrap_or(u32::MAX) } else { u32::MAX }))
                .collect();
            let r = Record { method, versions: 0, informative: mp.informative, all: mp.all, strings: mp.strings, proto: mp.proto, sketch: mp.sketch, callees };
            *records.entry(r).or_default() |= bit;
        }
    }
    db.records = records.into_iter().map(|(r, v)| Record { versions: v, ..r }).collect();
    db.class_records = class_records.into_iter().map(|(r, v)| ClassRecord { versions: v, ..r }).collect();
    Ok(())
}

/// One `fixtures/sigdb.conf` line: `library version artifact [lib=artifact ...]`.
struct Entry {
    library: String,
    version: String,
    artifact: String,
    libs: Vec<String>,
}

fn parse_conf(text: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() < 3 {
            return Err(format!("sigdb.conf:{}: expected `library version artifact [lib=artifact ...]`", n + 1));
        }
        let libs = w[3..]
            .iter()
            .map(|x| x.strip_prefix("lib=").map(str::to_string).ok_or_else(|| format!("sigdb.conf:{}: unknown option {x}", n + 1)))
            .collect::<Result<_, _>>()?;
        out.push(Entry { library: w[0].into(), version: w[1].into(), artifact: w[2].into(), libs });
    }
    Ok(out)
}

pub fn sigdb(only: &[String]) -> Result<(), String> {
    use std::fs;
    use std::process::Command;
    let tools = crate::find_tools()?;
    let tc = crate::toolchain()?;
    let root = crate::root();
    let entries = parse_conf(&fs::read_to_string(root.join("fixtures/sigdb.conf")).map_err(|e| e.to_string())?)?;
    let r8_jar = crate::fetch("r8-9.4.24", &tc)?;
    let r8_version = crate::run(Command::new(&tools.java).arg("-cp").arg(&r8_jar).args(["com.android.tools.r8.R8", "--version"]))?;
    let r8_version = r8_version.lines().next().unwrap_or_default().to_string();
    let mut dbs: BTreeMap<String, SigDb> = BTreeMap::new();
    for e in &entries {
        if !only.is_empty() && !only.contains(&e.library) {
            continue;
        }
        eprintln!("sigdb {} {}", e.library, e.version);
        let lib = crate::prepare_lib(&e.artifact, &tc)?;
        let work = crate::cache_dir().join(format!("sigdb-{}-{}", e.library, e.version));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).map_err(|x| x.to_string())?;
        let keep = work.join("keep.pro");
        fs::write(&keep, "-keep public class * { public protected *; }\n-keepattributes Signature,InnerClasses,EnclosingMethod,*Annotation*\n-ignorewarnings\n")
            .map_err(|x| x.to_string())?;
        let mut cmd = Command::new(&tools.java);
        cmd.arg("-cp").arg(&r8_jar).args(["com.android.tools.r8.R8", "--release", "--min-api", "24", "--lib"]).arg(&tools.android_jar);
        for d in &e.libs {
            cmd.arg("--lib").arg(crate::prepare_lib(d, &tc)?.jar);
        }
        cmd.arg("--pg-conf").arg(&keep);
        for r in &lib.rules {
            cmd.arg("--pg-conf").arg(r);
        }
        cmd.arg("--pg-map-output").arg(work.join("mapping.txt")).arg("--output").arg(&work).arg(&lib.jar);
        crate::run(&mut cmd)?;
        let mut dexes = Vec::new();
        for i in 1.. {
            let f = work.join(if i == 1 { "classes.dex".to_string() } else { format!("classes{i}.dex") });
            if !f.exists() {
                break;
            }
            dexes.push(fs::read(f).map_err(|x| x.to_string())?);
        }
        let parsed: Vec<eightr_dex::Dex> = dexes.iter().map(|b| eightr_dex::Dex::parse(b).map_err(|x| format!("{x:?}"))).collect::<Result<_, _>>()?;
        let refs: Vec<&eightr_dex::Dex> = parsed.iter().collect();
        let mut model = Model::load(&refs).map_err(|x| format!("{x:?}"))?;
        let mapping = Mapping::parse_normalized(&fs::read_to_string(work.join("mapping.txt")).map_err(|x| x.to_string())?).map_err(|x| format!("{x:?}"))?;
        let db = dbs.entry(e.library.clone()).or_insert_with(|| SigDb::new(&e.library, &r8_version));
        add_version(db, &e.version, &mut model, &mapping)?;
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
