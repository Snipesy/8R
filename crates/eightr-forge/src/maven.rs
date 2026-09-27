//! Maven resolution the way Gradle resolves an app's runtime classpath: Gradle module metadata
//! first (`.module`: Kotlin multiplatform roots redirect to their `-android`/`-jvm` variant through
//! `available-at`), the POM otherwise. Declared (pinned) versions win; every other module gets the
//! highest version requested by a module of the resolved graph.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use eightr_core::libdb::Coord;
use serde_json::Value;

use crate::tools::{fetch, read, read_string, sha256_hex, Result};

pub const GOOGLE: &str = "https://dl.google.com/dl/android/maven2";
pub const CENTRAL: &str = "https://repo1.maven.org/maven2";
const REPOS: &[&str] = &[GOOGLE, CENTRAL];

/// A resolved module: its artifact (none for BOMs and platforms) and its dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub url: Option<String>,
    pub deps: Vec<Coord>,
    /// A Kotlin multiplatform root whose only dependency is the platform variant it stands for.
    pub redirect: bool,
}

/// A resolved artifact of the closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resolved {
    pub coord: Coord,
    /// Declared by the app, directly or through multiplatform redirects.
    pub declared: bool,
    pub url: String,
    pub sha256: String,
    pub file: PathBuf,
}

impl Resolved {
    pub fn lock_line(&self) -> String {
        format!("{} {} {}", self.coord, self.sha256, self.url)
    }
}

/// Maven-style version order: numeric components numerically; a release sorts after its
/// qualified pre-releases (`1.0.0-alpha01` < `1.0.0-rc01` < `1.0.0`).
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(v: &str) -> (Vec<u64>, Option<(u8, u64, String)>) {
        let (core, q) = v.split_once('-').map_or((v, None), |(c, q)| (c, Some(q)));
        let nums = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
        let q = q.map(|q| {
            let l = q.to_ascii_lowercase();
            let rank = if l.starts_with("alpha") || l.starts_with('a') && l[1..].starts_with(|c: char| c.is_ascii_digit()) {
                0
            } else if l.starts_with("beta") {
                1
            } else if l.starts_with("rc") || l.starts_with("cr") {
                2
            } else {
                3
            };
            let n = l.trim_start_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok()).unwrap_or(0);
            (rank, n, l)
        });
        (nums, q)
    }
    let (na, qa) = split(a);
    let (nb, qb) = split(b);
    let len = na.len().max(nb.len());
    for i in 0..len {
        let (x, y) = (na.get(i).copied().unwrap_or(0), nb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    match (qa, qb) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

fn clean_version(v: &str) -> Option<String> {
    // `[1.2.3]`, `[1.2,2.0)`: the lower bound.
    let v = v.trim().trim_start_matches(['[', '(']);
    let v = v.split(',').next()?.trim_end_matches([']', ')']).trim();
    (!v.is_empty() && !v.contains('$')).then(|| v.to_string())
}

/// The Gradle module metadata's runtime variant for an Android app: (score, variant).
fn pick_variant(m: &Value) -> Option<&Value> {
    let vs = m.get("variants")?.as_array()?;
    let score = |v: &Value| -> Option<u32> {
        let at = v.get("attributes")?;
        if at.get("org.gradle.usage").and_then(Value::as_str) != Some("java-runtime") {
            return None;
        }
        if at.get("org.gradle.category").and_then(Value::as_str).is_some_and(|c| c != "library") {
            return None;
        }
        let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let platform = at.get("org.jetbrains.kotlin.platform.type").and_then(Value::as_str);
        let mut s = 1;
        if platform == Some("androidJvm") || name.contains("android") {
            s += 4;
        } else if platform == Some("jvm") || name.contains("jvm") {
            s += 2;
        }
        if name.contains("release") {
            s += 1;
        }
        Some(s)
    };
    // Highest score; ties by name, for determinism.
    vs.iter().filter_map(|v| Some((score(v)?, v.get("name").and_then(Value::as_str).unwrap_or(""), v))).max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(a.1))).map(|x| x.2)
}

/// Parses a `.module` file: Ok(None) when it has no usable runtime variant (platforms, BOMs).
pub fn parse_module(text: &str, dir_url: &str) -> Result<Result<Module>> {
    let m: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(v) = pick_variant(&m) else { return Ok(Ok(Module { url: None, deps: Vec::new(), redirect: false })) };
    if let Some(t) = v.get("available-at") {
        let c = Coord {
            group: t.get("group").and_then(Value::as_str).unwrap_or("").into(),
            artifact: t.get("module").and_then(Value::as_str).unwrap_or("").into(),
            version: t.get("version").and_then(Value::as_str).unwrap_or("").into(),
        };
        return Ok(Ok(Module { url: None, deps: vec![c], redirect: true }));
    }
    let mut deps = Vec::new();
    for d in v.get("dependencies").and_then(Value::as_array).into_iter().flatten() {
        let ver = d.get("version");
        let pick = ["strictly", "requires", "prefers"].iter().find_map(|k| ver.and_then(|x| x.get(k)).and_then(Value::as_str));
        let (Some(g), Some(a), Some(ver)) = (d.get("group").and_then(Value::as_str), d.get("module").and_then(Value::as_str), pick.and_then(clean_version)) else { continue };
        deps.push(Coord { group: g.into(), artifact: a.into(), version: ver });
    }
    let file = v.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(|f| f.get("url").and_then(Value::as_str)).find(|u| u.ends_with(".aar") || u.ends_with(".jar"));
    Ok(Ok(Module { url: file.map(|f| format!("{dir_url}/{f}")), deps, redirect: false }))
}

