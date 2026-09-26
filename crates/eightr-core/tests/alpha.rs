//! α-invariance (DESIGN.md §0.3): R8's choice of minified names and of class/file order is
//! arbitrary, so 8R's output must not depend on it.
//!
//! For each R8 fixture, the held-back mapping tells us exactly which names R8 *generated*.
//! We permute those generated names among themselves (a consistent renaming that yields
//! another equally valid R8 output), re-write the program with the canonical writer, split it
//! across a random number of dex files in random order, and require that every α-invariant
//! part of 8R's result is identical. Randomness lives only in this test.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use eightr_core::input::DexInput;
use eightr_core::program::{package_of, simple_name_of, ItemId};
use eightr_core::{run, Config, Outcome};
use eightr_dex::Dex;
use eightr_ir::model::Program as Model;
use eightr_ir::op::Op;
use eightr_ir::rename::Renaming;
use eightr_mapping::{Mapping, MemberKind};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn dotted_to_desc(d: &str) -> String {
    format!("L{};", d.replace('.', "/"))
}

fn tail(simple: &str) -> &str {
    simple.rsplit('$').next().unwrap_or(simple)
}

/// Builds a random renaming that permutes R8-generated names among themselves.
fn random_alpha(model: &Model, mapping: &Mapping, rng: &mut Rng) -> Renaming {
    let s = &model.syms;
    let program: BTreeSet<String> = model.classes.iter().map(|c| s.get(c.ty).to_string()).collect();

    let pins_classes = eightr_ir::reflect::Pins::of(&eightr_ir::reflect::sites(model)).classes;
    // Classes: a name is generated if R8 renamed the class and the new simple name isn't the
    // original plus a collision suffix. Permute tails within (package, outer prefix) groups.
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for cm in &mapping.classes {
        let obf = dotted_to_desc(&cm.obfuscated);
        if cm.obfuscated == cm.original || !program.contains(&obf) || pins_classes.contains(&obf) {
            continue;
        }
        let orig = dotted_to_desc(&cm.original);
        let (ot, nt) = (tail(simple_name_of(&orig)), tail(simple_name_of(&obf)));
        if nt.starts_with(ot) {
            continue;
        }
        let simple = simple_name_of(&obf);
        let prefix = &simple[..simple.len() - nt.len()];
        groups.entry((package_of(&obf).to_string(), prefix.to_string())).or_default().push(obf);
    }
    let mut classes = BTreeMap::new();
    for ((pkg, prefix), descs) in groups {
        let mut tails: Vec<String> = descs.iter().map(|d| tail(simple_name_of(d)).to_string()).collect();
        rng.shuffle(&mut tails);
        for (d, t) in descs.iter().zip(tails) {
            let sep = if pkg.is_empty() { "" } else { "/" };
            classes.insert(d.clone(), format!("L{pkg}{sep}{prefix}{t};"));
        }
    }

    // Members: names R8 generated, minus any name that also occurs unrenamed (kept program
    // members, library members), so the permutation can't collide with or touch originals.
    let by_obf = mapping.by_obfuscated();
    let mut renamed: BTreeSet<String> = BTreeSet::new();
    let mut kept: BTreeSet<String> = BTreeSet::from(["<init>".to_string(), "<clinit>".to_string()]);
    for c in &model.classes {
        let desc = s.get(c.ty);
        let dotted = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc).replace('/', ".");
        let cm = by_obf.get(dotted.as_str()).map(|&i| &mapping.classes[i]);
        let names = c.fields.iter().map(|f| s.get(f.name)).chain(c.methods.iter().map(|m| s.get(m.name)));
        for n in names {
            let was_renamed = cm.is_some_and(|cm| {
                cm.members.iter().any(|mm| match &mm.kind {
                    MemberKind::Field(f) => f.obfuscated == n && f.original_name != n,
                    MemberKind::Method(m) => m.obfuscated == n && m.original_name != n,
                }) && !cm.members.iter().any(|mm| match &mm.kind {
                    MemberKind::Field(f) => f.obfuscated == n && f.original_name == n,
                    MemberKind::Method(m) => m.obfuscated == n && m.original_name == n,
                })
            });
            if was_renamed { renamed.insert(n.to_string()) } else { kept.insert(n.to_string()) };
        }
        for m in c.methods.iter().filter_map(|m| m.code.as_ref()) {
            for i in &m.insns {
                let r = match &i.op {
                    Op::Invoke { method, .. } => Some((method.class, method.name)),
                    Op::InstanceGet { field, .. } | Op::InstancePut { field, .. } | Op::StaticGet { field, .. }
                    | Op::StaticPut { field, .. } => Some((field.class, field.name)),
                    _ => None,
                };
                if let Some((cl, n)) = r {
                    if !program.contains(s.get(cl)) {
                        kept.insert(s.get(n).to_string());
                    }
                }
            }
        }
    }
    // Names looked up reflectively through strings that can't be rewritten must stay put,
    // exactly as R8 would have kept them consistent.
    let pins = eightr_ir::reflect::Pins::of(&eightr_ir::reflect::sites(model));
    kept.extend(pins.fields.iter().map(|(_, n)| n.clone()));
    kept.extend(pins.methods.iter().map(|(_, n)| n.clone()));
    let pool: Vec<String> = renamed.difference(&kept).cloned().collect();
    let mut shuffled = pool.clone();
    rng.shuffle(&mut shuffled);
    let members = pool.into_iter().zip(shuffled).collect();
    Renaming { classes, members, ..Default::default() }
}

