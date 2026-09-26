//! Library versions from what the build leaves behind (docs/research/sdk-anchors.md §1): facts
//! about which library versions are in the app, never names.
//!
//! * `META-INF/<group>_<artifact>.version` (AndroidX, kotlinx): the version, verbatim.
//! * Root `<name>.properties` with `version=` (Play services, Firebase): the version, and the
//!   artifact from `client=` when present.
//! * `name/1.2.3` string constants (User-Agent style: `okhttp/4.12.0`, `Crashlytics Android
//!   SDK/20.1.1`); three version components (`HTTP/1.1` is a protocol).
//! * Two constant strings passed to one call, a library id and a version (Firebase registrars'
//!   `LibraryVersionComponent.create("fire-auth", "24.2.0")`).

use std::collections::BTreeSet;

use eightr_ir::model::Program as Model;
use eightr_ir::op::Op;

use crate::report::LibraryVersion;

/// `1.2`, `1.2.3`, `20.1.1-beta01`: a version string.
pub fn is_version(v: &str) -> bool {
    let (core, suffix) = v.split_once('-').map_or((v, ""), |(c, s)| (c, s));
    let parts: Vec<&str> = core.split('.').collect();
    (2..=4).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// Evidence from small text resources of the package (name, contents).
pub fn from_resources(files: &[(String, String)]) -> Vec<LibraryVersion> {
    let mut out = Vec::new();
    for (name, text) in files {
        if let Some(file) = name.strip_prefix("META-INF/").and_then(|n| n.strip_suffix(".version")) {
            let v = text.trim();
            if !is_version(v) {
                continue; // e.g. arch.core_core-runtime.version = "task" (an AndroidX bug)
            }
            let library = match file.split_once('_') {
                Some((group, artifact)) => format!("{group}:{artifact}"),
                None => file.to_string(),
            };
            out.push(LibraryVersion { library, versions: vec![v.to_string()], evidence: name.clone() });
        } else if let Some(stem) = name.strip_suffix(".properties").filter(|n| !n.contains('/')) {
            let prop = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key).and_then(|r| r.trim_start().strip_prefix('=')).map(|v| v.trim().to_string()));
            let Some(v) = prop("version").filter(|v| is_version(v)) else { continue };
            let library = prop("client").unwrap_or_else(|| stem.to_string());
            out.push(LibraryVersion { library, versions: vec![v], evidence: name.clone() });
        }
    }
    out
}

/// Evidence from string constants in the code.
pub fn from_code(p: &Model) -> Vec<LibraryVersion> {
    let s = &p.syms;
    let mut found: BTreeSet<(String, String, String)> = BTreeSet::new();
    for c in &p.classes {
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            let mut consts: std::collections::BTreeMap<u16, String> = std::collections::BTreeMap::new();
            for x in &b.insns {
                match &x.op {
                    Op::ConstString { dst, value } => {
                        let v = s.get(*value);
                        // `name/1.2.3`
                        if let Some((name, ver)) = v.rsplit_once('/') {
                            let name_ok = name.len() <= 40
                                && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
                                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b" ._-".contains(&b));
                            let ver = ver.strip_prefix('v').unwrap_or(ver);
                            // Three components: `HTTP/1.1`, `spdy/3.1` are protocols, not libraries.
                            let three = ver.split('-').next().is_some_and(|c| c.split('.').count() >= 3);
                            if name_ok && three && is_version(ver) {
                                found.insert((name.to_string(), ver.to_string(), format!("string \"{v}\"")));
                            }
                        }
                        consts.insert(*dst, v.to_string());
                        continue;
                    }
                    // (library id, version) passed to one static call (a registrar's factory).
                    Op::Invoke { kind: eightr_ir::op::InvokeKind::Static, args, .. } => {
                        let strs: Vec<&String> = args.iter().filter_map(|r| consts.get(r)).collect();
                        if let [id, ver] = strs[..] {
                            let slug = id.len() <= 60
                                && id.contains('-')
                                && id.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                                && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
                            if slug && is_version(ver) {
                                found.insert((id.to_string(), ver.to_string(), format!("call with \"{id}\", \"{ver}\"")));
                            }
                        }
                    }
                    _ => {}
                }
                if let Some((d, wide)) = x.op.def() {
                    consts.remove(&d);
                    if wide {
                        consts.remove(&(d + 1));
                    }
                }
            }
        }
    }
    found.into_iter().map(|(library, v, evidence)| LibraryVersion { library, versions: vec![v], evidence }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_resources() {
        assert!(is_version("1.12.1") && is_version("10.0.0-beta05") && is_version("2.0"));
        assert!(!is_version("task") && !is_version("1") && !is_version("1.x"));
        let files = vec![
            ("META-INF/androidx.compose.runtime_runtime.version".to_string(), "1.12.1\n".to_string()),
            ("META-INF/androidx.arch.core_core-runtime.version".to_string(), "task\n".to_string()),
            ("play-services-basement.properties".to_string(), "version=18.9.0\nclient=play-services-basement\n".to_string()),
            ("firebase-auth.properties".to_string(), "version=24.2.0\n".to_string()),
        ];
        let got: Vec<(String, String)> = from_resources(&files).into_iter().map(|l| (l.library, l.versions[0].clone())).collect();
        assert_eq!(
            got,
            [
                ("androidx.compose.runtime:runtime".to_string(), "1.12.1".to_string()),
                ("play-services-basement".to_string(), "18.9.0".to_string()),
                ("firebase-auth".to_string(), "24.2.0".to_string()),
            ]
        );
    }
}
