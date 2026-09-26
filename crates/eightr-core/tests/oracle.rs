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
use eightr_mapping::{Mapping, MemberKind, Metadata};
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
    run(&load(fixture, variant), &Config { verbose_labels: true, ..Default::default() }).unwrap()
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

/// Residual (dex) descriptor → original descriptor, through the mapping's class renames.
fn original_desc(d: &str, mapping: &Mapping, by_obf: &std::collections::BTreeMap<&str, usize>) -> String {
    let dims = d.bytes().take_while(|&b| b == b'[').count();
    let base = &d[dims..];
    let orig = match base.strip_prefix('L').and_then(|b| b.strip_suffix(';')) {
        Some(inner) => match by_obf.get(inner.replace('/', ".").as_str()) {
            Some(&i) => format!("L{};", mapping.classes[i].original.replace('.', "/")),
            None => base.to_string(),
        },
        None => base.to_string(),
    };
    format!("{}{}", &d[..dims], orig)
}

/// Residual method descriptor → original, through the mapping's class renames.
fn original_proto(p: &str, mapping: &Mapping, by_obf: &std::collections::BTreeMap<&str, usize>) -> String {
    let (params, ret) = eightr_ir::types::parse_proto(p).expect("valid proto");
    format!(
        "({}){}",
        params.iter().map(|t| original_desc(t, mapping, by_obf)).collect::<String>(),
        original_desc(ret, mapping, by_obf)
    )
}

fn dotted(descriptor: &str) -> String {
    descriptor.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(descriptor).replace('/', ".")
}

