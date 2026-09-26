//! `cargo xtask sigdb`: builds `sigdb/<library>.sigdb` from the libraries in `fixtures/sigdb.conf`
//! (docs/research/sigdb.md §5): each version run through the pinned R8 alone (its consumer rules and
//! `-keep public class * { public protected *; }`), fingerprinted by 8R, keyed by the originals
//! the build's mapping gives.

use std::collections::BTreeMap;

use eightr_core::sigdb::db::{ClassRecord, Record, SigDb};
use eightr_core::sigdb::print::{class_print, method_print, reflective_strings};
use eightr_ir::model::Program as Model;
use eightr_mapping::{Mapping, Metadata};

fn descriptor(dotted: &str) -> String {
    match dotted {
        "void" => "V".into(),
        "boolean" => "Z".into(),
        "byte" => "B".into(),
        "char" => "C".into(),
        "short" => "S".into(),
        "int" => "I".into(),
        "long" => "J".into(),
        "float" => "F".into(),
        "double" => "D".into(),
        t if t.ends_with("[]") => format!("[{}", descriptor(&t[..t.len() - 2])),
        t => format!("L{};", t.replace('.', "/")),
    }
}

/// Residual → original names from an R8 mapping.
pub struct Names {
    /// Residual class descriptor → original class descriptor.
    classes: BTreeMap<String, String>,
    /// (residual class, residual name, residual proto) → (original name, original proto).
    methods: BTreeMap<(String, String, String), (String, String)>,
}

