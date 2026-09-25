use std::borrow::Cow;

use crate::class::{AnnotationsDirectory, ClassData, ClassDef};
use crate::code::CodeItem;
use crate::cursor::Cursor;
use crate::debug::DebugInfo;
use crate::error::{DexError, ErrorKind, Result};
use crate::header::{Header, Section, NO_INDEX};
use crate::mutf8;
use crate::value::{self, Annotation, EncodedValue};

/// map_list type codes.
pub mod item_type {
    pub const HEADER_ITEM: u16 = 0x0000;
    pub const STRING_ID_ITEM: u16 = 0x0001;
    pub const TYPE_ID_ITEM: u16 = 0x0002;
    pub const PROTO_ID_ITEM: u16 = 0x0003;
    pub const FIELD_ID_ITEM: u16 = 0x0004;
    pub const METHOD_ID_ITEM: u16 = 0x0005;
    pub const CLASS_DEF_ITEM: u16 = 0x0006;
    pub const CALL_SITE_ID_ITEM: u16 = 0x0007;
    pub const METHOD_HANDLE_ITEM: u16 = 0x0008;
    pub const MAP_LIST: u16 = 0x1000;
    pub const TYPE_LIST: u16 = 0x1001;
    pub const ANNOTATION_SET_REF_LIST: u16 = 0x1002;
    pub const ANNOTATION_SET_ITEM: u16 = 0x1003;
    pub const CLASS_DATA_ITEM: u16 = 0x2000;
    pub const CODE_ITEM: u16 = 0x2001;
    pub const STRING_DATA_ITEM: u16 = 0x2002;
    pub const DEBUG_INFO_ITEM: u16 = 0x2003;
    pub const ANNOTATION_ITEM: u16 = 0x2004;
    pub const ENCODED_ARRAY_ITEM: u16 = 0x2005;
    pub const ANNOTATIONS_DIRECTORY_ITEM: u16 = 0x2006;
    pub const HIDDENAPI_CLASS_DATA_ITEM: u16 = 0xf000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapItem {
    pub ty: u16,
    pub size: u32,
    pub off: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoId {
    pub shorty_idx: u32,
    pub return_type_idx: u32,
    pub parameters_off: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldId {
    pub class_idx: u16,
    pub type_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodId {
    pub class_idx: u16,
    pub proto_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodHandle {
    pub kind: u16,
    pub field_or_method_idx: u16,
}

/// A parsed dex file. The header, map, and id tables are read eagerly (they are small and
/// fixed-size); everything in the data section is parsed on demand from the borrowed bytes.
pub struct Dex<'a> {
    data: &'a [u8],
    pub header: Header,
    pub map: Vec<MapItem>,
    call_site_ids: Section,
    method_handles: Section,
}

impl<'a> Dex<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Dex<'a>> {
        let header = Header::parse(data)?;
        let data = &data[..header.file_size as usize];
        let map = parse_map(data, header.map_off)?;
        let find = |ty: u16| {
            map.iter().find(|m| m.ty == ty).map(|m| Section { size: m.size, off: m.off }).unwrap_or_default()
        };
        let dex = Dex {
            data,
            call_site_ids: find(item_type::CALL_SITE_ID_ITEM),
            method_handles: find(item_type::METHOD_HANDLE_ITEM),
            header,
            map,
        };
        // Validate that every fixed-size table fits in the file.
        let h = &dex.header;
        for (name, s, width) in [
            ("string_ids", h.string_ids, 4u64),
            ("type_ids", h.type_ids, 4),
            ("proto_ids", h.proto_ids, 12),
            ("field_ids", h.field_ids, 8),
            ("method_ids", h.method_ids, 8),
            ("class_defs", h.class_defs, 32),
            ("call_site_ids", dex.call_site_ids, 4),
            ("method_handles", dex.method_handles, 8),
        ] {
            let end = u64::from(s.off) + u64::from(s.size) * width;
            if s.size > 0 && end > data.len() as u64 {
                return Err(DexError::new(s.off as usize, ErrorKind::Malformed(name)));
            }
        }
        Ok(dex)
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Adler-32 over everything after the checksum field.
    pub fn compute_checksum(&self) -> u32 {
        adler32(&self.data[12..])
    }

    /// SHA-1 over everything after the signature field.
    pub fn compute_signature(&self) -> [u8; 20] {
        use sha1::{Digest, Sha1};
        Sha1::digest(&self.data[32..]).into()
    }

    pub fn checksum_ok(&self) -> bool {
        self.compute_checksum() == self.header.checksum
    }

    pub fn signature_ok(&self) -> bool {
        self.compute_signature() == self.header.signature
    }

    fn entry(&self, table: &'static str, s: Section, width: u32, idx: u32) -> Result<Cursor<'a>> {
        if idx >= s.size {
            return Err(DexError::new(s.off as usize, ErrorKind::IndexOutOfRange { table, index: idx, size: s.size }));
        }
        Ok(Cursor::new(self.data, s.off as usize + (idx * width) as usize))
    }

    // ---- strings ----

    pub fn string_count(&self) -> u32 {
        self.header.string_ids.size
    }

    /// Raw MUTF-8 bytes of a string (without the terminating NUL) and its declared UTF-16 length.
    pub fn string_raw(&self, idx: u32) -> Result<(&'a [u8], u32)> {
        let off = self.entry("string_ids", self.header.string_ids, 4, idx)?.u32()?;
        let mut c = Cursor::new(self.data, off as usize);
        let utf16_len = c.uleb128()?;
        let start = c.pos();
        let rest = &self.data[start.min(self.data.len())..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| DexError::new(start, ErrorKind::Malformed("unterminated string_data_item")))?;
        Ok((&rest[..len], utf16_len))
    }

    pub fn string(&self, idx: u32) -> Result<Cow<'a, str>> {
        let (bytes, utf16_len) = self.string_raw(idx)?;
        let at = bytes.as_ptr() as usize - self.data.as_ptr() as usize;
        let err = || DexError::new(at, ErrorKind::BadMutf8);
        if bytes.is_ascii() {
            if bytes.len() as u32 != utf16_len {
                return Err(err());
            }
            return Ok(Cow::Borrowed(std::str::from_utf8(bytes).expect("ascii")));
        }
        let units = mutf8::to_utf16(bytes).ok_or_else(err)?;
        if units.len() as u32 != utf16_len {
            return Err(err());
        }
        Ok(Cow::Owned(String::from_utf16_lossy(&units)))
    }

    pub fn opt_string(&self, idx: Option<u32>) -> Result<Option<Cow<'a, str>>> {
        idx.map(|i| self.string(i)).transpose()
    }

    // ---- types, protos, fields, methods ----

    pub fn type_count(&self) -> u32 {
        self.header.type_ids.size
    }

    pub fn type_descriptor(&self, idx: u32) -> Result<Cow<'a, str>> {
        let s = self.entry("type_ids", self.header.type_ids, 4, idx)?.u32()?;
        self.string(s)
    }

    pub fn type_list(&self, off: u32) -> Result<Vec<u32>> {
        if off == 0 {
            return Ok(Vec::new());
        }
        let mut c = Cursor::new(self.data, off as usize);
        let n = c.u32()?;
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(u32::from(c.u16()?));
        }
        Ok(v)
    }

    pub fn proto_count(&self) -> u32 {
        self.header.proto_ids.size
    }

    pub fn proto_id(&self, idx: u32) -> Result<ProtoId> {
        let mut c = self.entry("proto_ids", self.header.proto_ids, 12, idx)?;
        Ok(ProtoId { shorty_idx: c.u32()?, return_type_idx: c.u32()?, parameters_off: c.u32()? })
    }

    /// Method descriptor, e.g. `(ILjava/lang/String;)V`.
    pub fn proto_descriptor(&self, idx: u32) -> Result<String> {
        let p = self.proto_id(idx)?;
        let mut s = String::from("(");
        for t in self.type_list(p.parameters_off)? {
            s.push_str(&self.type_descriptor(t)?);
        }
        s.push(')');
        s.push_str(&self.type_descriptor(p.return_type_idx)?);
        Ok(s)
    }

    pub fn field_count(&self) -> u32 {
        self.header.field_ids.size
    }

    pub fn field_id(&self, idx: u32) -> Result<FieldId> {
        let mut c = self.entry("field_ids", self.header.field_ids, 8, idx)?;
        Ok(FieldId { class_idx: c.u16()?, type_idx: c.u16()?, name_idx: c.u32()? })
    }

    pub fn method_count(&self) -> u32 {
        self.header.method_ids.size
    }

    pub fn method_id(&self, idx: u32) -> Result<MethodId> {
        let mut c = self.entry("method_ids", self.header.method_ids, 8, idx)?;
        Ok(MethodId { class_idx: c.u16()?, proto_idx: c.u16()?, name_idx: c.u32()? })
    }

    /// `Lcom/Foo;->bar(I)V`
    pub fn method_ref(&self, idx: u32) -> Result<String> {
        let m = self.method_id(idx)?;
        Ok(format!(
            "{}->{}{}",
            self.type_descriptor(u32::from(m.class_idx))?,
            self.string(m.name_idx)?,
            self.proto_descriptor(u32::from(m.proto_idx))?
        ))
    }

    /// `Lcom/Foo;->bar:I`
    pub fn field_ref(&self, idx: u32) -> Result<String> {
        let f = self.field_id(idx)?;
        Ok(format!(
            "{}->{}:{}",
            self.type_descriptor(u32::from(f.class_idx))?,
            self.string(f.name_idx)?,
            self.type_descriptor(u32::from(f.type_idx))?
        ))
    }

    pub fn call_site_count(&self) -> u32 {
        self.call_site_ids.size
    }

    pub fn call_site(&self, idx: u32) -> Result<Vec<EncodedValue>> {
        let off = self.entry("call_site_ids", self.call_site_ids, 4, idx)?.u32()?;
        self.encoded_array(off)
    }

    pub fn method_handle_count(&self) -> u32 {
        self.method_handles.size
    }

    pub fn method_handle(&self, idx: u32) -> Result<MethodHandle> {
        let mut c = self.entry("method_handles", self.method_handles, 8, idx)?;
        let kind = c.u16()?;
        c.u16()?;
        Ok(MethodHandle { kind, field_or_method_idx: c.u16()? })
    }

    // ---- classes ----

    pub fn class_count(&self) -> u32 {
        self.header.class_defs.size
    }

    pub fn class_def(&self, idx: u32) -> Result<ClassDef> {
        let mut c = self.entry("class_defs", self.header.class_defs, 32, idx)?;
        let opt = |v: u32| (v != NO_INDEX).then_some(v);
        Ok(ClassDef {
            class_idx: c.u32()?,
            access_flags: c.u32()?,
            superclass_idx: opt(c.u32()?),
            interfaces_off: c.u32()?,
            source_file_idx: opt(c.u32()?),
            annotations_off: c.u32()?,
            class_data_off: c.u32()?,
            static_values_off: c.u32()?,
        })
    }

    pub fn class_defs(&self) -> impl Iterator<Item = Result<ClassDef>> + '_ {
        (0..self.class_count()).map(|i| self.class_def(i))
    }

