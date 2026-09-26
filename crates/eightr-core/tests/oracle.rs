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

/// A class `r8/rebox-enum` re-created: an enum class absent from the input.
fn reboxed(p: &Program, class: eightr_core::program::ClassId) -> bool {
    let c = p.class(class);
    c.access & eightr_dex::class::access::ENUM != 0 && c.superclass.is_some_and(|s| p.str(s) == "Ljava/lang/Enum;")
}

/// A class `r8/split-merged-class` added (`<base>$$Split<id>;`) holds copies of its base's
/// members, so it's graded against the base's mapping entry.
fn split_base(desc: &str) -> String {
    match desc.split_once("$$Split") {
        Some((head, _)) => format!("{head};"),
        None => desc.to_string(),
    }
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
            let obf = dotted(&split_base(p.descriptor(class_id)));
            // An enum `r8/rebox-enum` re-created (R8 removed the class): its recovered name must
            // be one the mapping lists as an original; its members are graded by the D8 build.
            if !by_obf.contains_key(obf.as_str()) && reboxed(p, class_id) {
                assert!(mapping.classes.iter().any(|c| c.original == obf), "{ctx}: re-boxed enum name isn't an original class");
                checked += 1;
                continue;
            }
            let cm = &mapping.classes[*by_obf.get(obf.as_str()).unwrap_or_else(|| panic!("{ctx}: class not in mapping"))];
            // A synthesized class's own name is never original. Its *members* can't be graded
            // by the mapping: R8 hides synthetic frames, so a lambda bridge like `compare` shows
            // the inlined lambda body as its outermost frame. Those are graded against the D8
            // build (solved_names_exist_in_d8_ground_truth) when the class is app code, and
            // counted as unverifiable otherwise.
            if cm.is_synthesized() {
                assert!(member.is_some(), "{ctx}: S label on a synthesized class");
                // Split subclasses repeat their base's members: count those once, on the base.
                if !p.descriptor(class_id).contains("$$Split") {
                    unverifiable += 1;
                }
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
            let c_desc = &split_base(o.program.descriptor(class));
            let rebox = !by_obf.contains_key(dotted(c_desc).as_str()) && reboxed(&o.program, class);
            let original = by_obf.get(dotted(c_desc).as_str()).map(|&i| mapping.classes[i].original.clone()).or_else(|| rebox.then(|| dotted(c_desc)));
            let Some(original) = original else { panic!("{ctx}: class not in mapping") };
            let g_desc = format!("L{};", original.replace('.', "/"));
            // The D8 build holds only the app's own classes. Library classes (shrunk into the
            // R8 build) and R8-synthesized classes have no counterpart; the mapping grades those.
            let Some(gid) = g.find(&g_desc) else { continue };
            // Horizontal/vertical merging folds several original classes into one residual
            // class; its members may come from any of them. The mapping names every original
            // owner it folded in.
            let empty = eightr_mapping::ClassMapping { line: 0, original: String::new(), obfuscated: String::new(), metadata: vec![], members: vec![] };
            let cm = by_obf.get(dotted(c_desc).as_str()).map_or(&empty, |&i| &mapping.classes[i]);
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

/// Outline detection (`r8/outline-inline`, `r8/bu-outline-inline`) against the mapping. Truth:
/// methods R8 marked `com.android.tools.r8.outline` (classic), and bottom-up throw outlines,
/// which carry no marker but are synthesized as `Holder$N.m`, returning void and ending in
/// `throw`. Every detection must be in the truth (precision), and every truth method detected
/// (recall), including in fixtures full of look-alike synthetics (`r94_desugar`).
#[test]
fn outline_detection_matches_mapping() {
    let (mut checked, mut bottom_up_truth) = (0, 0);
    for fixture in fixture_names() {
        let Ok(text) = fs::read_to_string(fixtures_root().join(&fixture).join("r8/mapping.txt")) else { continue };
        let mapping = Mapping::parse(&text).unwrap();
        let out = outcome(&fixture, "r8");
        let plain = run(&load(&fixture, "r8"), &Config { no_rewrites: true, ..Default::default() }).unwrap();
        let detected: std::collections::BTreeSet<(String, String)> = out
            .report
            .rewrites
            .iter()
            .filter(|r| r.rule == eightr_rules::OUTLINE_INLINE || r.rule == eightr_rules::BU_OUTLINE_INLINE)
            .map(|r| {
                let (class, rest) = r.item.split_once("->").unwrap();
                let name = rest.split('(').next().unwrap();
                let dotted = class.strip_prefix('L').and_then(|c| c.strip_suffix(';')).unwrap().replace('/', ".");
                (dotted, name.to_string())
            })
            .collect();
        let mut truth = std::collections::BTreeSet::new();
        for c in &mapping.classes {
            for m in &c.members {
                let MemberKind::Method(x) = &m.kind else { continue };
                let synthesized = m.metadata.iter().any(|md| md.parsed == Metadata::Synthesized);
                let owner = x.original_owner.as_deref().unwrap_or(&c.original);
                let holder = owner.rsplit_once('$').is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                let bottom_up = synthesized
                    && holder
                    && x.original_name == "m"
                    && x.return_type == "void"
                    && ends_in_throw(&plain.program.model, &c.obfuscated, &x.obfuscated);
                bottom_up_truth += usize::from(bottom_up);
                if bottom_up || m.metadata.iter().any(|md| md.parsed == Metadata::Outline) {
                    truth.insert((c.obfuscated.clone(), x.obfuscated.clone()));
                }
            }
        }
        for d in &detected {
            assert!(truth.contains(d), "{fixture}: {}.{} was inlined as an outline but isn't one", d.0, d.1);
            checked += 1;
        }
        for t in &truth {
            assert!(detected.contains(t), "{fixture}: outline {}.{} was not detected", t.0, t.1);
        }
    }
    assert!(checked >= 8, "only {checked} outline detections checked");
    assert!(bottom_up_truth >= 5, "only {bottom_up_truth} bottom-up outlines in the mappings");
}

/// Whether residual method `class.name` (dotted class) ends in `throw` in the R8 build.
fn ends_in_throw(model: &eightr_ir::model::Program, class: &str, name: &str) -> bool {
    let desc = format!("L{};", class.replace('.', "/"));
    model.find(&desc).is_some_and(|i| {
        model.classes[i].methods.iter().any(|m| {
            model.syms.get(m.name) == name
                && m.code.as_ref().and_then(|b| b.insns.last()).is_some_and(|x| matches!(x.op, eightr_ir::op::Op::Throw { .. }))
        })
    })
}

/// Merged-class detection (`r8/split-merged-class`) against the mapping: every split class must
/// hold a `$r8$classId` field per the mapping (precision), and most of those must be split
/// (recall; the rest are refused on purpose: abstract merged classes, class literals of the
/// group, constructors that store the id twice).
#[test]
fn merged_class_detection_matches_mapping() {
    let (mut truth_total, mut found, mut checked) = (0, 0, 0);
    for fixture in fixture_names() {
        let Ok(text) = fs::read_to_string(fixtures_root().join(&fixture).join("r8/mapping.txt")) else { continue };
        let mapping = Mapping::parse(&text).unwrap();
        let truth: std::collections::BTreeSet<String> = mapping
            .classes
            .iter()
            .filter(|c| c.fields().any(|f| f.original_name == "$r8$classId"))
            .map(|c| format!("L{};", c.obfuscated.replace('.', "/")))
            .collect();
        let out = outcome(&fixture, "r8");
        let split: std::collections::BTreeSet<&str> =
            out.report.rewrites.iter().filter(|r| r.rule == eightr_rules::SPLIT_MERGED_CLASS).map(|r| r.item.as_str()).collect();
        for s in &split {
            // With -dontobfuscate, R8 lists no unrenamed field: the input names it itself.
            let named = out.program.model.find(s).is_some_and(|i| {
                let m = &out.program.model;
                m.classes[i].fields.iter().any(|f| m.syms.get(f.name) == "$r8$classId")
            });
            assert!(truth.contains(*s) || named, "{fixture}: {s} was split but R8 didn't merge into it");
            checked += 1;
        }
        truth_total += truth.len();
        found += truth.iter().filter(|t| split.contains(t.as_str())).count();
    }
    assert!(checked >= 50, "only {checked} splits checked");
    assert!(found * 100 >= truth_total * 85, "merged-class recall {found}/{truth_total} below 85%");
}

/// Enum unboxing: the utility's S member names match the mapping (the generic S test can't
/// grade members of synthesized classes), and the recovered enums match `r94_enum`'s source.
#[test]
fn enum_unboxing_evidence_matches_ground_truth() {
    let mut graded = 0;
    for fixture in fixture_names() {
        let Ok(text) = fs::read_to_string(fixtures_root().join(&fixture).join("r8/mapping.txt")) else { continue };
        let mapping = Mapping::parse(&text).unwrap();
        let by_obf = mapping.by_obfuscated();
        let out = outcome(&fixture, "r8");
        let p = &out.program;
        for ((item, _), label) in out.labels.iter() {
            if !label.rules.contains(&eightr_rules::ENUM_UNBOXING_UTILITY) {
                continue;
            }
            let (ItemId::Field { class, .. } | ItemId::Method { class, .. } | ItemId::Class { class }) = *item;
            let cm = &mapping.classes[by_obf[dotted(p.descriptor(class)).as_str()]];
            let obf_name = match *item {
                ItemId::Field { class, index } => p.str(p.class(class).fields[index as usize].name).to_string(),
                ItemId::Method { class, index } => p.str(p.class(class).methods[index as usize].name).to_string(),
                ItemId::Class { .. } => unreachable!(),
            };
            let originals: Vec<&str> = cm
                .members
                .iter()
                .filter_map(|m| match &m.kind {
                    MemberKind::Field(f) if f.obfuscated == obf_name => Some(f.original_name.as_str()),
                    MemberKind::Method(x) if x.obfuscated == obf_name => Some(x.original_name.as_str()),
                    _ => None,
                })
                .collect();
            let value = label.value.as_deref().unwrap();
            assert!(originals.contains(&value), "{fixture}: {} named {value}, mapping says {originals:?}", p.describe(*item));
            graded += 1;
        }
    }
    assert!(graded >= 3, "only {graded} enum-utility names graded");

    let out = outcome("r94_enum", "r8");
    // Color: inlined valueOf proves the names and gives the name. Planet and Op: only name()
    // chains, whose strings could equally be a String field's values (unproven).
    let got: Vec<(Option<&str>, Vec<&str>, bool)> =
        out.report.enums.iter().map(|e| (e.canonical_name.as_deref(), e.constants.iter().map(String::as_str).collect(), e.proven)).collect();
    assert_eq!(got, vec![
        (None, vec!["ADD", "MUL", "SUB"], false),
        (None, vec!["MERCURY", "VENUS", "EARTH", "MARS"], false),
        (Some("com.example.enums.Color"), vec!["RED", "GREEN", "BLUE"], true),
    ]);
    // No fixture proves a table that isn't an enum (string switches look like valueOf).
    for fixture in fixture_names() {
        let out = outcome(&fixture, "r8");
        for e in out.report.enums.iter().filter(|e| e.proven) {
            assert!(fixture == "r94_enum", "{fixture}: proved {:?} {:?}", e.canonical_name, e.constants);
        }
    }
}
