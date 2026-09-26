//! Execution equivalence (DESIGN.md §0.2.1: semantics are preserved). Each fixture's `main`
//! runs on the original R8 build and on 8R's output (with rewrites off and on); stdout and
//! the exit code must match exactly.
//!
//! Backends:
//! * **JVM via dex2jar** (`cargo xtask fetch dex2jar`): dex → jar → class version 49 (no stack
//!   maps needed), run with verification relaxed. dex2jar can't express some dex idioms R8
//!   emits (e.g. `new-instance Sub` + `invoke-direct Super.<init>` from constructor
//!   inlining), so a fixture whose *unmodified* build doesn't run on a backend is skipped for
//!   that backend, with the reason printed.
//! * **ART via adb** (set `EIGHTR_ADB` to an adb binary with a device/emulator attached):
//!   `dalvikvm64` on the device. Authoritative; used when available.
//!
//! Fixture mains never print class names, so renaming alone can't change their output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eightr_core::input::DexInput;
use eightr_core::output::emit;
use eightr_core::{run, Config};

const ARGS: [&str; 2] = ["a", "b"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// (fixture, main class) for fixtures with an entry point.
fn fixtures_with_main() -> Vec<(String, String)> {
    let src = root().join("fixtures/src");
    let mut v = Vec::new();
    for e in fs::read_dir(&src).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        let conf = fs::read_to_string(src.join(&name).join("fixture.conf")).unwrap_or_default();
        if let Some(main) = conf.lines().find_map(|l| l.strip_prefix("main").map(|r| r.trim_start_matches([' ', '=']).trim().to_string())) {
            if root().join("fixtures/out").join(&name).join("r8/classes.dex").exists() {
                v.push((name, main));
            }
        }
    }
    v.sort();
    v
}

#[derive(Debug, PartialEq, Eq)]
struct Run {
    stdout: String,
    exit: Option<i32>,
    stderr: String,
}

impl Run {
    /// The backend itself couldn't load the program (not a behavior difference).
    fn backend_failure(&self) -> Option<&str> {
        ["java.lang.VerifyError", "java.lang.InstantiationError", "java.lang.ClassFormatError", "java.lang.NoClassDefFoundError", "Could not find or load main class"]
            .into_iter()
            .find(|m| self.stderr.contains(m))
    }
}

trait Backend {
    fn name(&self) -> &'static str;
    fn run(&self, dex: &[Vec<u8>], main: &str, work: &Path) -> Run;
}

struct Jvm {
    tools: PathBuf,
}

