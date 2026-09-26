//! Step 6: 8R's emitted program. Renaming must be a consistent bijection that preserves
//! behavior (every override kept, none created, no field shadowing), the mapping file must
//! describe exactly what was renamed, the output must be valid dex, and 8R must be idempotent.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eightr_core::input::DexInput;
use eightr_core::output::{emit, Output};
use eightr_core::{run, Config};
use eightr_dex::class::access;
use eightr_dex::Dex;
use eightr_ir::model::Program as Model;
use eightr_ir::rename::Renaming;
use eightr_mapping::{Mapping, MemberKind};

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut v: Vec<(String, Vec<u8>)> = fs::read_dir(&root)
        .unwrap()
        .flat_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            ["d8", "r8"].map(|variant| (format!("{name}/{variant}"), root.join(&name).join(variant).join("classes.dex")))
        })
        .filter(|(_, p)| p.exists())
        .map(|(n, p)| (n, fs::read(p).unwrap()))
        .collect();
    v.sort();
    v
}

fn load(files: &[Vec<u8>]) -> Model {
    let dexes: Vec<Dex> = files.iter().map(|b| Dex::parse(b).unwrap()).collect();
    let refs: Vec<&Dex> = dexes.iter().collect();
    Model::load(&refs).unwrap()
}

fn undo(bytes: &[u8]) -> (eightr_core::Outcome, Output) {
    undo_with(bytes, false)
}

/// Names-only runs, for invariants that assume the output has the input's class set (the
/// renaming bijection, override preservation, the mapping file). Structural rewrites are
/// checked by execution (tests/exec.rs) and by the validity/idempotence/α tests.
fn undo_names(bytes: &[u8]) -> (eightr_core::Outcome, Output) {
    undo_with(bytes, true)
}

fn undo_with(bytes: &[u8], no_rewrites: bool) -> (eightr_core::Outcome, Output) {
    let outcome =
        run(&[DexInput { name: "classes.dex".into(), bytes: bytes.to_vec() }], &Config { no_rewrites, ..Default::default() }).unwrap();
    let out = emit(&outcome).unwrap();
    (outcome, out)
}

fn out_files(o: &Output) -> Vec<Vec<u8>> {
    o.dex.iter().map(|d| d.1.clone()).collect()
}

/// The inverse of a renaming, keyed in *post*-rename terms.
fn inverse(r: &Renaming, before: &Model) -> Renaming {
    let s = &before.syms;
    let fwd_class = |d: &str| r.classes.get(d).cloned().unwrap_or_else(|| d.to_string());
    let fwd_type = |d: &str| {
        let dims = d.bytes().take_while(|&b| b == b'[').count();
        format!("{}{}", &d[..dims], fwd_class(&d[dims..]))
    };
    let fwd_proto = |p: &str| {
        let (ps, ret) = eightr_ir::types::parse_proto(p).unwrap();
        format!("({}){}", ps.iter().map(|t| fwd_type(t)).collect::<String>(), fwd_type(ret))
    };
    let mut inv = Renaming { classes: r.classes.iter().map(|(a, b)| (b.clone(), a.clone())).collect(), ..Default::default() };
    for c in &before.classes {
        let d = s.get(c.ty);
        for f in &c.fields {
            if let Some(new) = r.fields.get(&(d.to_string(), s.get(f.name).to_string(), s.get(f.ty).to_string())) {
                inv.fields.insert((fwd_class(d), new.clone(), fwd_type(s.get(f.ty))), s.get(f.name).to_string());
            }
        }
        for m in &c.methods {
            if let Some(new) = r.methods.get(&(d.to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string())) {
                inv.methods.insert((fwd_class(d), new.clone(), fwd_proto(s.get(m.proto))), s.get(m.name).to_string());
            }
        }
    }
    inv
}

#[test]
fn renaming_is_a_consistent_bijection() {
    for (name, bytes) in fixtures() {
        let (outcome, out) = undo_names(&bytes);
        let before = load(std::slice::from_ref(&bytes));
        let mut after = load(&out_files(&out));
        inverse(&outcome.renaming, &before).apply(&mut after);
        let (a, b) = (eightr_ir::print::program(&before), eightr_ir::print::program(&after));
        if a != b {
            let first = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
            let ctx = |t: &str| t.lines().skip(first.saturating_sub(3)).take(6).collect::<Vec<_>>().join("\n");
            panic!("{name}: applying the inverse renaming to the output doesn't restore the input (line {first}):\n--- input\n{}\n--- restored\n{}", ctx(&a), ctx(&b));
        }
    }
}

