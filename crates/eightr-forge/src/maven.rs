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

/// Maven-style version order: numeric components numerically; pre-releases (`alpha`, `beta`,
/// `rc`) before the release, other qualifiers (`-android`, `-jre`) after it, among themselves
/// lexically (`1.0.0-alpha01` < `1.0.0-rc01` < `1.0.0` < `1.0.0-android`).
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(v: &str) -> (Vec<u64>, (u8, u64, String)) {
        let (core, q) = v.split_once('-').map_or((v, None), |(c, q)| (c, Some(q)));
        let nums = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
        let q = match q {
            None => (3, 0, String::new()),
            Some(q) => {
                let l = q.to_ascii_lowercase();
                let rank = if l.starts_with("alpha") || l.starts_with('a') && l[1..].starts_with(|c: char| c.is_ascii_digit()) {
                    0
                } else if l.starts_with("beta") || l.starts_with('b') && l[1..].starts_with(|c: char| c.is_ascii_digit()) {
                    1
                } else if l.starts_with("rc") || l.starts_with("cr") {
                    2
                } else {
                    4
                };
                let n = l.trim_start_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok()).unwrap_or(0);
                (rank, n, l)
            }
        };
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
    qa.cmp(&qb)
}

fn clean_version(v: &str) -> Option<String> {
    // `[1.2.3]`, `[1.2,2.0)`: the lower bound.
    let v = v.trim().trim_start_matches(['[', '(']);
    let v = v.split(',').next()?.trim_end_matches([']', ')']).trim();
    (!v.is_empty() && !v.contains('$')).then(|| v.to_string())
}

fn category(v: &Value) -> &str {
    v.get("attributes").and_then(|a| a.get("org.gradle.category")).and_then(Value::as_str).unwrap_or("library")
}

