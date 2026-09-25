//! Developer tasks. `cargo xtask fixtures` regenerates `fixtures/out/` from `fixtures/src/`.
//!
//! Each fixture directory holds Java sources plus a `fixture.conf`:
//!
//! ```text
//! min_api = 21
//! keep = -keep class com.example.Main { *; }     # one ProGuard rule per `keep` line
//! ```
//!
//! For each fixture this produces:
//!   d8/classes.dex    D8 --debug build: ground truth ("G" in DESIGN.md §6.1)
//!   r8/classes.dex    R8 --release build: obfuscated input ("O")
//!   r8/mapping.txt    R8 mapping ("M")
//!   */dexdump.txt     `dexdump -d` output, the differential oracle for our dex reader
//!   BUILD.txt         tool versions, so a regenerated fixture's provenance is visible in diffs
//!
//! Outputs are checked in so the normal test suite needs no JDK or Android SDK.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

type Result<T> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fixtures") => fixtures(&args[1..]),
        _ => Err("usage: cargo xtask fixtures [NAME...]".into()),
    };
    if let Err(e) = result {
        eprintln!("xtask: {e}");
        exit(1);
    }
}

struct Tools {
    java: PathBuf,
    javac: PathBuf,
    r8_jar: PathBuf,
    dexdump: PathBuf,
    android_jar: PathBuf,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Highest-sorting subdirectory, comparing dotted numeric versions numerically.
fn newest_subdir(dir: &Path) -> Result<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    let key = |p: &PathBuf| -> Vec<u64> {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        name.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect()
    };
    entries.sort_by_key(key);
    entries.pop().ok_or_else(|| format!("no subdirectories in {}", dir.display()))
}

fn find_tools() -> Result<Tools> {
    let sdk = env::var_os("ANDROID_HOME")
        .or_else(|| env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Android/Sdk")))
        .ok_or("set ANDROID_HOME")?;
    let build_tools = match env::var_os("EIGHTR_BUILD_TOOLS") {
        Some(p) => PathBuf::from(p),
        None => newest_subdir(&sdk.join("build-tools"))?,
    };
    let r8_jar = env::var_os("EIGHTR_R8_JAR").map(PathBuf::from).unwrap_or_else(|| build_tools.join("lib/d8.jar"));
    let tools = Tools {
        java: PathBuf::from("java"),
        javac: PathBuf::from("javac"),
        r8_jar,
        dexdump: build_tools.join("dexdump"),
        android_jar: newest_subdir(&sdk.join("platforms"))?.join("android.jar"),
    };
    for p in [&tools.r8_jar, &tools.dexdump, &tools.android_jar] {
        if !p.exists() {
            return Err(format!("missing {}", p.display()));
        }
    }
    Ok(tools)
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?} failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(s)
}

fn files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            files_with_ext(&p, ext, out)?;
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
    out.sort();
    Ok(())
}

struct Conf {
    min_api: u32,
    keep: Vec<String>,
}

fn parse_conf(path: &Path) -> Result<Conf> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut conf = Conf { min_api: 21, keep: Vec::new() };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (k, v) = line.split_once('=').ok_or_else(|| format!("bad line in {}: {line}", path.display()))?;
        match k.trim() {
            "min_api" => conf.min_api = v.trim().parse().map_err(|_| format!("bad min_api: {v}"))?,
            "keep" => conf.keep.push(v.trim().to_string()),
            other => return Err(format!("unknown key {other} in {}", path.display())),
        }
    }
    Ok(conf)
}

fn fixtures(only: &[String]) -> Result<()> {
    let tools = find_tools()?;
    let root = root();
    let src_root = root.join("fixtures/src");
    let out_root = root.join("fixtures/out");
    let work_root = root.join("target/xtask-fixtures");
    let r8_version = run(Command::new(&tools.java).arg("-cp").arg(&tools.r8_jar).args(["com.android.tools.r8.R8", "--version"]))?;
    let javac_version = run(Command::new(&tools.javac).arg("-version"))?;

    let mut names: Vec<String> = fs::read_dir(&src_root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if !only.is_empty() {
        names.retain(|n| only.contains(n));
    }

    for name in names {
        eprintln!("fixture {name}");
        let src = src_root.join(&name);
        let conf = parse_conf(&src.join("fixture.conf"))?;
        let work = work_root.join(&name);
        let out = out_root.join(&name);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&out);
        for d in [work.join("classes"), out.join("d8"), out.join("r8")] {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        }

        let mut sources = Vec::new();
        files_with_ext(&src, "java", &mut sources)?;
        run(Command::new(&tools.javac)
            .args(["--release", "11", "-g", "-encoding", "UTF-8", "-d"])
            .arg(work.join("classes"))
            .args(&sources))?;
        let mut classes = Vec::new();
        files_with_ext(&work.join("classes"), "class", &mut classes)?;

        let min_api = conf.min_api.to_string();
        run(Command::new(&tools.java)
            .arg("-cp")
            .arg(&tools.r8_jar)
            .args(["com.android.tools.r8.D8", "--debug", "--min-api", &min_api, "--lib"])
            .arg(&tools.android_jar)
            .arg("--output")
            .arg(out.join("d8"))
            .args(&classes))?;

        let rules = work.join("rules.pro");
        fs::write(&rules, conf.keep.join("\n") + "\n").map_err(|e| e.to_string())?;
        run(Command::new(&tools.java)
            .arg("-cp")
            .arg(&tools.r8_jar)
            .args(["com.android.tools.r8.R8", "--release", "--min-api", &min_api, "--lib"])
            .arg(&tools.android_jar)
            .arg("--pg-conf")
            .arg(&rules)
            .arg("--pg-map-output")
            .arg(out.join("r8/mapping.txt"))
            .arg("--output")
            .arg(out.join("r8"))
            .args(&classes))?;

        for variant in ["d8", "r8"] {
            // Run from the variant dir with a relative path so the output has no absolute paths.
            let dump = run(Command::new(&tools.dexdump).current_dir(out.join(variant)).args(["-d", "classes.dex"]))?;
            fs::write(out.join(variant).join("dexdump.txt"), dump).map_err(|e| e.to_string())?;
        }

        let build = format!(
            "# Generated by `cargo xtask fixtures`. Do not edit.\nr8: {}\njavac: {}\nmin_api: {}\n",
            r8_version.trim(),
            javac_version.trim(),
            conf.min_api
        );
        fs::write(out.join("BUILD.txt"), build).map_err(|e| e.to_string())?;
    }
    Ok(())
}