/// The text between `<tag>` and `</tag>` of each top-level occurrence in `xml`.
fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

fn text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    blocks(xml, tag).first().map(|s| s.trim())
}

/// Parses a POM: packaging and compile/runtime, non-optional dependencies with literal versions.
pub fn parse_pom(xml: &str, base_url: &str) -> Module {
    // Drop comments and dependencyManagement (BOM imports, version constraints).
    let mut s = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<!--") {
        s.push_str(&rest[..i]);
        rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
    }
    s.push_str(rest);
    let mut clean = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("<dependencyManagement>") {
        clean.push_str(&rest[..i]);
        rest = rest[i..].find("</dependencyManagement>").map_or("", |j| &rest[i + j + "</dependencyManagement>".len()..]);
    }
    clean.push_str(rest);
    let packaging = text(&clean, "packaging").unwrap_or("jar");
    let mut deps = Vec::new();
    for deps_block in blocks(&clean, "dependencies") {
        for d in blocks(deps_block, "dependency") {
            let scope = text(d, "scope").unwrap_or("compile");
            if !matches!(scope, "compile" | "runtime") || text(d, "optional") == Some("true") {
                continue;
            }
            let (Some(g), Some(a), Some(v)) = (text(d, "groupId"), text(d, "artifactId"), text(d, "version").and_then(clean_version)) else { continue };
            deps.push(Coord { group: g.into(), artifact: a.into(), version: v });
        }
    }
    let url = match packaging {
        "pom" => None,
        "aar" => Some(format!("{base_url}.aar")),
        _ => Some(format!("{base_url}.jar")),
    };
    Module { url, deps, redirect: false }
}

fn module(c: &Coord) -> Result<Module> {
    for repo in REPOS {
        let dir = format!("{repo}/{}/{}/{}", c.group.replace('.', "/"), c.artifact, c.version);
        let base = format!("{dir}/{}-{}", c.artifact, c.version);
        let Some(pom) = fetch(&format!("{base}.pom"))? else { continue };
        if let Some(m) = fetch(&format!("{base}.module"))? {
            if let Ok(module) = parse_module(&read_string(&m)?, &dir)? {
                return Ok(module);
            }
        }
        return Ok(parse_pom(&read_string(&pom)?, &base));
    }
    Err(format!("{c}: not found on Google Maven or Maven Central"))
}