/// The Gradle module metadata's runtime library variant for an Android app.
fn pick_variant(m: &Value) -> Option<&Value> {
    let vs = m.get("variants")?.as_array()?;
    let score = |v: &Value| -> Option<u32> {
        let at = v.get("attributes")?;
        if at.get("org.gradle.usage").and_then(Value::as_str) != Some("java-runtime") || category(v) != "library" {
            return None;
        }
        // The main component only (not test fixtures or other capabilities).
        if v.get("capabilities").and_then(Value::as_array).is_some_and(|c| !c.is_empty()) {
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

/// Parses a `.module` file. `None`: no runtime library variant and not a platform either, so the
/// POM decides.
pub fn parse_module(text: &str, dir_url: &str) -> Result<Option<Module>> {
    let m: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(v) = pick_variant(&m) else {
        let platform = m.get("variants").and_then(Value::as_array).into_iter().flatten().any(|v| matches!(category(v), "platform" | "enforced-platform"));
        return Ok(platform.then(|| Module { url: None, deps: Vec::new(), redirect: false }));
    };
    if let Some(t) = v.get("available-at") {
        let c = Coord {
            group: t.get("group").and_then(Value::as_str).unwrap_or("").into(),
            artifact: t.get("module").and_then(Value::as_str).unwrap_or("").into(),
            version: t.get("version").and_then(Value::as_str).unwrap_or("").into(),
        };
        return Ok(Some(Module { url: None, deps: vec![c], redirect: true }));
    }
    let mut deps = Vec::new();
    for d in v.get("dependencies").and_then(Value::as_array).into_iter().flatten() {
        let ver = d.get("version");
        let pick = ["strictly", "requires", "prefers"].iter().find_map(|k| ver.and_then(|x| x.get(k)).and_then(Value::as_str));
        let (Some(g), Some(a), Some(ver)) = (d.get("group").and_then(Value::as_str), d.get("module").and_then(Value::as_str), pick.and_then(clean_version)) else { continue };
        deps.push(Coord { group: g.into(), artifact: a.into(), version: ver });
    }
    let file = v.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(|f| f.get("url").and_then(Value::as_str)).find(|u| u.ends_with(".aar") || u.ends_with(".jar"));
    Ok(Some(Module { url: file.map(|f| format!("{dir_url}/{f}")), deps, redirect: false }))
}

/// The text between `<tag>` and `</tag>` of each occurrence in `xml` (not nested in itself).
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

/// `xml` without the `<tag>…</tag>` sections.
fn without(xml: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        rest = rest[i..].find(&close).map_or("", |j| &rest[i + j + close.len()..]);
    }
    out.push_str(rest);
    out
}

/// One POM's own content, top level only.
#[derive(Debug, Default)]
struct Pom {
    group: Option<String>,
    version: Option<String>,
    packaging: String,
    parent: Option<Coord>,
    props: BTreeMap<String, String>,
    /// dependencyManagement: (group, artifact) → version (unsubstituted).
    managed: BTreeMap<(String, String), String>,
    /// (group, artifact, version or none, scope, optional), unsubstituted.
    deps: Vec<(String, String, Option<String>, String, bool)>,
}

fn raw_pom(xml: &str) -> Pom {
    // Comments (`<!-- … -->`) out first.
    let mut body = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<!--") {
        body.push_str(&rest[..i]);
        rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
    }
    body.push_str(rest);
    let dm = blocks(&body, "dependencyManagement").first().map(|s| s.to_string()).unwrap_or_default();
    let mut top = body.clone();
    for t in ["build", "profiles", "reporting", "dependencyManagement", "distributionManagement", "pluginRepositories", "repositories"] {
        top = without(&top, t);
    }
    let parent = blocks(&top, "parent").first().and_then(|p| Some(Coord { group: text(p, "groupId")?.into(), artifact: text(p, "artifactId")?.into(), version: text(p, "version")?.into() }));
    let own = without(&top, "parent");
    let own = without(&own, "dependencies");
    let props_block = blocks(&top, "properties").first().map(|s| s.to_string()).unwrap_or_default();
    let mut props = BTreeMap::new();
    let mut rest = props_block.as_str();
    while let Some(i) = rest.find('<') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('>') else { break };
        let name = &after[..j];
        if name.starts_with('/') || name.ends_with('/') || name.contains(' ') {
            rest = &after[j + 1..];
            continue;
        }
        let close = format!("</{name}>");
        let Some(k) = after[j + 1..].find(&close) else { break };
        props.insert(name.to_string(), after[j + 1..j + 1 + k].trim().to_string());
        rest = &after[j + 1 + k + close.len()..];
    }
    let dep_list = |block: &str| -> Vec<(String, String, Option<String>, String, bool)> {
        blocks(block, "dependency")
            .into_iter()
            .filter_map(|d| {
                let d = without(d, "exclusions");
                Some((text(&d, "groupId")?.to_string(), text(&d, "artifactId")?.to_string(), text(&d, "version").map(str::to_string), text(&d, "scope").unwrap_or("compile").to_string(), text(&d, "optional") == Some("true")))
            })
            .collect()
    };
    let managed = dep_list(&dm).into_iter().filter(|d| d.3 != "import").filter_map(|d| Some(((d.0, d.1), d.2?))).collect();
    let deps = blocks(&top, "dependencies").into_iter().flat_map(dep_list).collect();
    Pom {
        group: text(&own, "groupId").map(str::to_string),
        version: text(&own, "version").map(str::to_string),
        packaging: text(&own, "packaging").unwrap_or("jar").to_string(),
        parent,
        props,
        managed,
        deps,
    }
}