/// (class, method name, descriptor) for virtual methods, and the supertype closure.
fn overrides(m: &Model) -> BTreeSet<(String, String, String, String)> {
    let s = &m.syms;
    let by: BTreeMap<&str, &eightr_ir::model::Class> = m.classes.iter().map(|c| (s.get(c.ty), c)).collect();
    let mut out = BTreeSet::new();
    for c in &m.classes {
        // All program supertypes.
        let mut stack: Vec<&str> = c.superclass.iter().chain(&c.interfaces).map(|t| s.get(*t)).collect();
        let mut seen = BTreeSet::new();
        while let Some(t) = stack.pop() {
            if !seen.insert(t) {
                continue;
            }
            if let Some(sc) = by.get(t) {
                stack.extend(sc.superclass.iter().chain(&sc.interfaces).map(|x| s.get(*x)));
                for mm in &c.methods {
                    let virt = |a: u32| a & (access::STATIC | access::PRIVATE | access::CONSTRUCTOR) == 0;
                    if virt(mm.access) && sc.methods.iter().any(|x| virt(x.access) && x.name == mm.name && x.proto == mm.proto) {
                        out.insert((s.get(c.ty).to_string(), s.get(mm.name).to_string(), s.get(mm.proto).to_string(), t.to_string()));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn overrides_are_preserved_and_none_created() {
    for (name, bytes) in fixtures() {
        let (outcome, out) = undo_names(&bytes);
        let before = load(std::slice::from_ref(&bytes));
        let after = load(&out_files(&out));
        // Map the "before" override relation through the renaming and compare as sets.
        let inv = inverse(&outcome.renaming, &before);
        let mut after_in_before_terms = after.clone();
        inv.apply(&mut after_in_before_terms);
        assert_eq!(
            overrides(&before),
            overrides(&after_in_before_terms),
            "{name}: override relation changed"
        );
        // And directly: the output's override set has the same size (no merges/splits).
        assert_eq!(overrides(&before).len(), overrides(&after).len(), "{name}: override count changed");
    }
}

/// Renamed fields get program-wide unique names, so no reference can resolve to a different
/// (shadowing) field after renaming. (Methods are covered by the override test.)
#[test]
fn renamed_fields_are_unique_program_wide() {
    for (name, bytes) in fixtures() {
        let (outcome, _) = undo_names(&bytes);
        let mut seen = BTreeSet::new();
        for new in outcome.renaming.fields.values() {
            assert!(seen.insert(new.clone()), "{name}: field name {new} assigned twice");
        }
    }
}

#[test]
fn mapping_file_describes_the_renaming() {
    for (name, bytes) in fixtures() {
        let (_, out) = undo_names(&bytes);
        let before = load(std::slice::from_ref(&bytes));
        let after = load(&out_files(&out));
        let m = Mapping::parse(&out.mapping).unwrap_or_else(|e| panic!("{name}: {e}"));
        let desc = |dotted: &str| format!("L{};", dotted.replace('.', "/"));
        for c in &m.classes {
            // Left = recovered (in the output), right = current (in the input).
            let (new, old) = (desc(&c.original), desc(&c.obfuscated));
            assert!(after.find(&new).is_some(), "{name}: mapping names output class {new} that doesn't exist");
            let bi = before.find(&old).unwrap_or_else(|| panic!("{name}: mapping names input class {old} that doesn't exist"));
            let ai = after.find(&new).unwrap();
            for mm in &c.members {
                match &mm.kind {
                    MemberKind::Field(f) => {
                        assert!(before.classes[bi].fields.iter().any(|x| before.syms.get(x.name) == f.obfuscated), "{name}: {old} has no field {}", f.obfuscated);
                        assert!(after.classes[ai].fields.iter().any(|x| after.syms.get(x.name) == f.original_name), "{name}: {new} has no field {}", f.original_name);
                    }
                    MemberKind::Method(x) => {
                        assert!(before.classes[bi].methods.iter().any(|y| before.syms.get(y.name) == x.obfuscated), "{name}: {old} has no method {}", x.obfuscated);
                        assert!(after.classes[ai].methods.iter().any(|y| after.syms.get(y.name) == x.original_name), "{name}: {new} has no method {}", x.original_name);
                    }
                }
            }
        }
    }
}

#[test]
fn output_is_valid_dex() {
    for (name, bytes) in fixtures() {
        let (_, out) = undo(&bytes);
        for (file, b) in &out.dex {
            let dex = Dex::parse(b).unwrap_or_else(|e| panic!("{name}/{file}: {e}"));
            assert!(dex.checksum_ok() && dex.signature_ok(), "{name}/{file}");
            dex.validate().unwrap_or_else(|e| panic!("{name}/{file}: {e}"));
        }
    }
}

fn sdk_tool(rel: &str) -> Option<PathBuf> {
    let sdk = std::env::var_os("ANDROID_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Android/Sdk")))?;
    let mut versions: Vec<PathBuf> = fs::read_dir(sdk.join("build-tools")).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(rel)).find(|p| p.exists())
}

/// D8 re-dexes the output (a strict structural check of references and signatures).
#[test]
fn d8_accepts_output() {
    let Some(jar) = sdk_tool("lib/d8.jar") else {
        eprintln!("skipping: d8.jar not found");
        return;
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("output-d8");
    for (name, bytes) in fixtures().into_iter().filter(|(n, _)| n.ends_with("/r8")) {
        let (_, out) = undo(&bytes);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("out")).unwrap();
        let mut cmd = Command::new("java");
        cmd.arg("-cp").arg(&jar).args(["com.android.tools.r8.D8", "--min-api", "26", "--output"]).arg(dir.join("out"));
        for (file, b) in &out.dex {
            let p = dir.join(file);
            fs::write(&p, b).unwrap();
            cmd.arg(p);
        }
        let res = cmd.output().unwrap();
        assert!(res.status.success(), "{name}: D8 rejected 8R output:\n{}", String::from_utf8_lossy(&res.stderr));
    }
}

/// 8R(8R(x)) == 8R(x): structural names are recognized as canonical.
#[test]
fn undo_is_idempotent() {
    for (name, bytes) in fixtures() {
        let (_, once) = undo(&bytes);
        let files = out_files(&once);
        let inputs: Vec<DexInput> = files
            .iter()
            .enumerate()
            .map(|(i, b)| DexInput { name: if i == 0 { "classes.dex".into() } else { format!("classes{}.dex", i + 1) }, bytes: b.clone() })
            .collect();
        let outcome = run(&inputs, &Config::default()).unwrap();
        let twice = emit(&outcome).unwrap();
        assert!(out_files(&twice) == files, "{name}: running 8R on its own output changed it");
    }
}

/// End to end with a real decompiler: jadx loads 8R's mapping against the *original* R8 dex
/// and shows recovered names. Runs when jadx was fetched (`cargo xtask fetch jadx`).
#[test]
fn jadx_applies_mapping() {
    let jadx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/xtask-cache/jadx/bin/jadx");
    if !jadx.exists() {
        eprintln!("skipping: run `cargo xtask fetch jadx` to enable");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out/kotlin_basic/r8/classes.dex");
    let (_, out) = undo(&fs::read(&root).unwrap());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("jadx-check");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let map = dir.join("8r-mapping.txt");
    fs::write(&map, &out.mapping).unwrap();
    // 8R writes standard ProGuard orientation (recovered -> current); jadx reads it inverted.
    let res = Command::new(&jadx)
        .args(["-q", "-d"])
        .arg(dir.join("src"))
        .arg("--mappings-path")
        .arg(&map)
        .args(["-Prename-mappings.format=PROGUARD_FILE", "-Prename-mappings.invert=yes"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(res.status.success(), "jadx failed: {}", String::from_utf8_lossy(&res.stderr));
    let mut all = String::new();
    let mut stack = vec![dir.join("src")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { stack.push(p) } else { all.push_str(&fs::read_to_string(&p).unwrap_or_default()) }
        }
    }
    // The recovered lateinit name and structural class names appear in decompiled source.
    assert!(all.contains("public String db;"), "jadx output doesn't show the recovered field name");
    assert!(all.contains("class Class_"), "jadx output doesn't show structural class names");
}

/// Every method and field the output references on a program class resolves, through
/// superclasses and interfaces, to a member that exists (a library class ends the search
/// successfully). Catches rewrites that delete something still in use.
#[test]
fn every_program_reference_resolves() {
    fn resolves(p: &Model, class: &str, found: &dyn Fn(&eightr_ir::model::Class) -> bool, depth: u32) -> bool {
        let Some(i) = p.find(class) else { return true }; // library (or an array type)
        let c = &p.classes[i];
        if found(c) || depth > 64 {
            return true;
        }
        c.superclass.iter().chain(&c.interfaces).any(|s| resolves(p, p.syms.get(*s), found, depth + 1))
    }
    let mut checked = 0usize;
    for (name, bytes) in fixtures() {
        let (_, out) = undo(&bytes);
        let p = load(&out_files(&out));
        for c in &p.classes {
            for m in c.methods.iter().filter_map(|m| m.code.as_ref()) {
                for insn in &m.insns {
                    use eightr_ir::op::Op;
                    match &insn.op {
                        Op::Invoke { method, .. } => {
                            let ok = resolves(&p, p.syms.get(method.class), &|k| k.methods.iter().any(|x| x.name == method.name && x.proto == method.proto), 0);
                            assert!(ok, "{name}: {} calls missing {}->{}{}", p.syms.get(c.ty), p.syms.get(method.class), p.syms.get(method.name), p.syms.get(method.proto));
                            checked += 1;
                        }
                        Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. } | Op::StaticPut { field, .. } => {
                            let ok = resolves(&p, p.syms.get(field.class), &|k| k.fields.iter().any(|x| x.name == field.name && x.ty == field.ty), 0);
                            assert!(ok, "{name}: {} uses missing field {}->{}", p.syms.get(c.ty), p.syms.get(field.class), p.syms.get(field.name));
                            checked += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    assert!(checked > 1000, "only {checked} references checked");
}