impl Backend for Jvm {
    fn name(&self) -> &'static str {
        "jvm(dex2jar)"
    }

    fn run(&self, dex: &[Vec<u8>], main: &str, work: &Path) -> Run {
        let _ = fs::remove_dir_all(work);
        fs::create_dir_all(work).unwrap();
        let mut jars = Vec::new();
        for (i, bytes) in dex.iter().enumerate() {
            let d = work.join(format!("classes{i}.dex"));
            let raw = work.join(format!("classes{i}.jar"));
            let jar = work.join(format!("classes{i}-49.jar"));
            fs::write(&d, bytes).unwrap();
            let ok = Command::new("sh").arg(self.tools.join("d2j-dex2jar.sh")).args(["-f", "-o"]).arg(&raw).arg(&d).output().unwrap();
            assert!(ok.status.success(), "dex2jar failed: {}", String::from_utf8_lossy(&ok.stderr));
            // Class version 49 is verified by the old inference verifier (no stack maps).
            let ok = Command::new("sh").arg(self.tools.join("d2j-class-version-switch.sh")).arg("5").arg(&raw).arg(&jar).output().unwrap();
            assert!(ok.status.success(), "class-version-switch failed: {}", String::from_utf8_lossy(&ok.stderr));
            jars.push(jar);
        }
        let cp = std::env::join_paths(&jars).unwrap();
        let out = Command::new("java")
            // Dex typing is laxer than the JVM's (relaxed field types, protected access);
            // behavior, not verification, is what's compared.
            .args(["-XX:+UnlockDiagnosticVMOptions", "-XX:-BytecodeVerificationRemote", "-cp"])
            .arg(cp)
            .arg(main)
            .args(ARGS)
            .output()
            .unwrap();
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            exit: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

struct Art {
    adb: PathBuf,
}

impl Backend for Art {
    fn name(&self) -> &'static str {
        "art(adb)"
    }

    fn run(&self, dex: &[Vec<u8>], main: &str, work: &Path) -> Run {
        let _ = fs::remove_dir_all(work);
        fs::create_dir_all(work).unwrap();
        let mut remote = Vec::new();
        for (i, bytes) in dex.iter().enumerate() {
            let d = work.join(format!("classes{i}.dex"));
            fs::write(&d, bytes).unwrap();
            let r = format!("/data/local/tmp/8r-exec-{i}.dex");
            let ok = Command::new(&self.adb).arg("push").arg(&d).arg(&r).output().unwrap();
            assert!(ok.status.success(), "adb push failed: {}", String::from_utf8_lossy(&ok.stderr));
            remote.push(r);
        }
        let shell = format!("dalvikvm64 -cp {} {main} {}; echo \"__exit=$?\"", remote.join(":"), ARGS.join(" "));
        let out = Command::new(&self.adb).args(["shell", &shell]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        let (stdout, code) = match text.rsplit_once("__exit=") {
            Some((s, c)) => (s.to_string(), c.trim().parse().ok()),
            None => (text.clone(), None),
        };
        Run { stdout, exit: code, stderr: String::from_utf8_lossy(&out.stderr).into_owned() }
    }
}

fn backends() -> Vec<Box<dyn Backend>> {
    let mut v: Vec<Box<dyn Backend>> = Vec::new();
    let tools = root().join("target/xtask-cache/dex2jar/dex-tools-2.4.38");
    if tools.join("d2j-dex2jar.sh").exists() && Command::new("java").arg("-version").output().is_ok() {
        v.push(Box::new(Jvm { tools }));
    } else {
        eprintln!("jvm backend unavailable: run `cargo xtask fetch dex2jar`");
    }
    if let Some(adb) = std::env::var_os("EIGHTR_ADB").map(PathBuf::from) {
        v.push(Box::new(Art { adb }));
    }
    v
}

fn eightr_output(input: &[u8], no_rewrites: bool) -> Vec<Vec<u8>> {
    let outcome = run(&[DexInput { name: "classes.dex".into(), bytes: input.to_vec() }], &Config { no_rewrites, ..Default::default() }).unwrap();
    emit(&outcome).unwrap().dex.into_iter().map(|(_, b)| b).collect()
}

#[test]
fn outputs_behave_like_inputs() {
    let backends = backends();
    if backends.is_empty() {
        eprintln!("skipping: no execution backend");
        return;
    }
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("exec");
    let mut compared = 0;
    for backend in &backends {
        for (fixture, main) in fixtures_with_main() {
            let input = fs::read(root().join("fixtures/out").join(&fixture).join("r8/classes.dex")).unwrap();
            let base = backend.run(std::slice::from_ref(&input), &main, &work.join("base"));
            if let Some(why) = base.backend_failure() {
                eprintln!("[{}] {fixture}: skipped, backend can't run the unmodified build ({why})", backend.name());
                continue;
            }
            assert!(!base.stdout.is_empty(), "[{}] {fixture}: baseline printed nothing", backend.name());
            // A fixture whose own output varies between runs (timing, identity hashes) can't be
            // compared; fail loudly so the fixture gets fixed.
            let again = backend.run(std::slice::from_ref(&input), &main, &work.join("base2"));
            assert!(again.stdout == base.stdout, "[{}] {fixture}: fixture output is nondeterministic\n{}\n---\n{}", backend.name(), base.stdout, again.stdout);
            for no_rewrites in [true, false] {
                let out = eightr_output(&input, no_rewrites);
                let got = backend.run(&out, &main, &work.join("out"));
                let mode = if no_rewrites { "names only" } else { "with rewrites" };
                assert!(
                    got.stdout == base.stdout && got.exit == base.exit,
                    "[{}] {fixture} ({mode}): behavior changed\n--- input (exit {:?})\n{}\n--- 8R output (exit {:?})\n{}\n--- stderr\n{}",
                    backend.name(),
                    base.exit,
                    base.stdout,
                    got.exit,
                    got.stdout,
                    got.stderr
                );
                compared += 1;
            }
        }
    }
    eprintln!("compared {compared} runs");
    assert!(compared >= 8, "too few fixtures actually executed ({compared})");
}
