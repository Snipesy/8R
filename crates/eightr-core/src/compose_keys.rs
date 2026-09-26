//! `compose/lib-key`: library composables named by their durable group keys
//! (docs/research/compose-keys.md §8, docs/sources/compose.md `compose/lib-key`).
//!
//! A restartable composable opens with `startRestartGroup(K)`, K a hash of its source-level
//! identity (name, parameter types, package, file) that survives R8. The key DB
//! (`sigdb/compose-keys.ckdb`, built by `cargo xtask compose-keys` from the Compose AARs) maps
//! entry keys to the library function they open, with the other keys of its body (replace,
//! movable and lambda groups). An app method whose entry key is in the DB for exactly one
//! function, with a compatible shape, is that function: D; S when a second key of the function
//! occurs in the method or the lambdas it creates (a coincidental match of two 32-bit hashes on
//! one method is negligible).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use eightr_ir::lift::Body;
use eightr_ir::model::Program as Model;
use eightr_ir::op::{Const, Op};
use eightr_ir::types::parse_proto;
use serde::{Deserialize, Serialize};

use crate::compose::Composer;

const MAGIC: &[u8; 8] = b"8RCKDB01";
/// The DB shipped with 8R.
const EMBEDDED: &[u8] = include_bytes!("../../../sigdb/compose-keys.ckdb");
/// Keys below this magnitude are too likely to be ordinary constants.
pub const MIN_KEY: i32 = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyDb {
    /// (artifact, versions); a `versions` mask bit `i` is `versions[i]` of the entry's artifact.
    pub artifacts: Vec<(String, Vec<String>)>,
    /// (artifact index, owner descriptor, name, descriptor).
    pub functions: Vec<(u32, String, String, String)>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entry {
    /// The entry (restart group) key.
    pub key: i32,
    pub function: u32,
    pub versions: u32,
    /// Other survivable keys of the body: replace/movable/reusable groups, lambda keys.
    pub inner: Vec<i32>,
}

impl KeyDb {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let mut z = flate2::write::DeflateEncoder::new(&mut out, flate2::Compression::best());
        z.write_all(serde_json::to_string(self).expect("serializable").as_bytes()).expect("in-memory write");
        z.finish().expect("in-memory write");
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<KeyDb, String> {
        let body = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not a compose key DB")?;
        let mut raw = String::new();
        flate2::read::DeflateDecoder::new(body).read_to_string(&mut raw).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }

    pub fn embedded() -> &'static KeyDb {
        static DB: std::sync::OnceLock<KeyDb> = std::sync::OnceLock::new();
        DB.get_or_init(|| KeyDb::decode(EMBEDDED).unwrap_or_default())
    }
}

/// Group-call names whose key argument survives R8 (compose-keys.md §1.2).
const GROUP_CALLS: &[&str] = &["startRestartGroup", "startReplaceGroup", "startReplaceableGroup", "startMovableGroup", "startReusableGroup"];
/// Lambda constructions carrying a key.
const LAMBDA_CALLS: &[&str] = &["composableLambdaInstance", "composableLambda", "rememberComposableLambda", "composableLambdaNInstance", "composableLambdaN"];

/// Int constants (|k| ≥ MIN_KEY) passed to `calls` in `body`, by straight-line tracking.
fn call_keys(body: &Body, accept: &dyn Fn(&eightr_ir::op::MethodRef) -> bool) -> Vec<i32> {
    let mut consts: BTreeMap<u16, i32> = BTreeMap::new();
    let mut out = Vec::new();
    for x in &body.insns {
        if let Op::Invoke { method, args, .. } = &x.op {
            if accept(method) {
                out.extend(args.iter().filter_map(|r| consts.get(r)).filter(|k| k.unsigned_abs() >= MIN_KEY as u32));
            }
        }
        if let Some((d, wide)) = x.op.def() {
            consts.remove(&d);
            if wide {
                consts.remove(&(d + 1));
            }
            if let Op::Const { dst, value: Const::Narrow(k) } = x.op {
                consts.insert(dst, k);
            }
        }
    }
    out
}

/// Library side (names available): per method of an unminified library, its entry restart key
/// and inner keys. (owner, name, descriptor, entry key, inner keys).
pub fn extract(p: &Model) -> Vec<(String, String, String, i32, Vec<i32>)> {
    let s = &p.syms;
    let mut out = Vec::new();
    for c in &p.classes {
        for m in &c.methods {
            let Some(b) = &m.code else { continue };
            let Some((call, key, _)) = crate::compose::entry_call(p, b) else { continue };
            if s.get(call.name) != "startRestartGroup" || key.unsigned_abs() < MIN_KEY as u32 {
                continue;
            }
            let group = |r: &eightr_ir::op::MethodRef| GROUP_CALLS.contains(&s.get(r.name)) || LAMBDA_CALLS.contains(&s.get(r.name)) || s.get(r.name) == "<init>" && s.get(r.class).ends_with("/ComposableLambdaImpl;");
            let mut inner: Vec<i32> = call_keys(b, &group).into_iter().filter(|&k| k != key).collect();
            inner.sort();
            inner.dedup();
            out.push((s.get(c.ty).to_string(), s.get(m.name).to_string(), s.get(m.proto).to_string(), key, inner));
        }
    }
    out.sort();
    out
}