/// Parses a POM with its parent chain (`parent` fetches a parent POM's text): properties and
/// `project.*` substituted, versions from `dependencyManagement` where a dependency has none,
/// compile/runtime non-optional dependencies. (BOM imports and exclusions are not modelled.)
pub fn parse_pom(xml: &str, base_url: &str, parent: &mut dyn FnMut(&Coord) -> Result<Option<String>>) -> Result<Module> {
    let own = raw_pom(xml);
    let mut chain = vec![own];
    while let Some(pc) = chain.last().and_then(|p| p.parent.clone()) {
        if chain.len() > 10 {
            break;
        }
        let Some(text) = parent(&pc)? else { break };
        chain.push(raw_pom(&text));
    }
    let mut props: BTreeMap<String, String> = BTreeMap::new();
    let mut managed: BTreeMap<(String, String), String> = BTreeMap::new();
    // Ancestors first, so the child overrides.
    for p in chain.iter().rev() {
        props.extend(p.props.clone());
        managed.extend(p.managed.clone());
    }
    let me = &chain[0];
    let pv = me.parent.as_ref().map(|c| c.version.clone());
    let version = me.version.clone().or_else(|| pv.clone()).unwrap_or_default();
    let group = me.group.clone().or_else(|| me.parent.as_ref().map(|c| c.group.clone())).unwrap_or_default();
    for (k, v) in [("project.version", version.clone()), ("version", version), ("project.groupId", group.clone()), ("groupId", group), ("project.parent.version", pv.unwrap_or_default())] {
        props.insert(k.to_string(), v);
    }
    let subst = |v: &str| -> String {
        let mut v = v.to_string();
        for _ in 0..6 {
            let Some(i) = v.find("${") else { break };
            let Some(j) = v[i..].find('}') else { break };
            let key = &v[i + 2..i + j];
            let Some(val) = props.get(key) else { break };
            v = format!("{}{}{}", &v[..i], val, &v[i + j + 1..]);
        }
        v
    };
    let mut deps = Vec::new();
    for (g, a, v, scope, optional) in &me.deps {
        if !matches!(scope.as_str(), "compile" | "runtime") || *optional {
            continue;
        }
        let (g, a) = (subst(g), subst(a));
        let v = v.clone().or_else(|| managed.get(&(g.clone(), a.clone())).cloned());
        let Some(v) = v.map(|v| subst(&v)).as_deref().and_then(clean_version) else { continue };
        deps.push(Coord { group: g, artifact: a, version: v });
    }
    let url = match me.packaging.as_str() {
        "pom" => None,
        "aar" => Some(format!("{base_url}.aar")),
        _ => Some(format!("{base_url}.jar")),
    };
    Ok(Module { url, deps, redirect: false })
}

fn pom_text(c: &Coord) -> Result<Option<(String, String, String)>> {
    for repo in REPOS {
        let dir = format!("{repo}/{}/{}/{}", c.group.replace('.', "/"), c.artifact, c.version);
        let base = format!("{dir}/{}-{}", c.artifact, c.version);
        if let Some(pom) = fetch(&format!("{base}.pom"))? {
            return Ok(Some((read_string(&pom)?, dir, base)));
        }
    }
    Ok(None)
}

/// A module from the repositories: `None` when neither has it.
fn module(c: &Coord) -> Result<Option<Module>> {
    let Some((pom, dir, base)) = pom_text(c)? else { return Ok(None) };
    if let Some(m) = fetch(&format!("{base}.module"))? {
        if let Some(module) = parse_module(&read_string(&m)?, &dir)? {
            return Ok(Some(module));
        }
    }
    parse_pom(&pom, &base, &mut |pc| Ok(pom_text(pc)?.map(|x| x.0))).map(Some)
}

/// The selected graph: (group, artifact) → version, the modules read, and coordinates no
/// repository has.
pub struct Graph {
    pub selected: BTreeMap<(String, String), String>,
    pub modules: BTreeMap<Coord, Module>,
    pub missing: BTreeSet<Coord>,
}

