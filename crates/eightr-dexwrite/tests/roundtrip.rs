use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eightr_dex::Dex;
use eightr_dexwrite::{write_all, write_split};
use eightr_ir::model::Program;
use eightr_ir::print;

fn fixture_dexes() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut v: Vec<PathBuf> = fs::read_dir(root)
        .unwrap()
        .flat_map(|f| ["d8", "r8"].map(|x| f.as_ref().unwrap().path().join(x).join("classes.dex")))
        .filter(|p| p.exists())
        .collect();
    v.sort();
    v
}

fn load(bytes: &[u8]) -> Program {
    let dex = Dex::parse(bytes).unwrap();
    Program::load(&[&dex]).unwrap()
}

fn load_many(files: &[Vec<u8>]) -> Program {
    let dexes: Vec<Dex> = files.iter().map(|b| Dex::parse(b).unwrap()).collect();
    let refs: Vec<&Dex> = dexes.iter().collect();
    Program::load(&refs).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dexwrite");
    fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn round_trip_preserves_program() {
    for path in fixture_dexes() {
        let original = load(&fs::read(&path).unwrap());
        let written = write_all(&original).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let reread = load(&written);
        let (a, b) = (print::program(&original), print::program(&reread));
        if a != b {
            let first = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
            let ctx: Vec<_> = a.lines().zip(b.lines()).skip(first.saturating_sub(3)).take(8).collect();
            panic!("{}: program changed on round trip near line {first}:\n{ctx:#?}", path.display());
        }
    }
}

#[test]
fn output_is_valid_and_canonical() {
    for path in fixture_dexes() {
        let original = load(&fs::read(&path).unwrap());
        let once = write_all(&original).unwrap();
        let dex = Dex::parse(&once).unwrap();
        assert!(dex.checksum_ok() && dex.signature_ok(), "{}", path.display());
        dex.validate().unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // Canonical: writing what we read back gives the same bytes.
        let twice = write_all(&load(&once)).unwrap();
        assert!(once == twice, "{}: writer is not idempotent", path.display());
    }
}

#[test]
fn output_independent_of_model_order() {
    for path in fixture_dexes() {
        let mut p = load(&fs::read(&path).unwrap());
        let a = write_all(&p).unwrap();
        // Reverse every member list and the class list: nothing semantic changes. (Interface
        // order is semantic and stays.)
        for c in &mut p.classes {
            c.fields.reverse();
            c.methods.reverse();
        }
        p.classes.reverse();
        let all: Vec<usize> = (0..p.classes.len()).collect();
        let b = eightr_dexwrite::write(&p, &all).unwrap();
        assert!(a == b, "{}: output depends on model order", path.display());
    }
}

#[test]
fn multidex_split_preserves_program() {
    for path in fixture_dexes() {
        let original = load(&fs::read(&path).unwrap());
        if original.classes.len() < 2 {
            continue;
        }
        let files = write_split(&original, 2, |i| i % 2).unwrap();
        let reread = load_many(&files);
        assert_eq!(print::program(&original), print::program(&reread), "{}", path.display());
    }
}

fn sdk_tool(rel: &str) -> Option<PathBuf> {
    let sdk = std::env::var_os("ANDROID_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Android/Sdk")))?;
    let bt = sdk.join("build-tools");
    let mut versions: Vec<PathBuf> = fs::read_dir(&bt).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(rel)).find(|p| p.exists())
}

/// AOSP's own dex parser must accept everything we write.
#[test]
fn dexdump_accepts_output() {
    let Some(dexdump) = sdk_tool("dexdump") else {
        eprintln!("skipping: dexdump not found (set ANDROID_HOME)");
        return;
    };
    for path in fixture_dexes() {
        let written = write_all(&load(&fs::read(&path).unwrap())).unwrap();
        let out_path = scratch("dexdump_check.dex");
        fs::write(&out_path, &written).unwrap();
        let out = Command::new(&dexdump).args(["-d"]).arg(&out_path).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && !stderr.contains("Failure") && !stderr.contains("ERROR"),
            "{}: dexdump rejected output:\n{stderr}", path.display());
    }
}

/// D8 reads dex input strictly; re-dexing our output must succeed.
#[test]
fn d8_accepts_output() {
    let Some(jar) = sdk_tool("lib/d8.jar") else {
        eprintln!("skipping: d8.jar not found");
        return;
    };
    if Command::new("java").arg("-version").output().is_err() {
        eprintln!("skipping: java not found");
        return;
    }
    for path in fixture_dexes() {
        let written = write_all(&load(&fs::read(&path).unwrap())).unwrap();
        let input = scratch("d8_check_in.dex");
        let outdir = scratch("d8_check_out");
        let _ = fs::remove_dir_all(&outdir);
        fs::create_dir_all(&outdir).unwrap();
        fs::write(&input, &written).unwrap();
        let out = Command::new("java")
            .arg("-cp")
            .arg(&jar)
            .args(["com.android.tools.r8.D8", "--min-api", "26", "--output"])
            .arg(&outdir)
            .arg(&input)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}: D8 rejected output:\n{}", path.display(), String::from_utf8_lossy(&out.stderr));
    }
}
