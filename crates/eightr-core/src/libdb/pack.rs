//! A LibDB pack: fingerprints of library code as R8 emitted it in the forge's scenario builds of
//! one build profile (docs/research/libdb.md). Built by `8r-forge` (the only place a mapping is
//! read: to key the records); 8R only reads it.
//!
//! Records are keyed by the original (class, name, proto) the scenario's mapping gives; one record
//! per distinct body, with the set of scenarios that produced it. `unique` marks a body hash that
//! belongs to exactly one original method across every scenario of the pack.
//!
//! File format (`*.8rpack`): `8RPACK06`, then a raw-deflate stream of little-endian fields
//! (strings as u32 length + UTF-8, lists as u32 count + items).

use std::io::{Read, Write};

use super::profile::Profile;
use crate::sigdb::print::SKETCH;

const MAGIC: &[u8; 8] = b"8RPACK06";
/// Largest inflated pack accepted (a corrupt or hostile file can't exhaust memory).
const MAX_RAW: u64 = 2 << 30;
/// No artifact (a class the closure doesn't own, e.g. R8's own).
pub const NO_ARTIFACT: u32 = u32::MAX;
/// A callee or frame method outside the key table.
pub const NO_METHOD: u32 = u32::MAX;
/// A field access outside the field table.
pub const NO_FIELD: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub profile: Profile,
    /// The resolved closure: `group:artifact:version sha256 url`, sorted.
    pub lock: Vec<String>,
    /// The scenario catalog the pack was forged from (verbatim).
    pub catalog: String,
    /// Tool identities: (`r8`, version + sha256), (`javac`, version), (`android.jar`, sha256), ...
    pub tools: Vec<(String, String)>,
    /// Scenario names; bit `i` of a `scenarios` mask is `scenarios[i]` (at most 64).
    pub scenarios: Vec<String>,
    /// `group:artifact:version` of the closure, and whether the app declared it (directly or
    /// through a multiplatform redirect).
    pub artifacts: Vec<(String, bool)>,
    /// (original class descriptor, owning artifact or [`NO_ARTIFACT`]).
    pub classes: Vec<(String, u32)>,
    /// (class index, original name, original proto descriptor).
    pub methods: Vec<(u32, String, String)>,
    /// (class index, original name, original type descriptor).
    pub fields: Vec<(u32, String, String)>,
    pub records: Vec<Record>,
    pub class_records: Vec<ClassRecord>,
    /// Inline stacks, innermost frame first: (method index, original line or -1).
    pub stacks: Vec<Vec<(u32, i32)>>,
    /// The app this pack was forged for (its scenarios keep what that app's build kept), as
    /// `libdb::app_id` of its dex files; empty for a pack of the profile alone.
    pub app: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Record {
    pub method: u32,
    pub scenarios: u64,
    pub informative: bool,
    /// The body hash maps to this method only, across the whole pack.
    pub unique: bool,
    /// Keyed as `f` but taken from a default-argument bridge `f$default` (`f` specialized with its
    /// defaults): an app may call the same body either name.
    pub bridge: bool,
    /// The owner is a synthetic-looking class (`Foo$1`, `$lambda`, …) whose siblings can share a body.
    pub synthetic_owner: bool,
    pub all: u64,
    pub strings: u64,
    pub proto: u64,
    pub sketch: [u32; SKETCH],
    /// Program callees: (erased call token, callee method index or [`NO_METHOD`]).
    pub callees: Vec<(u64, u32)>,
    /// Program field accesses in order: (erased access token, field index or [`NO_FIELD`]).
    pub fields: Vec<(u64, u32)>,
    /// Inlined code: (first instruction, end instruction exclusive, stack index), over the body
    /// as fingerprinted (after 8R's rewrites).
    pub frames: Vec<(u32, u32, u32)>,
    /// Instructions in the body as fingerprinted.
    pub insns: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassRecord {
    pub class: u32,
    pub scenarios: u64,
    pub c2: u64,
    pub c3: u64,
    /// The original superclass, when it is a program class of that build (else [`NO_CLASS`]).
    pub sup: u32,
}

/// No class (a [`ClassRecord::sup`] outside the program).
pub const NO_CLASS: u32 = u32::MAX;

/// A dex member simple name (or `<init>`/`<clinit>`): names from a pack end up in the output.
fn simple_name(n: &str) -> bool {
    n == "<init>" || n == "<clinit>" || (!n.is_empty() && !n.chars().any(|c| c.is_whitespace() || c.is_control() || "/;[()<>.:".contains(c)))
}

/// A field or class type descriptor.
fn type_descriptor(t: &str) -> bool {
    let base = t.trim_start_matches('[');
    match base {
        "Z" | "B" | "C" | "S" | "I" | "J" | "F" | "D" => true,
        "V" => base.len() == t.len(),
        _ => base.strip_prefix('L').and_then(|x| x.strip_suffix(';')).is_some_and(|x| x.split('/').all(simple_name) && !x.contains('<')),
    }
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend(v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend(s.as_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl R<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("truncated pack")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|e| e.to_string())
    }
    /// A list count, bounded by the bytes left (each item takes at least one byte).
    fn len(&mut self) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > self.b.len() - self.at {
            return Err("corrupt pack: list longer than the file".into());
        }
        Ok(n)
    }
}

