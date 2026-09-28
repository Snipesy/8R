//! Scenario generators, driven only by the public API (docs/research/libdb.md §6):
//!
//! * **roots**: `-keep,allowobfuscation,allowoptimization` rules on a hash-sample of public
//!   methods. No consumer code.
//! * **callers**: Java code calling a hash-sample, each call in its own `try`, arguments from
//!   `gen.O`'s `native` methods (nothing for R8 to infer). javac compiles it against the closure;
//!   calls javac rejects are dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::api::{Entry, ACC_ABSTRACT, ACC_INTERFACE, ACC_PUBLIC, ACC_STATIC};
use crate::tools::{mkdirs, read, write, Result, Tools};

/// Whether the sample `seed` at fraction `frac` takes `e`.
pub fn sampled(e: &Entry, frac: f64, seed: u64) -> bool {
    let h = Sha256::digest(format!("{seed}|{}|{}|{}", e.class, e.name, e.desc).as_bytes());
    let v = u64::from_le_bytes(h[..8].try_into().expect("8 bytes"));
    ((v % 1_000_000) as f64) < frac * 1_000_000.0
}

pub fn java_name(desc: &str) -> String {
    let dims = desc.bytes().take_while(|&b| b == b'[').count();
    let base = match &desc[dims..] {
        "V" => "void".to_string(),
        "Z" => "boolean".into(),
        "B" => "byte".into(),
        "C" => "char".into(),
        "S" => "short".into(),
        "I" => "int".into(),
        "J" => "long".into(),
        "F" => "float".into(),
        "D" => "double".into(),
        t => t.trim_start_matches('L').trim_end_matches(';').replace('/', "."),
    };
    format!("{base}{}", "[]".repeat(dims))
}

/// Keep rules for the sampled entries, grouped by class.
pub fn roots(entries: &[&Entry]) -> String {
    let mut by_class: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for e in entries {
        let Some((ps, r)) = eightr_ir::types::parse_proto(&e.desc) else { continue };
        let ps: Vec<String> = ps.iter().map(|t| java_name(t)).collect();
        let head = if e.name == "<init>" { "<init>".to_string() } else { format!("{} {}", java_name(r), e.name) };
        by_class.entry(&e.class).or_default().push(format!("    {head}({});", ps.join(",")));
    }
    let mut out = String::new();
    for (c, ms) in by_class {
        out.push_str(&format!("-keep,allowobfuscation,allowoptimization class {} {{\n{}\n}}\n", java_name(c), ms.join("\n")));
    }
    out
}

const PRIMS: &[(&str, &str, &str)] = &[("Z", "boolean", "z"), ("B", "byte", "b"), ("C", "char", "c"), ("S", "short", "s"), ("I", "int", "i"), ("J", "long", "j"), ("F", "float", "f"), ("D", "double", "d")];

fn source_name(desc: &str) -> String {
    // Nested classes: `a.B$C` is `a.B.C` in source.
    java_name(desc).replace('$', ".")
}

fn arg(t: &str) -> String {
    match PRIMS.iter().find(|p| p.0 == t) {
        Some(p) => format!("O.{}()", p.2),
        None => format!("(({}) O.a())", source_name(t)),
    }
}

/// Java's reserved words: legal JVM names (Kotlin allows them), not Java identifiers.
const JAVA_KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue", "default", "do", "double", "else", "enum", "extends", "final",
    "finally", "float", "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native", "new", "package", "private", "protected", "public",
    "return", "short", "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void", "volatile", "while", "true", "false", "null",
    "_",
];

/// Whether an entry can be written as Java source at all.
fn expressible(e: &Entry) -> bool {
    let c = &e.class;
    let keyword = |n: &str| JAVA_KEYWORDS.contains(&n);
    if keyword(&e.name) || c.trim_start_matches('L').trim_end_matches(';').split(['/', '$']).any(keyword) {
        return false;
    }
    let bad_class = c.contains("$$") || c.contains('-') || c.split('$').skip(1).any(|p| p.starts_with(|x: char| x.is_ascii_digit()));
    !bad_class && !e.name.contains('$') && !e.name.contains('-') && e.access & ACC_PUBLIC != 0
}

