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
            // A D8 build with too few composables to find the composer (the library composables
            // are only in the R8 build) still references the runtime's Composer.
            let uses_compose = gm.syms.lookup("Landroidx/compose/runtime/Composer;").is_some();
            assert!(found.is_none() || uses_compose, "{name}: composer found in a build without Compose");
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
        let missed: Vec<&i32> = tk.iter().filter(|k| r8_consts.contains(k) && !fk.contains(k)).collect();
        assert!(missed.is_empty(), "{name}: restartable composables not found: keys {missed:?}");

        // Roles: in the D8 build every role names a method already called that; in the R8 build
        // (graded against the mapping by oracle.rs) the core roles are all found.
        let truth_roles = eightr_core::compose::roles(&gm, &truth);
        for r in &truth_roles.composer {
            assert_eq!(r.method.0, r.name, "{name}: D8 role {r:?}");
        }
        assert_eq!(truth_roles.update_changed_flags.as_ref().map(|m| m.1.as_str()), Some("updateChangedFlags"), "{name}: D8");
        let roles = eightr_core::compose::roles(&om, &found);
        let named: std::collections::BTreeSet<&str> = roles.composer.iter().map(|r| r.name).collect();
        for core in ["startRestartGroup", "endRestartGroup", "skipToGroupEnd", "changed"] {
            assert!(named.contains(core), "{name}: role {core} not found in the R8 build ({named:?})");
        }
        // The skip check of the compiler era that built the D8 twin.
        let era: Vec<&str> = truth_roles.composer.iter().map(|r| r.name).filter(|n| matches!(*n, "shouldExecute" | "getSkipping")).collect();
        assert_eq!(era.len(), 1, "{name}: D8 skip check {era:?}");
        assert!(named.contains(era[0]), "{name}: {} not found in the R8 build ({named:?})", era[0]);
        assert!(roles.update_changed_flags.is_some(), "{name}: updateChangedFlags not found in the R8 build");

        // ComposableSingletons: the D8 build's lambda$K fields (its lambda class is on the
        // classpath only, so they're read by name) are all found in the R8 build.
        let r8_keys: std::collections::BTreeSet<i32> = eightr_core::compose::singletons(&om, &found).iter().map(|x| x.2).collect();
        let d8_keys: std::collections::BTreeSet<i32> = gm
            .classes
            .iter()
            .flat_map(|c| &c.fields)
            .filter_map(|f| gm.syms.get(f.name).strip_prefix("lambda$").and_then(|k| k.parse().ok()))
            .collect();
        // Recall on the app's fields (precision is graded against the mapping, library fields included).
        assert!(d8_keys.is_subset(&r8_keys) && !r8_keys.is_empty(), "{name}: R8 singletons {r8_keys:?} vs D8 {d8_keys:?}");
    }
    assert!(compose_fixtures >= 3, "only {compose_fixtures} Compose fixtures");
}