impl Pack {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::default();
        w.str(&self.profile.canonical());
        w.str(&self.app);
        w.len(self.lock.len());
        for l in &self.lock {
            w.str(l);
        }
        w.str(&self.catalog);
        w.len(self.tools.len());
        for (k, v) in &self.tools {
            w.str(k);
            w.str(v);
        }
        w.len(self.scenarios.len());
        for s in &self.scenarios {
            w.str(s);
        }
        w.len(self.artifacts.len());
        for (a, d) in &self.artifacts {
            w.str(a);
            w.u8(u8::from(*d));
        }
        w.len(self.classes.len());
        for (c, a) in &self.classes {
            w.str(c);
            w.u32(*a);
        }
        for table in [&self.methods, &self.fields] {
            w.len(table.len());
            for (c, n, p) in table {
                w.u32(*c);
                w.str(n);
                w.str(p);
            }
        }
        w.len(self.records.len());
        for r in &self.records {
            w.u32(r.method);
            w.u64(r.scenarios);
            w.u8(u8::from(r.informative) | (u8::from(r.unique) << 1) | (u8::from(r.bridge) << 2) | (u8::from(r.synthetic_owner) << 3));
            w.u64(r.all);
            w.u64(r.strings);
            w.u64(r.proto);
            for x in r.sketch {
                w.u32(x);
            }
            for list in [&r.callees, &r.fields] {
                w.len(list.len());
                for (t, k) in list {
                    w.u64(*t);
                    w.u32(*k);
                }
            }
            w.u32(r.insns);
            w.len(r.frames.len());
            for (a, b, s) in &r.frames {
                w.u32(*a);
                w.u32(*b);
                w.u32(*s);
            }
        }
        w.len(self.class_records.len());
        for r in &self.class_records {
            w.u32(r.class);
            w.u64(r.scenarios);
            w.u64(r.c2);
            w.u64(r.c3);
            w.u32(r.sup);
        }
        w.len(self.stacks.len());
        for st in &self.stacks {
            w.len(st.len());
            for (m, l) in st {
                w.u32(*m);
                w.i32(*l);
            }
        }
        let mut out = MAGIC.to_vec();
        let mut z = flate2::write::DeflateEncoder::new(&mut out, flate2::Compression::best());
        z.write_all(&w.0).expect("in-memory write");
        z.finish().expect("in-memory write");
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Pack, String> {
        let body = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not a LibDB pack")?;
        let mut raw = Vec::new();
        flate2::read::DeflateDecoder::new(body).take(MAX_RAW + 1).read_to_end(&mut raw).map_err(|e| e.to_string())?;
        if raw.len() as u64 > MAX_RAW {
            return Err("pack too large".into());
        }
        let mut r = R { b: &raw, at: 0 };
        let profile = Profile::parse(&r.str()?)?;
        let app = r.str()?;
        let lock = (0..r.len()?).map(|_| r.str()).collect::<Result<_, _>>()?;
        let catalog = r.str()?;
        let tools = (0..r.len()?).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<_, String>>()?;
        let scenarios: Vec<String> = (0..r.len()?).map(|_| r.str()).collect::<Result<_, _>>()?;
        let artifacts: Vec<(String, bool)> = (0..r.len()?).map(|_| Ok((r.str()?, r.u8()? != 0))).collect::<Result<_, String>>()?;
        let classes: Vec<(String, u32)> = (0..r.len()?).map(|_| Ok((r.str()?, r.u32()?))).collect::<Result<_, String>>()?;
        let methods: Vec<(u32, String, String)> = (0..r.len()?).map(|_| Ok((r.u32()?, r.str()?, r.str()?))).collect::<Result<_, String>>()?;
        let fields: Vec<(u32, String, String)> = (0..r.len()?).map(|_| Ok((r.u32()?, r.str()?, r.str()?))).collect::<Result<_, String>>()?;
        let n = r.len()?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            let method = r.u32()?;
            let scenarios = r.u64()?;
            let flags = r.u8()?;
            let (all, strings, proto) = (r.u64()?, r.u64()?, r.u64()?);
            let mut sketch = [0u32; SKETCH];
            for x in &mut sketch {
                *x = r.u32()?;
            }
            let callees = (0..r.len()?).map(|_| Ok((r.u64()?, r.u32()?))).collect::<Result<_, String>>()?;
            let fields = (0..r.len()?).map(|_| Ok((r.u64()?, r.u32()?))).collect::<Result<_, String>>()?;
            let insns = r.u32()?;
            let frames = (0..r.len()?).map(|_| Ok((r.u32()?, r.u32()?, r.u32()?))).collect::<Result<_, String>>()?;
            records.push(Record { method, scenarios, informative: flags & 1 != 0, unique: flags & 2 != 0, bridge: flags & 4 != 0, synthetic_owner: flags & 8 != 0, all, strings, proto, sketch, callees, fields, frames, insns });
        }
        let class_records: Vec<ClassRecord> = (0..r.len()?).map(|_| Ok(ClassRecord { class: r.u32()?, scenarios: r.u64()?, c2: r.u64()?, c3: r.u64()?, sup: r.u32()? })).collect::<Result<_, String>>()?;
        let stacks: Vec<Vec<(u32, i32)>> = (0..r.len()?).map(|_| (0..r.len()?).map(|_| Ok((r.u32()?, r.i32()?))).collect::<Result<Vec<_>, String>>()).collect::<Result<_, _>>()?;
        if r.at != raw.len() {
            return Err("trailing bytes in pack".into());
        }
        if scenarios.len() > 64 {
            return Err("pack has more than 64 scenarios".into());
        }
        // Cross-references in range, scenario bits within the scenarios.
        let bits = if scenarios.len() == 64 { u64::MAX } else { (1u64 << scenarios.len()) - 1 };
        let (nc, nm, ns, na) = (classes.len(), methods.len(), stacks.len(), artifacts.len());
        let bad = classes.iter().any(|(_, a): &(String, u32)| *a != NO_ARTIFACT && *a as usize >= na)
            || methods.iter().chain(&fields).any(|(c, _, _): &(u32, String, String)| *c as usize >= nc)
            || records.iter().any(|r| {
                r.method as usize >= nm
                    || r.scenarios & !bits != 0
                    || r.callees.iter().any(|(_, k)| *k != NO_METHOD && *k as usize >= nm)
                    || r.fields.iter().any(|(_, k)| *k != NO_FIELD && *k as usize >= fields.len())
                    || r.frames.iter().any(|(a, b, st)| a > b || *st as usize >= ns)
            })
            || class_records.iter().any(|r: &ClassRecord| r.class as usize >= nc || (r.sup != NO_CLASS && r.sup as usize >= nc) || r.scenarios & !bits != 0)
            || stacks.iter().any(|st: &Vec<(u32, i32)>| st.iter().any(|(m, _)| *m as usize >= nm));
        if bad {
            return Err("corrupt pack: index out of range".into());
        }
        let bad_name = classes.iter().any(|(c, _)| !type_descriptor(c) || c.starts_with('['))
            || methods.iter().any(|(_, n, pr)| !simple_name(n) || eightr_ir::types::parse_proto(pr).is_none_or(|(ps, r)| !ps.iter().all(|t| type_descriptor(t) && *t != "V") || !type_descriptor(r)))
            || fields.iter().any(|(_, n, t)| !simple_name(n) || n.starts_with('<') || !type_descriptor(t) || t == "V");
        if bad_name {
            return Err("corrupt pack: invalid class or member name".into());
        }
        Ok(Pack { profile, lock, catalog, tools, scenarios, artifacts, classes, methods, fields, records, class_records, stacks, app })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libdb::profile::Coord;