/// One Java statement calling `e`, or `None`.
fn call(e: &Entry) -> Option<String> {
    if !expressible(e) {
        return None;
    }
    let (ps, _) = eightr_ir::types::parse_proto(&e.desc)?;
    let args = ps.iter().map(|t| arg(t)).collect::<Vec<_>>().join(", ");
    let cls = source_name(&e.class);
    Some(if e.name == "<init>" {
        if e.class_access & (ACC_INTERFACE | ACC_ABSTRACT) != 0 {
            return None;
        }
        format!("new {cls}({args});")
    } else if e.access & ACC_STATIC != 0 {
        format!("{cls}.{}({args});", e.name)
    } else {
        format!("(({cls}) O.a()).{}({args});", e.name)
    })
}

const CALLS_PER_CLASS: usize = 40;

fn opaque_source() -> String {
    let mut s = String::from("package gen;\n\n/** Opaque values: native, so R8 can infer nothing about them. */\npublic final class O {\n    public static native Object a();\n");
    for (_, t, n) in PRIMS {
        s.push_str(&format!("    public static native {t} {n}();\n"));
    }
    s.push_str("}\n");
    s
}

fn caller_source(k: usize, calls: &[String]) -> String {
    let body: String = calls.iter().map(|c| format!("        try {{ {c} }} catch (Throwable t) {{ }}\n")).collect();
    format!("package gen;\n\npublic final class G{k} {{\n    public static void run() {{\n{body}    }}\n}}\n")
}

/// Error lines by file name, or `None` when an error isn't attributable to a line of ours.
type JavacErrors = Option<BTreeMap<String, BTreeSet<usize>>>;

/// javac `files` into `classes`: Ok(()) or the error lines by file name → line numbers
/// (`None` when an error isn't attributable to a line of ours).
fn javac(tools: &Tools, cp: &str, classes: &Path, files: &[PathBuf]) -> Result<std::result::Result<(), JavacErrors>> {
    let _ = std::fs::remove_dir_all(classes);
    mkdirs(classes)?;
    let out = Command::new(&tools.javac)
        .args(["--release", "17", "-nowarn", "-Xmaxerrs", "1000000", "-proc:none", "-implicit:none", "-encoding", "UTF-8", "-cp", cp, "-d"])
        .arg(classes)
        .args(files)
        .output()
        .map_err(|e| format!("javac: {e}"))?;
    if out.status.success() {
        return Ok(Ok(()));
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let mut bad: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut unattributed = false;
    for line in text.lines() {
        let Some(i) = line.find(": error:") else { continue };
        let head = &line[..i];
        let parsed = head.rsplit_once(':').and_then(|(f, n)| Some((Path::new(f).file_name()?.to_str()?.to_string(), n.parse::<usize>().ok()?)));
        match parsed {
            Some((f, n)) if f.starts_with('G') => {
                bad.entry(f).or_default().insert(n);
            }
            _ => unattributed = true,
        }
    }
    Ok(Err(if unattributed || bad.is_empty() { None } else { Some(bad) }))
}

/// Drops the call lines `lines` of `file`: false (nothing changed) when an error is on another
/// line (the file's structure), in which case the whole file must go.
fn drop_lines(file: &Path, lines: &BTreeSet<usize>) -> Result<bool> {
    let text = String::from_utf8_lossy(&read(file)?).into_owned();
    let all: Vec<&str> = text.lines().collect();
    if lines.iter().any(|&n| all.get(n - 1).is_none_or(|l| !l.trim_start().starts_with("try {"))) {
        return Ok(false);
    }
    let kept: Vec<&str> = all.iter().enumerate().filter(|(i, _)| !lines.contains(&(i + 1))).map(|x| *x.1).collect();
    write(file, kept.join("\n") + "\n")?;
    Ok(true)
}

/// Compiles, dropping rejected calls: line-attributed errors remove their lines; otherwise each
/// file is compiled alone and files that still fail are dropped.
fn compile(tools: &Tools, cp: &str, dir: &Path, o: &Path, mut files: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let classes = dir.join("classes");
    for _ in 0..40 {
        let mut all = vec![o.to_path_buf()];
        all.extend(files.iter().cloned());
        match javac(tools, cp, &classes, &all)? {
            Ok(()) => return Ok(files),
            Err(Some(bad)) => {
                let mut keep = Vec::new();
                for f in &files {
                    match bad.get(f.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
                        Some(lines) if !drop_lines(f, lines)? => {}
                        _ => keep.push(f.clone()),
                    }
                }
                files = keep;
            }
            Err(None) => {
                // Isolate, in parallel: a file whose errors can't be pinned to lines is dropped.
                let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
                let next = std::sync::atomic::AtomicUsize::new(0);
                let kept: std::sync::Mutex<BTreeMap<usize, bool>> = std::sync::Mutex::new(BTreeMap::new());
                let first_err: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
                std::thread::scope(|sc| {
                    for w in 0..workers {
                        let (next, kept, first_err, files) = (&next, &kept, &first_err, &files);
                        sc.spawn(move || loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let Some(f) = files.get(i) else { break };
                            let single = dir.join(format!("single{w}"));
                            let mut ok = false;
                            for _ in 0..6 {
                                match javac(tools, cp, &single, &[o.to_path_buf(), f.clone()]) {
                                    Ok(Ok(())) => {
                                        ok = true;
                                        break;
                                    }
                                    Ok(Err(Some(bad))) => {
                                        let mut dropped = true;
                                        for lines in bad.values() {
                                            match drop_lines(f, lines) {
                                                Ok(d) => dropped &= d,
                                                Err(e) => {
                                                    first_err.lock().expect("no poisoned lock").get_or_insert(e);
                                                    dropped = false;
                                                }
                                            }
                                        }
                                        if !dropped {
                                            break;
                                        }
                                    }
                                    Ok(Err(None)) => break,
                                    Err(e) => {
                                        first_err.lock().expect("no poisoned lock").get_or_insert(e);
                                        break;
                                    }
                                }
                            }
                            kept.lock().expect("no poisoned lock").insert(i, ok);
                        });
                    }
                });
                if let Some(e) = first_err.into_inner().expect("no poisoned lock") {
                    return Err(e);
                }
                let kept = kept.into_inner().expect("no poisoned lock");
                files = files.iter().enumerate().filter(|(i, _)| kept.get(i) == Some(&true)).map(|x| x.1.clone()).collect();
            }
        }
    }
    Err("javac: generated callers still don't compile after 40 rounds".into())
}

