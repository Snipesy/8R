//! load → preflight → program → evidence → un-passes (undo order) → report.

use std::collections::BTreeMap;

use eightr_dex::Dex;
use eightr_rules::Attribute;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::input::{self, DexInput};
use crate::labels::Labels;
use crate::marker::Marker;
use crate::passes::{self, Context, NameStats};
use crate::program::{ItemId, Program};
use crate::report::{Counts, Finding, InputSummary, LabelEntry, Report, Severity};
use crate::sources::{self, DetectedSource};

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Include every non-identity label in the report.
    pub verbose_labels: bool,
    /// Skip structural rewrite passes (outline inlining, class un-merging, ...); only
    /// names are recovered. Used to isolate rewrites in tests.
    pub no_rewrites: bool,
}

/// Read-only facts gathered before any pass runs.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub markers: Vec<Marker>,
    pub sources: Vec<DetectedSource>,
    pub name_stats: NameStats,
}

pub struct Outcome {
    pub program: Program,
    pub labels: Labels,
    pub report: Report,
    /// Every recovered (S) and structural (D) name, ready to apply to the program.
    pub renaming: eightr_ir::rename::Renaming,
}

pub fn run(inputs: &[DexInput], config: &Config) -> Result<Outcome> {
    // Canonical order first: nothing downstream may depend on the order the caller used.
    let mut inputs = inputs.to_vec();
    input::canonical_order(&mut inputs);

    let mut findings = Vec::new();
    let mut summaries = Vec::new();
    let mut dexes = Vec::new();
    for input in &inputs {
        let dex = Dex::parse(&input.bytes).map_err(|error| Error::Dex { input: input.name.clone(), error })?;
        let (checksum_ok, signature_ok) = (dex.checksum_ok(), dex.signature_ok());
        if !checksum_ok || !signature_ok {
            findings.push(Finding {
                severity: Severity::Warning,
                message: format!("{}: checksum or signature mismatch (file modified after build?)", input.name),
            });
        }
        summaries.push(InputSummary {
            name: input.name.clone(),
            sha256: hex(&Sha256::digest(&input.bytes)),
            dex_version: dex.header.version,
            classes: dex.class_count(),
            checksum_ok,
            signature_ok,
        });
        dexes.push((input.name.clone(), dex));
    }

    let markers = collect_markers(&dexes);
    if markers.is_empty() {
        findings.push(Finding {
            severity: Severity::Info,
            message: "no D8/R8 marker found; build tool identified by heuristics only".into(),
        });
    }
    let sources = sources::detect(&dexes, &markers);
    let dex_refs: Vec<&Dex> = dexes.iter().map(|(_, d)| d).collect();
    let mut model = eightr_ir::model::Program::load(&dex_refs).map_err(|e| match e {
        eightr_ir::model::LoadError::Dex { input, error } => Error::Dex { input: dexes[input].0.clone(), error },
        eightr_ir::model::LoadError::DuplicateClass { descriptor, inputs } => {
            Error::DuplicateClass { descriptor, inputs: inputs.map(|i| dexes[i].0.clone()) }
        }
    })?;
    let rewrites = if config.no_rewrites { Vec::new() } else { crate::rewrites::run_all(&mut model)? };
    let mut program = Program { model };
    let name_stats = name_stats(&dexes, &program);
    let evidence = Evidence { markers: markers.clone(), sources: sources.clone(), name_stats };

    let mut labels = Labels::default();
    let mut enums = Vec::new();
    for pass in passes::all() {
        pass.run(&mut Context { program: &mut program, evidence: &evidence, labels: &mut labels, findings: &mut findings, enums: &mut enums })?;
    }

    let naming = crate::naming::name(&program, &mut labels, &mut findings)?;
    let mut renaming = naming.renaming;
    // Recovered S values (e.g. lateinit field names) join the structural names.
    for ((item, attr), label) in labels.iter() {
        let (Some(value), true) = (&label.value, label.class == eightr_rules::Class::Solved) else { continue };
        if *attr != Attribute::MemberName {
            continue;
        }
        let (class, index, field) = match *item {
            ItemId::Field { class, index } => (class, index, true),
            ItemId::Method { class, index } => (class, index, false),
            ItemId::Class { .. } => continue,
        };
        let c = program.class(class);
        let d = program.descriptor(class).to_string();
        if field {
            let f = &c.fields[index as usize];
            renaming.fields.insert((d, program.str(f.name).to_string(), program.str(f.ty).to_string()), value.clone());
        } else {
            let m = &c.methods[index as usize];
            renaming.methods.insert((d, program.str(m.name).to_string(), program.str(m.proto).to_string()), value.clone());
        }
    }
    findings.sort();
    findings.dedup();
    let mut report = build_report(&program, &labels, summaries, markers, sources, findings, rewrites, config);
    report.enums = enums;
    Ok(Outcome { program, labels, report, renaming })
}