    #[test]
    fn round_trip() {
        let p = Pack {
            profile: Profile { r8: "9.4.24".into(), min_api: 26, mode: "full".into(), libraries: vec![Coord::parse("a.b:c:1.0").unwrap()] },
            lock: vec!["a.b:c:1.0 00ff https://x/c.jar".into()],
            catalog: "lib-alone all\n".into(),
            tools: vec![("r8".into(), "9.4.24".into())],
            scenarios: vec!["all".into()],
            artifacts: vec![("a.b:c:1.0".into(), true)],
            classes: vec![("La/b/C;".into(), 0), ("LR8$$;".into(), NO_ARTIFACT)],
            methods: vec![(0, "f".into(), "()V".into())],
            fields: vec![(0, "count".into(), "I".into())],
            records: vec![Record { method: 0, scenarios: 1, informative: true, unique: true, bridge: false, synthetic_owner: true, all: 7, strings: 0, proto: 3, sketch: [9; SKETCH], callees: vec![(5, NO_METHOD)], fields: vec![(6, 0), (6, NO_FIELD)], frames: vec![(0, 3, 0)], insns: 4 }],
            class_records: vec![ClassRecord { class: 0, scenarios: 1, c2: 1, c3: 2, sup: NO_CLASS }],
            stacks: vec![vec![(0, 12), (0, -1)]],
            app: "ab12".into(),
        };
        let bytes = p.encode();
        assert_eq!(Pack::decode(&bytes).unwrap(), p);
        assert!(Pack::decode(&bytes[..bytes.len() - 3]).is_err());
        // Names end up in the output dex: invalid ones reject the pack.
        for (n, pr) in [("a b", "()V"), ("", "()V"), ("x;y", "()V"), ("f", "(V)V"), ("f", "(Q)V")] {
            let mut q = p.clone();
            q.methods[0] = (0, n.into(), pr.into());
            assert!(Pack::decode(&q.encode()).is_err(), "{n}{pr}");
        }
    }
}