/// Generates, compiles and jars the callers of `entries` into `dir/gen.jar`: (jar, calls kept).
pub fn callers(tools: &Tools, jars: &[PathBuf], entries: &[&Entry], dir: &Path) -> Result<(PathBuf, usize)> {
    let _ = std::fs::remove_dir_all(dir);
    let src = dir.join("src/gen");
    mkdirs(&src)?;
    let calls: Vec<String> = entries.iter().filter_map(|e| call(e)).collect();
    let o = src.join("O.java");
    write(&o, opaque_source())?;
    let mut files = Vec::new();
    for (k, chunk) in calls.chunks(CALLS_PER_CLASS).enumerate() {
        let f = src.join(format!("G{k}.java"));
        write(&f, caller_source(k, chunk))?;
        files.push(f);
    }
    let mut cp_parts: Vec<PathBuf> = jars.to_vec();
    cp_parts.push(tools.android_jar.clone());
    let cp = std::env::join_paths(&cp_parts).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let files = compile(tools, &cp, dir, &o, files)?;
    let kept = files.iter().map(|f| read(f).map(|b| String::from_utf8_lossy(&b).matches("try {").count())).sum::<Result<usize>>()?;
    // Jar the classes in sorted order, fixed timestamps.
    let classes = dir.join("classes");
    let mut paths = Vec::new();
    collect(&classes, &mut paths)?;
    paths.sort();
    let jar = dir.join("gen.jar");
    let file = std::fs::File::create(&jar).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for p in paths {
        let name = p.strip_prefix(&classes).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(&read(&p)?).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok((jar, kept))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "class") {
            out.push(p);
        }
    }
    Ok(())
}