/// Inverse of a renaming, applied to descriptors, protos, and member names.
struct Inverse {
    classes: BTreeMap<String, String>,
    members: BTreeMap<String, String>,
}

impl Inverse {
    fn of(r: &Renaming) -> Inverse {
        Inverse {
            classes: r.classes.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
            members: r.members.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        }
    }
    fn identity() -> Inverse {
        Inverse { classes: BTreeMap::new(), members: BTreeMap::new() }
    }
    fn desc(&self, d: &str) -> String {
        let dims = d.bytes().take_while(|&b| b == b'[').count();
        match self.classes.get(&d[dims..]) {
            Some(o) => format!("{}{}", &d[..dims], o),
            None => d.to_string(),
        }
    }
    fn proto(&self, p: &str) -> String {
        let (params, ret) = eightr_ir::types::parse_proto(p).expect("valid proto");
        format!("({}){}", params.iter().map(|x| self.desc(x)).collect::<String>(), self.desc(ret))
    }
    fn member(&self, n: &str) -> String {
        self.members.get(n).cloned().unwrap_or_else(|| n.to_string())
    }
}

/// Every label, keyed by the item's identity in the *original* (unscrambled) program. For
/// α-invariance, the scrambled run mapped back through the inverse renaming must equal the
/// original run exactly (equivariance: output(α(x)) = α(output(x))).
fn projection(out: &Outcome, inv: &Inverse) -> String {
    let p = &out.program;
    let mut lines: Vec<String> = Vec::new();
    for ((item, attr), label) in out.labels.iter() {
        let key = match *item {
            ItemId::Class { class } => format!("C {}", inv.desc(p.descriptor(class))),
            ItemId::Field { class, index } => {
                let f = &p.class(class).fields[index as usize];
                format!("F {}->{}:{}", inv.desc(p.descriptor(class)), inv.member(p.str(f.name)), inv.desc(p.str(f.ty)))
            }
            ItemId::Method { class, index } => {
                let m = &p.class(class).methods[index as usize];
                format!("M {}->{}{}", inv.desc(p.descriptor(class)), inv.member(p.str(m.name)), inv.proto(p.str(m.proto)))
            }
        };
        // For N labels (structural ties) the representative value may follow input order; the
        // candidate set is what must be invariant.
        let value = if label.candidates.is_some() { None } else { label.value.as_ref() };
        lines.push(format!("{key} {attr:?} {:?} {:?} {:?} {:?}", label.class, label.rules, value, label.candidates));
    }
    lines.sort();
    let r = &out.report;
    format!(
        "summary {}\nrules {}\nmarkers {}\nsources {}\nlabels:\n{}",
        serde_json::to_string(&r.summary).unwrap(),
        serde_json::to_string(&r.rules).unwrap(),
        serde_json::to_string(&r.markers).unwrap(),
        serde_json::to_string(&r.sources).unwrap(),
        lines.join("\n")
    )
}