/// A library composable identified in the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibComposable {
    #[serde(skip)]
    pub class: usize,
    #[serde(skip)]
    pub method: usize,
    /// `Lclass;->name(proto)` in input names.
    pub method_ref: String,
    pub key: i32,
    /// Original owner, name and descriptor from the DB.
    pub owner: String,
    pub name: String,
    pub descriptor: String,
    pub artifact: String,
    /// Corroborated by a second key of the function (S), or by the entry key alone (D).
    pub corroborated: bool,
}

/// Reference-type parameters, as a multiset of "is a reference" counts (types are renamed:
/// only counts compare).
fn shape(proto: &str) -> Option<(usize, usize, usize)> {
    let (ps, _) = parse_proto(proto)?;
    let refs = ps.iter().filter(|t| t.starts_with('L') || t.starts_with('[')).count();
    let ints = ps.iter().filter(|t| **t == "I").count();
    Some((ps.len(), refs, ints))
}

/// Library composables among the restartable composables of the app.
pub fn identify(p: &Model, c: &Composer, db: &KeyDb) -> Vec<LibComposable> {
    let s = &p.syms;
    let mut by_key: BTreeMap<i32, Vec<&Entry>> = BTreeMap::new();
    for e in &db.entries {
        by_key.entry(e.key).or_default().push(e);
    }
    let mut out = Vec::new();
    for &(ci, mi, key) in &c.restartable {
        if key.unsigned_abs() < MIN_KEY as u32 {
            continue;
        }
        let Some(entries) = by_key.get(&key) else { continue };
        let fns: BTreeSet<u32> = entries.iter().map(|e| e.function).collect();
        let [f] = fns.iter().copied().collect::<Vec<_>>()[..] else { continue };
        let (artifact, owner, name, desc) = &db.functions[f as usize];
        let m = &p.classes[ci].methods[mi];
        let Some(b) = &m.code else { continue };
        // Order-free shape: R8 only removes params (and may add a receiver-turned-param).
        let (Some((n, refs, ints)), Some((fn_, frefs, _))) = (shape(s.get(m.proto)), shape(desc)) else { continue };
        // (+1: an instance composable R8 made static carries its receiver as a parameter.)
        if n > fn_ + 1 || refs > frefs + 1 || ints == 0 {
            continue;
        }
        // Corroboration: another key of the function in the method or in what it instantiates.
        let inner: BTreeSet<i32> = entries.iter().filter(|e| e.function == f).flat_map(|e| e.inner.iter().copied()).collect();
        let mut seen: BTreeSet<i32> = consts(b);
        for x in &b.insns {
            if let Op::NewInstance { ty, .. } = &x.op {
                if let Some(k) = p.find(s.get(*ty)) {
                    for mm in &p.classes[k].methods {
                        if let Some(bb) = &mm.code {
                            seen.extend(consts(bb));
                        }
                    }
                }
            }
        }
        let corroborated = inner.iter().any(|k| seen.contains(k));
        out.push(LibComposable {
            class: ci,
            method: mi,
            method_ref: format!("{}->{}{}", s.get(p.classes[ci].ty), s.get(m.name), s.get(m.proto)),
            key,
            owner: owner.clone(),
            name: name.clone(),
            descriptor: desc.clone(),
            artifact: db.artifacts.get(*artifact as usize).map_or(String::new(), |a| a.0.clone()),
            corroborated,
        });
    }
    out
}

fn consts(b: &Body) -> BTreeSet<i32> {
    b.insns
        .iter()
        .filter_map(|x| match x.op {
            Op::Const { value: Const::Narrow(k), .. } if k.unsigned_abs() >= MIN_KEY as u32 => Some(k),
            _ => None,
        })
        .collect()
}

/// Per artifact, the DB versions whose key sets (of the functions identified) the app's constants
/// cover most (compose-keys.md §7): (artifact, best versions, covered, total of the first). Several versions
/// tie when the identified functions' keys didn't change between them.
pub fn versions(p: &Model, found: &[LibComposable], db: &KeyDb) -> Vec<(String, Vec<String>, usize, usize)> {
    let mut app: BTreeSet<i32> = BTreeSet::new();
    for c in &p.classes {
        for m in &c.methods {
            if let Some(b) = &m.code {
                app.extend(consts(b));
            }
        }
    }
    let fns: BTreeSet<(String, String, String)> = found.iter().map(|f| (f.owner.clone(), f.name.clone(), f.descriptor.clone())).collect();
    let mut out = Vec::new();
    for (ai, (artifact, versions)) in db.artifacts.iter().enumerate() {
        // (covered, total) per version.
        let mut scores: Vec<(usize, usize, usize)> = Vec::new();
        for vi in 0..versions.len() {
            let mut keys: BTreeSet<i32> = BTreeSet::new();
            for e in &db.entries {
                let (a, o, n, d) = &db.functions[e.function as usize];
                if *a as usize == ai && e.versions & (1 << vi) != 0 && fns.contains(&(o.clone(), n.clone(), d.clone())) {
                    keys.insert(e.key);
                    keys.extend(&e.inner);
                }
            }
            if !keys.is_empty() {
                scores.push((keys.iter().filter(|k| app.contains(k)).count(), keys.len(), vi));
            }
        }
        // Most keys present (not recall: R8 drops unreached lambdas and their keys, which would
        // favour versions with fewer keys).
        let Some(&(bc, bt, _)) = scores.iter().max_by_key(|x| x.0) else { continue };
        let best: Vec<String> = scores.iter().filter(|x| x.0 == bc).map(|x| versions[x.2].clone()).collect();
        out.push((artifact.clone(), best, bc, bt));
    }
    out
}