/// Default-argument callers (`defaults` scenarios): calls of Kotlin default-argument bridges the
/// way kotlinc emits them for a call that takes every default (constant `0`/`null` for the
/// defaulted parameters, the bridge's mask, a `null` marker), other arguments opaque. R8 then
/// propagates those constants into the bridge and the function, as it does in an app that calls
/// `setContent { … }` or `Modifier.padding(8.dp)`. Written as class files directly: javac can't
/// call synthetic bridges.
pub fn default_callers(entries: &[&crate::api::DefaultEntry], dir: &Path) -> Result<(PathBuf, usize)> {
    use crate::classfile::{ClassWriter, ACC_FINAL, ACC_NATIVE, ACC_PUBLIC as PUB, ACC_STATIC as STATIC, ACC_SUPER};
    let _ = std::fs::remove_dir_all(dir);
    mkdirs(dir)?;
    let mut classes: Vec<(String, Vec<u8>)> = Vec::new();
    let mut o = ClassWriter::default();
    o.method(PUB | STATIC | ACC_NATIVE, "a", "()Ljava/lang/Object;", None);
    for (t, _, n) in PRIMS {
        o.method(PUB | STATIC | ACC_NATIVE, n, &format!("(){t}"), None);
    }
    classes.push(("gen/O.class".into(), o.finish("gen/O", "java/lang/Object", PUB | ACC_FINAL | ACC_SUPER)));
    let usable: Vec<&&crate::api::DefaultEntry> = entries.iter().filter(|d| {
        let c = &d.entry.class;
        !c.contains('-') && eightr_ir::types::parse_proto(&d.entry.desc).is_some()
    }).collect();
    // Every call twice, from two classes (`D*`, `E*`): with one call site R8 inlines the callee into
    // it and no method is left to fingerprint; an app's callee survives because its callers are
    // big or many. Constants both sites agree on still propagate.
    for (prefix, (k, chunk)) in ["D", "E"].iter().flat_map(|p| usable.chunks(CALLS_PER_CLASS).enumerate().map(move |x| (*p, x))) {
        let mut w = ClassWriter::default();
        // One call per method (`c0`, `c1`, …): a call R8 knows always throws (a reified inline
        // function's compiled body) makes the rest of its method dead code.
        for (ci, d) in chunk.iter().enumerate() {
            let mut code: Vec<u8> = Vec::new();
            let mut max_stack = 0u16;
            let e = &d.entry;
            let (ps, ret) = eightr_ir::types::parse_proto(&e.desc).expect("filtered");
            let owner = e.class.trim_start_matches('L').trim_end_matches(';').to_string();
            let n = ps.len();
            let mut depth: u16 = 0;
            let ctor = e.name == "<init>";
            if ctor {
                if e.class_access & (ACC_INTERFACE | ACC_ABSTRACT) != 0 {
                    continue;
                }
                let c = w.class(&owner);
                code.push(0xbb);
                code.extend(c.to_be_bytes());
                code.push(0x59);
                depth = 2;
            }
            for (i, t) in ps.iter().enumerate() {
                let wide = *t == "J" || *t == "D";
                if i == n - 2 {
                    let c = w.integer(d.mask as i32);
                    code.push(0x13);
                    code.extend(c.to_be_bytes());
                } else if i == n - 1 {
                    code.push(0x01);
                } else if d.defaulted.contains(&i) {
                    code.push(match *t {
                        "J" => 0x09,
                        "F" => 0x0b,
                        "D" => 0x0e,
                        "Z" | "B" | "C" | "S" | "I" => 0x03,
                        _ => 0x01,
                    });
                } else if let Some(p) = PRIMS.iter().find(|p| p.0 == *t) {
                    let m = w.method_ref("gen/O", p.2, &format!("(){}", p.0), false);
                    code.push(0xb8);
                    code.extend(m.to_be_bytes());
                } else {
                    let m = w.method_ref("gen/O", "a", "()Ljava/lang/Object;", false);
                    code.push(0xb8);
                    code.extend(m.to_be_bytes());
                    let cast = if t.starts_with('[') { t.to_string() } else { t.trim_start_matches('L').trim_end_matches(';').to_string() };
                    if cast != "java/lang/Object" {
                        let c = w.class(&cast);
                        code.push(0xc0);
                        code.extend(c.to_be_bytes());
                    }
                }
                depth += if wide { 2 } else { 1 };
                max_stack = max_stack.max(depth);
            }
            let interface = e.class_access & ACC_INTERFACE != 0;
            let m = w.method_ref(&owner, &e.name, &e.desc, interface);
            if ctor {
                code.push(0xb7);
                code.extend(m.to_be_bytes());
                code.push(0x57);
            } else {
                code.push(0xb8);
                code.extend(m.to_be_bytes());
                match ret {
                    "V" => {}
                    "J" | "D" => {
                        code.push(0x58);
                        max_stack = max_stack.max(2);
                    }
                    _ => {
                        code.push(0x57);
                        max_stack = max_stack.max(1);
                    }
                }
            }
            code.push(0xb1);
            w.method(PUB | STATIC, &format!("c{ci}"), "()V", Some((max_stack.max(1), 0, code)));
        }
        classes.push((format!("gen/{prefix}{k}.class"), w.finish(&format!("gen/{prefix}{k}"), "java/lang/Object", PUB | ACC_FINAL | ACC_SUPER)));
    }
    let jar = dir.join("gen.jar");
    let file = std::fs::File::create(&jar).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, bytes) in &classes {
        z.start_file(name.as_str(), opts).map_err(|e| e.to_string())?;
        z.write_all(bytes).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok((jar, usable.len()))
}

