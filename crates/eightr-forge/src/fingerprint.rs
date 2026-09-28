//! One scenario build → fingerprint records keyed by original names. The residual program is
//! normalized by 8R's own rewrites first (outlines inlined back, merged classes split), exactly as
//! an app is before matching; classes 8R split, R8's synthesized code and classes the closure
//! doesn't own (the generated callers) get no records.
//!
//! Inline frames come from the mapping: each instruction's residual line (its debug position, or
//! its pc when the method has none: R8's pc encoding) → the stack of frames R8 recorded there. They
//! are computed before the rewrites and carried to the normalized body by pc, only where the
//! instruction is unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use eightr_core::sigdb::print::{class_print, method_print, platform_stable, reflective_strings, SKETCH};
use eightr_ir::model::Program as Model;
use eightr_mapping::{ClassMapping, Mapping, MethodMapping};

use crate::names::{descriptor, Names};
use crate::tools::{read, read_string, Result};

/// Bumped whenever this module's output for the same scenario output changes (part of the pack
/// key, not of the scenario cache: R8 doesn't run again).
pub const REVISION: u32 = 3;

/// An original method: (class descriptor, name, proto).
pub type Key = (String, String, String);
/// Inlined code: (first insn, end insn, stack innermost first: (method, line)).
pub type Frames = Vec<(u32, u32, Vec<(Key, i32)>)>;
/// A residual method's pc → (op hash, inline stack).
pub type FrameTable = BTreeMap<u32, (u64, Vec<(Key, i32)>)>;
type Runs<'a> = BTreeMap<&'a str, Vec<(u32, u32, Vec<&'a MethodMapping>)>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MethodOut {
    pub key: Key,
    pub informative: bool,
    pub all: u64,
    pub strings: u64,
    pub proto: u64,
    pub sketch: [u32; SKETCH],
    pub callees: Vec<(u64, Option<Key>)>,
    pub frames: Frames,
}

#[derive(Debug, Default)]
pub struct ScenarioOut {
    pub methods: Vec<MethodOut>,
    /// (original class, C2, C3).
    pub classes: Vec<(String, u64, u64)>,
}

pub fn load_dex_dir(dir: &Path) -> Result<Model> {
    let mut bytes = Vec::new();
    for i in 1.. {
        let f = dir.join(if i == 1 { "classes.dex".to_string() } else { format!("classes{i}.dex") });
        if !f.exists() {
            break;
        }
        bytes.push(read(&f)?);
    }
    if bytes.is_empty() {
        return Err(format!("{}: no classes.dex", dir.display()));
    }
    let dexes: Vec<eightr_dex::Dex> = bytes.iter().map(|b| eightr_dex::Dex::parse(b).map_err(|e| format!("{e:?}"))).collect::<Result<_>>()?;
    let refs: Vec<&eightr_dex::Dex> = dexes.iter().collect();
    Model::load(&refs).map_err(|e| format!("{e:?}"))
}

/// `void f(int,java.lang.String)` + owner → an original key.
fn frame_key(owner_dotted: &str, m: &MethodMapping) -> Key {
    let ps: String = m.params.iter().map(|t| descriptor(t)).collect();
    (descriptor(owner_dotted), m.original_name.clone(), format!("({ps}){}", descriptor(&m.return_type)))
}

/// A class's inline runs: residual method name → [(first line, last line, inlined frames innermost
/// first)]. A run's last line is the method itself; R8 ≥ 9 wraps a method it moved or bridged in a
/// synthesized same-name frame, and then the method is the frame inside it (as
/// `ClassMapping::outermost_methods` reads it). R8's own synthesized frames aren't original code
/// and are left out of the stacks.
fn runs(cm: &ClassMapping) -> Runs<'_> {
    use eightr_mapping::Metadata;
    let all: Vec<(&MethodMapping, bool)> = cm.methods().map(|(m, md)| (m, md.iter().any(|x| x.parsed == Metadata::Synthesized))).collect();
    let mut out = Runs::new();
    let mut i = 0;
    while i < all.len() {
        let m = all[i].0;
        let Some((a, b)) = m.minified_range else {
            i += 1;
            continue;
        };
        let mut j = i;
        while j + 1 < all.len() && all[j + 1].0.obfuscated == m.obfuscated && all[j + 1].0.minified_range == m.minified_range {
            j += 1;
        }
        let mut outer = j;
        if all[j].1 && j > i && all[j - 1].0.original_name == all[j].0.original_name {
            outer = j - 1;
        }
        let inlined: Vec<&MethodMapping> = all[i..outer].iter().filter(|x| !x.1).map(|x| x.0).collect();
        out.entry(m.obfuscated.as_str()).or_default().push((a, b, inlined));
        i = j + 1;
    }
    out
}