    pub fn class_data(&self, def: &ClassDef) -> Result<ClassData> {
        if def.class_data_off == 0 {
            return Ok(ClassData::default());
        }
        ClassData::parse(self.data, def.class_data_off)
    }

    pub fn static_values(&self, def: &ClassDef) -> Result<Vec<EncodedValue>> {
        if def.static_values_off == 0 {
            return Ok(Vec::new());
        }
        self.encoded_array(def.static_values_off)
    }

    pub fn code_item(&self, off: u32) -> Result<Option<CodeItem>> {
        if off == 0 {
            return Ok(None);
        }
        CodeItem::parse(self.data, off).map(Some)
    }

    pub fn debug_info(&self, off: u32) -> Result<Option<DebugInfo>> {
        if off == 0 {
            return Ok(None);
        }
        DebugInfo::parse(self.data, off).map(Some)
    }

    pub fn encoded_array(&self, off: u32) -> Result<Vec<EncodedValue>> {
        value::parse_array(&mut Cursor::new(self.data, off as usize))
    }

    pub fn annotations_directory(&self, off: u32) -> Result<Option<AnnotationsDirectory>> {
        if off == 0 {
            return Ok(None);
        }
        AnnotationsDirectory::parse(self.data, off).map(Some)
    }

    pub fn annotation_set(&self, off: u32) -> Result<Vec<Annotation>> {
        if off == 0 {
            return Ok(Vec::new());
        }
        let mut c = Cursor::new(self.data, off as usize);
        let n = c.u32()?;
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(Annotation::parse(self.data, c.u32()?)?);
        }
        Ok(v)
    }

    /// Offsets of annotation sets, one per parameter (0 = none).
    pub fn annotation_set_ref_list(&self, off: u32) -> Result<Vec<u32>> {
        let mut c = Cursor::new(self.data, off as usize);
        let n = c.u32()?;
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(c.u32()?);
        }
        Ok(v)
    }

    /// All strings, in string-id order. Useful for marker detection and evidence scans.
    pub fn strings(&self) -> impl Iterator<Item = Result<Cow<'a, str>>> + '_ {
        (0..self.string_count()).map(|i| self.string(i))
    }

    /// Walks and parses every structure reachable from the header. Used by fuzz tests and
    /// `8r info --validate`; returns the first error.
    pub fn validate(&self) -> Result<()> {
        for s in self.strings() {
            s?;
        }
        for i in 0..self.type_count() {
            self.type_descriptor(i)?;
        }
        for i in 0..self.proto_count() {
            self.proto_descriptor(i)?;
        }
        for i in 0..self.field_count() {
            self.field_ref(i)?;
        }
        for i in 0..self.method_count() {
            self.method_ref(i)?;
        }
        for i in 0..self.call_site_count() {
            self.call_site(i)?;
        }
        for i in 0..self.method_handle_count() {
            self.method_handle(i)?;
        }
        for def in self.class_defs() {
            let def = def?;
            self.type_descriptor(def.class_idx)?;
            if let Some(s) = def.superclass_idx {
                self.type_descriptor(s)?;
            }
            for t in self.type_list(def.interfaces_off)? {
                self.type_descriptor(t)?;
            }
            self.opt_string(def.source_file_idx)?;
            self.static_values(&def)?;
            if let Some(dir) = self.annotations_directory(def.annotations_off)? {
                self.annotation_set(dir.class_annotations_off)?;
                for &(_, off) in dir.fields.iter().chain(&dir.methods) {
                    self.annotation_set(off)?;
                }
                for &(_, off) in &dir.parameters {
                    for set in self.annotation_set_ref_list(off)? {
                        self.annotation_set(set)?;
                    }
                }
            }
            let data = self.class_data(&def)?;
            for f in data.fields() {
                self.field_ref(f.field_idx)?;
            }
            for m in data.methods() {
                self.method_ref(m.method_idx)?;
                if let Some(code) = self.code_item(m.code_off)? {
                    code.decode()?;
                    self.debug_info(code.debug_info_off)?;
                }
            }
        }
        Ok(())
    }
}

fn parse_map(data: &[u8], off: u32) -> Result<Vec<MapItem>> {
    let mut c = Cursor::new(data, off as usize);
    let n = c.u32()?;
    let mut v = Vec::new();
    for _ in 0..n {
        let ty = c.u16()?;
        c.u16()?;
        v.push(MapItem { ty, size: c.u32()?, off: c.u32()? });
    }
    Ok(v)
}

pub fn adler32(bytes: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest n such that b doesn't overflow before the modulo.
    for chunk in bytes.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_known_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        let big = vec![0xffu8; 100_000];
        // Reference computed with zlib.adler32.
        assert_eq!(adler32(&big), 0x149a_302c);
    }
}
