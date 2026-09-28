//! The public API of the closure, read straight from the class files (no D8 needed): what the
//! generated scenarios call or keep, and which artifact owns each class.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use crate::tools::{read, Result};

pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_PROTECTED: u16 = 0x0004;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_BRIDGE: u16 = 0x0040;
pub const ACC_INTERFACE: u16 = 0x0200;
pub const ACC_ABSTRACT: u16 = 0x0400;
pub const ACC_SYNTHETIC: u16 = 0x1000;

/// One class file's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInfo {
    /// Descriptor, `Lpkg/Name;`.
    pub descriptor: String,
    pub access: u16,
    /// (name, descriptor, access).
    pub methods: Vec<(String, String, u16)>,
    /// (name, type descriptor).
    pub fields: Vec<(String, String)>,
    /// Fields read from Android resource classes (`…/R$<type>`): (class, name, type).
    pub r_refs: Vec<(String, String, String)>,
    /// Kotlin default-argument bridges (`f$default(…, int mask, Object)` and constructors ending in
    /// `(…, int, DefaultConstructorMarker)`): (name, descriptor) → (mask of the bits the bridge
    /// tests, indices of the parameters those bits default).
    pub defaults: BTreeMap<(String, String), (u32, Vec<usize>)>,
}

/// How many mask ints a default-argument bridge has: the bridge's parameters are its function's
/// (plus a leading receiver for members), then the masks, then the marker. `None` without the
/// function.
fn mask_count(name: &str, desc: &str, methods: &[(String, String, u16)]) -> Option<usize> {
    let target = if name == "<init>" { "<init>" } else { name.strip_suffix("$default")? };
    let (ps, _) = eightr_ir::types::parse_proto(desc)?;
    let n = ps.len().checked_sub(1)?;
    for (mn, md, _) in methods {
        if mn != target || (mn == name && md == desc) {
            continue;
        }
        let Some((tp, _)) = eightr_ir::types::parse_proto(md) else { continue };
        for skip in [0, 1] {
            let Some(rest) = ps.get(skip..n) else { continue };
            if rest.len() > tp.len() && rest[..tp.len()] == tp[..] && rest[tp.len()..].iter().all(|t| *t == "I") {
                return Some(rest.len() - tp.len());
            }
        }
    }
    None
}

/// Instruction length at `pc` of JVM bytecode (`None`: malformed).
fn insn_len(code: &[u8], pc: usize) -> Option<usize> {
    let op = *code.get(pc)?;
    let word = |at: usize| -> Option<i64> { Some(i64::from(i32::from_be_bytes(code.get(at..at + 4)?.try_into().ok()?))) };
    Some(match op {
        0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => 2,
        0x11 | 0x13 | 0x14 | 0x84 | 0x99..=0xa8 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 | 0xc6 | 0xc7 => 3,
        0xc5 => 4,
        0xb9 | 0xba | 0xc8 | 0xc9 => 5,
        0xc4 => {
            if *code.get(pc + 1)? == 0x84 {
                6
            } else {
                4
            }
        }
        0xaa => {
            let base = (pc + 4) & !3;
            let (lo, hi) = (word(base + 4)?, word(base + 8)?);
            base - pc + 12 + 4 * (hi - lo + 1).max(0) as usize
        }
        0xab => {
            let base = (pc + 4) & !3;
            let n = word(base + 4)?.max(0) as usize;
            base - pc + 8 + 8 * n
        }
        0xca..=0xff => return None,
        _ => 1,
    })
}