fn op_hash(op: &eightr_ir::op::Op) -> u64 {
    eightr_core::sigdb::print::fnv(format!("{op:?}").as_bytes())
}

/// Per residual method: pc → (op hash, inline stack), for instructions with inlined frames.
pub type PreFrames = BTreeMap<(String, String, String), FrameTable>;

pub fn pre_frames(model: &Model, mapping: &Mapping) -> PreFrames {
    let by_obf = mapping.by_obfuscated();
    let s = &model.syms;
    let mut out = PreFrames::new();
    for c in &model.classes {
        let rc = s.get(c.ty);
        let dotted = rc.trim_start_matches('L').trim_end_matches(';').replace('/', ".");
        let Some(&k) = by_obf.get(dotted.as_str()) else { continue };
        let cm = &mapping.classes[k];
        let rs = runs(cm);
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            let Some(list) = rs.get(s.get(m.name)) else { continue };
            let mut table = BTreeMap::new();
            let mut pos = b.positions.iter().peekable();
            let mut line: Option<i64> = None;
            for (idx, x) in b.insns.iter().enumerate() {
                while let Some(&&(pi, l)) = pos.peek() {
                    if pi as usize > idx {
                        break;
                    }
                    line = Some(l);
                    pos.next();
                }
                let l = if b.positions.is_empty() { i64::from(x.pc) } else { line.unwrap_or(0) };
                let Ok(l) = u32::try_from(l) else { continue };
                // Overloads share a residual name; their ranges are disjoint.
                let Some((_, _, frames)) = list.iter().find(|(a, e, _)| *a <= l && l <= *e) else { continue };
                if frames.is_empty() {
                    continue;
                }
                let stack: Vec<(Key, i32)> = frames
                    .iter()
                    .map(|f| {
                        let owner = f.original_owner.clone().unwrap_or_else(|| cm.original.clone());
                        (frame_key(&owner, f), f.original_line(l).and_then(|x| i32::try_from(x).ok()).unwrap_or(-1))
                    })
                    .collect();
                table.insert(x.pc, (op_hash(&x.op), stack));
            }
            if !table.is_empty() {
                out.insert((rc.to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string()), table);
            }
        }
    }
    out
}

/// The inline frames of a normalized body: runs of instructions carried over by pc (and unchanged)
/// from the residual method's table.
pub fn body_frames(table: &FrameTable, b: &eightr_ir::lift::Body) -> Frames {
    let mut fr = Frames::new();
    for (idx, x) in b.insns.iter().enumerate() {
        let Some((_, stack)) = table.get(&x.pc).filter(|(h, _)| *h == op_hash(&x.op)) else { continue };
        let idx = idx as u32;
        match fr.last_mut() {
            Some(last) if last.1 == idx && &last.2 == stack => last.1 = idx + 1,
            _ => fr.push((idx, idx + 1, stack.clone())),
        }
    }
    fr
}

/// The function a Kotlin default-argument bridge `f$default(…, int mask…, Object)` stands for,
/// when that function (same class, name `f`, the bridge's parameters without the masks and marker,
/// with or without the leading receiver) is inlined into the bridge as the outermost inlined frame.
fn default_target(key: &Key, frames: &Frames) -> Option<Key> {
    let name = key.1.strip_suffix("$default")?;
    let (ps, ret) = eightr_ir::types::parse_proto(&key.2)?;
    let n = ps.len().checked_sub(1)?;
    if !ps[n].starts_with('L') {
        return None;
    }
    let masks = ps[..n].iter().rev().take_while(|t| **t == "I").count();
    // At least one mask; with no way to tell a trailing int parameter from a second mask, the
    // candidates below try each split.
    let mut protos = Vec::new();
    for k in 1..=masks {
        let m = n - k;
        protos.push(format!("({}){ret}", ps[..m].concat()));
        if m > 0 {
            protos.push(format!("({}){ret}", ps[1..m].concat()));
        }
    }
    frames.iter().filter_map(|x| x.2.last()).map(|x| &x.0).find(|k| k.0 == key.0 && k.1 == name && protos.contains(&k.2)).cloned()
}