/// Keep rules making the program classes a generated caller passes opaque values as instantiable,
/// as an app's are (AGP keeps its manifest components' constructors; its own code subclasses and
/// constructs library types). Otherwise R8 sees no instance of, say, `ComponentActivity` and
/// compiles every cast to it as a `ClassCastException`. Concrete classes only; abstract classes and
/// interfaces would need generated subclasses.
pub fn instantiable(descs: &[&str], access: &std::collections::BTreeMap<String, u16>) -> String {
    let mut types: BTreeSet<&str> = BTreeSet::new();
    for d in descs {
        let Some((ps, _)) = eightr_ir::types::parse_proto(d) else { continue };
        for t in ps {
            let base = t.trim_start_matches('[');
            if base.starts_with('L') && access.get(base).is_some_and(|a| a & (ACC_INTERFACE | ACC_ABSTRACT) == 0) {
                types.insert(base);
            }
        }
    }
    types.iter().map(|t| format!("-keep,allowobfuscation,allowoptimization class {} {{ <init>(...); }}\n", java_name(t))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(class: &str, name: &str, desc: &str, access: u16, class_access: u16) -> Entry {
        Entry { artifact: 0, class: class.into(), class_access, name: name.into(), desc: desc.into(), access }
    }

    #[test]
    fn calls_and_roots() {
        let st = entry("La/B$C;", "f", "(ILjava/lang/String;[J)V", ACC_PUBLIC | ACC_STATIC, ACC_PUBLIC);
        assert_eq!(call(&st).unwrap(), "a.B.C.f(O.i(), ((java.lang.String) O.a()), ((long[]) O.a()));");
        let inst = entry("La/B;", "g", "()I", ACC_PUBLIC, ACC_PUBLIC);
        assert_eq!(call(&inst).unwrap(), "((a.B) O.a()).g();");
        let ctor = entry("La/B;", "<init>", "(Z)V", ACC_PUBLIC, ACC_PUBLIC | ACC_ABSTRACT);
        assert_eq!(call(&ctor), None);
        assert_eq!(call(&entry("La/B$1;", "g", "()V", ACC_PUBLIC, ACC_PUBLIC)), None);
        assert_eq!(call(&entry("La/B;", "g$lib", "()V", ACC_PUBLIC, ACC_PUBLIC)), None);
        let r = roots(&[&st, &inst]);
        assert!(r.contains("-keep,allowobfuscation,allowoptimization class a.B$C {\n    void f(int,java.lang.String,long[]);\n}"), "{r}");
        assert!(r.contains("class a.B {\n    int g();\n}"));
    }

    #[test]
    fn sampling_is_deterministic_and_seeded() {
        let es: Vec<Entry> = (0..2000).map(|i| entry("La/B;", &format!("m{i}"), "()V", ACC_PUBLIC, ACC_PUBLIC)).collect();
        let a: Vec<bool> = es.iter().map(|e| sampled(e, 0.1, 1)).collect();
        let b: Vec<bool> = es.iter().map(|e| sampled(e, 0.1, 1)).collect();
        let c: Vec<bool> = es.iter().map(|e| sampled(e, 0.1, 2)).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let n = a.iter().filter(|x| **x).count();
        assert!((140..260).contains(&n), "{n}");
    }
}
