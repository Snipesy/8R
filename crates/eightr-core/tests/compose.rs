//! Compose facts found structurally (crates/eightr-core/src/compose.rs), graded against the
//! fixtures' unminified D8 builds, where names and the compiler's output are intact.

use std::fs;
use std::path::{Path, PathBuf};

use eightr_dex::Dex;
use eightr_ir::model::Program as Model;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn load(p: &Path) -> Model {
    let bytes = fs::read(p).unwrap();
    let dex = Dex::parse(&bytes).unwrap();
    Model::load(&[&dex]).unwrap()
}

/// The R8 build's restartable composables (by entry key) are exactly the D8 build's app
/// composables that R8 kept: every key found in R8 is a D8 entry key, and every D8 key that
/// survives as a constant in the R8 build is found. Non-Compose fixtures have no composer.
#[test]
fn composer_and_restartable_composables_match_ground_truth() {
    let mut names: Vec<String> = fs::read_dir(root()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut compose_fixtures = 0;
    for name in names {
        let (r8, d8) = (root().join(&name).join("r8/classes.dex"), root().join(&name).join("d8/classes.dex"));
        if !r8.exists() || !d8.exists() {
            continue;
        }
        let (om, gm) = (load(&r8), load(&d8));
        let found = eightr_core::compose::find(&om);
        let truth = eightr_core::compose::find(&gm);
        let Some(truth) = truth else {
            assert!(found.is_none(), "{name}: composer found in a build without Compose");
            continue;
        };
        compose_fixtures += 1;
        assert_eq!(truth.class, "Landroidx/compose/runtime/Composer;", "{name}: D8 build's composer");
        assert_eq!(truth.start_restart_group.0, "startRestartGroup", "{name}");
        let found = found.unwrap_or_else(|| panic!("{name}: no composer found in the R8 build"));
        let keys = |c: &eightr_core::compose::Composer| c.restartable.iter().map(|x| x.2).collect::<std::collections::BTreeSet<i32>>();
        let (fk, tk) = (keys(&found), keys(&truth));
        assert!(fk.is_subset(&tk), "{name}: keys not in the D8 build: {:?}", fk.difference(&tk).collect::<Vec<_>>());
        // D8 keys present anywhere as constants in the R8 build must be found.
        let text = fs::read(&r8).unwrap();
        let r8_consts: std::collections::BTreeSet<i32> = om
            .classes
            .iter()
            .flat_map(|c| c.methods.iter())
            .filter_map(|m| m.code.as_ref())
            .flat_map(|b| b.insns.iter())
            .filter_map(|i| match i.op {
                eightr_ir::op::Op::Const { value: eightr_ir::op::Const::Narrow(k), .. } => Some(k),
                _ => None,
            })
            .collect();
        let _ = text;
        let missed: Vec<&i32> = tk.iter().filter(|k| r8_consts.contains(k) && !fk.contains(k)).collect();
        assert!(missed.is_empty(), "{name}: restartable composables not found: keys {missed:?}");
    }
    assert!(compose_fixtures >= 3, "only {compose_fixtures} Compose fixtures");
}