/// Fingerprints one scenario's output directory (dex files + `mapping.txt`). `owned` says whether
/// an original class belongs to the closure.
pub fn scenario(dir: &Path, owned: &dyn Fn(&str) -> bool) -> Result<ScenarioOut> {
    let mut model = load_dex_dir(dir)?;
    let mapping = Mapping::parse_normalized(&read_string(&dir.join("mapping.txt"))?).map_err(|e| format!("{e:?}"))?;
    let names = Names::new(&mapping);
    let frames = pre_frames(&model, &mapping);
    let program: BTreeSet<String> = model.classes.iter().map(|c| model.syms.get(c.ty).to_string()).collect();
    let rewrites = eightr_core::rewrites::run_all(&mut model).map_err(|e| e.to_string())?;
    let split: BTreeSet<String> = rewrites.iter().filter(|r| r.rule == eightr_rules::SPLIT_MERGED_CLASS).map(|r| r.item.clone()).collect();
    let model = &model;
    let s = &model.syms;
    let stable = |d: &str| platform_stable(d);
    let reflective = reflective_strings(model);
    let mut out = ScenarioOut::default();
    for (ci, c) in model.classes.iter().enumerate() {
        let rc = s.get(c.ty);
        if !program.contains(rc) || split.contains(rc) || names.synthesized_classes.contains(rc) {
            continue;
        }
        let oc = names.class(rc);
        if owned(&oc) {
            let cp = class_print(model, ci, &stable);
            out.classes.push((oc, cp.c2, cp.c3));
        }
        for (mi, m) in c.methods.iter().enumerate() {
            let Some(mp) = method_print(model, ci, mi, &stable, &reflective) else { continue };
            let (rn, rp) = (s.get(m.name), s.get(m.proto));
            let Some(mut key) = names.method(rc, rn, rp) else { continue };
            if !owned(&key.0) {
                continue;
            }
            let callees = mp.callees.iter().map(|(tok, (cc, cn, cpr))| (*tok, if program.contains(cc.as_str()) { names.method(cc, cn, cpr) } else { None })).collect();
            let fr = match (frames.get(&(rc.to_string(), rn.to_string(), rp.to_string())), &m.code) {
                (Some(table), Some(b)) => body_frames(table, b),
                _ => Vec::new(),
            };
            // A Kotlin default-argument bridge with its own function inlined into it is that
            // function specialized with its defaults: the body an app has where the bridge was
            // inlined into the call site and the function survives (`setContent { … }`).
            if let Some(target) = default_target(&key, &fr) {
                key = target;
            }
            out.methods.push(MethodOut { key, informative: mp.informative, all: mp.all, strings: mp.strings, proto: mp.proto, sketch: mp.sketch, callees, frames: fr });
        }
    }
    out.methods.sort();
    out.classes.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: &str, n: &str, p: &str) -> Key {
        (c.into(), n.into(), p.into())
    }

    /// `f(x: Int = 0) = f(x, 1)`: `f(I)` inlined into `f$default` with `f(II)` inside it keys the
    /// bridge as `f(I)` (the outermost frame whose proto the bridge stands for), not `f(II)`.
    #[test]
    fn default_bridge_target_is_the_outermost_matching_frame() {
        let bridge = k("La/K;", "f$default", "(IILjava/lang/Object;)V");
        let frames: Frames = vec![(0, 3, vec![(k("La/K;", "f", "(II)V"), 5), (k("La/K;", "f", "(I)V"), 2)])];
        assert_eq!(default_target(&bridge, &frames), Some(k("La/K;", "f", "(I)V")));
        // Only the inner overload inlined: no key change.
        let frames: Frames = vec![(0, 3, vec![(k("La/K;", "f", "(II)V"), 5)])];
        assert_eq!(default_target(&bridge, &frames), None);
        // A member function: the bridge's receiver isn't a parameter of the target.
        let member = k("La/K;", "g$default", "(La/K;Ljava/lang/String;ILjava/lang/Object;)I");
        let frames: Frames = vec![(0, 3, vec![(k("La/K;", "g", "(Ljava/lang/String;)I"), 7)])];
        assert_eq!(default_target(&member, &frames), Some(k("La/K;", "g", "(Ljava/lang/String;)I")));
    }
}
