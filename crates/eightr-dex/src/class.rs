use crate::cursor::Cursor;
use crate::error::{ErrorKind, Result};

pub mod access {
    pub const PUBLIC: u32 = 0x1;
    pub const PRIVATE: u32 = 0x2;
    pub const PROTECTED: u32 = 0x4;
    pub const STATIC: u32 = 0x8;
    pub const FINAL: u32 = 0x10;
    pub const SYNCHRONIZED: u32 = 0x20;
    pub const VOLATILE: u32 = 0x40;
    pub const BRIDGE: u32 = 0x40;
    pub const TRANSIENT: u32 = 0x80;
    pub const VARARGS: u32 = 0x80;
    pub const NATIVE: u32 = 0x100;
    pub const INTERFACE: u32 = 0x200;
    pub const ABSTRACT: u32 = 0x400;
    pub const STRICT: u32 = 0x800;
    pub const SYNTHETIC: u32 = 0x1000;
    pub const ANNOTATION: u32 = 0x2000;
    pub const ENUM: u32 = 0x4000;
    pub const CONSTRUCTOR: u32 = 0x10000;
    pub const DECLARED_SYNCHRONIZED: u32 = 0x20000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassDef {
    pub class_idx: u32,
    pub access_flags: u32,
    /// `None` for `java.lang.Object` (NO_INDEX).
    pub superclass_idx: Option<u32>,
    pub interfaces_off: u32,
    pub source_file_idx: Option<u32>,
    pub annotations_off: u32,
    pub class_data_off: u32,
    pub static_values_off: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedField {
    pub field_idx: u32,
    pub access_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedMethod {
    pub method_idx: u32,
    pub access_flags: u32,
    /// 0 for abstract/native methods.
    pub code_off: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassData {
    pub static_fields: Vec<EncodedField>,
    pub instance_fields: Vec<EncodedField>,
    pub direct_methods: Vec<EncodedMethod>,
    pub virtual_methods: Vec<EncodedMethod>,
}

impl ClassData {
    pub(crate) fn parse(data: &[u8], off: u32) -> Result<ClassData> {
        let mut c = Cursor::new(data, off as usize);
        let sizes = [c.uleb128()?, c.uleb128()?, c.uleb128()?, c.uleb128()?];
        let fields = |c: &mut Cursor, n: u32| -> Result<Vec<EncodedField>> {
            let mut idx: u32 = 0;
            let mut v = Vec::new();
            for i in 0..n {
                let diff = c.uleb128()?;
                if i > 0 && diff == 0 {
                    return Err(c.err(ErrorKind::Malformed("duplicate field in class_data")));
                }
                idx = idx.checked_add(diff).ok_or_else(|| c.err(ErrorKind::Malformed("field index overflow")))?;
                v.push(EncodedField { field_idx: idx, access_flags: c.uleb128()? });
            }
            Ok(v)
        };
        let methods = |c: &mut Cursor, n: u32| -> Result<Vec<EncodedMethod>> {
            let mut idx: u32 = 0;
            let mut v = Vec::new();
            for i in 0..n {
                let diff = c.uleb128()?;
                if i > 0 && diff == 0 {
                    return Err(c.err(ErrorKind::Malformed("duplicate method in class_data")));
                }
                idx = idx.checked_add(diff).ok_or_else(|| c.err(ErrorKind::Malformed("method index overflow")))?;
                v.push(EncodedMethod { method_idx: idx, access_flags: c.uleb128()?, code_off: c.uleb128()? });
            }
            Ok(v)
        };
        Ok(ClassData {
            static_fields: fields(&mut c, sizes[0])?,
            instance_fields: fields(&mut c, sizes[1])?,
            direct_methods: methods(&mut c, sizes[2])?,
            virtual_methods: methods(&mut c, sizes[3])?,
        })
    }

    pub fn methods(&self) -> impl Iterator<Item = &EncodedMethod> {
        self.direct_methods.iter().chain(&self.virtual_methods)
    }

    pub fn fields(&self) -> impl Iterator<Item = &EncodedField> {
        self.static_fields.iter().chain(&self.instance_fields)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnnotationsDirectory {
    pub class_annotations_off: u32,
    /// (field_idx, annotation_set_off)
    pub fields: Vec<(u32, u32)>,
    /// (method_idx, annotation_set_off)
    pub methods: Vec<(u32, u32)>,
    /// (method_idx, annotation_set_ref_list_off)
    pub parameters: Vec<(u32, u32)>,
}

impl AnnotationsDirectory {
    pub(crate) fn parse(data: &[u8], off: u32) -> Result<AnnotationsDirectory> {
        let mut c = Cursor::new(data, off as usize);
        let class_annotations_off = c.u32()?;
        let (nf, nm, np) = (c.u32()?, c.u32()?, c.u32()?);
        let mut pairs = |n: u32| -> Result<Vec<(u32, u32)>> {
            let mut v = Vec::new();
            for _ in 0..n {
                v.push((c.u32()?, c.u32()?));
            }
            Ok(v)
        };
        Ok(AnnotationsDirectory {
            class_annotations_off,
            fields: pairs(nf)?,
            methods: pairs(nm)?,
            parameters: pairs(np)?,
        })
    }
}

/// Renders access flags the way dexdump does (for tests and dumps).
pub fn access_string(flags: u32, kind: FlagKind) -> String {
    use access::*;
    let mut names = Vec::new();
    let table: &[(u32, &str)] = match kind {
        FlagKind::Class => &[
            (PUBLIC, "PUBLIC"), (PRIVATE, "PRIVATE"), (PROTECTED, "PROTECTED"), (STATIC, "STATIC"),
            (FINAL, "FINAL"), (INTERFACE, "INTERFACE"), (ABSTRACT, "ABSTRACT"),
            (SYNTHETIC, "SYNTHETIC"), (ANNOTATION, "ANNOTATION"), (ENUM, "ENUM"),
        ],
        FlagKind::Field => &[
            (PUBLIC, "PUBLIC"), (PRIVATE, "PRIVATE"), (PROTECTED, "PROTECTED"), (STATIC, "STATIC"),
            (FINAL, "FINAL"), (VOLATILE, "VOLATILE"), (TRANSIENT, "TRANSIENT"),
            (SYNTHETIC, "SYNTHETIC"), (ENUM, "ENUM"),
        ],
        FlagKind::Method => &[
            (PUBLIC, "PUBLIC"), (PRIVATE, "PRIVATE"), (PROTECTED, "PROTECTED"), (STATIC, "STATIC"),
            (FINAL, "FINAL"), (SYNCHRONIZED, "SYNCHRONIZED"), (BRIDGE, "BRIDGE"),
            (VARARGS, "VARARGS"), (NATIVE, "NATIVE"), (ABSTRACT, "ABSTRACT"), (STRICT, "STRICT"),
            (SYNTHETIC, "SYNTHETIC"), (CONSTRUCTOR, "CONSTRUCTOR"),
            (DECLARED_SYNCHRONIZED, "DECLARED_SYNCHRONIZED"),
        ],
    };
    for &(bit, name) in table {
        if flags & bit != 0 {
            names.push(name);
        }
    }
    names.join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    Class,
    Field,
    Method,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_data_diffs() {
        // 1 static field, 0 instance, 1 direct, 1 virtual
        let bytes = [1, 0, 1, 1, /*sf*/ 3, 0x08, /*dm*/ 2, 0x81, 0x80, 0x04, 0x70, /*vm*/ 5, 1, 0];
        let d = ClassData::parse(&bytes, 0).unwrap();
        assert_eq!(d.static_fields, vec![EncodedField { field_idx: 3, access_flags: 0x8 }]);
        assert_eq!(d.direct_methods, vec![EncodedMethod { method_idx: 2, access_flags: 0x10001, code_off: 0x70 }]);
        // virtual method indices restart from 0
        assert_eq!(d.virtual_methods, vec![EncodedMethod { method_idx: 5, access_flags: 1, code_off: 0 }]);
    }

    #[test]
    fn rejects_duplicate_member() {
        let bytes = [2, 0, 0, 0, 3, 0, 0, 0];
        assert!(ClassData::parse(&bytes, 0).is_err());
    }

    #[test]
    fn flags() {
        assert_eq!(access_string(0x10001, FlagKind::Method), "PUBLIC CONSTRUCTOR");
        assert_eq!(access_string(0x4019, FlagKind::Field), "PUBLIC STATIC FINAL ENUM");
    }
}
