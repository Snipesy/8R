//! `8r-forge build`: profile → resolved closure → scenario builds (R8 of the profile's version,
//! min-api and mode) → fingerprints → one pack, cached by everything that went in.
//!
//! Every input is pinned: the closure by sha256 (the lock), the catalog verbatim, R8 by sha256,
//! `android.jar` by sha256, javac by version. The pack's file name is a hash of all of them, so a
//! second run with the same inputs reuses it; a fresh run produces the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use eightr_core::libdb::pack::{ClassRecord, Record, NO_ARTIFACT, NO_METHOD};
use eightr_core::libdb::{Coord, Pack, Profile};

use crate::api::{Api, Entry};
use crate::artifacts::{prepare, Lib};
use crate::catalog::{Kind, Scenario, Scope};
use crate::fingerprint::{Key, ScenarioOut};
use crate::tools::{cache_root, mkdirs, r8_jar, read, run, sha256_hex, write, Result, Tools};

/// Bumped whenever the forge's output for the same inputs changes.
pub const FORGE_VERSION: &str = "3";

/// Per scenario kind, bumped whenever that generator's output changes: part of the kind's
/// scenario cache key and of the pack key, so only its scenarios run again.
fn generator_version(k: Kind) -> u32 {
    match k {
        Kind::LibAlone | Kind::Roots => 1,
        // 2: opaque argument types instantiable; 3: Java keywords filtered; 4: contextual keywords allowed.
        Kind::Callers => 4,
        // 2: every call from two call sites; 3: one call per method; 4: argument types instantiable;
        // 5: `ifeq` only, bit 31, single-mask bridges.
        Kind::Defaults => 5,
    }
}

pub struct Options {
    pub catalog: String,
    /// Extra pins (`--pin`), overriding the profile's version of the same module.
    pub pins: Vec<Coord>,
    /// Parallel scenario builds.
    pub jobs: usize,
    pub log: bool,
}

macro_rules! log {
    ($o:expr, $($t:tt)*) => { if $o.log { eprintln!($($t)*); } };
}

/// The inputs that determine a pack.
struct Inputs {
    profile: Profile,
    closure: Vec<crate::maven::Resolved>,
    lock: Vec<String>,
    catalog: String,
    scenarios: Vec<Scenario>,
    tools: Tools,
    r8: PathBuf,
    tool_ids: Vec<(String, String)>,
    hash: String,
    /// The inputs of the R8 runs alone (scenario outputs are cached by this and the scenario).
    build_hash: String,
    /// Cores each R8 JVM may use (machine cores / parallel jobs).
    cpus_per_jvm: usize,
}

fn inputs(profile: &Profile, opts: &Options) -> Result<Inputs> {
    let mut pins: BTreeMap<(String, String), Coord> = profile.libraries.iter().map(|c| ((c.group.clone(), c.artifact.clone()), c.clone())).collect();
    for c in &opts.pins {
        pins.insert((c.group.clone(), c.artifact.clone()), c.clone());
    }
    let pins: Vec<Coord> = pins.into_values().collect();
    log!(opts, "resolving {} declared libraries", pins.len());
    let res = crate::maven::resolve(&pins)?;
    for c in &res.missing {
        log!(opts, "warning: {c} is on neither Google Maven nor Maven Central; left out");
    }
    let closure = res.artifacts;
    let mut lock: Vec<String> = closure.iter().map(|r| r.lock_line()).collect();
    lock.extend(res.missing.iter().map(|c| format!("{c} missing -")));
    lock.sort();
    let scenarios = crate::catalog::parse(&opts.catalog)?;
    let tools = Tools::find()?;
    let (r8, r8_sha) = r8_jar(&profile.r8)?;
    let tool_ids = vec![
        ("forge".to_string(), FORGE_VERSION.to_string()),
        ("fingerprint".to_string(), eightr_core::libdb::code_id()),
        ("r8".to_string(), format!("{} {r8_sha}", profile.r8)),
        ("javac".to_string(), tools.javac_version.clone()),
        ("android.jar".to_string(), tools.android_jar_sha256.clone()),
        // Recorded in the pack (packs for one profile from different generators are told apart);
        // not an input of the R8 runs' shared key (each scenario's own key has its version).
        ("generators".to_string(), {
            let mut g: Vec<String> = scenarios.iter().map(|s| format!("{}={}", s.name, generator_version(s.kind))).collect();
            g.push(format!("fingerprint-rev={}", crate::fingerprint::REVISION));
            g.join(",")
        }),
    ];
    let mut h = String::new();
    h.push_str(&profile.canonical());
    h.push('\n');
    for c in &pins {
        h.push_str(&format!("pin {c}\n"));
    }
    for l in &lock {
        h.push_str(l);
        h.push('\n');
    }
    // What the R8 runs depend on (not the fingerprint code, which only reads their output).
    for (k, v) in tool_ids.iter().filter(|t| t.0 != "fingerprint" && t.0 != "generators") {
        h.push_str(&format!("\n{k}={v}"));
    }
    let build_hash = sha256_hex(h.as_bytes())[..16].to_string();
    h.push_str(&opts.catalog);
    for s in &scenarios {
        h.push_str(&format!("\ngenerator {} {}", s.name, generator_version(s.kind)));
    }
    h.push_str(&format!("\nforge-fingerprint {}", crate::fingerprint::REVISION));
    for (k, v) in tool_ids.iter().filter(|t| t.0 == "fingerprint" || t.0 == "generators") {
        h.push_str(&format!("\n{k}={v}"));
    }
    let hash = sha256_hex(h.as_bytes())[..16].to_string();
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let cpus_per_jvm = (cores / opts.jobs.max(1)).max(1);
    Ok(Inputs { profile: profile.clone(), closure, lock, catalog: opts.catalog.clone(), scenarios, tools, r8, tool_ids, hash, build_hash, cpus_per_jvm })
}