/// The mask bits a default-argument bridge tests and the parameters they default: kotlinc emits
/// `iload mask; <bit>; iand; ifeq L; <default value>; <x>store p; L:` per defaulted parameter.
fn default_bits(code: &[u8], mask_slot: usize, param_slots: &[usize], ints: &BTreeMap<u16, i32>) -> (u32, Vec<usize>) {
    // Decoded instructions: (pc, opcode, operand).
    let mut insns: Vec<(usize, u8, i64)> = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        let Some(n) = insn_len(code, pc) else { break };
        let op = code[pc];
        let operand = match op {
            0x02..=0x08 => i64::from(op) - 3,
            0x10 => i64::from(code.get(pc + 1).copied().unwrap_or(0) as i8),
            0x11 => i64::from(i16::from_be_bytes([code.get(pc + 1).copied().unwrap_or(0), code.get(pc + 2).copied().unwrap_or(0)])),
            0x12 => ints.get(&u16::from(code.get(pc + 1).copied().unwrap_or(0))).map_or(i64::MIN, |&v| i64::from(v)),
            0x13 => ints.get(&u16::from_be_bytes([code.get(pc + 1).copied().unwrap_or(0), code.get(pc + 2).copied().unwrap_or(0)])).map_or(i64::MIN, |&v| i64::from(v)),
            0x15..=0x19 | 0x36..=0x3a => i64::from(code.get(pc + 1).copied().unwrap_or(0)),
            0x1a..=0x2d => i64::from((op - 0x1a) % 4),
            0x3b..=0x4e => i64::from((op - 0x3b) % 4),
            0x99..=0xa7 | 0xc6 | 0xc7 => pc as i64 + i64::from(i16::from_be_bytes([code.get(pc + 1).copied().unwrap_or(0), code.get(pc + 2).copied().unwrap_or(0)])),
            _ => 0,
        };
        insns.push((pc, op, operand));
        pc += n;
    }
    let is_iload_mask = |(_, op, v): (usize, u8, i64)| (op == 0x15 || (0x1a..=0x1d).contains(&op)) && v == mask_slot as i64;
    let is_const = |(_, op, v): (usize, u8, i64)| matches!(op, 0x02..=0x08 | 0x10 | 0x11 | 0x12 | 0x13) && v != i64::MIN;
    let mut mask = 0u32;
    let mut params = Vec::new();
    for i in 0..insns.len().saturating_sub(3) {
        let (a, b, c, d) = (insns[i], insns[i + 1], insns[i + 2], insns[i + 3]);
        let bit = if is_iload_mask(a) && is_const(b) {
            b.2
        } else if is_const(a) && is_iload_mask(b) {
            a.2
        } else {
            continue;
        };
        // kotlinc tests `(mask & bit) == 0` → `ifeq L` over the default's code. Bit 31 is the
        // negative int constant.
        let bit = if (i64::from(i32::MIN)..0).contains(&bit) { i64::from(bit as i32 as u32) } else { bit };
        if c.1 != 0x7e || d.1 != 0x99 || bit <= 0 || bit > i64::from(u32::MAX) {
            continue;
        }
        mask |= bit as u32;
        // The last store before L names the parameter.
        let end = d.2 as usize;
        let store = insns[i + 4..].iter().take_while(|x| x.0 < end).filter(|x| (0x36..=0x3a).contains(&x.1) || (0x3b..=0x4e).contains(&x.1)).last();
        if let Some(&(_, _, slot)) = store {
            if let Some(k) = param_slots.iter().position(|&s| s as i64 == slot) {
                params.push(k);
            }
        }
    }
    params.sort_unstable();
    params.dedup();
    (mask, params)
}


struct Cur<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cur<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn skip_attributes(&mut self) -> Option<()> {
        for _ in 0..self.u16()? {
            self.u16()?;
            let n = self.u32()? as usize;
            self.take(n)?;
        }
        Some(())
    }
}

