//! The build profile of an app: what a library-fingerprint pack must be forged for
//! (docs/research/libdb.md). Every field is an S fact read from the app: the R8 version, min-api
//! and mode from its marker, and the Maven coordinates of the libraries whose exact versions the
//! package declares (`META-INF/<group>_<artifact>.version`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::input::DexInput;
use crate::marker::Marker;
use crate::report::{LibraryVersion, Report};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Coord {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

impl Coord {
    /// `group:artifact:version`.
    pub fn parse(s: &str) -> Option<Coord> {
        let mut it = s.split(':');
        let (g, a, v) = (it.next()?, it.next()?, it.next()?);
        (it.next().is_none() && !g.is_empty() && !a.is_empty() && !v.is_empty()).then(|| Coord { group: g.into(), artifact: a.into(), version: v.into() })
    }
}

impl std::fmt::Display for Coord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.group, self.artifact, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// R8 version, e.g. `9.4.24`.
    pub r8: String,
    pub min_api: u32,
    /// `full` or `compatibility`.
    pub mode: String,
    /// Declared libraries, sorted.
    pub libraries: Vec<Coord>,
}

impl Profile {
    /// Canonical JSON (sorted libraries, fixed field order).
    pub fn canonical(&self) -> String {
        let mut p = self.clone();
        p.libraries.sort();
        p.libraries.dedup();
        serde_json::to_string(&p).expect("serializable")
    }

    /// Short content hash of the canonical form.
    pub fn key(&self) -> String {
        let h = Sha256::digest(self.canonical().as_bytes());
        h.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    pub fn parse(json: &str) -> Result<Profile, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    /// The profile of an app from its report: `None` without an R8 marker.
    pub fn from_report(r: &Report) -> Option<Profile> {
        Profile::from_parts(&r.markers, &r.libraries)
    }

    /// The profile of an app from its dex files and package resources alone (no pipeline run).
    pub fn from_inputs(inputs: &[DexInput], resources: &[(String, String)]) -> Result<Option<Profile>, String> {
        let mut dexes = Vec::new();
        for i in inputs {
            dexes.push((i.name.clone(), eightr_dex::Dex::parse(&i.bytes).map_err(|e| format!("{}: {e:?}", i.name))?));
        }
        let markers = crate::pipeline::collect_markers(&dexes);
        Ok(Profile::from_parts(&markers, &crate::libraries::from_resources(resources)))
    }

    /// The profile from already collected markers and the package resources.
    pub fn from_markers(markers: &[Marker], resources: &[(String, String)]) -> Option<Profile> {
        Profile::from_parts(markers, &crate::libraries::from_resources(resources))
    }

    fn from_parts(markers: &[Marker], libs: &[LibraryVersion]) -> Option<Profile> {
        let m = markers.iter().find(|m| m.tool == "R8")?;
        let mut libraries: Vec<Coord> = libs.iter().filter(|l| l.versions.len() == 1).filter_map(|l| coord_of(&l.library, &l.evidence, &l.versions[0])).collect();
        libraries.sort();
        libraries.dedup();
        Some(Profile {
            r8: m.version()?.to_string(),
            min_api: u32::try_from(m.min_api()?).ok()?,
            mode: m.r8_mode().unwrap_or("full").to_string(),
            libraries,
        })
    }
}

/// The Maven coordinate a `.version` resource names. AndroidX names the file
/// `<group>_<artifact>.version`; kotlinx libraries name it `kotlinx_<name>.version` for
/// `org.jetbrains.kotlinx:kotlinx-<name>` (underscores as dashes). Other evidence names no
/// coordinate.
fn coord_of(library: &str, evidence: &str, version: &str) -> Option<Coord> {
    if !evidence.starts_with("META-INF/") || !evidence.ends_with(".version") {
        return None;
    }
    let (g, a) = library.split_once(':')?;
    let (group, artifact) = if g == "kotlinx" { ("org.jetbrains.kotlinx".to_string(), format!("kotlinx-{}", a.replace('_', "-"))) } else { (g.to_string(), a.to_string()) };
    group.contains('.').then(|| Coord { group, artifact, version: version.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_from_version_files_only() {
        assert_eq!(coord_of("androidx.activity:activity", "META-INF/androidx.activity_activity.version", "1.13.0"), Coord::parse("androidx.activity:activity:1.13.0"));
        assert_eq!(coord_of("kotlinx:coroutines_core", "META-INF/kotlinx_coroutines_core.version", "1.11.0"), Coord::parse("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0"));
        assert_eq!(coord_of("androidx.compose:runtime", "compose keys 3/4", "1.12.1"), None);
        assert_eq!(coord_of("fire-core", "call with \"fire-core\"", "22.2.1"), None);
    }

    #[test]
    fn key_ignores_library_order() {
        let a = Profile { r8: "9.4.24".into(), min_api: 26, mode: "full".into(), libraries: vec![Coord::parse("a.b:c:1").unwrap(), Coord::parse("a.b:d:2").unwrap()] };
        let mut b = a.clone();
        b.libraries.reverse();
        assert_eq!(a.key(), b.key());
        assert_eq!(Profile::parse(&a.canonical()).unwrap().canonical(), a.canonical());
    }
}
