//! End-to-end forge runs. They need the network (Google Maven, Maven Central), `java`, `javac` and
//! an Android SDK, so they are ignored by default: `cargo test -p eightr-forge -- --ignored`.

use eightr_core::libdb::{Pack, Profile};
use eightr_forge::build::{build, Options};

/// A small profile forged twice from an empty cache (downloads kept) gives the same bytes.
#[test]
#[ignore]
fn packs_are_reproducible() {
    let cache = std::env::temp_dir().join(format!("8r-forge-test-{}", std::process::id()));
    std::env::set_var("EIGHTR_FORGE_CACHE", &cache);
    let profile = Profile::parse(r#"{"r8":"9.4.24","min_api":24,"mode":"full","libraries":[{"group":"com.squareup.okio","artifact":"okio-jvm","version":"3.6.0"}]}"#).unwrap();
    let opts = Options { catalog: "lib-alone all\nroots r1 frac=0.2 seed=1\ncallers c1 frac=0.2 seed=1\n".into(), pins: vec![], jobs: 2, log: false, app: None };
    let (path, pack) = build(&profile, &opts).unwrap();
    let first = std::fs::read(&path).unwrap();
    assert!(pack.records.len() > 500 && pack.scenarios.len() == 3);
    assert_eq!(Pack::decode(&first).unwrap(), pack);
    std::fs::remove_dir_all(cache.join("packs")).unwrap();
    std::fs::remove_dir_all(cache.join("work")).unwrap();
    let (path2, _) = build(&profile, &opts).unwrap();
    assert_eq!(path, path2);
    assert_eq!(std::fs::read(&path2).unwrap(), first);
    let _ = std::fs::remove_dir_all(&cache);
}