/// Every S label on a name must agree with the mapping R8 wrote.
#[test]
fn solved_names_match_held_back_mapping() {
    let mut checked = 0;
    let mut unverifiable = 0;
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
            let obf = dotted(p.descriptor(class_id));
            let cm = &mapping.classes[*by_obf.get(obf.as_str()).unwrap_or_else(|| panic!("{ctx}: class not in mapping"))];
            // A synthesized class's own name is never original. Its *members* can't be graded
            // by the mapping: R8 hides synthetic frames, so a lambda bridge like `compare` shows
            // the inlined lambda body as its outermost frame. Those are graded against the D8
            // build (solved_names_exist_in_d8_ground_truth) when the class is app code, and
            // counted as unverifiable otherwise.
            if cm.is_synthesized() {
                assert!(member.is_some(), "{ctx}: S label on a synthesized class");
                unverifiable += 1;
                continue;
            }
            match (attr, member) {
                (Attribute::ClassName, None) => {
                    let simple = |d: &str| d.rsplit('.').next().unwrap_or(d).to_string();
                    assert_eq!(simple(&cm.original), simple(&obf), "{ctx}: R8 renamed {} -> {}", cm.original, obf);
                }
                (Attribute::Package, None) => {
                    let pkg = |d: &str| d.rsplit_once('.').map(|(p, _)| p.to_string()).unwrap_or_default();
                    assert_eq!(pkg(&cm.original), pkg(&obf), "{ctx}: R8 moved {} -> {}", cm.original, obf);
                }
                (Attribute::MemberName, Some((false, index))) => {
                    let f = &class.fields[index as usize];
                    let (f_name, f_ty) = (p.str(f.name), p.str(f.ty));
                    let claimed = label.value.as_deref().unwrap_or(f_name);
                    // R8 omits members it neither renamed nor attached line info to, so an
                    // absent member means "unchanged".
                    let matches: Vec<_> = cm
                        .members
                        .iter()
                        .filter_map(|mm| match &mm.kind {
                            MemberKind::Field(m)
                                if m.obfuscated == f_name && java_to_descriptor(&m.ty) == original_desc(f_ty, &mapping, &by_obf) =>
                            {
                                Some((m, &mm.metadata))
                            }
                            _ => None,
                        })
                        .collect();
                    assert!(!matches.is_empty() || claimed == f_name, "{ctx}: recovered {claimed:?} but R8 didn't rename this field");
                    for (m, md) in matches {
                        assert!(!md.iter().any(|x| x.parsed == Metadata::Synthesized), "{ctx}: S label on a synthesized field");
                        assert_eq!(m.original_name, claimed, "{ctx}");
                    }
                }
                (Attribute::MemberName, Some((true, index))) => {
                    let meth = &class.methods[index as usize];
                    let (meth_name, meth_proto) = (p.str(meth.name), p.str(meth.proto));
                    let claimed = label.value.as_deref().unwrap_or(meth_name);
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
                            // Mapping signatures use original types; a residualsignature entry
                            // (when present) is in residual types.
                            m.obfuscated == meth_name
                                && own
                                && match residual {
                                    Some(r) => r == meth_proto,
                                    None => sig == original_proto(meth_proto, &mapping, &by_obf),
                                }
                        })
                        .collect();
                    assert!(!matches.is_empty() || claimed == meth_name, "{ctx}: recovered {claimed:?} but R8 didn't rename this method");
                    for (m, md) in matches {
                        // Constructor names are the one exception: every constructor is named
                        // <init>, so the name claim holds even for a synthesized constructor (whose
                        // *signature* isn't original; that's a different attribute).
                        let ctor = meth_name == "<init>" || meth_name == "<clinit>";
                        assert!(ctor || !md.iter().any(|x| x.parsed == Metadata::Synthesized), "{ctx}: S label on a synthesized method");
                        assert_eq!(m.original_name, claimed, "{ctx}");
                    }
                }
                other => panic!("{ctx}: unexpected S label {other:?}"),
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no S labels were graded");
    eprintln!("graded {checked} S labels against the mapping; {unverifiable} on synthesized-class members deferred to the D8 check");
    assert!(unverifiable * 20 < checked, "too many S labels can't be graded by the mapping ({unverifiable} of {checked})");
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
            let recovered = label.value.clone();
            let (ItemId::Class { class } | ItemId::Field { class, .. } | ItemId::Method { class, .. }) = *item;
            let c = o.program.class(class);
            let c_desc = o.program.descriptor(class);
            let original = by_obf.get(dotted(c_desc).as_str()).map(|&i| mapping.classes[i].original.clone());
            let Some(original) = original else { panic!("{ctx}: class not in mapping") };
            let g_desc = format!("L{};", original.replace('.', "/"));
            // The D8 build holds only the app's own classes. Library classes (shrunk into the
            // R8 build) and R8-synthesized classes have no counterpart; the mapping grades those.
            let Some(gid) = g.find(&g_desc) else { continue };
            // Horizontal/vertical merging folds several original classes into one residual
            // class; its members may come from any of them. The mapping names every original
            // owner it folded in.
            let cm = &mapping.classes[by_obf[dotted(c_desc).as_str()]];
            let mut owners = vec![gid];
            for mm in &cm.members {
                let owner = match &mm.kind {
                    MemberKind::Field(f) => f.original_owner.as_ref(),
                    MemberKind::Method(m) => m.original_owner.as_ref(),
                };
                if let Some(id) = owner.and_then(|o| g.find(&format!("L{};", o.replace('.', "/")))) {
                    owners.push(id);
                }
            }
            // A member the mapping attributes to a class outside the app (vertical merging of
            // a library superclass) can only be graded by the mapping.
            let member_name = match *item {
                ItemId::Field { index, .. } => Some(o.program.str(c.fields[index as usize].name)),
                ItemId::Method { index, .. } => Some(o.program.str(c.methods[index as usize].name)),
                ItemId::Class { .. } => None,
            };
            let claimed = recovered.as_deref().or(member_name);
            let from_library = member_name.is_some_and(|n| {
                cm.members.iter().any(|mm| {
                    let (obf, owner) = match &mm.kind {
                        MemberKind::Field(f) => (&f.obfuscated, f.original_owner.as_ref()),
                        MemberKind::Method(m) => (&m.obfuscated, m.original_owner.as_ref()),
                    };
                    obf == n && owner.is_some_and(|o| g.find(&format!("L{};", o.replace('.', "/"))).is_none())
                })
            });
            if from_library {
                continue;
            }
            match *item {
                ItemId::Field { .. } => {
                    let n = claimed.expect("member");
                    let found = owners.iter().any(|&id| g.class(id).fields.iter().any(|f| g.str(f.name) == n));
                    assert!(found, "{ctx}: field absent from D8 build");
                }
                ItemId::Method { .. } => {
                    let n = claimed.expect("member");
                    let found = owners.iter().any(|&id| g.class(id).methods.iter().any(|m| g.str(m.name) == n));
                    assert!(found, "{ctx}: method absent from D8 build");
                }
                ItemId::Class { .. } => {}
            }
        }
    }
}

/// Same input, any order, any number of runs: byte-identical report.
#[test]
fn report_is_deterministic() {
    // R8 builds of different fixtures reuse minified names (a/a, …), so combine D8 builds of
    // fixtures whose class names are disjoint into one multidex input.
    let mut inputs: Vec<DexInput> = ["hello", "names_stress", "opcodes", "shapes"]
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut d = load(f, "d8").remove(0);
            d.name = if i == 0 { "classes.dex".into() } else { format!("classes{}.dex", i + 1) };
            d
        })
        .collect();
    let cfg = Config { verbose_labels: true, ..Default::default() };
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
    // EIGHTR_UPDATE_METRICS=1 records the current numbers even if they dropped: for
    // intentional changes (e.g. a fixture's source changed). Review the diff of metrics.json.
    let update = std::env::var_os("EIGHTR_UPDATE_METRICS").is_some();
    assert!(update || regressions.is_empty(), "S coverage regressed: {regressions:?}");
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
    let descs: Vec<_> = p.class_ids().map(|id| p.descriptor(id).to_string()).collect();
    let mut sorted = descs.clone();
    sorted.sort();
    assert_eq!(descs, sorted);
}
