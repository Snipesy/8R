//! Grades 8R against ground truth (DESIGN.md §6.1).
//!
//! 8R never sees a mapping file. These tests do: R8's mapping for each fixture is held back
//! and used to check that every S label 8R emits is actually correct. The D8 build of the same
//! source ("G") is a second, independent oracle.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use eightr_core::input::DexInput;
use eightr_core::program::{ItemId, Program};
use eightr_core::{run, Config, Outcome};
use eightr_mapping::{Mapping, Metadata};
use eightr_rules::{Attribute, Class};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn fixture_names() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(fixtures_root())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn load(fixture: &str, variant: &str) -> Vec<DexInput> {
    let p = fixtures_root().join(fixture).join(variant).join("classes.dex");
    vec![DexInput { name: "classes.dex".into(), bytes: fs::read(p).unwrap() }]
}

fn outcome(fixture: &str, variant: &str) -> Outcome {
    run(&load(fixture, variant), &Config { verbose_labels: true }).unwrap()
}

/// `java.lang.String[]` → `[Ljava/lang/String;`
fn java_to_descriptor(t: &str) -> String {
    let (base, dims) = {
        let mut base = t;
        let mut dims = 0;
        while let Some(b) = base.strip_suffix("[]") {
            base = b;
            dims += 1;
        }
        (base, dims)
    };
    let prim = match base {
        "void" => "V", "boolean" => "Z", "byte" => "B", "char" => "C", "short" => "S",
        "int" => "I", "long" => "J", "float" => "F", "double" => "D",
        _ => "",
    };
    let d = if prim.is_empty() { format!("L{};", base.replace('.', "/")) } else { prim.to_string() };
    "[".repeat(dims) + &d
}

fn dotted(descriptor: &str) -> String {
    descriptor.trim_start_matches('L').trim_end_matches(';').replace('/', ".")
}

