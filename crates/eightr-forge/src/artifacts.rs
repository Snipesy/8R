//! A resolved artifact as the Android Gradle plugin hands it to R8: its class jars (an AAR's
//! `classes.jar` and `libs/*.jar`) and its consumer keep rules (an AAR's `proguard.txt`; a jar's
//! `META-INF/com.android.tools/r8*` directories whose version range contains the R8 version, else
//! `META-INF/proguard/*.pro`).

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::maven::{version_cmp, Resolved};
use crate::tools::{read, write, Result};

#[derive(Debug, Clone)]
pub struct Lib {
    pub coord: String,
    pub jars: Vec<PathBuf>,
    pub rules: Vec<PathBuf>,
    /// An AAR's `R.txt` (the resources it declares, `int <type> <name> <value>` and
    /// `int[] styleable <name> { … }` lines).
    pub r_txt: Option<String>,
}

fn entries(zip: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let bytes = read(zip)?;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let wanted = name == "classes.jar" || name == "proguard.txt" || name == "R.txt" || (name.starts_with("libs/") && name.ends_with(".jar")) || (name.starts_with("META-INF/") && name.ends_with(".pro"));
        if wanted {
            let mut b = Vec::new();
            f.read_to_end(&mut b).map_err(|e| e.to_string())?;
            out.push((name, b));
        }
    }
    out.sort();
    Ok(out)
}

/// Whether an AGP rule directory (`r8`, `r8-from-8.0.0`, `r8-upto-8.2.0`, `r8-from-X-upto-Y`)
/// applies to R8 `version` (from inclusive, upto exclusive).
pub fn r8_dir_applies(dir: &str, version: &str) -> bool {
    let Some(rest) = dir.strip_prefix("r8") else { return false };
    let (mut from, mut upto) = (None, None);
    let mut r = rest;
    if let Some(x) = r.strip_prefix("-from-") {
        let (f, tail) = x.split_once("-upto-").map_or((x, None), |(a, b)| (a, Some(b)));
        from = Some(f);
        upto = tail;
        r = "";
    } else if let Some(x) = r.strip_prefix("-upto-") {
        upto = Some(x);
        r = "";
    }
    r.is_empty() && from.is_none_or(|f| version_cmp(version, f).is_ge()) && upto.is_none_or(|u| version_cmp(version, u).is_lt())
}

/// Unpacks `r` into `<cache>/libs/<sha256>/` (once).
pub fn prepare(r: &Resolved, r8_version: &str) -> Result<Lib> {
    let dir = crate::tools::cache_root().join("libs").join(&r.sha256);
    let ents = entries(&r.file)?;
    let mut jars = Vec::new();
    let mut rules = Vec::new();
    let mut r_txt = None;
    let is_aar = r.url.ends_with(".aar");
    let put = |name: &str, bytes: &[u8]| -> Result<PathBuf> {
        let p = dir.join(name.replace('/', "_"));
        if !p.exists() {
            write(&p, bytes)?;
        }
        Ok(p)
    };
    if is_aar {
        for (n, b) in &ents {
            if n == "classes.jar" || n.starts_with("libs/") {
                jars.push(put(n, b)?);
            } else if n == "proguard.txt" {
                rules.push(put(n, b)?);
            } else if n == "R.txt" {
                r_txt = Some(String::from_utf8_lossy(b).into_owned());
            }
        }
    } else {
        jars.push(r.file.clone());
        let r8: Vec<&(String, Vec<u8>)> = ents
            .iter()
            .filter(|(n, _)| n.strip_prefix("META-INF/com.android.tools/").and_then(|x| x.split_once('/')).is_some_and(|(d, _)| r8_dir_applies(d, r8_version)))
            .collect();
        let chosen: Vec<&(String, Vec<u8>)> = if r8.is_empty() { ents.iter().filter(|(n, _)| n.starts_with("META-INF/proguard/")).collect() } else { r8 };
        for (n, b) in chosen {
            rules.push(put(n, b)?);
        }
    }
    Ok(Lib { coord: r.coord.to_string(), jars, rules, r_txt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agp_rule_directories() {
        assert!(r8_dir_applies("r8", "9.4.24"));
        assert!(r8_dir_applies("r8-from-8.0.0", "9.4.24"));
        assert!(!r8_dir_applies("r8-upto-8.0.0", "9.4.24"));
        assert!(r8_dir_applies("r8-from-1.6.0-upto-10.0.0", "9.4.24"));
        assert!(!r8_dir_applies("r8-from-9.5.0", "9.4.24"));
        assert!(!r8_dir_applies("proguard", "9.4.24"));
    }
}