impl Names {
    pub fn new(mapping: &Mapping) -> Names {
        let classes: BTreeMap<String, String> = mapping.classes.iter().map(|c| (descriptor(&c.obfuscated), descriptor(&c.original))).collect();
        let inverse: BTreeMap<&str, &str> = classes.iter().map(|(r, o)| (o.as_str(), r.as_str())).collect();
        let residual_type = |t: &str| -> String {
            let dims = t.bytes().take_while(|&b| b == b'[').count();
            match inverse.get(&t[dims..]) {
                Some(r) => format!("{}{}", &t[..dims], r),
                None => t.to_string(),
            }
        };
        let mut methods = BTreeMap::new();
        for c in &mapping.classes {
            let rc = descriptor(&c.obfuscated);
            for (m, md) in c.outermost_methods() {
                if md.iter().any(|x| x.parsed == Metadata::Synthesized) {
                    continue;
                }
                let ps: Vec<String> = m.params.iter().map(|t| descriptor(t)).collect();
                let original = format!("({}){}", ps.concat(), descriptor(&m.return_type));
                let residual = md
                    .iter()
                    .find_map(|x| match &x.parsed {
                        Metadata::ResidualSignature(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| format!("({}){}", ps.iter().map(|t| residual_type(t)).collect::<String>(), residual_type(&descriptor(&m.return_type))));
                methods.insert((rc.clone(), m.obfuscated.clone(), residual), (m.original_name.clone(), original));
            }
        }
        Names { classes, methods }
    }

    pub fn class(&self, residual: &str) -> String {
        self.classes.get(residual).cloned().unwrap_or_else(|| residual.to_string())
    }

    /// Original (name, proto) of a residual method; unmapped methods kept their names, and their
    /// proto maps type by type.
    pub fn method(&self, class: &str, name: &str, proto: &str) -> (String, String) {
        if let Some(x) = self.methods.get(&(class.to_string(), name.to_string(), proto.to_string())) {
            return x.clone();
        }
        let map = |t: &str| {
            let dims = t.bytes().take_while(|&b| b == b'[').count();
            format!("{}{}", &t[..dims], self.class(&t[dims..]))
        };
        let proto = match eightr_ir::types::parse_proto(proto) {
            Some((ps, r)) => format!("({}){}", ps.iter().map(|t| map(t)).collect::<String>(), map(r)),
            None => proto.to_string(),
        };
        (name.to_string(), proto)
    }
}


fn intern_class(db: &mut SigDb, index: &mut BTreeMap<String, u32>, name: String) -> u32 {
    *index.entry(name.clone()).or_insert_with(|| {
        db.classes.push(name);
        (db.classes.len() - 1) as u32
    })
}

/// Adds one version's residual program. `universe` holds the original descriptors of every
/// class of every library in the DB universe: references to them are erased too, as an app
/// that shrank them would have renamed them.
pub fn add_version(db: &mut SigDb, version: &str, model: &Model, mapping: &Mapping, universe: &dyn Fn(&str) -> bool) {
    let bit = match db.versions.iter().position(|v| v == version) {
        Some(i) => 1u32 << i,
        None => {
            db.versions.push(version.to_string());
            1u32 << (db.versions.len() - 1)
        }
    };
    let names = Names::new(mapping);
    let reflective = reflective_strings(model);
    let s = &model.syms;
    let stable = |d: &str| model.find(d).is_none() && !universe(d) && !universe(&names.class(d));
    let mut class_index: BTreeMap<String, u32> = db.classes.iter().enumerate().map(|(i, c)| (c.clone(), i as u32)).collect();
    let mut method_index: BTreeMap<(u32, String, String), u32> =
        db.methods.iter().enumerate().map(|(i, (c, n, p))| ((*c, n.clone(), p.clone()), i as u32)).collect();
    // (method index, class index); interning the method only when it exists.
    let mut key_of = |db: &mut SigDb, rc: &str, rn: &str, rp: &str| -> (u32, u32) {
        let class = intern_class(db, &mut class_index, names.class(rc));
        if rn == "<clinit>" && rp == "()V" && !model.find(rc).is_some_and(|k| model.classes[k].methods.iter().any(|m| s.get(m.name) == "<clinit>")) {
            return (u32::MAX, class);
        }
        let (n, p) = names.method(rc, rn, rp);
        let m = *method_index.entry((class, n.clone(), p.clone())).or_insert_with(|| {
            db.methods.push((class, n, p));
            (db.methods.len() - 1) as u32
        });
        (m, class)
    };
    let mut records: BTreeMap<Record, u32> = db.records.drain(..).map(|r| (Record { versions: 0, ..r.clone() }, r.versions)).collect();
    let mut class_records: BTreeMap<ClassRecord, u32> = db.class_records.drain(..).map(|r| (ClassRecord { versions: 0, ..r.clone() }, r.versions)).collect();
    for (ci, c) in model.classes.iter().enumerate() {
        let rc = s.get(c.ty);
        let class = key_of(db, rc, "<clinit>", "()V").1;
        let cp = class_print(model, ci, &stable);
        *class_records.entry(ClassRecord { class, versions: 0, c2: cp.c2, c3: cp.c3 }).or_default() |= bit;
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(mp) = method_print(model, ci, mi, &stable, &reflective) else { continue };
            let method = key_of(db, rc, s.get(m.name), s.get(m.proto)).0;
            let callees = mp
                .callees
                .iter()
                .map(|(tok, (cc, cn, cpr))| (*tok, if model.find(cc).is_some() { key_of(db, cc, cn, cpr).0 } else { u32::MAX }))
                .collect();
            let r = Record {
                method,
                versions: 0,
                informative: mp.informative,
                all: mp.all,
                strings: mp.strings,
                proto: mp.proto,
                sketch: mp.sketch,
                callees,
            };
            *records.entry(r).or_default() |= bit;
        }
    }
    db.records = records.into_iter().map(|(r, v)| Record { versions: v, ..r }).collect();
    db.class_records = class_records.into_iter().map(|(r, v)| ClassRecord { versions: v, ..r }).collect();
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

/// Class descriptors in a jar.
fn jar_classes(jar: &std::path::Path) -> Result<Vec<String>, String> {
    Ok(crate::zip_entries(jar)?
        .into_iter()
        .filter_map(|e| e.strip_suffix(".class").filter(|c| !c.starts_with("META-INF/") && !c.ends_with("module-info")).map(|c| format!("L{c};")))
        .collect())
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
    // The universe: every class of every configured library and dependency.
    let mut universe = std::collections::BTreeSet::new();
    let mut artifacts: Vec<&str> = entries.iter().flat_map(|e| std::iter::once(e.artifact.as_str()).chain(e.libs.iter().map(String::as_str))).collect();
    artifacts.sort();
    artifacts.dedup();
    for a in &artifacts {
        universe.extend(jar_classes(&crate::prepare_lib(a, &tc)?.jar)?);
    }
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
        let model = Model::load(&refs).map_err(|x| format!("{x:?}"))?;
        let mapping = Mapping::parse_normalized(&fs::read_to_string(work.join("mapping.txt")).map_err(|x| x.to_string())?).map_err(|x| format!("{x:?}"))?;
        let db = dbs.entry(e.library.clone()).or_insert_with(|| SigDb::new(&e.library, &r8_version));
        add_version(db, &e.version, &model, &mapping, &|d| universe.contains(d));
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