/// Resolves the closure of `pinned` (declared coordinates; they keep their versions).
pub fn resolve(pinned: &[Coord]) -> Result<Vec<Resolved>> {
    let pins: BTreeMap<(String, String), String> = pinned.iter().map(|c| ((c.group.clone(), c.artifact.clone()), c.version.clone())).collect();
    let mut want = pins.clone();
    let mut modules: BTreeMap<Coord, Module> = BTreeMap::new();
    // want[k+1] = pins ∪ highest requested version over the graph reachable under want[k].
    for _round in 0..32 {
        let mut next = pins.clone();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut stack: Vec<(String, String)> = pins.keys().cloned().collect();
        while let Some(ga) = stack.pop() {
            if !seen.insert(ga.clone()) {
                continue;
            }
            let v = next.get(&ga).or_else(|| want.get(&ga)).cloned().expect("requested");
            let c = Coord { group: ga.0.clone(), artifact: ga.1.clone(), version: v };
            if !modules.contains_key(&c) {
                let m = module(&c)?;
                modules.insert(c.clone(), m);
            }
            for d in &modules[&c].deps {
                let k = (d.group.clone(), d.artifact.clone());
                if !pins.contains_key(&k) {
                    let take = match next.get(&k) {
                        Some(n) => version_cmp(&d.version, n).is_gt(),
                        None => true,
                    };
                    if take {
                        next.insert(k.clone(), d.version.clone());
                    }
                }
                stack.push(k);
            }
        }
        let stable = next == want;
        want = next;
        if stable {
            break;
        }
    }
    // Declared: the pins and what their redirects stand for.
    let mut declared: BTreeSet<(String, String)> = pins.keys().cloned().collect();
    let mut stack: Vec<(String, String)> = declared.iter().cloned().collect();
    while let Some(ga) = stack.pop() {
        let Some(v) = want.get(&ga) else { continue };
        let c = Coord { group: ga.0.clone(), artifact: ga.1.clone(), version: v.clone() };
        if let Some(m) = modules.get(&c).filter(|m| m.redirect) {
            for d in &m.deps {
                if declared.insert((d.group.clone(), d.artifact.clone())) {
                    stack.push((d.group.clone(), d.artifact.clone()));
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((g, a), v) in &want {
        let c = Coord { group: g.clone(), artifact: a.clone(), version: v.clone() };
        let m = match modules.get(&c) {
            Some(m) => m.clone(),
            None => module(&c)?,
        };
        let Some(url) = m.url else { continue };
        let file = fetch(&url)?.ok_or_else(|| format!("{c}: artifact missing at {url}"))?;
        let sha256 = sha256_hex(&read(&file)?);
        let is_declared = declared.contains(&(g.clone(), a.clone()));
        out.push(Resolved { coord: c, declared: is_declared, url, sha256, file });
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_like_maven() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("1.10.0", "1.9.9"), Greater);
        assert_eq!(version_cmp("1.0.0-alpha01", "1.0.0"), Less);
        assert_eq!(version_cmp("1.0.0-rc01", "1.0.0-beta02"), Greater);
        assert_eq!(version_cmp("1.0.0-alpha10", "1.0.0-alpha02"), Greater);
        assert_eq!(version_cmp("2.0", "2.0.0"), Equal);
    }

    #[test]
    fn module_metadata_variants_and_redirects() {
        let kmp = r#"{"variants":[
            {"name":"metadataApiElements","attributes":{"org.gradle.usage":"kotlin-metadata"}},
            {"name":"jvmRuntimeElements-published","attributes":{"org.gradle.usage":"java-runtime","org.gradle.category":"library","org.jetbrains.kotlin.platform.type":"jvm"},
             "available-at":{"url":"../../x-jvm/1.0/x-jvm-1.0.module","group":"g","module":"x-jvm","version":"1.0"}},
            {"name":"androidRuntimeElements-published","attributes":{"org.gradle.usage":"java-runtime","org.gradle.category":"library","org.jetbrains.kotlin.platform.type":"androidJvm"},
             "available-at":{"url":"../../x-android/1.0/x-android-1.0.module","group":"g","module":"x-android","version":"1.0"}}]}"#;
        let m = parse_module(kmp, "https://r/g/x/1.0").unwrap().unwrap();
        assert_eq!(m.deps, vec![Coord::parse("g:x-android:1.0").unwrap()]);
        assert_eq!(m.url, None);
        assert!(m.redirect);
        let leaf = r#"{"variants":[{"name":"releaseRuntimeElements","attributes":{"org.gradle.usage":"java-runtime","org.gradle.category":"library"},
            "dependencies":[{"group":"a","module":"b","version":{"requires":"2.0"}},{"group":"c","module":"d","version":{"strictly":"[1.5]"}}],
            "files":[{"name":"x.aar","url":"x-android-1.0.aar"}]},
            {"name":"platform","attributes":{"org.gradle.usage":"java-runtime","org.gradle.category":"platform"}}]}"#;
        let m = parse_module(leaf, "https://r/g/x-android/1.0").unwrap().unwrap();
        assert_eq!(m.url.as_deref(), Some("https://r/g/x-android/1.0/x-android-1.0.aar"));
        assert_eq!(m.deps, vec![Coord::parse("a:b:2.0").unwrap(), Coord::parse("c:d:1.5").unwrap()]);
        let bom = r#"{"variants":[{"name":"apiElements","attributes":{"org.gradle.usage":"java-runtime","org.gradle.category":"platform"}}]}"#;
        assert_eq!(parse_module(bom, "u").unwrap().unwrap(), Module { url: None, deps: vec![], redirect: false });
    }

    #[test]
    fn pom_dependencies() {
        let pom = "<project><packaging>aar</packaging><!-- <dependency><groupId>x</groupId></dependency> -->
            <dependencyManagement><dependencies><dependency><groupId>bom</groupId><artifactId>b</artifactId><version>1</version></dependency></dependencies></dependencyManagement>
            <dependencies>
              <dependency><groupId>a</groupId><artifactId>b</artifactId><version>1.2</version></dependency>
              <dependency><groupId>t</groupId><artifactId>junit</artifactId><version>4</version><scope>test</scope></dependency>
              <dependency><groupId>o</groupId><artifactId>p</artifactId><version>1</version><optional>true</optional></dependency>
              <dependency><groupId>r</groupId><artifactId>s</artifactId><version>[2.0]</version><scope>runtime</scope></dependency>
            </dependencies></project>";
        let m = parse_pom(pom, "https://r/g/x/1.0/x-1.0");
        assert_eq!(m.url.as_deref(), Some("https://r/g/x/1.0/x-1.0.aar"));
        assert_eq!(m.deps, vec![Coord::parse("a:b:1.2").unwrap(), Coord::parse("r:s:2.0").unwrap()]);
        assert_eq!(parse_pom("<project><packaging>pom</packaging></project>", "u").url, None);
    }
}
