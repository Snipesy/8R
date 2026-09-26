//! Developer tasks. `cargo xtask fixtures` regenerates `fixtures/out/` from `fixtures/src/`.
//!
//! Each fixture directory holds Java *or* Kotlin sources plus a `fixture.conf`:
//!
//! ```text
//! min_api = 21
//! keep = -keep class com.example.Main { *; }     # one ProGuard rule per `keep` line
//! lib = kxs-core                                 # library from fixtures/toolchain.conf
//! plugin = kotlinx-serialization                 # kotlinc plugin (kotlinx-serialization, compose)
//! ```
//!
//! Libraries are program input to R8 (shrunk into the app, as in a real build) together with
//! their consumer keep rules, and classpath-only for the D8 ground truth. Kotlin fixtures
//! always get the kotlinc distribution's stdlib.
//!
//! For each fixture this produces:
//!   d8/classes.dex    D8 --debug build: ground truth ("G" in DESIGN.md §6.1)
//!   r8/classes.dex    R8 --release build: obfuscated input ("O")
//!   r8/mapping.txt    R8 mapping ("M")
//!   */dexdump.txt     `dexdump -d` output, the differential oracle for our dex reader
//!   BUILD.txt         tool versions, so a regenerated fixture's provenance is visible in diffs
//!
//! Outputs are checked in so the normal test suite needs no JDK or Android SDK.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

type Result<T> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fixtures") => fixtures(&args[1..]),
        Some("platform-api") => platform_api(),
        Some("fetch") => fetch_tools(&args[1..]),
        _ => Err("usage: cargo xtask fixtures [NAME...] | platform-api | fetch jadx".into()),
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
    libs: Vec<String>,
    plugins: Vec<String>,
    /// Toolchain artifact for D8/R8 (default: the SDK build-tools d8.jar).
    r8: Option<String>,
    /// Take sources from another fixture's directory (for twins built with another R8).
    sources: Option<String>,
}

fn parse_conf(path: &Path) -> Result<Conf> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut conf = Conf { min_api: 21, keep: Vec::new(), libs: Vec::new(), plugins: Vec::new(), r8: None, sources: None };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (k, v) = line.split_once('=').ok_or_else(|| format!("bad line in {}: {line}", path.display()))?;
        match k.trim() {
            "min_api" => conf.min_api = v.trim().parse().map_err(|_| format!("bad min_api: {v}"))?,
            "keep" => conf.keep.push(v.trim().to_string()),
            "lib" => conf.libs.push(v.trim().to_string()),
            "plugin" => conf.plugins.push(v.trim().to_string()),
            "r8" => conf.r8 = Some(v.trim().to_string()),
            "sources" => conf.sources = Some(v.trim().to_string()),
            other => return Err(format!("unknown key {other} in {}", path.display())),
        }
    }
    Ok(conf)
}

// ---- pinned artifacts ----

struct Artifact {
    sha256: String,
    url: String,
}

fn toolchain() -> Result<BTreeMap<String, Artifact>> {
    let path = root().join("fixtures/toolchain.conf");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [name, sha256, url] = parts[..] else { return Err(format!("bad toolchain line: {line}")) };
        out.insert(name.to_string(), Artifact { sha256: sha256.to_string(), url: url.to_string() });
    }
    Ok(out)
}

fn sha256_of(path: &Path) -> Result<String> {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .or_else(|_| Command::new("shasum").args(["-a", "256"]).arg(path).output())
        .map_err(|e| format!("sha256: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string())
}

fn cache_dir() -> PathBuf {
    root().join("target/xtask-cache")
}

/// Downloads (if needed) and verifies an artifact; returns its local path.
fn fetch(name: &str, tc: &BTreeMap<String, Artifact>) -> Result<PathBuf> {
    let a = tc.get(name).ok_or_else(|| format!("unknown artifact {name} (see fixtures/toolchain.conf)"))?;
    let file = cache_dir().join(a.url.rsplit('/').next().unwrap());
    fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    if !file.exists() || sha256_of(&file)? != a.sha256 {
        eprintln!("  fetching {}", a.url);
        run(Command::new("curl").args(["-sSLf", "-o"]).arg(&file).arg(&a.url))?;
    }
    let got = sha256_of(&file)?;
    if got != a.sha256 {
        let _ = fs::remove_file(&file);
        return Err(format!("{name}: sha256 mismatch (expected {}, got {got})", a.sha256));
    }
    Ok(file)
}

/// A prepared library: its class jar and the consumer rules R8 should see (as AGP passes them).
struct Lib {
    jar: PathBuf,
    rules: Vec<PathBuf>,
}

fn zip_entries(zip: &Path) -> Result<Vec<String>> {
    Ok(run(Command::new("unzip").arg("-Z1").arg(zip))?.lines().map(str::to_string).collect())
}

fn extract(zip: &Path, entry: &str, to: &Path) -> Result<()> {
    let out = Command::new("unzip").arg("-p").arg(zip).arg(entry).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("unzip {entry} from {}", zip.display()));
    }
    fs::write(to, out.stdout).map_err(|e| e.to_string())
}