#[test]
fn outputs_are_alpha_invariant() {
    let mut fixtures: Vec<String> = fs::read_dir(fixtures_root())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fixtures.sort();
    let seeds = std::env::var("EIGHTR_ALPHA_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(6u64);
    let mut total_renamed = 0;
    for fixture in fixtures {
        let dir = fixtures_root().join(&fixture).join("r8");
        let bytes = fs::read(dir.join("classes.dex")).unwrap();
        let mapping = Mapping::parse(&fs::read_to_string(dir.join("mapping.txt")).unwrap()).unwrap();
        let cfg = Config::default();
        let original = run(&[DexInput { name: "classes.dex".into(), bytes: bytes.clone() }], &cfg).unwrap();
        let expected = projection(&original, &Inverse::identity());
        let expected_out = eightr_core::output::emit(&original).unwrap();

        for seed in 1..=seeds {
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x1000_0001) ^ fixture.len() as u64);
            let dex = Dex::parse(&bytes).unwrap();
            let mut model = Model::load(&[&dex]).unwrap();
            let renaming = random_alpha(&model, &mapping, &mut rng);
            total_renamed += renaming.classes.len() + renaming.members.len();
            if std::env::var_os("EIGHTR_ALPHA_DEBUG").is_some() {
                eprintln!("{fixture} seed {seed}: classes {:?}\n  members {:?}", renaming.classes, renaming.members);
            }
            renaming.apply(&mut model);
            let inverse = Inverse::of(&renaming);

            let files = 1 + rng.below(3);
            let assign: Vec<usize> = (0..model.classes.len()).map(|_| rng.below(files)).collect();
            let written = eightr_dexwrite::write_split(&model, files, |i| assign[i]).unwrap();
            let mut inputs: Vec<DexInput> = written
                .into_iter()
                .enumerate()
                .filter(|(_, b)| Dex::parse(b).map(|d| d.class_count() > 0).unwrap_or(false))
                .map(|(i, bytes)| DexInput { name: if i == 0 { "classes.dex".into() } else { format!("classes{}.dex", i + 1) }, bytes })
                .collect();
            rng.shuffle(&mut inputs);
            let scrambled = run(&inputs, &cfg).unwrap();
            let got = projection(&scrambled, &inverse);
            if got != expected {
                let first = expected.lines().zip(got.lines()).position(|(a, b)| a != b).unwrap_or(0);
                panic!(
                    "{fixture} seed {seed}: output depends on R8's arbitrary choices (first difference at line {first}):\n  expected: {}\n  got:      {}",
                    expected.lines().nth(first).unwrap_or(""),
                    got.lines().nth(first).unwrap_or("")
                );
            }
            // The strongest form: 8R's emitted program is byte-identical.
            let got_out = eightr_core::output::emit(&scrambled).unwrap();
            let same_dex = expected_out.dex.iter().map(|d| &d.1).eq(got_out.dex.iter().map(|d| &d.1));
            if !same_dex {
                let dump = |o: &eightr_core::output::Output| {
                    let dexes: Vec<Dex> = o.dex.iter().map(|d| Dex::parse(&d.1).unwrap()).collect();
                    let refs: Vec<&Dex> = dexes.iter().collect();
                    eightr_ir::print::program(&Model::load(&refs).unwrap())
                };
                let (a, b) = (dump(&expected_out), dump(&got_out));
                let first = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
                let ctx = |t: &str| t.lines().skip(first.saturating_sub(4)).take(6).collect::<Vec<_>>().join("\n");
                panic!("{fixture} seed {seed}: emitted dex differs under scrambling at line {first}:\n--- expected\n{}\n--- got\n{}", ctx(&a), ctx(&b));
            }
        }
    }
    assert!(total_renamed > 100, "scrambling should actually rename things (renamed {total_renamed})");
}
