//! L3 offline (docs/research/libdb.md §11): a pack merged from fixture builds of the Compose
//! runtime (R8 output and mapping already in `fixtures/out`, so no network) is applied to another
//! fixture app built from the same libraries. Every S library label must be its mapping's truth,
//! S classes go back to their packages with the `@eightr.Library` tag, and 8R's output is a fixed
//! point given the pack, and α-invariant with it.

use std::path::PathBuf;

#[path = "../../eightr-core/tests/support/alpha.rs"]
#[allow(dead_code)]
mod alpha;

use eightr_core::input::DexInput;
use eightr_core::libdb::pack::NO_ARTIFACT;
use eightr_core::libdb::{Pack, Profile};
use eightr_core::output::emit;
use eightr_core::{run, Config};

fn r8_out(fixture: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out").join(fixture).join("r8")
}

fn library(class: &str) -> bool {
    class.starts_with("Landroidx/") || class.starts_with("Lkotlinx/")
}

/// The pack of `scenarios` (fixture builds, each graded by its own mapping).
fn pack(scenarios: &[&str]) -> Pack {
    let outs: Vec<_> = scenarios.iter().map(|s| eightr_forge::fingerprint::scenario(&r8_out(s), &library).unwrap()).collect();
    let head = Pack {
        profile: Profile::parse(r#"{"r8":"9.4.24","min_api":24,"mode":"full","libraries":[]}"#).unwrap(),
        lock: Vec::new(),
        catalog: String::new(),
        tools: vec![("fingerprint".into(), eightr_core::libdb::code_id())],
        scenarios: scenarios.iter().map(|s| s.to_string()).collect(),
        artifacts: vec![("androidx.compose.runtime:runtime:test".into(), true)],
        classes: Vec::new(),
        methods: Vec::new(),
        fields: Vec::new(),
        records: Vec::new(),
        class_records: Vec::new(),
        stacks: Vec::new(),
        app: String::new(),
    };
    eightr_forge::build::merge(head, &|c| if library(c) { 0 } else { NO_ARTIFACT }, &outs)
}

const APP: &str = "compose_witness2";

fn the_pack() -> Pack {
    pack(&["compose_shapes", "compose_witness"])
}

#[test]
fn solved_library_labels_are_the_mappings_truth() {
    let pack = the_pack();
    let mut grades = eightr_forge::harness::Grades::new();
    eightr_forge::harness::grade(&r8_out(APP), &r8_out(APP).join("mapping.txt"), &pack, &mut grades).unwrap();
    eightr_forge::harness::print(&grades);
    let solved = |rule: &str, attr: &str| grades.get(&(rule.to_string(), attr.to_string(), "S".to_string())).map_or(0, |t| t.total);
    for ((rule, attr, tier), t) in &grades {
        if tier == "S" {
            assert_eq!(t.correct, t.total, "{rule} {attr}: wrong S labels {:?}", t.wrong);
        }
    }
    // The rules this exercises.
    assert!(solved("r8/libdb-method-name", "MemberName") > 50);
    assert!(solved("r8/libdb-class-name", "ClassName") > 0);
    assert!(solved("r8/libdb-class-name", "Package") > 0);
    assert!(solved("r8/libdb-field-name", "MemberName") > 0);
}

fn dex(outcome: &eightr_core::Outcome) -> Vec<DexInput> {
    emit(outcome).unwrap().dex.into_iter().map(|(name, bytes)| DexInput { name, bytes }).collect()
}

#[test]
fn packages_restored_tagged_and_output_is_a_fixed_point() {
    let inputs = eightr_core::input::load(&r8_out(APP)).unwrap();
    // A pack forged for this app (its code's id), as `8r-forge build APK` makes.
    let plain = run(&inputs, &Config::default()).unwrap();
    let app = eightr_core::libdb::app_id(&plain.report.inputs.iter().map(|x| x.sha256.clone()).collect::<Vec<_>>());
    let mut pack = the_pack();
    pack.app = app.clone();
    let first = run(&inputs, &Config { packs: vec![pack.clone()], libdb_trust: true, ..Default::default() }).unwrap();
    // Restored: S classes whose final descriptor is in their original package.
    let restored = first.renaming.classes.iter().filter(|(old, new)| eightr_core::program::package_of(old) != eightr_core::program::package_of(new) && library(new)).count();
    assert!(restored > 0, "no library class went back to its package");
    let out = dex(&first);
    let tag = eightr_core::passes::libdb_names::LIBRARY.as_bytes();
    assert!(out.iter().any(|d| d.bytes.windows(tag.len()).any(|w| w == tag)), "no @eightr.Library annotation in the output");
    // 8R on its own output, the pack given as to any app (not trusted): its tags say the code is
    // this app's, so the pack applies and the output is the same bytes.
    let second = run(&out, &Config { packs: vec![pack.clone()], ..Default::default() }).unwrap();
    assert!(second.report.findings.iter().any(|f| f.message.contains("matches the build profile")), "the app's pack wasn't used on 8R's output");
    for (a, b) in out.iter().zip(&dex(&second)) {
        assert!(a.bytes == b.bytes, "{} changed on a second run", a.name);
    }
    // A pack of another app isn't.
    pack.app = "0".repeat(64);
    let other = run(&out, &Config { packs: vec![pack], ..Default::default() }).unwrap();
    assert!(other.report.findings.iter().any(|f| f.message.contains("forged for another app")));
}

/// α-invariance with a pack: R8's arbitrary names and file order don't change what the pack
/// names (the scrambled app is matched against the same pack, trusted as for grading).
#[test]
fn library_names_are_alpha_invariant() {
    let cfg = Config { packs: vec![the_pack()], libdb_trust: true, ..Default::default() };
    assert!(alpha::check(APP, &cfg, 1..=3) > 100);
}