/// Where the pack for these inputs lives.
fn pack_path(inp: &Inputs) -> PathBuf {
    cache_root().join("packs").join(format!("{}-{}.8rpack", inp.profile.key(), inp.hash))
}

/// Runs R8 for one scenario into `dir` (skipped when `dir/done` exists).
fn run_scenario(inp: &Inputs, libs: &[Lib], api: &Api, declared: &BTreeSet<u32>, s: &Scenario, dir: &Path) -> Result<String> {
    let done = dir.join("done");
    if done.exists() {
        return Ok(String::from_utf8_lossy(&read(&done)?).into_owned());
    }
    let _ = std::fs::remove_dir_all(dir);
    mkdirs(dir)?;
    let all_jars: Vec<PathBuf> = libs.iter().flat_map(|l| l.jars.iter().cloned()).collect();
    let in_scope = |e: &Entry| s.scope == Scope::Closure || declared.contains(&e.artifact);
    let sample: Vec<&Entry> = api.entries.iter().filter(|e| in_scope(e) && crate::gen::sampled(e, s.frac, s.seed)).collect();
    let mut rules = String::from("-dontwarn **\n-ignorewarnings\n");
    let mut program = all_jars.clone();
    let note = match s.kind {
        Kind::LibAlone => {
            rules.push_str("-keep public class * { public protected *; }\n");
            "keep public API".to_string()
        }
        Kind::Roots => {
            rules.push_str(&crate::gen::roots(&sample));
            format!("{} roots", sample.len())
        }
        Kind::Defaults => {
            let sample: Vec<&crate::api::DefaultEntry> = api.defaults.iter().filter(|d| in_scope(&d.entry) && crate::gen::sampled(&d.entry, s.frac, s.seed)).collect();
            let (jar, n) = crate::gen::default_callers(&sample, &dir.join("gen"))?;
            rules.push_str("-keep class gen.** { *; }\n");
            // Bridges are static: receivers are parameters.
            let descs: Vec<&str> = sample.iter().map(|d| d.entry.desc.as_str()).collect();
            rules.push_str(&crate::gen::instantiable(&descs, &api.access));
            program.push(jar);
            format!("{n} default-argument calls")
        }
        Kind::Callers => {
            let (jar, kept) = crate::gen::callers(&inp.tools, &all_jars, &sample, &dir.join("gen"))?;
            rules.push_str("-keep class gen.** { *; }\n");
            // Receivers of instance calls are opaque values of their class too.
            let receivers: Vec<String> = sample.iter().filter(|e| e.access & crate::api::ACC_STATIC == 0 && e.name != "<init>").map(|e| format!("({})V", e.class)).collect();
            let descs: Vec<&str> = sample.iter().map(|e| e.desc.as_str()).chain(receivers.iter().map(String::as_str)).collect();
            rules.push_str(&crate::gen::instantiable(&descs, &api.access));
            program.push(jar);
            format!("{kept} of {} calls compile", sample.len())
        }
    };
    let conf = dir.join("scenario.pro");
    write(&conf, &rules)?;
    let out = dir.join("out");
    mkdirs(&out)?;
    let mut cmd = Command::new(&inp.tools.java);
    cmd.arg(format!("-Xmx{}", std::env::var("EIGHTR_FORGE_XMX").unwrap_or_else(|_| "8g".into())));
    // Each JVM sized to its share of the cores: R8's, the GC's and the JIT's threads otherwise
    // all assume the whole machine and `jobs` JVMs oversubscribe it (about 28% more CPU).
    cmd.arg(format!("-XX:ActiveProcessorCount={}", inp.cpus_per_jvm));
    cmd.arg("-cp").arg(&inp.r8).args(["com.android.tools.r8.R8", "--release", "--min-api"]).arg(inp.profile.min_api.to_string());
    if inp.profile.mode == "compatibility" {
        cmd.arg("--pg-compat");
    }
    cmd.arg("--lib").arg(&inp.tools.android_jar);
    for l in libs {
        for r in &l.rules {
            cmd.arg("--pg-conf").arg(r);
        }
    }
    cmd.arg("--pg-conf").arg(&conf).arg("--pg-map-output").arg(out.join("mapping.txt")).arg("--output").arg(&out);
    cmd.args(&program);
    run(&mut cmd).map_err(|e| format!("scenario {}: {}", s.name, e.lines().take(20).collect::<Vec<_>>().join("\n")))?;
    write(&done, &note)?;
    Ok(note)
}

