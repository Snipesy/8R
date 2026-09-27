//! `8r-forge grade`: a developer check of a pack against an app whose R8 mapping is known (a
//! fixture). Exact matching only: an informative app method whose body hash is unique in the app
//! and belongs to exactly one method of the pack. Both sides are normalized by 8R's rewrites.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use eightr_core::libdb::pack::NO_ARTIFACT;
use eightr_core::libdb::Pack;
use eightr_core::sigdb::print::{method_print, platform_stable, reflective_strings};
use eightr_mapping::Mapping;

use crate::names::Names;
use crate::tools::{read_string, Result};

type NamedFrames = Vec<(u32, u32, Vec<(String, String, String, i32)>)>;

#[derive(Debug, Default)]
pub struct Grade {
    /// App methods with code whose original class the pack's closure owns.
    pub library_methods: usize,
    pub matched: usize,
    pub correct: usize,
    /// Of the matches: how many the pack marks universe-unique, and how many of those are right.
    pub unique_matched: usize,
    pub unique_correct: usize,
    pub wrong: Vec<String>,
    /// Correct matches where the app's own mapping records inlined frames; of those, how many
    /// the pack's frame table reproduces exactly.
    pub with_frames: usize,
    pub frames_equal: usize,
}

pub fn grade(pack: &Pack, app_dir: &Path, mapping: &Path) -> Result<Grade> {
    let mut model = crate::fingerprint::load_dex_dir(app_dir)?;
    let mapping = Mapping::parse_normalized(&read_string(mapping)?).map_err(|e| format!("{e:?}"))?;
    let names = Names::new(&mapping);
    let pre = crate::fingerprint::pre_frames(&model, &mapping);
    let residual: BTreeSet<String> = model.classes.iter().map(|c| model.syms.get(c.ty).to_string()).collect();
    eightr_core::rewrites::run_all(&mut model).map_err(|e| e.to_string())?;
    let mut records_by_method: BTreeMap<(u32, u64), &eightr_core::libdb::pack::Record> = BTreeMap::new();
    for r in &pack.records {
        records_by_method.entry((r.method, r.all)).or_insert(r);
    }
    let owned: BTreeSet<&str> = pack.classes.iter().filter(|c| c.1 != NO_ARTIFACT).map(|c| c.0.as_str()).collect();
    let mut by_hash: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    let mut unique: BTreeSet<u64> = BTreeSet::new();
    for r in pack.records.iter().filter(|r| r.informative) {
        by_hash.entry(r.all).or_default().insert(r.method);
        if r.unique {
            unique.insert(r.all);
        }
    }
    let stable = |d: &str| platform_stable(d);
    let reflective = reflective_strings(&model);
    let s = &model.syms;
    let mut prints = Vec::new();
    let mut count: BTreeMap<u64, usize> = BTreeMap::new();
    for (ci, c) in model.classes.iter().enumerate() {
        for mi in 0..c.methods.len() {
            if let Some(p) = method_print(&model, ci, mi, &stable, &reflective) {
                *count.entry(p.all).or_default() += 1;
                prints.push(p);
            }
        }
    }
    let mut g = Grade::default();
    for p in &prints {
        let c = &model.classes[p.class];
        let m = &c.methods[p.method];
        // Classes 8R created (split merged classes) have no original to grade against.
        let truth = if residual.contains(s.get(c.ty)) { names.method(s.get(c.ty), s.get(m.name), s.get(m.proto)) } else { None };
        if truth.is_none() && !residual.contains(s.get(c.ty)) {
            continue;
        }
        if truth.as_ref().is_some_and(|t| owned.contains(t.0.as_str())) {
            g.library_methods += 1;
        }
        let Some(cands) = by_hash.get(&p.all).filter(|_| p.informative && count[&p.all] == 1) else { continue };
        if cands.len() != 1 {
            continue;
        }
        let k = *cands.iter().next().expect("one");
        let (ci, n, pr) = &pack.methods[k as usize];
        let got = (pack.classes[*ci as usize].0.clone(), n.clone(), pr.clone());
        let ok = truth.as_ref() == Some(&got);
        g.matched += 1;
        g.correct += usize::from(ok);
        if unique.contains(&p.all) {
            g.unique_matched += 1;
            g.unique_correct += usize::from(ok);
        }
        if ok {
            let key = (s.get(c.ty).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string());
            let mine = match (pre.get(&key), &m.code) {
                (Some(t), Some(b)) => crate::fingerprint::body_frames(t, b),
                _ => Vec::new(),
            };
            if !mine.is_empty() {
                g.with_frames += 1;
                let theirs: NamedFrames = records_by_method
                    .get(&(k, p.all))
                    .map(|r| {
                        r.frames
                            .iter()
                            .map(|(a, b, st)| (*a, *b, pack.stacks[*st as usize].iter().map(|(mk, l)| {
                                let (mc, mn, mp) = &pack.methods[*mk as usize];
                                (pack.classes[*mc as usize].0.clone(), mn.clone(), mp.clone(), *l)
                            }).collect()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mine: NamedFrames = mine.into_iter().map(|(a, b, st)| (a, b, st.into_iter().map(|((x, y, z), l)| (x, y, z, l)).collect())).collect();
                if mine == theirs {
                    g.frames_equal += 1;
                }
            }
        }
        if !ok && g.wrong.len() < 15 {
            g.wrong.push(format!("{}->{}{}  (truth {:?})", got.0, got.1, got.2, truth));
        }
    }
    Ok(g)
}
