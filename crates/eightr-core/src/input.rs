//! Loading dex files from a `.dex`, an APK/AAB/zip, or a directory.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};

/// One dex file. `name` is the file name only (never a path), so reports don't depend on
/// where the input lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexInput {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// `classes.dex` → 1, `classes2.dex` → 2, …; anything else sorts after, by name.
fn dex_order(name: &str) -> (u32, String) {
    let base = name.rsplit('/').next().unwrap_or(name);
    let n = base
        .strip_prefix("classes")
        .and_then(|s| s.strip_suffix(".dex"))
        .and_then(|s| if s.is_empty() { Some(1) } else { s.parse().ok() })
        .unwrap_or(u32::MAX);
    (n, name.to_string())
}

fn is_multidex_name(name: &str) -> bool {
    dex_order(name).0 != u32::MAX
}

/// Sorts inputs into canonical multidex order. Callers may pass inputs in any order.
pub fn canonical_order(inputs: &mut [DexInput]) {
    inputs.sort_by_key(|i| dex_order(&i.name));
}

pub fn load(path: &Path) -> Result<Vec<DexInput>> {
    let display = path.display().to_string();
    let io = |e| Error::Io(display.clone(), e);
    let mut inputs = Vec::new();
    if path.is_dir() {
        for entry in fs::read_dir(path).map_err(io)? {
            let p = entry.map_err(io)?.path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if p.is_file() && is_multidex_name(&name) {
                inputs.push(DexInput { bytes: fs::read(&p).map_err(|e| Error::Io(p.display().to_string(), e))?, name });
            }
        }
    } else {
        let bytes = fs::read(path).map_err(io)?;
        if bytes.starts_with(b"PK") {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| Error::Zip(e.to_string()))?;
            for i in 0..zip.len() {
                let mut f = zip.by_index(i).map_err(|e| Error::Zip(e.to_string()))?;
                let name = f.name().to_string();
                // APK: classes*.dex at the root. AAB: base/dex/classes*.dex.
                let top_level = !name.contains('/') || name.starts_with("base/dex/");
                if top_level && is_multidex_name(&name) {
                    let mut bytes = Vec::new();
                    f.read_to_end(&mut bytes).map_err(|e| Error::Io(name.clone(), e))?;
                    inputs.push(DexInput { name: name.rsplit('/').next().unwrap().to_string(), bytes });
                }
            }
        } else {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            inputs.push(DexInput { name, bytes });
        }
    }
    if inputs.is_empty() {
        return Err(Error::NoDexFiles(display));
    }
    canonical_order(&mut inputs);
    Ok(inputs)
}

/// Small text resources carrying library versions: `META-INF/*.version` and root
/// `*.properties` of an APK/AAB/zip (AAB: under `base/root/`). Empty for other inputs.
pub fn resources(path: &Path) -> Vec<(String, String)> {
    let Ok(bytes) = fs::read(path) else { return Vec::new() };
    if !bytes.starts_with(b"PK") {
        return Vec::new();
    }
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let Ok(mut f) = zip.by_index(i) else { continue };
        let name = f.name().strip_prefix("base/root/").unwrap_or(f.name()).to_string();
        let wanted = (name.starts_with("META-INF/") && name.ends_with(".version") && name.matches('/').count() == 1)
            || (name.ends_with(".properties") && !name.contains('/'));
        if !wanted || f.size() > 4096 {
            continue;
        }
        let mut text = String::new();
        if f.read_to_string(&mut text).is_ok() {
            out.push((name, text));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multidex_order() {
        let mut v: Vec<DexInput> = ["classes10.dex", "classes2.dex", "classes.dex", "other.dex"]
            .iter()
            .map(|n| DexInput { name: n.to_string(), bytes: vec![] })
            .collect();
        canonical_order(&mut v);
        let names: Vec<_> = v.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["classes.dex", "classes2.dex", "classes10.dex", "other.dex"]);
    }
}