fn prepare_lib(name: &str, tc: &BTreeMap<String, Artifact>) -> Result<Lib> {
    let file = fetch(name, tc)?;
    let dir = cache_dir().join(format!("lib-{name}"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let entries = zip_entries(&file)?;
    let (jar, rule_entries): (PathBuf, Vec<String>) = if file.extension().is_some_and(|e| e == "aar") {
        let jar = dir.join("classes.jar");
        extract(&file, "classes.jar", &jar)?;
        (jar, entries.into_iter().filter(|e| e == "proguard.txt").collect())
    } else {
        // AGP prefers R8-specific rules when a jar ships them.
        let r8: Vec<String> = entries.iter().filter(|e| e.starts_with("META-INF/com.android.tools/r8") && e.ends_with(".pro")).cloned().collect();
        let rules = if r8.is_empty() {
            entries.into_iter().filter(|e| e.starts_with("META-INF/proguard/") && e.ends_with(".pro")).collect()
        } else {
            r8
        };
        (file.clone(), rules)
    };
    let mut rules = Vec::new();
    for (i, e) in rule_entries.iter().enumerate() {
        let to = dir.join(format!("rules-{i}.pro"));
        extract(&file, e, &to)?;
        rules.push(to);
    }
    Ok(Lib { jar, rules })
}

/// Unpacks the pinned kotlinc distribution; returns its root (containing bin/ and lib/).
fn kotlinc_home(tc: &BTreeMap<String, Artifact>) -> Result<PathBuf> {
    let zip = fetch("kotlinc", tc)?;
    let dir = cache_dir().join(format!("kotlinc-{}", &tc["kotlinc"].sha256[..12]));
    if !dir.join("kotlinc/bin/kotlinc").exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        run(Command::new("unzip").args(["-q", "-o"]).arg(&zip).arg("-d").arg(&dir))?;
    }
    Ok(dir.join("kotlinc"))
}

fn classpath(paths: &[PathBuf]) -> std::ffi::OsString {
    std::env::join_paths(paths).expect("valid paths")
}

fn fixtures(only: &[String]) -> Result<()> {
    let tools = find_tools()?;
    let root = root();
    let src_root = root.join("fixtures/src");
    let out_root = root.join("fixtures/out");
    let work_root = root.join("target/xtask-fixtures");
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
        let src = match &conf.sources {
            Some(other) => src_root.join(other),
            None => src,
        };
        let work = work_root.join(&name);
        let out = out_root.join(&name);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&out);
        for d in [work.join("classes"), out.join("d8"), out.join("r8")] {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        }

        let tc = toolchain()?;
        let r8_jar = match &conf.r8 {
            Some(a) => fetch(a, &tc)?,
            None => tools.r8_jar.clone(),
        };
        let r8_version = run(Command::new(&tools.java).arg("-cp").arg(&r8_jar).args(["com.android.tools.r8.R8", "--version"]))?;
        let mut libs: Vec<Lib> = conf.libs.iter().map(|l| prepare_lib(l, &tc)).collect::<Result<_>>()?;
        let mut java_sources = Vec::new();
        files_with_ext(&src, "java", &mut java_sources)?;
        let mut kt_sources = Vec::new();
        files_with_ext(&src, "kt", &mut kt_sources)?;
        let mut tool_versions = String::new();
        if !kt_sources.is_empty() {
            let home = kotlinc_home(&tc)?;
            libs.insert(0, Lib { jar: home.join("lib/kotlin-stdlib.jar"), rules: Vec::new() });
            let lib_jars: Vec<PathBuf> = libs.iter().map(|l| l.jar.clone()).collect();
            let mut cmd = Command::new(home.join("bin/kotlinc"));
            cmd.args(["-jvm-target", "11", "-no-reflect", "-module-name", &name, "-d"]).arg(work.join("classes"));
            cmd.arg("-cp").arg(classpath(&lib_jars));
            for p in &conf.plugins {
                let jar = match p.as_str() {
                    "kotlinx-serialization" => "kotlinx-serialization-compiler-plugin.jar",
                    "compose" => "compose-compiler-plugin.jar",
                    other => return Err(format!("unknown plugin {other}")),
                };
                let mut arg = std::ffi::OsString::from("-Xplugin=");
                arg.push(home.join("lib").join(jar));
                cmd.arg(arg);
            }
            cmd.args(&kt_sources);
            run(&mut cmd)?;
            tool_versions.push_str(&format!("kotlinc: {}\n", fs::read_to_string(home.join("build.txt")).unwrap_or_default().trim()));
        }
        if !java_sources.is_empty() {
            let mut cmd = Command::new(&tools.javac);
            cmd.args(["--release", "11", "-g", "-encoding", "UTF-8", "-d"]).arg(work.join("classes"));
            if !kt_sources.is_empty() || !libs.is_empty() {
                let mut cp: Vec<PathBuf> = vec![work.join("classes")];
                cp.extend(libs.iter().map(|l| l.jar.clone()));
                cmd.arg("-cp").arg(classpath(&cp));
            }
            run(cmd.args(&java_sources))?;
        }
        for l in &conf.libs {
            tool_versions.push_str(&format!("lib: {l} {}\n", tc[l].url));
        }
        let mut classes = Vec::new();
        files_with_ext(&work.join("classes"), "class", &mut classes)?;
        classes.retain(|c| !c.ends_with("module-info.class"));

        let min_api = conf.min_api.to_string();
        // Ground truth: the app's own classes only; libraries are classpath.
        let mut d8 = Command::new(&tools.java);
        d8.arg("-cp").arg(&r8_jar).args(["com.android.tools.r8.D8", "--debug", "--min-api", &min_api, "--lib"]).arg(&tools.android_jar);
        for l in &libs {
            d8.arg("--classpath").arg(&l.jar);
        }
        run(d8.arg("--output").arg(out.join("d8")).args(&classes))?;

        let rules = work.join("rules.pro");
        fs::write(&rules, conf.keep.join("\n") + "\n").map_err(|e| e.to_string())?;
        // Release build: libraries are program input, with their consumer rules, as in AGP.
        let mut r8 = Command::new(&tools.java);
        r8.arg("-cp").arg(&r8_jar).args(["com.android.tools.r8.R8", "--release", "--min-api", &min_api, "--lib"]).arg(&tools.android_jar);
        r8.arg("--pg-conf").arg(&rules);
        for l in &libs {
            for r in &l.rules {
                r8.arg("--pg-conf").arg(r);
            }
        }
        r8.arg("--pg-map-output").arg(out.join("r8/mapping.txt")).arg("--output").arg(out.join("r8")).args(&classes);
        for l in &libs {
            r8.arg(&l.jar);
        }
        run(&mut r8)?;

        for variant in ["d8", "r8"] {
            // Large outputs (libraries shrunk in) would make huge dumps; the differential test
            // covers the small ones, and skips fixtures without a dump.
            let dex = out.join(variant).join("classes.dex");
            if fs::metadata(&dex).map(|m| m.len()).unwrap_or(0) > 100 * 1024 {
                continue;
            }
            // Run from the variant dir with a relative path so the output has no absolute paths.
            let dump = run(Command::new(&tools.dexdump).current_dir(out.join(variant)).args(["-d", "classes.dex"]))?;
            fs::write(out.join(variant).join("dexdump.txt"), dump).map_err(|e| e.to_string())?;
        }

        let build = format!(
            "# Generated by `cargo xtask fixtures`. Do not edit.\nr8: {}\njavac: {}\n{tool_versions}min_api: {}\n",
            r8_version.trim(),
            javac_version.trim(),
            conf.min_api
        );
        fs::write(out.join("BUILD.txt"), build).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- platform API table ----

/// Reads a class file's name and its overridable methods: (class descriptor, [(name, descriptor)]).
fn parse_class(b: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let u16at = |i: usize| -> Option<usize> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize) };
    let u32at = |i: usize| -> Option<usize> { Some(u32::from_be_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]) as usize) };
    if u32at(0)? != 0xCAFE_BABE {
        return None;
    }
    let count = u16at(8)?;
    let mut utf8: Vec<Option<String>> = vec![None; count];
    let mut class_name: Vec<usize> = vec![0; count];
    let mut i = 10;
    let mut k = 1;
    while k < count {
        let tag = *b.get(i)?;
        i += 1;
        match tag {
            1 => {
                let len = u16at(i)?;
                utf8[k] = Some(String::from_utf8_lossy(b.get(i + 2..i + 2 + len)?).into_owned());
                i += 2 + len;
            }
            7 => {
                class_name[k] = u16at(i)?;
                i += 2;
            }
            8 | 16 | 19 | 20 => i += 2,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => i += 4,
            5 | 6 => {
                i += 8;
                k += 1;
            }
            15 => i += 3,
            _ => return None,
        }
        k += 1;
    }
    let access = u16at(i)?;
    let this = class_name[u16at(i + 2)?];
    let name = utf8.get(this)?.clone()?;
    let _ = access;
    let n_if = u16at(i + 6)?;
    i += 8 + 2 * n_if;
    let skip_members = |mut i: usize, keep: &mut dyn FnMut(usize, usize, usize)| -> Option<usize> {
        let n = u16at(i)?;
        i += 2;
        for _ in 0..n {
            keep(u16at(i)?, u16at(i + 2)?, u16at(i + 4)?);
            let attrs = u16at(i + 6)?;
            i += 8;
            for _ in 0..attrs {
                i += 6 + u32at(i + 2)?;
            }
        }
        Some(i)
    };
    i = skip_members(i, &mut |_, _, _| {})?;
    let mut methods = Vec::new();
    skip_members(i, &mut |acc, n, d| {
        const PRIVATE: usize = 0x2;
        const STATIC: usize = 0x8;
        const FINAL: usize = 0x10;
        if acc & (PRIVATE | STATIC | FINAL) == 0 {
            if let (Some(Some(n)), Some(Some(d))) = (utf8.get(n), utf8.get(d)) {
                methods.push((n.clone(), d.clone()));
            }
        }
    })?;
    Some((format!("L{name};"), methods))
}

