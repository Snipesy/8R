//! The public API of the closure, read straight from the class files (no D8 needed): what the
//! generated scenarios call or keep, and which artifact owns each class.

use std::collections::BTreeMap;
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
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
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
    for _ in 0..c.u16()? {
        c.take(6)?;
        c.skip_attributes()?;
    }
    let mut methods = Vec::new();
    for _ in 0..c.u16()? {
        let (acc, ni, di) = (c.u16()?, c.u16()?, c.u16()?);
        methods.push((utf8.get(&ni)?.clone(), utf8.get(&di)?.clone(), acc));
        c.skip_attributes()?;
    }
    Some(ClassInfo { descriptor: format!("L{name};"), access, methods })
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

/// The API index of the closure: ownership of every class, and the public entry points.
#[derive(Debug, Default)]
pub struct Api {
    /// Class descriptor → artifact index (first owner wins, in closure order).
    pub owner: BTreeMap<String, u32>,
    pub entries: Vec<Entry>,
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
                    if c.access & ACC_PUBLIC == 0 {
                        continue;
                    }
                    for (n, d, acc) in &c.methods {
                        if acc & (ACC_PUBLIC | ACC_PROTECTED) == 0 || acc & (ACC_SYNTHETIC | ACC_BRIDGE) != 0 || n == "<clinit>" {
                            continue;
                        }
                        api.entries.push(Entry { artifact: ai as u32, class: c.descriptor.clone(), class_access: c.access, name: n.clone(), desc: d.clone(), access: *acc });
                    }
                }
            }
        }
        api.entries.sort();
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