/// Gradle-style selection: pins keep their versions; every other module gets the highest version
/// requested by a module of the graph reachable under the current selection, to a fixpoint (the
/// dependencies of an evicted version don't count).
pub fn select(pinned: &[Coord], lookup: &mut dyn FnMut(&Coord) -> Result<Option<Module>>) -> Result<Graph> {
    let pins: BTreeMap<(String, String), String> = pinned.iter().map(|c| ((c.group.clone(), c.artifact.clone()), c.version.clone())).collect();
    let mut want = pins.clone();
    let mut modules: BTreeMap<Coord, Module> = BTreeMap::new();
    let mut missing: BTreeSet<Coord> = BTreeSet::new();
    for _round in 0..64 {
        let mut next = pins.clone();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut stack: Vec<(String, String)> = pins.keys().rev().cloned().collect();
        while let Some(ga) = stack.pop() {
            if !seen.insert(ga.clone()) {
                continue;
            }
            // This round walks the previous round's selection (new modules: the highest seen).
            let v = want.get(&ga).or_else(|| next.get(&ga)).cloned().expect("requested");
            let c = Coord { group: ga.0.clone(), artifact: ga.1.clone(), version: v };
            if !modules.contains_key(&c) && !missing.contains(&c) {
                match lookup(&c)? {
                    Some(m) => {
                        modules.insert(c.clone(), m);
                    }
                    None => {
                        missing.insert(c.clone());
                    }
                }
            }
            let Some(m) = modules.get(&c) else { continue };
            for d in &m.deps {
                let k = (d.group.clone(), d.artifact.clone());
                if !pins.contains_key(&k) && next.get(&k).is_none_or(|n| version_cmp(&d.version, n).is_gt()) {
                    next.insert(k.clone(), d.version.clone());
                }
                stack.push(k);
            }
        }
        if next == want {
            let missing = missing.into_iter().filter(|c| want.get(&(c.group.clone(), c.artifact.clone())) == Some(&c.version)).collect();
            return Ok(Graph { selected: want, modules, missing });
        }
        want = next;
    }
    Err("dependency resolution did not converge in 64 rounds".into())
}

/// A resolution: the artifacts, and declared or required coordinates no repository has.
pub struct Resolution {
    pub artifacts: Vec<Resolved>,
    pub missing: Vec<Coord>,
}