/// Generates crates/eightr-core/data/platform-api.txt from the newest android.jar: every
/// platform class, and every overridable platform method whose name has the shape of an
/// R8-generated name (only those can collide with minified names).
fn platform_api() -> Result<()> {
    let tools = find_tools()?;
    let work = root().join("target/xtask-platform-api");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    run(Command::new("unzip").args(["-q", "-o"]).arg(&tools.android_jar).arg("-d").arg(&work))?;
    let mut files = Vec::new();
    files_with_ext(&work, "class", &mut files)?;
    let mut classes = std::collections::BTreeSet::new();
    let mut methods = std::collections::BTreeSet::new();
    let short = |n: &str| {
        let b = n.as_bytes();
        (1..=6).contains(&b.len()) && b[0].is_ascii_alphabetic() && b[1..].iter().all(u8::is_ascii_alphanumeric)
    };
    for f in &files {
        let bytes = fs::read(f).map_err(|e| e.to_string())?;
        let Some((class, ms)) = parse_class(&bytes) else { continue };
        classes.insert(class);
        for (n, d) in ms {
            if short(&n) {
                methods.insert(format!("{n} {d}"));
            }
        }
    }
    let platform = tools.android_jar.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = format!(
        "# Generated by `cargo xtask platform-api` from {platform}/android.jar. Do not edit.\n# {} classes, {} short overridable methods\n[methods]\n",
        classes.len(),
        methods.len()
    );
    for m in &methods {
        out.push_str(m);
        out.push('\n');
    }
    out.push_str("[classes]\n");
    for c in &classes {
        out.push_str(c);
        out.push('\n');
    }
    let dest = root().join("crates/eightr-core/data/platform-api.txt");
    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&dest, out).map_err(|e| e.to_string())?;
    eprintln!("wrote {} ({} classes, {} short methods)", dest.display(), classes.len(), methods.len());
    Ok(())
}

/// `cargo xtask fetch jadx`: downloads and unpacks pinned external tools used by optional
/// end-to-end tests (target/xtask-cache/<name>/).
fn fetch_tools(names: &[String]) -> Result<()> {
    let tc = toolchain()?;
    for name in names {
        let zip = fetch(name, &tc)?;
        let dir = cache_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        run(Command::new("unzip").args(["-q", "-o"]).arg(&zip).arg("-d").arg(&dir))?;
        eprintln!("{name}: {}", dir.display());
    }
    Ok(())
}