/// Every S label on a name must agree with the mapping R8 wrote.
#[test]
fn solved_names_match_held_back_mapping() {
    let mut checked = 0;
    for fixture in fixture_names() {
        let mapping = Mapping::parse(&fs::read_to_string(fixtures_root().join(&fixture).join("r8/mapping.txt")).unwrap()).unwrap();
        let by_obf = mapping.by_obfuscated();
        let out = outcome(&fixture, "r8");
        let p = &out.program;
        for ((item, attr), label) in out.labels.iter() {
            if label.class != Class::Solved {
                continue;
            }
            let ctx = format!("{fixture}: {} {attr:?} (rules {:?})", p.describe(*item), label.rules);
            let (class_id, member) = match *item {
                ItemId::Class { class } => (class, None),
                ItemId::Field { class, index } => (class, Some((false, index))),
                ItemId::Method { class, index } => (class, Some((true, index))),
            };
            let class = p.class(class_id);
            let obf = dotted(&class.descriptor);
            let cm = &mapping.classes[*by_obf.get(obf.as_str()).unwrap_or_else(|| panic!("{ctx}: class not in mapping"))];
            match (attr, member) {
                (Attribute::ClassName | Attribute::Package, None) => {
                    assert_eq!(cm.original, obf, "{ctx}: claimed original, but R8 renamed {} -> {}", cm.original, obf);
                }
                (Attribute::MemberName, Some((false, index))) => {
                    let f = &class.fields[index as usize];
                    // R8 omits members it neither renamed nor attached line info to, so an
                    // absent member means "unchanged".
                    let matches: Vec<_> = cm.fields().filter(|m| m.obfuscated == f.name && java_to_descriptor(&m.ty) == f.ty).collect();
                    for m in matches {
                        assert_eq!(m.original_name, f.name, "{ctx}");
                    }
                }
                (Attribute::MemberName, Some((true, index))) => {
                    let meth = &class.methods[index as usize];
                    // Residual entries for this method: same obfuscated name, same class, and a
                    // signature matching the dex proto (or its residualsignature).
                    let matches: Vec<_> = cm
                        .outermost_methods()
                        .into_iter()
                        .filter(|(m, md)| {
                            let own = m.original_owner.as_deref().is_none_or(|o| o == cm.original);
                            let sig = format!(
                                "({}){}",
                                m.params.iter().map(|t| java_to_descriptor(t)).collect::<String>(),
                                java_to_descriptor(&m.return_type)
                            );
                            let residual = md.iter().find_map(|x| match &x.parsed {
                                Metadata::ResidualSignature(s) => Some(s.clone()),
                                _ => None,
                            });
                            m.obfuscated == meth.name && own && residual.unwrap_or(sig) == meth.proto
                        })
                        .collect();
                    for (m, _) in matches {
                        assert_eq!(m.original_name, meth.name, "{ctx}");
                    }
                }
                other => panic!("{ctx}: unexpected S label {other:?}"),
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no S labels were graded");
}

/// Independent check: S names from the R8 build must exist in the D8 build of the same source.
/// The mapping is used only to find a renamed class's original descriptor for the lookup.
#[test]
fn solved_names_exist_in_d8_ground_truth() {
    for fixture in fixture_names() {
        let mapping = Mapping::parse(&fs::read_to_string(fixtures_root().join(&fixture).join("r8/mapping.txt")).unwrap()).unwrap();
        let by_obf = mapping.by_obfuscated();
        let o = outcome(&fixture, "r8");
        let g = outcome(&fixture, "d8").program;
        for ((item, _), label) in o.labels.iter() {
            if label.class != Class::Solved {
                continue;
            }
            let ctx = format!("{fixture}: {}", o.program.describe(*item));
            let (ItemId::Class { class } | ItemId::Field { class, .. } | ItemId::Method { class, .. }) = *item;
            let c = o.program.class(class);
            let original = by_obf.get(dotted(&c.descriptor).as_str()).map(|&i| mapping.classes[i].original.clone());
            let Some(original) = original else { panic!("{ctx}: class not in mapping") };
            let g_desc = format!("L{};", original.replace('.', "/"));
            let Some(&gid) = g.by_descriptor.get(&g_desc) else {
                // Synthesized classes (e.g. merge targets, enum-unboxing utilities) have no
                // counterpart in the unoptimized build.
                assert!(mapping.classes[by_obf[dotted(&c.descriptor).as_str()]].is_synthesized(), "{ctx}: absent from D8 build");
                continue;
            };
            let gc = g.class(gid);
            match *item {
                ItemId::Field { index, .. } => {
                    let n = &c.fields[index as usize].name;
                    assert!(gc.fields.iter().any(|f| &f.name == n), "{ctx}: field absent from D8 build");
                }
                ItemId::Method { index, .. } => {
                    let n = &c.methods[index as usize].name;
                    assert!(gc.methods.iter().any(|m| &m.name == n), "{ctx}: method absent from D8 build");
                }
                ItemId::Class { .. } => {}
            }
        }
    }
}

/// Same input, any order, any number of runs: byte-identical report.
#[test]
fn report_is_deterministic() {
    // R8 builds of different fixtures reuse minified names (a/a, …), so combine the D8 builds,
    // whose class names are distinct, into one multidex input.
    let mut inputs: Vec<DexInput> = fixture_names()
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut d = load(f, "d8").remove(0);
            d.name = if i == 0 { "classes.dex".into() } else { format!("classes{}.dex", i + 1) };
            d
        })
        .collect();
    let cfg = Config { verbose_labels: true };
    let a = run(&inputs, &cfg).unwrap().report.to_json();
    inputs.reverse();
    let b = run(&inputs, &cfg).unwrap().report.to_json();
    inputs.rotate_left(1);
    let c = run(&inputs, &cfg).unwrap().report.to_json();
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert_eq!(a, run(&inputs, &cfg).unwrap().report.to_json());
}

#[test]
fn duplicate_class_across_dex_is_an_error() {
    let mut a = load("hello", "d8");
    let mut b = load("hello", "d8");
    b[0].name = "classes2.dex".into();
    a.append(&mut b);
    let err = run(&a, &Config::default()).err().expect("duplicate must fail");
    assert!(err.to_string().contains("Lcom/example/Hello;"), "{err}");
}

/// Ratchet (DESIGN.md §6.1): S coverage per fixture may only go up. Set
/// EIGHTR_UPDATE_METRICS=1 to record improvements.
#[test]
fn solved_coverage_ratchet() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/metrics.json");
    let recorded: BTreeMap<String, BTreeMap<String, u64>> =
        fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut current: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for fixture in fixture_names() {
        let out = outcome(&fixture, "r8");
        for (attr, counts) in &out.report.summary {
            current.entry(fixture.clone()).or_default().insert(format!("{attr:?}"), counts.solved);
        }
    }
    let mut regressions = Vec::new();
    for (fixture, attrs) in &recorded {
        for (attr, &was) in attrs {
            let now = current.get(fixture).and_then(|a| a.get(attr)).copied().unwrap_or(0);
            if now < was {
                regressions.push(format!("{fixture}/{attr}: {was} -> {now}"));
            }
        }
    }
    assert!(regressions.is_empty(), "S coverage regressed: {regressions:?}");
    if std::env::var_os("EIGHTR_UPDATE_METRICS").is_some() || recorded.is_empty() {
        fs::write(&path, serde_json::to_string_pretty(&current).unwrap() + "\n").unwrap();
    } else if current != recorded {
        eprintln!("S coverage improved; run with EIGHTR_UPDATE_METRICS=1 to record: {current:?}");
    }
}

#[test]
fn program_ids_ignore_class_def_order() {
    // Program ids are by descriptor; this guards the invariant the report relies on.
    let p: Program = outcome("opcodes", "d8").program;
    let descs: Vec<_> = p.classes.iter().map(|c| c.descriptor.clone()).collect();
    let mut sorted = descs.clone();
    sorted.sort();
    assert_eq!(descs, sorted);
}