/// Resolves the closure of `pinned` (declared coordinates; they keep their versions).
pub fn resolve(pinned: &[Coord]) -> Result<Resolution> {
    let g = select(pinned, &mut |c| module(c))?;
    let pins: BTreeSet<(String, String)> = pinned.iter().map(|c| (c.group.clone(), c.artifact.clone())).collect();
    // Declared: the pins and what their redirects stand for.
    let mut declared = pins.clone();
    let mut stack: Vec<(String, String)> = declared.iter().cloned().collect();
    while let Some(ga) = stack.pop() {
        let Some(v) = g.selected.get(&ga) else { continue };
        let c = Coord { group: ga.0.clone(), artifact: ga.1.clone(), version: v.clone() };
        if let Some(m) = g.modules.get(&c).filter(|m| m.redirect) {
            for d in &m.deps {
                if declared.insert((d.group.clone(), d.artifact.clone())) {
                    stack.push((d.group.clone(), d.artifact.clone()));
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((grp, a), v) in &g.selected {
        let c = Coord { group: grp.clone(), artifact: a.clone(), version: v.clone() };
        let Some(url) = g.modules.get(&c).and_then(|m| m.url.clone()) else { continue };
        let file = fetch(&url)?.ok_or_else(|| format!("{c}: artifact missing at {url}"))?;
        let sha256 = sha256_hex(&read(&file)?);
        let is_declared = declared.contains(&(grp.clone(), a.clone()));
        out.push(Resolved { coord: c, declared: is_declared, url, sha256, file });
    }
    out.sort();
    Ok(Resolution { artifacts: out, missing: g.missing.into_iter().collect() })
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
        assert_eq!(parse_module(bom, "u").unwrap(), Some(Module { url: None, deps: vec![], redirect: false }));
        // No library variant and not a platform: the POM decides.
        let odd = r#"{"variants":[{"name":"x","attributes":{"org.gradle.usage":"java-api"}}]}"#;
        assert_eq!(parse_module(odd, "u").unwrap(), None);
    }

    #[test]
    fn pom_dependencies() {
        let pom = "<project><packaging>aar</packaging><!-- <dependency><groupId>x</groupId></dependency> -->
            <dependencyManagement><dependencies><dependency><groupId>m</groupId><artifactId>n</artifactId><version>7</version></dependency></dependencies></dependencyManagement>
            <dependencies>
              <dependency><groupId>a</groupId><artifactId>b</artifactId><version>1.2</version></dependency>
              <dependency><groupId>t</groupId><artifactId>junit</artifactId><version>4</version><scope>test</scope></dependency>
              <dependency><groupId>o</groupId><artifactId>p</artifactId><version>1</version><optional>true</optional></dependency>
              <dependency><groupId>r</groupId><artifactId>s</artifactId><version>[2.0]</version><scope>runtime</scope></dependency>
              <dependency><groupId>m</groupId><artifactId>n</artifactId></dependency>
            </dependencies>
            <build><plugins><plugin><dependencies><dependency><groupId>plug</groupId><artifactId>in</artifactId><version>9</version></dependency></dependencies></plugin></plugins></build></project>";
        let m = parse_pom(pom, "https://r/g/x/1.0/x-1.0", &mut |_| Ok(None)).unwrap();
        assert_eq!(m.url.as_deref(), Some("https://r/g/x/1.0/x-1.0.aar"));
        assert_eq!(m.deps, vec![Coord::parse("a:b:1.2").unwrap(), Coord::parse("r:s:2.0").unwrap(), Coord::parse("m:n:7").unwrap()]);
        assert_eq!(parse_pom("<project><packaging>pom</packaging></project>", "u", &mut |_| Ok(None)).unwrap().url, None);
    }

    #[test]
    fn pom_properties_and_parents() {
        let child = "<project><parent><groupId>g</groupId><artifactId>parent</artifactId><version>3.1</version></parent>
            <artifactId>c</artifactId><properties><okio.version>3.6.0</okio.version></properties>
            <dependencies>
              <dependency><groupId>${project.groupId}</groupId><artifactId>sib</artifactId><version>${project.version}</version></dependency>
              <dependency><groupId>com.squareup.okio</groupId><artifactId>okio</artifactId><version>${okio.version}</version></dependency>
              <dependency><groupId>x</groupId><artifactId>managed</artifactId></dependency>
            </dependencies></project>";
        let parent = "<project><groupId>g</groupId><artifactId>parent</artifactId><version>3.1</version>
            <dependencyManagement><dependencies><dependency><groupId>x</groupId><artifactId>managed</artifactId><version>${m.v}</version></dependency></dependencies></dependencyManagement>
            <properties><m.v>5.5</m.v></properties></project>";
        let m = parse_pom(child, "u", &mut |c| Ok((c.artifact == "parent").then(|| parent.to_string()))).unwrap();
        assert_eq!(m.deps, vec![Coord::parse("g:sib:3.1").unwrap(), Coord::parse("com.squareup.okio:okio:3.6.0").unwrap(), Coord::parse("x:managed:5.5").unwrap()]);
    }

    /// Only the selected versions' dependencies count (x:1's z doesn't; x:2's w does).
    #[test]
    fn selection_follows_selected_versions() {
        let db: BTreeMap<&str, Vec<&str>> = [("a:a:1", vec!["x:x:2"]), ("b:b:1", vec!["y:y:1"]), ("y:y:1", vec!["x:x:1"]), ("x:x:1", vec!["z:z:1"]), ("x:x:2", vec!["w:w:1"]), ("z:z:1", vec![]), ("w:w:1", vec![])].into_iter().collect();
        let mut lookup = |c: &Coord| -> Result<Option<Module>> {
            Ok(db.get(c.to_string().as_str()).map(|ds| Module { url: Some(c.to_string()), deps: ds.iter().map(|d| Coord::parse(d).unwrap()).collect(), redirect: false }))
        };
        let pins = [Coord::parse("a:a:1").unwrap(), Coord::parse("b:b:1").unwrap()];
        let g = select(&pins, &mut lookup).unwrap();
        let got: Vec<String> = g.selected.iter().map(|((gr, a), v)| format!("{gr}:{a}:{v}")).collect();
        assert_eq!(got, ["a:a:1", "b:b:1", "w:w:1", "x:x:2", "y:y:1"]);
        let g2 = select(&[Coord::parse("q:q:1").unwrap()], &mut lookup).unwrap();
        assert_eq!(g2.missing.len(), 1);
    }

    #[test]
    fn qualifiers() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("33.0.0-android", "33.0.0"), Greater);
        assert_eq!(version_cmp("33.0.0-jre", "33.0.0-android"), Greater);
        assert_eq!(version_cmp("1.0.0-beta01", "1.0.0"), Less);
    }
}