/// Interning of pack keys.
#[derive(Default)]
struct Interner {
    classes: BTreeMap<String, u32>,
    methods: BTreeMap<Key, u32>,
    stacks: BTreeMap<Vec<(u32, i32)>, u32>,
}

/// Merges scenario outputs (in scenario order) into a pack.
fn merge(inp: &Inputs, owner: &dyn Fn(&str) -> u32, outs: &[ScenarioOut]) -> Pack {
    let mut it = Interner::default();
    let mut pack = Pack {
        profile: inp.profile.clone(),
        lock: inp.lock.clone(),
        catalog: inp.catalog.clone(),
        tools: inp.tool_ids.clone(),
        scenarios: inp.scenarios.iter().map(|s| s.name.clone()).collect(),
        artifacts: inp.closure.iter().map(|r| (r.coord.to_string(), r.declared)).collect(),
        classes: Vec::new(),
        methods: Vec::new(),
        records: Vec::new(),
        class_records: Vec::new(),
        stacks: Vec::new(),
    };
    // Keys first, in sorted order, so indices don't depend on scenario order.
    let mut classes: BTreeSet<String> = BTreeSet::new();
    let mut methods: BTreeSet<Key> = BTreeSet::new();
    for o in outs {
        for m in &o.methods {
            methods.insert(m.key.clone());
            for (_, c) in &m.callees {
                if let Some(c) = c {
                    methods.insert(c.clone());
                }
            }
            for (_, _, st) in &m.frames {
                for (k, _) in st {
                    methods.insert(k.clone());
                }
            }
        }
        for (c, _, _) in &o.classes {
            classes.insert(c.clone());
        }
    }
    for m in &methods {
        classes.insert(m.0.clone());
    }
    for c in classes {
        it.classes.insert(c.clone(), pack.classes.len() as u32);
        let a = owner(&c);
        pack.classes.push((c, a));
    }
    for m in methods {
        it.methods.insert(m.clone(), pack.methods.len() as u32);
        pack.methods.push((it.classes[&m.0], m.1, m.2));
    }
    // Stacks, sorted.
    let mut stacks: BTreeSet<Vec<(u32, i32)>> = BTreeSet::new();
    let conv = |it: &Interner, st: &[(Key, i32)]| -> Vec<(u32, i32)> { st.iter().map(|(k, l)| (it.methods[k], *l)).collect() };
    for o in outs {
        for m in &o.methods {
            for (_, _, st) in &m.frames {
                stacks.insert(conv(&it, st));
            }
        }
    }
    for st in stacks {
        it.stacks.insert(st.clone(), pack.stacks.len() as u32);
        pack.stacks.push(st);
    }
    let mut records: BTreeMap<Record, u64> = BTreeMap::new();
    let mut class_records: BTreeMap<ClassRecord, u64> = BTreeMap::new();
    for (si, o) in outs.iter().enumerate() {
        let bit = 1u64 << si;
        for m in &o.methods {
            let r = Record {
                method: it.methods[&m.key],
                scenarios: 0,
                informative: m.informative,
                unique: false,
                all: m.all,
                strings: m.strings,
                proto: m.proto,
                sketch: m.sketch,
                callees: m.callees.iter().map(|(t, c)| (*t, c.as_ref().map_or(NO_METHOD, |c| it.methods[c]))).collect(),
                frames: m.frames.iter().map(|(a, b, st)| (*a, *b, it.stacks[&conv(&it, st)])).collect(),
            };
            *records.entry(r).or_default() |= bit;
        }
        for (c, c2, c3) in &o.classes {
            *class_records.entry(ClassRecord { class: it.classes[c], scenarios: 0, c2: *c2, c3: *c3 }).or_default() |= bit;
        }
    }
    // A body hash is unique when every record having it is the same method.
    let mut by_hash: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    for r in records.keys() {
        by_hash.entry(r.all).or_default().insert(r.method);
    }
    pack.records = records.into_iter().map(|(r, v)| Record { scenarios: v, unique: by_hash[&r.all].len() == 1, ..r }).collect();
    pack.class_records = class_records.into_iter().map(|(r, v)| ClassRecord { scenarios: v, ..r }).collect();
    pack
}