fn name_stats(dexes: &[(String, Dex)], program: &Program) -> NameStats {
    let mut types = std::collections::BTreeSet::new();
    let mut members = std::collections::BTreeSet::new();
    for (_, dex) in dexes {
        types.extend((0..dex.type_count()).filter_map(|i| dex.type_descriptor(i).ok()));
        for i in 0..dex.field_count() {
            if let Ok(f) = dex.field_id(i) {
                members.extend(dex.string(f.name_idx).ok());
            }
        }
        for i in 0..dex.method_count() {
            if let Ok(m) = dex.method_id(i) {
                members.extend(dex.string(m.name_idx).ok());
            }
        }
    }
    let packages: std::collections::BTreeSet<&str> =
        program.class_ids().map(|id| crate::program::package_of(program.descriptor(id))).collect();
    NameStats { type_descriptors: types.len() as u64, member_names: members.len() as u64, packages: packages.len() as u64 }
}

fn collect_markers(dexes: &[(String, Dex)]) -> Vec<Marker> {
    let mut markers: Vec<Marker> = Vec::new();
    for (_, dex) in dexes {
        for s in dex.strings().flatten() {
            if let Some(m) = Marker::parse(&s) {
                if !markers.contains(&m) {
                    markers.push(m);
                }
            }
        }
    }
    markers.sort_by(|a, b| (&a.tool, serde_json::to_string(&a.fields).ok()).cmp(&(&b.tool, serde_json::to_string(&b.fields).ok())));
    markers
}

/// Which attributes apply to an item.
fn attributes(program: &Program, item: ItemId) -> &'static [Attribute] {
    use Attribute::*;
    match item {
        ItemId::Class { .. } => &[Package, ClassName, SourceFile],
        ItemId::Field { .. } => &[MemberName, Signature],
        ItemId::Method { class, index } => {
            if program.class(class).methods[index as usize].code.is_some() {
                &[MemberName, Signature, Body, Lines]
            } else {
                &[MemberName, Signature]
            }
        }
    }
}

fn all_items(program: &Program) -> Vec<ItemId> {
    let mut v = Vec::new();
    for class in program.class_ids() {
        v.push(ItemId::Class { class });
        let c = program.class(class);
        v.extend((0..c.fields.len() as u32).map(|index| ItemId::Field { class, index }));
        v.extend((0..c.methods.len() as u32).map(|index| ItemId::Method { class, index }));
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    program: &Program,
    labels: &Labels,
    inputs: Vec<InputSummary>,
    markers: Vec<Marker>,
    sources: Vec<DetectedSource>,
    findings: Vec<Finding>,
    rewrites: Vec<crate::rewrites::RewriteRecord>,
    config: &Config,
) -> Report {
    use eightr_rules::Class::*;
    let mut summary: BTreeMap<Attribute, Counts> = BTreeMap::new();
    for item in all_items(program) {
        for &attr in attributes(program, item) {
            let c = summary.entry(attr).or_default();
            match labels.get(item, attr).map(|l| l.class) {
                None => c.untouched += 1,
                Some(Solved) => c.solved += 1,
                Some(Deterministic) => c.deterministic += 1,
                Some(NonDeterministic) => c.nondeterministic += 1,
            }
        }
    }
    let mut applications: BTreeMap<&'static str, u64> = BTreeMap::new();
    for r in &rewrites {
        *applications.entry(r.rule).or_default() += 1;
    }
    let mut entries = Vec::new();
    for ((item, attr), label) in labels.iter() {
        for r in &label.rules {
            *applications.entry(r).or_default() += 1;
        }
        if config.verbose_labels {
            entries.push(LabelEntry {
                item: program.describe(*item),
                attribute: *attr,
                class: label.class,
                value: label.value.clone(),
                rules: label.rules.clone(),
                candidates: label.candidates.clone(),
            });
        }
    }
    entries.sort_by(|a, b| (&a.item, a.attribute).cmp(&(&b.item, b.attribute)));
    Report {
        eightr_version: env!("CARGO_PKG_VERSION"),
        inputs,
        markers,
        sources,
        findings,
        rules: Report::rule_usage(&applications),
        rewrites,
        enums: Vec::new(),
        summary,
        labels: entries,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
