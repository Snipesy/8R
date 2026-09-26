//! The library signature DB: per library, the fingerprints of every method of every configured
//! version as R8 leaves it (built by `cargo xtask sigdb`, docs/research/sigdb.md §5), keyed by the
//! original (class, name, proto) the build's mapping gives. A record is stored once per distinct
//! body, with the set of versions having it.
//!
//! Built by `cargo xtask sigdb` (the only place a mapping is read: to key the DB); 8R only reads it.
//!
//! File format (`<library>.sigdb`): `8RSIGDB2`, then a raw-deflate stream of little-endian
//! fields (strings as u32 length + UTF-8, lists as u32 count + items).

use std::io::{Read, Write};

use super::print::SKETCH;

const MAGIC: &[u8; 8] = b"8RSIGDB2";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SigDb {
    pub library: String,
    /// Version labels (at most 64); bit `i` of a `versions` mask is `versions[i]`.
    pub versions: Vec<String>,
    /// The R8 that built the residual code.
    pub r8: String,
    /// Original class descriptors.
    pub classes: Vec<String>,
    /// (class index, original name, original proto descriptor).
    pub methods: Vec<(u32, String, String)>,
    pub records: Vec<Record>,
    pub class_records: Vec<ClassRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Record {
    pub method: u32,
    pub versions: u64,
    pub informative: bool,
    pub all: u64,
    pub strings: u64,
    pub proto: u64,
    pub sketch: [u32; SKETCH],
    /// Program callees: (erased call token, callee method index or `u32::MAX`).
    pub callees: Vec<(u64, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassRecord {
    pub class: u32,
    pub versions: u64,
    pub c2: u64,
    pub c3: u64,
}

impl SigDb {
    pub fn new(library: &str, r8: &str) -> SigDb {
        SigDb { library: library.into(), r8: r8.into(), ..Default::default() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::new();
        let str_ = |w: &mut Vec<u8>, s: &str| {
            w.extend((s.len() as u32).to_le_bytes());
            w.extend(s.as_bytes());
        };
        let u32_ = |w: &mut Vec<u8>, v: u32| w.extend(v.to_le_bytes());
        let u64_ = |w: &mut Vec<u8>, v: u64| w.extend(v.to_le_bytes());
        str_(&mut w, &self.library);
        str_(&mut w, &self.r8);
        u32_(&mut w, self.versions.len() as u32);
        for v in &self.versions {
            str_(&mut w, v);
        }
        u32_(&mut w, self.classes.len() as u32);
        for c in &self.classes {
            str_(&mut w, c);
        }
        u32_(&mut w, self.methods.len() as u32);
        for (c, n, p) in &self.methods {
            u32_(&mut w, *c);
            str_(&mut w, n);
            str_(&mut w, p);
        }
        u32_(&mut w, self.records.len() as u32);
        for r in &self.records {
            u32_(&mut w, r.method);
            u64_(&mut w, r.versions);
            w.push(u8::from(r.informative));
            u64_(&mut w, r.all);
            u64_(&mut w, r.strings);
            u64_(&mut w, r.proto);
            for x in r.sketch {
                u32_(&mut w, x);
            }
            u32_(&mut w, r.callees.len() as u32);
            for (t, k) in &r.callees {
                u64_(&mut w, *t);
                u32_(&mut w, *k);
            }
        }
        u32_(&mut w, self.class_records.len() as u32);
        for r in &self.class_records {
            u32_(&mut w, r.class);
            u64_(&mut w, r.versions);
            u64_(&mut w, r.c2);
            u64_(&mut w, r.c3);
        }
        let mut out = MAGIC.to_vec();
        let mut z = flate2::write::DeflateEncoder::new(&mut out, flate2::Compression::best());
        z.write_all(&w).expect("in-memory write");
        z.finish().expect("in-memory write");
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<SigDb, String> {
        let body = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not a sigdb file")?;
        let mut raw = Vec::new();
        flate2::read::DeflateDecoder::new(body).read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let mut r = Reader { b: &raw, at: 0 };
        let library = r.str()?;
        let r8 = r.str()?;
        let versions = (0..r.u32()?).map(|_| r.str()).collect::<Result<_, _>>()?;
        let classes = (0..r.u32()?).map(|_| r.str()).collect::<Result<_, _>>()?;
        let n = r.u32()?;
        let mut methods = Vec::with_capacity(n as usize);
        for _ in 0..n {
            methods.push((r.u32()?, r.str()?, r.str()?));
        }
        let n = r.u32()?;
        let mut records = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let method = r.u32()?;
            let versions = r.u64()?;
            let informative = r.u8()? != 0;
            let (all, strings, proto) = (r.u64()?, r.u64()?, r.u64()?);
            let mut sketch = [0u32; SKETCH];
            for x in &mut sketch {
                *x = r.u32()?;
            }
            let k = r.u32()?;
            let mut callees = Vec::with_capacity(k as usize);
            for _ in 0..k {
                callees.push((r.u64()?, r.u32()?));
            }
            records.push(Record { method, versions, informative, all, strings, proto, sketch, callees });
        }
        let n = r.u32()?;
        let mut class_records = Vec::with_capacity(n as usize);
        for _ in 0..n {
            class_records.push(ClassRecord { class: r.u32()?, versions: r.u64()?, c2: r.u64()?, c3: r.u64()? });
        }
        if r.at != raw.len() {
            return Err("trailing bytes".into());
        }
        Ok(SigDb { library, versions, r8, classes, methods, records, class_records })
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("truncated sigdb")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|e| e.to_string())
    }
}