/// Resolves only (the lock, without building).
pub fn lock(profile: &Profile, pins: &[Coord]) -> Result<Vec<String>> {
    let mut all = profile.libraries.clone();
    all.retain(|c| !pins.iter().any(|p| p.group == c.group && p.artifact == c.artifact));
    all.extend(pins.iter().cloned());
    let res = crate::maven::resolve(&all)?;
    let mut out: Vec<String> = res.artifacts.iter().map(|r| r.lock_line()).collect();
    out.extend(res.missing.iter().map(|c| format!("{c} missing -")));
    out.sort();
    Ok(out)
}

/// Builds (or reuses) the pack of `profile`: (path, pack).
pub fn build(profile: &Profile, opts: &Options) -> Result<(PathBuf, Pack)> {
    let inp = inputs(profile, opts)?;
    let path = pack_path(&inp);
    if path.exists() {
        log!(opts, "cached: {}", path.display());
        let pack = Pack::decode(&read(&path)?)?;
        return Ok((path, pack));
    }
    log!(opts, "closure: {} artifacts; {} scenarios; R8 {}; min-api {}", inp.closure.len(), inp.scenarios.len(), inp.profile.r8, inp.profile.min_api);
    let libs: Vec<Lib> = inp.closure.iter().map(|r| prepare(r, &inp.profile.r8)).collect::<Result<_>>()?;
    let api = Api::build(&libs)?;
    let declared: BTreeSet<u32> = inp.closure.iter().enumerate().filter(|(_, r)| r.declared).map(|(i, _)| i as u32).collect();
    log!(opts, "API: {} classes, {} public entry points ({} declared artifacts)", api.owner.len(), api.entries.len(), declared.len());
    let work = cache_root().join("work").join(format!("{}-{}", inp.profile.key(), inp.build_hash));
    // A scenario's directory: its name and definition, so catalog edits reuse unchanged ones.
    let scenario_dir = |s: &Scenario| work.join(format!("{}-{}", s.name, &sha256_hex(format!("{:?}|{}|{}|{:?}|{}", s.kind, s.frac, s.seed, s.scope, generator_version(s.kind)).as_bytes())[..8]));
    // Scenario builds, `jobs` at a time; results by index.
    let jobs = opts.jobs.max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let owner_of = |c: &str| api.owner.get(c).copied();
    // Each worker runs a scenario's R8 and then fingerprints it; results by index, so the merge
    // order is the catalog's whatever finishes first.
    let results: std::sync::Mutex<BTreeMap<usize, Result<(String, ScenarioOut)>>> = std::sync::Mutex::new(BTreeMap::new());
    std::thread::scope(|sc| {
        for _ in 0..jobs {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(s) = inp.scenarios.get(i) else { break };
                let r = run_scenario(&inp, &libs, &api, &declared, s, &scenario_dir(s)).and_then(|note| {
                    let o = crate::fingerprint::scenario(&scenario_dir(s).join("out"), &|c| owner_of(c).is_some())?;
                    log!(opts, "  scenario {}: {note}; {} methods fingerprinted", s.name, o.methods.len());
                    Ok((note, o))
                });
                results.lock().expect("no poisoned lock").insert(i, r);
            });
        }
    });
    let results = results.into_inner().expect("no poisoned lock");
    let mut outs = Vec::new();
    for (i, r) in results {
        outs.push(r.map_err(|e| format!("{}: {e}", inp.scenarios[i].name))?.1);
    }
    let pack = merge(&inp, &|c| owner_of(c).unwrap_or(NO_ARTIFACT), &outs);
    write(&path, pack.encode())?;
    Ok((path, pack))
}