/// Parses the parts of a class file the API index needs; `None` if it isn't one.
pub fn parse_class(b: &[u8]) -> Option<ClassInfo> {
    let mut c = Cur { b, at: 0 };
    if c.u32()? != 0xCAFE_BABE {
        return None;
    }
    c.u16()?;
    c.u16()?;
    let n = c.u16()? as usize;
    // Constant pool: UTF-8 entries by index, class entries → name index.
    let mut utf8: BTreeMap<u16, String> = BTreeMap::new();
    let mut ints: BTreeMap<u16, i32> = BTreeMap::new();
    let mut field_refs: BTreeMap<u16, (u16, u16)> = BTreeMap::new();
    let mut nats: BTreeMap<u16, (u16, u16)> = BTreeMap::new();
    let mut class_name: BTreeMap<u16, u16> = BTreeMap::new();
    let mut i = 1;
    while i < n {
        let idx = i as u16;
        match c.u8()? {
            1 => {
                let len = c.u16()? as usize;
                // Modified UTF-8; class and member names are plain in practice.
                utf8.insert(idx, String::from_utf8_lossy(c.take(len)?).into_owned());
            }
            7 => {
                class_name.insert(idx, c.u16()?);
            }
            8 | 16 | 19 | 20 => {
                c.u16()?;
            }
            15 => {
                c.take(3)?;
            }
            3 => {
                ints.insert(idx, c.u32()? as i32);
            }
            9 => {
                field_refs.insert(idx, (c.u16()?, c.u16()?));
            }
            12 => {
                nats.insert(idx, (c.u16()?, c.u16()?));
            }
            4 | 10 | 11 | 17 | 18 => {
                c.u32()?;
            }
            5 | 6 => {
                c.take(8)?;
                i += 1;
            }
            _ => return None,
        }
        i += 1;
    }
    let access = c.u16()?;
    let this = c.u16()?;
    let name = utf8.get(class_name.get(&this)?)?.clone();
    c.u16()?;
    let k = c.u16()? as usize;
    c.take(2 * k)?;
    let mut fields = Vec::new();
    for _ in 0..c.u16()? {
        let (_, ni, di) = (c.u16()?, c.u16()?, c.u16()?);
        fields.push((utf8.get(&ni)?.clone(), utf8.get(&di)?.clone()));
        c.skip_attributes()?;
    }
    let mut methods = Vec::new();
    let mut defaults = BTreeMap::new();
    for _ in 0..c.u16()? {
        let (acc, ni, di) = (c.u16()?, c.u16()?, c.u16()?);
        let (mn, md) = (utf8.get(&ni)?.clone(), utf8.get(&di)?.clone());
        let bridge = mn.ends_with("$default") || (mn == "<init>" && md.ends_with("ILkotlin/jvm/internal/DefaultConstructorMarker;)V"));
        let mut code = None;
        for _ in 0..c.u16()? {
            let an = c.u16()?;
            let n = c.u32()? as usize;
            let body = c.take(n)?;
            if bridge && utf8.get(&an).is_some_and(|x| x == "Code") && body.len() >= 8 {
                let len = u32::from_be_bytes(body[4..8].try_into().ok()?) as usize;
                code = body.get(8..8 + len).map(<[u8]>::to_vec);
            }
        }
        if let (Some(code), Some((ps, _))) = (code, eightr_ir::types::parse_proto(&md)) {
            // Exactly one mask int, then the marker (functions of more than 32 parameters have
            // several masks: see `mask_count`, applied once all methods are read).
            let n = ps.len();
            if n >= 2 && ps[n - 2] == "I" && ps[n - 1].starts_with('L') {
                let mut slot = usize::from(acc & ACC_STATIC == 0);
                let mut slots = Vec::new();
                for t in &ps {
                    slots.push(slot);
                    slot += if *t == "J" || *t == "D" { 2 } else { 1 };
                }
                let (mask, params) = default_bits(&code, slots[n - 2], &slots[..n - 2], &ints);
                if mask != 0 {
                    defaults.insert((mn.clone(), md.clone()), (mask, params));
                }
            }
        }
        methods.push((mn, md, acc));
    }
    let mut r_refs = Vec::new();
    for (ci, ni) in field_refs.values() {
        let (Some(cn), Some((n, t))) = (class_name.get(ci).and_then(|u| utf8.get(u)), nats.get(ni)) else { continue };
        let is_r = cn.rsplit('/').next().is_some_and(|x| x.strip_prefix("R$").is_some_and(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')));
        if let (true, Some(n), Some(t)) = (is_r, utf8.get(n), utf8.get(t)) {
            r_refs.push((format!("L{cn};"), n.clone(), t.clone()));
        }
    }
    r_refs.sort();
    r_refs.dedup();
    // Bridges with more than one mask int aren't modelled: drop them.
    defaults.retain(|(n, d), _| mask_count(n, d, &methods) == Some(1));
    Some(ClassInfo { descriptor: format!("L{name};"), access, methods, fields, r_refs, defaults })
}

/// The classes of a jar, sorted by descriptor (multi-release and module-info entries skipped).
pub fn jar_classes(jar: &Path) -> Result<Vec<ClassInfo>> {
    let bytes = read(jar)?;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("{}: {e}", jar.display()))?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let n = f.name().to_string();
        if !n.ends_with(".class") || n.starts_with("META-INF/") || n.ends_with("module-info.class") || n.ends_with("package-info.class") {
            continue;
        }
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| e.to_string())?;
        if let Some(c) = parse_class(&b) {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.descriptor.cmp(&b.descriptor));
    Ok(out)
}

/// A public entry point: (artifact index, class, class access, method name, descriptor, access).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub artifact: u32,
    pub class: String,
    pub class_access: u16,
    pub name: String,
    pub desc: String,
    pub access: u16,
}

/// A class's (methods (name, descriptor), fields (name, type)).
pub type Members = (Vec<(String, String)>, Vec<(String, String)>);

