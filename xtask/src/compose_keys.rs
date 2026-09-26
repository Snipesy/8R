//! `cargo xtask compose-keys`: builds `sigdb/compose-keys.ckdb` from the Compose AARs pinned in
//! `fixtures/compose-keys.conf` (docs/research/compose-keys.md §8): each version's classes.jar
//! dexed by D8, then every restartable composable's entry key and inner keys, by name (the
//! libraries are unminified).

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use eightr_core::compose_keys::{Entry, KeyDb};
use eightr_ir::model::Program as Model;

pub fn compose_keys() -> Result<(), String> {
    let tools = crate::find_tools()?;
    let root = crate::root();
    let conf = fs::read_to_string(root.join("fixtures/compose-keys.conf")).map_err(|e| e.to_string())?;
    let tc = crate::toolchain()?;
    let r8_jar = crate::fetch("r8-9.4.24", &tc)?;
    let mut db = KeyDb::default();
    // (artifact, function key) → function index; (function, key, inner) → versions.
    let mut fns: BTreeMap<(u32, String, String, String), u32> = BTreeMap::new();
    let mut entries: BTreeMap<(u32, i32, Vec<i32>), u64> = BTreeMap::new();
    for line in conf.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let [artifact, version, sha256, url] = w[..] else { return Err(format!("bad compose-keys.conf line: {line}")) };
        let name = format!("ck-{artifact}-{version}");
        let one: BTreeMap<String, crate::Artifact> = BTreeMap::from([(name.clone(), crate::Artifact { sha256: sha256.into(), url: url.into() })]);
        let aar = crate::fetch(&name, &one)?;
        let work = crate::cache_dir().join(format!("compose-keys-{artifact}-{version}"));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let jar = work.join("classes.jar");
        crate::extract(&aar, "classes.jar", &jar)?;
        crate::run(
            Command::new(&tools.java)
                .arg("-cp")
                .arg(&r8_jar)
                .args(["com.android.tools.r8.D8", "--release", "--min-api", "24", "--lib"])
                .arg(&tools.android_jar)
                .arg("--output")
                .arg(&work)
                .arg(&jar),
        )?;
        let mut bytes = Vec::new();
        for i in 1.. {
            let f = work.join(if i == 1 { "classes.dex".to_string() } else { format!("classes{i}.dex") });
            if !f.exists() {
                break;
            }
            bytes.push(fs::read(f).map_err(|e| e.to_string())?);
        }
        let dexes: Vec<eightr_dex::Dex> = bytes.iter().map(|b| eightr_dex::Dex::parse(b).map_err(|e| format!("{e:?}"))).collect::<Result<_, _>>()?;
        let refs: Vec<&eightr_dex::Dex> = dexes.iter().collect();
        let model = Model::load(&refs).map_err(|e| format!("{e:?}"))?;
        let ai = match db.artifacts.iter().position(|a| a.0 == artifact) {
            Some(i) => i,
            None => {
                db.artifacts.push((artifact.to_string(), Vec::new()));
                db.artifacts.len() - 1
            }
        };
        db.artifacts[ai].1.push(version.to_string());
        if db.artifacts[ai].1.len() > 64 {
            return Err(format!("{artifact}: more than 64 versions"));
        }
        let bit = 1u64 << (db.artifacts[ai].1.len() - 1);
        let found = eightr_core::compose_keys::extract(&model);
        eprintln!("compose-keys {artifact} {version}: {} restartable composables", found.len());
        for (owner, n, desc, key, inner) in found {
            let next = fns.len() as u32;
            let f = *fns.entry((ai as u32, owner.clone(), n.clone(), desc.clone())).or_insert_with(|| {
                db.functions.push((ai as u32, owner, n, desc));
                next
            });
            *entries.entry((f, key, inner)).or_default() |= bit;
        }
    }
    db.entries = entries.into_iter().map(|((function, key, inner), versions)| Entry { key, function, versions, inner }).collect();
    let bytes = db.encode();
    eprintln!("{} functions, {} entries, {} KB", db.functions.len(), db.entries.len(), bytes.len() / 1024);
    fs::write(root.join("sigdb/compose-keys.ckdb"), bytes).map_err(|e| e.to_string())
}
