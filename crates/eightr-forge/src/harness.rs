//! `8r-forge harness`: L3's truth harness (docs/research/libdb.md §11). Scenario builds are real R8
//! output with mappings; each is used as "the app" with a pack that drops the records only that
//! scenario produced (leave-one-out), and every library label 8R gives is graded against the
//! scenario's mapping. Fixtures (an app with its own mapping, not part of the pack) are graded with
//! the pack as is.

use std::collections::BTreeMap;
use std::path::Path;

use eightr_core::libdb::Pack;
use eightr_mapping::Mapping;

use crate::names::Names;
use crate::tools::{read_string, Result};

/// The library rules graded, and the tier each gives.
const RULES: &[&str] = &["r8/sigdb-method-name", "r8/libdb-method-name", "r8/libdb-class-name", "r8/libdb-field-name", "r8/libdb-field-hint"];

#[derive(Debug, Default, Clone)]
pub struct Tally {
    pub total: usize,
    pub correct: usize,
    /// Up to 20 wrong labels, for reading.
    pub wrong: Vec<String>,
}

impl Tally {
    fn add(&mut self, ok: bool, what: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.correct += 1;
        } else if self.wrong.len() < 20 {
            self.wrong.push(what());
        }
    }
}

/// Grades by (rule, attribute, tier S/D).
pub type Grades = BTreeMap<(String, String, String), Tally>;

/// The pack without what only scenario `name` contributed (its bit cleared everywhere).
pub fn without_scenario(pack: &Pack, name: &str) -> Pack {
    let Some(i) = pack.scenarios.iter().position(|s| s == name) else { return pack.clone() };
    let bit = 1u64 << i;
    let mut p = pack.clone();
    p.records.retain(|r| r.scenarios & !bit != 0);
    for r in &mut p.records {
        r.scenarios &= !bit;
    }
    p.class_records.retain(|r| r.scenarios & !bit != 0);
    for r in &mut p.class_records {
        r.scenarios &= !bit;
    }
    // Uniqueness is a property of the pack the app is matched against.
    let mut by_hash: BTreeMap<u64, std::collections::BTreeSet<u32>> = BTreeMap::new();
    for r in &p.records {
        by_hash.entry(r.all).or_default().insert(r.method);
    }
    for r in &mut p.records {
        r.unique = by_hash[&r.all].len() == 1;
    }
    p
}

fn simple(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map_or(inner, |x| x.1)
}

fn package(desc: &str) -> &str {
    let inner = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(desc);
    inner.rsplit_once('/').map_or("", |x| x.0)
}

/// Runs 8R on `app` (a directory of dex files) with `pack` (trusted) and grades its library labels
/// against `mapping`.
pub fn grade(app: &Path, mapping: &Path, pack: &Pack, grades: &mut Grades) -> Result<()> {
    let inputs = eightr_core::input::load(app).map_err(|e| e.to_string())?;
    let cfg = eightr_core::Config { verbose_labels: true, packs: vec![pack.clone()], libdb_trust: true, ..Default::default() };
    let outcome = eightr_core::run(&inputs, &cfg).map_err(|e| e.to_string())?;
    let names = Names::new(&Mapping::parse_normalized(&read_string(mapping)?).map_err(|e| format!("{e:?}"))?);
    for l in &outcome.report.labels {
        let Some(rule) = l.rules.iter().find(|r| RULES.contains(r)) else { continue };
        let Some(value) = &l.value else { continue };
        let attr = format!("{:?}", l.attribute);
        let tier = if l.class == eightr_rules::Class::Solved { "S" } else { "D" };
        let key = (rule.to_string(), attr.clone(), tier.to_string());
        // Truth; items in classes 8R created (split merged classes) have none.
        let truth: Option<String> = if let Some((class, rest)) = l.item.split_once("->") {
            if let Some(i) = rest.find('(') {
                names.method(class, &rest[..i], &rest[i..]).map(|t| t.1)
            } else {
                let (n, ty) = rest.split_once(':').unwrap_or((rest, ""));
                names.field(class, n, ty).map(|t| t.1)
            }
        } else {
            Some(names.class(&l.item))
        }
        .filter(|t: &String| !t.is_empty());
        let Some(truth) = truth else { continue };
        let ok = match l.attribute {
            eightr_rules::Attribute::ClassName => simple(&truth) == value,
            eightr_rules::Attribute::Package => package(&truth) == value,
            _ => truth == *value,
        };
        // `f` for the app's `f$default` (same code, `f` specialized with its defaults): reported
        // apart, neither right nor wrong.
        if !ok && truth == format!("{value}$default") {
            grades.entry((key.0, format!("{attr}(f$default)"), key.2)).or_default().add(true, String::new);
            continue;
        }
        grades.entry(key).or_default().add(ok, || format!("{} {attr} = {value} (truth {truth})", l.item));
    }
    Ok(())
}

/// Leave-one-out over the scenario directories of a forge work dir (`<name>-<hash>[-app…]/out`).
pub fn leave_one_out(pack: &Pack, scenario_dirs: &[std::path::PathBuf], grades: &mut Grades, log: bool) -> Result<()> {
    for dir in scenario_dirs {
        let base = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name = base.split('-').next().unwrap_or(base);
        if !pack.scenarios.iter().any(|s| s == name) {
            return Err(format!("{}: no scenario {name} in the pack", dir.display()));
        }
        let p = without_scenario(pack, name);
        let before: usize = grades.values().map(|t| t.total).sum();
        grade(&dir.join("out"), &dir.join("out/mapping.txt"), &p, grades)?;
        if log {
            eprintln!("  {name}: {} labels graded", grades.values().map(|t| t.total).sum::<usize>() - before);
        }
    }
    Ok(())
}

pub fn print(grades: &Grades) {
    for ((rule, attr, tier), t) in grades {
        let pct = if t.total == 0 { 0.0 } else { 100.0 * t.correct as f64 / t.total as f64 };
        println!("{rule:24} {attr:12} {tier}  {:>6} / {:<6}  {pct:6.2}%", t.correct, t.total);
        if tier == "S" || pct < 98.0 {
            for w in &t.wrong {
                println!("      wrong: {w}");
            }
        }
    }
}