/// Parameter roles and bindings (crates/eightr-core/src/composables.rs) against the D8 twins'
/// debug-info parameter names (exact truth: nothing removed or reordered), and the R8 builds
/// against the D8 twin with the same entry key (counts, bounds, and types at bound positions).
#[test]
fn composable_parameter_roles_match_ground_truth() {
    let mut names: Vec<String> = fs::read_dir(root()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let (mut d8_changed, mut d8_changed_found, mut d8_defaults, mut d8_defaults_found) = (0, 0, 0, 0);
    let (mut slot_checked, mut bit_checked, mut r8_checked, mut r8_slot_types, mut r8_bit_types) = (0, 0, 0, 0, 0);
    for name in names {
        let (r8, d8) = (root().join(&name).join("r8/classes.dex"), root().join(&name).join("d8/classes.dex"));
        if !r8.exists() || !d8.exists() {
            continue;
        }
        let (om, gm) = (load(&r8), load(&d8));
        let Some(gc) = eightr_core::compose::find(&gm) else { continue };
        let groles = eightr_core::compose::roles(&gm, &gc);
        let truth = eightr_core::composables::composables(&gm, &gc, &groles);
        // D8: key → (param types, param names, static).
        let mut by_key: std::collections::BTreeMap<i32, (Vec<String>, Vec<String>, bool)> = std::collections::BTreeMap::new();
        for c in &truth {
            let m = &gm.classes[c.class].methods[c.method];
            let b = m.code.as_ref().unwrap();
            let (types, _) = eightr_ir::types::parse_proto(gm.syms.get(m.proto)).unwrap();
            let mut pn: Vec<String> = (0..types.len()).map(|j| b.parameter_names.get(j).copied().flatten().map_or(String::new(), |n| gm.syms.get(n).to_string())).collect();
            // kotlinc leaves `$default` unnamed: the unnamed ints after the `$changed` ints.
            if let Some(last) = pn.iter().rposition(|n| n.starts_with("$changed")) {
                for j in last + 1..pn.len() {
                    if pn[j].is_empty() && types[j] == "I" {
                        pn[j] = "$default".into();
                    }
                }
            }
            let ctx = format!("{name} D8 {}", c.signature);
            assert_eq!(c.composer.map(|j| pn[j].as_str()), Some("$composer"), "{ctx}: {pn:?}");
            for &j in &c.changed {
                assert!(pn[j].starts_with("$changed"), "{ctx}: $changed claimed for {} ({pn:?})", pn[j]);
            }
            for &j in &c.defaults {
                assert!(pn[j].starts_with("$default"), "{ctx}: $default claimed for {} ({pn:?})", pn[j]);
            }
            if !c.restart.is_empty() {
                d8_changed += pn.iter().filter(|n| n.starts_with("$changed")).count();
                d8_changed_found += c.changed.len();
            }
            d8_defaults += pn.iter().filter(|n| n.starts_with("$default")).count();
            if std::env::var_os("EIGHTR_DEBUG_MISSES").is_some() && pn.iter().filter(|n| n.starts_with("$default")).count() > c.defaults.len() {
                eprintln!("MISS $default: {ctx}");
            }
            d8_defaults_found += c.defaults.len();
            // Real params come before $composer; slot j (the dispatch receiver takes the last).
            let composer = c.composer.unwrap();
            let changed_ints: Vec<usize> = (0..pn.len()).filter(|&j| pn[j].starts_with("$changed")).collect();
            for &(j, q, s) in &c.slots {
                assert!(j < composer, "{ctx}: slot binding on synthetic param {j}");
                assert_eq!((s as usize, Some(q)), (j % 10, changed_ints.get(j / 10).copied()), "{ctx}: slot binding of param {j} ({pn:?})");
                slot_checked += 1;
            }
            let receivers = pn.iter().take_while(|n| n.starts_with("$this") || n.starts_with("$context")).count();
            for &(j, i) in &c.default_bits {
                assert_eq!(i as usize + receivers, j, "{ctx}: default bit of param {j} ({pn:?})");
                bit_checked += 1;
            }
            let slots = composer as u32 + u32::from(!m.is_static());
            assert!(c.slots_at_least <= slots, "{ctx}: slot bound {} > {slots}", c.slots_at_least);
            assert_eq!(c.removed_at_least, 0, "{ctx}: nothing is removed in D8");
            by_key.insert(c.key, (types.iter().map(|t| t.to_string()).collect(), pn, m.is_static()));
        }

        let oc = eightr_core::compose::find(&om).unwrap();
        let oroles = eightr_core::compose::roles(&om, &oc);
        for c in eightr_core::composables::composables(&om, &oc, &oroles) {
            let Some((gtypes, pn, gstatic)) = by_key.get(&c.key) else { continue };
            let ctx = format!("{name} R8 {} (key {})", c.signature, c.key);
            let m = &om.classes[c.class].methods[c.method];
            let (types, _) = eightr_ir::types::parse_proto(om.syms.get(m.proto)).unwrap();
            let count = |p: &str| pn.iter().filter(|n| n.starts_with(p)).count();
            assert!(c.composer.is_some(), "{ctx}: no $composer");
            assert!(c.changed.len() <= count("$changed"), "{ctx}: {} $changed, D8 has {}", c.changed.len(), count("$changed"));
            assert!(c.defaults.len() <= count("$default"), "{ctx}: {} $default, D8 has {}", c.defaults.len(), count("$default"));
            let gcomposer = pn.iter().position(|n| n == "$composer").unwrap();
            let slots = gcomposer as u32 + u32::from(!gstatic);
            assert!(c.slots_at_least <= slots, "{ctx}: slot bound {} > original {slots}", c.slots_at_least);
            // Not from 8R's own role claims (a false `$default` would loosen its own bound): the
            // residual has at least `params - 1 - (D8's synthetic ints)` real params left.
            let real_left = types.len().saturating_sub(1 + count("$changed") + count("$default"));
            assert!(c.removed_at_least as usize <= (slots as usize).saturating_sub(real_left), "{ctx}: removed bound {} ({pn:?})", c.removed_at_least);
            // A primitive stays the same primitive; a reference stays a reference.
            let kind = |t: &str| if t.starts_with('L') || t.starts_with('[') { "L".to_string() } else { t.to_string() };
            if count("$changed") == 1 {
                for &(j, _, s) in &c.slots {
                    assert_eq!(kind(types[j]), kind(&gtypes[s as usize]), "{ctx}: slot {s} type");
                    r8_slot_types += 1;
                }
            }
            let receivers = pn.iter().take_while(|n| n.starts_with("$this") || n.starts_with("$context")).count();
            for &(j, i) in &c.default_bits {
                assert_eq!(kind(types[j]), kind(&gtypes[i as usize + receivers]), "{ctx}: default bit {i} type");
                r8_bit_types += 1;
            }
            r8_checked += 1;
        }
    }
    eprintln!(
        "D8: $changed {d8_changed_found}/{d8_changed}, $default {d8_defaults_found}/{d8_defaults}, slot bindings {slot_checked}, default bindings {bit_checked}; R8: {r8_checked} composables, {r8_slot_types} slot / {r8_bit_types} default types"
    );
    assert_eq!(d8_changed_found, d8_changed, "D8 $changed recall");
    assert!(d8_defaults_found * 10 >= d8_defaults * 8, "D8 $default recall {d8_defaults_found}/{d8_defaults}");
    assert!(slot_checked >= 20 && bit_checked >= 5 && r8_checked >= 20, "too few checks");
}

/// Library composables named by key (crates/eightr-core/src/compose_keys.rs) against the
/// compose_lib mapping: every corroborated (S) identification is right, and the entry-key-only
/// (D) ones nearly all.
#[test]
fn library_composables_by_key_match_mapping() {
    use eightr_mapping::{Mapping, MemberKind};
    let dir = root().join("compose_lib/r8");
    let m = load(&dir.join("classes.dex"));
    let mapping = Mapping::parse_normalized(&fs::read_to_string(dir.join("mapping.txt")).unwrap()).unwrap();
    let c = eightr_core::compose::find(&m).unwrap();
    let found = eightr_core::compose_keys::identify(&m, &c, eightr_core::compose_keys::KeyDb::embedded());
    let (mut s_ok, mut s_all, mut d_ok, mut d_all) = (0, 0, 0, 0);
    for f in &found {
        let (class, rest) = f.method_ref.split_once("->").unwrap();
        let name = rest.split('(').next().unwrap();
        let dotted = class.strip_prefix('L').and_then(|x| x.strip_suffix(';')).unwrap().replace('/', ".");
        let originals: std::collections::BTreeSet<String> = mapping
            .classes
            .iter()
            .filter(|cm| cm.obfuscated == dotted)
            .flat_map(|cm| cm.outermost_methods())
            .filter(|(mm, _)| mm.obfuscated == name)
            .map(|(mm, _)| mm.original_name.clone())
            .collect();
        let ok = originals.len() == 1 && originals.contains(&f.name);
        if f.corroborated {
            s_all += 1;
            s_ok += usize::from(ok);
            assert!(ok, "S key match {} (key {}) named {} but the mapping says {originals:?}", f.method_ref, f.key, f.name);
        } else {
            d_all += 1;
            d_ok += usize::from(ok);
            if !ok {
                eprintln!("D key match {} named {} but the mapping says {originals:?}", f.method_ref, f.name);
            }
        }
        let _ = MemberKind::Field;
    }
    eprintln!("compose_lib key matches: S {s_ok}/{s_all}, D {d_ok}/{d_all}");
    assert!(s_all >= 20, "only {s_all} corroborated matches");
    assert!(d_ok * 100 >= d_all * 95, "D precision {d_ok}/{d_all}");
    // Version resolution: material3 1.4.0 and compose 1.10.x.
    let versions = eightr_core::compose_keys::versions(&m, &found, eightr_core::compose_keys::KeyDb::embedded());
    eprintln!("{versions:?}");
    let v = |a: &str| versions.iter().find(|x| x.0 == a).map(|x| x.1.clone()).unwrap_or_default();
    assert_eq!(v("material3"), ["1.4.0"]);
    assert_eq!(v("foundation"), ["1.10.6"]);
    // Every artifact's tied set contains the true version (compose 1.10.6).
    for (a, vs, _, _) in &versions {
        assert!(vs.iter().any(|x| x == "1.10.6" || (a == "material3" && x == "1.4.0")), "{a}: {vs:?}");
    }
}