/// A public Kotlin default-argument bridge: the entry, the mask of all its defaults, and which
/// parameters (indices into the bridge's descriptor) take them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DefaultEntry {
    pub entry: Entry,
    pub mask: u32,
    pub defaulted: Vec<usize>,
}

/// The API index of the closure: ownership of every class, and the public entry points.
#[derive(Debug, Default)]
pub struct Api {
    /// Class descriptor → artifact index (first owner wins, in closure order).
    pub owner: BTreeMap<String, u32>,
    /// Class descriptor → access flags.
    pub access: BTreeMap<String, u16>,
    /// Class descriptor → its members.
    pub members: BTreeMap<String, Members>,
    /// Resource-class fields the closure reads: (class, name, type).
    pub r_refs: BTreeSet<(String, String, String)>,
    pub entries: Vec<Entry>,
    pub defaults: Vec<DefaultEntry>,
}

impl Api {
    pub fn build(libs: &[crate::artifacts::Lib]) -> Result<Api> {
        let mut api = Api::default();
        for (ai, lib) in libs.iter().enumerate() {
            for jar in &lib.jars {
                for c in jar_classes(jar)? {
                    if api.owner.contains_key(&c.descriptor) {
                        continue;
                    }
                    api.owner.insert(c.descriptor.clone(), ai as u32);
                    api.access.insert(c.descriptor.clone(), c.access);
                    api.members.insert(c.descriptor.clone(), (c.methods.iter().map(|m| (m.0.clone(), m.1.clone())).collect(), c.fields.clone()));
                    api.r_refs.extend(c.r_refs.iter().cloned());
                    if c.access & ACC_PUBLIC == 0 {
                        continue;
                    }
                    for (n, d, acc) in &c.methods {
                        if let Some((mask, defaulted)) = c.defaults.get(&(n.clone(), d.clone())) {
                            if acc & ACC_PUBLIC != 0 {
                                let entry = Entry { artifact: ai as u32, class: c.descriptor.clone(), class_access: c.access, name: n.clone(), desc: d.clone(), access: *acc };
                                api.defaults.push(DefaultEntry { entry, mask: *mask, defaulted: defaulted.clone() });
                            }
                        }
                        if acc & (ACC_PUBLIC | ACC_PROTECTED) == 0 || acc & (ACC_SYNTHETIC | ACC_BRIDGE) != 0 || n == "<clinit>" {
                            continue;
                        }
                        api.entries.push(Entry { artifact: ai as u32, class: c.descriptor.clone(), class_access: c.access, name: n.clone(), desc: d.clone(), access: *acc });
                    }
                }
            }
        }
        api.entries.sort();
        api.defaults.sort();
        Ok(api)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny hand-assembled class: `public class p/A { public static void f(I)V; private g()V }`.
    #[test]
    fn parses_a_class_file() {
        let mut b: Vec<u8> = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52];
        let utf = |b: &mut Vec<u8>, s: &str| {
            b.push(1);
            b.extend((s.len() as u16).to_be_bytes());
            b.extend(s.as_bytes());
        };
        b.extend(8u16.to_be_bytes()); // cp count = 7 entries + 1
        utf(&mut b, "p/A"); // 1
        b.extend([7, 0, 1]); // 2: class p/A
        utf(&mut b, "f"); // 3
        utf(&mut b, "(I)V"); // 4
        utf(&mut b, "g"); // 5
        utf(&mut b, "()V"); // 6
        b.extend([5, 0, 0, 0, 0, 0, 0, 0, 1]); // 7: long (takes two slots)
        // The count said 8 → indices 1..7; the long at 7 would take 8 too: bump the count.
        b[8..10].copy_from_slice(&9u16.to_be_bytes());
        b.extend(0x0021u16.to_be_bytes()); // public super
        b.extend(2u16.to_be_bytes()); // this
        b.extend(0u16.to_be_bytes()); // super
        b.extend(0u16.to_be_bytes()); // interfaces
        b.extend(0u16.to_be_bytes()); // fields
        b.extend(2u16.to_be_bytes()); // methods
        for (acc, n, d) in [(0x0009u16, 3u16, 4u16), (0x0002, 5, 6)] {
            b.extend(acc.to_be_bytes());
            b.extend(n.to_be_bytes());
            b.extend(d.to_be_bytes());
            b.extend(0u16.to_be_bytes());
        }
        let c = parse_class(&b).unwrap();
        assert_eq!(c.descriptor, "Lp/A;");
        assert_eq!(c.methods, vec![("f".to_string(), "(I)V".to_string(), 9), ("g".into(), "()V".into(), 2)]);
    }
}
