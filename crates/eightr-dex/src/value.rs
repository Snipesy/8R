//! `encoded_value`, `encoded_array`, and annotation structures.

use crate::cursor::Cursor;
use crate::error::{DexError, ErrorKind, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum EncodedValue {
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    MethodType(u32),
    MethodHandle(u32),
    String(u32),
    Type(u32),
    Field(u32),
    Method(u32),
    Enum(u32),
    Array(Vec<EncodedValue>),
    Annotation(EncodedAnnotation),
    Null,
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncodedAnnotation {
    pub type_idx: u32,
    /// (name string index, value)
    pub elements: Vec<(u32, EncodedValue)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Build,
    Runtime,
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub visibility: Visibility,
    pub annotation: EncodedAnnotation,
}

/// Nesting limit for arrays/annotations, so hostile input can't blow the stack.
const MAX_DEPTH: u32 = 64;

/// Reads `size` little-endian bytes, sign-extending from the top byte.
fn signed(c: &mut Cursor, size: usize) -> Result<i64> {
    let b = c.bytes(size)?;
    let mut v: u64 = 0;
    for (i, &x) in b.iter().enumerate() {
        v |= u64::from(x) << (8 * i);
    }
    let shift = 64 - 8 * size as u32;
    Ok(((v << shift) as i64) >> shift)
}

fn unsigned(c: &mut Cursor, size: usize) -> Result<u64> {
    let b = c.bytes(size)?;
    Ok(b.iter().enumerate().fold(0u64, |v, (i, &x)| v | (u64::from(x) << (8 * i))))
}

/// Floats/doubles are right-zero-extended: the stored bytes are the high-order bytes.
fn right_extended(c: &mut Cursor, size: usize, width: usize) -> Result<u64> {
    Ok(unsigned(c, size)? << (8 * (width - size)))
}

impl EncodedValue {
    #[cfg(test)]
    pub(crate) fn parse(c: &mut Cursor) -> Result<EncodedValue> {
        Self::parse_depth(c, 0)
    }

    fn parse_depth(c: &mut Cursor, depth: u32) -> Result<EncodedValue> {
        if depth > MAX_DEPTH {
            return Err(c.err(ErrorKind::Malformed("encoded_value nested too deeply")));
        }
        let start = c.pos();
        let header = c.u8()?;
        let arg = usize::from(header >> 5);
        let ty = header & 0x1f;
        let size = arg + 1;
        let bad = |what| DexError::new(start, ErrorKind::Malformed(what));
        let check = |max: usize| if size > max { Err(bad("encoded_value size too large")) } else { Ok(()) };
        let idx = |c: &mut Cursor| -> Result<u32> {
            check(4)?;
            Ok(unsigned(c, size)? as u32)
        };
        Ok(match ty {
            0x00 => {
                check(1)?;
                EncodedValue::Byte(signed(c, 1)? as i8)
            }
            0x02 => {
                check(2)?;
                EncodedValue::Short(signed(c, size)? as i16)
            }
            0x03 => {
                check(2)?;
                EncodedValue::Char(unsigned(c, size)? as u16)
            }
            0x04 => {
                check(4)?;
                EncodedValue::Int(signed(c, size)? as i32)
            }
            0x06 => EncodedValue::Long(signed(c, size)?),
            0x10 => {
                check(4)?;
                EncodedValue::Float(f32::from_bits(right_extended(c, size, 4)? as u32))
            }
            0x11 => EncodedValue::Double(f64::from_bits(right_extended(c, size, 8)?)),
            0x15 => EncodedValue::MethodType(idx(c)?),
            0x16 => EncodedValue::MethodHandle(idx(c)?),
            0x17 => EncodedValue::String(idx(c)?),
            0x18 => EncodedValue::Type(idx(c)?),
            0x19 => EncodedValue::Field(idx(c)?),
            0x1a => EncodedValue::Method(idx(c)?),
            0x1b => EncodedValue::Enum(idx(c)?),
            0x1c => {
                if arg != 0 {
                    return Err(bad("VALUE_ARRAY with nonzero arg"));
                }
                EncodedValue::Array(parse_array_depth(c, depth + 1)?)
            }
            0x1d => {
                if arg != 0 {
                    return Err(bad("VALUE_ANNOTATION with nonzero arg"));
                }
                EncodedValue::Annotation(EncodedAnnotation::parse_depth(c, depth + 1)?)
            }
            0x1e => {
                if arg != 0 {
                    return Err(bad("VALUE_NULL with nonzero arg"));
                }
                EncodedValue::Null
            }
            0x1f => match arg {
                0 => EncodedValue::Boolean(false),
                1 => EncodedValue::Boolean(true),
                _ => return Err(bad("VALUE_BOOLEAN arg not 0 or 1")),
            },
            _ => return Err(bad("unknown encoded_value type")),
        })
    }
}

pub(crate) fn parse_array(c: &mut Cursor) -> Result<Vec<EncodedValue>> {
    parse_array_depth(c, 0)
}

fn parse_array_depth(c: &mut Cursor, depth: u32) -> Result<Vec<EncodedValue>> {
    let size = c.uleb128()?;
    // Each value is at least one byte; don't pre-allocate from an untrusted count.
    let mut v = Vec::new();
    for _ in 0..size {
        v.push(EncodedValue::parse_depth(c, depth)?);
    }
    Ok(v)
}

impl EncodedAnnotation {
    pub(crate) fn parse(c: &mut Cursor) -> Result<EncodedAnnotation> {
        Self::parse_depth(c, 0)
    }

    fn parse_depth(c: &mut Cursor, depth: u32) -> Result<EncodedAnnotation> {
        let type_idx = c.uleb128()?;
        let size = c.uleb128()?;
        let mut elements = Vec::new();
        for _ in 0..size {
            let name = c.uleb128()?;
            elements.push((name, EncodedValue::parse_depth(c, depth)?));
        }
        Ok(EncodedAnnotation { type_idx, elements })
    }
}

impl Annotation {
    pub(crate) fn parse(data: &[u8], off: u32) -> Result<Annotation> {
        let mut c = Cursor::new(data, off as usize);
        let visibility = match c.u8()? {
            0 => Visibility::Build,
            1 => Visibility::Runtime,
            2 => Visibility::System,
            _ => return Err(DexError::new(off as usize, ErrorKind::Malformed("bad annotation visibility"))),
        };
        Ok(Annotation { visibility, annotation: EncodedAnnotation::parse(&mut c)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(b: &[u8]) -> Result<EncodedValue> {
        EncodedValue::parse(&mut Cursor::new(b, 0))
    }

    #[test]
    fn integers_sign_extend() {
        assert_eq!(parse(&[0x00, 0xff]).unwrap(), EncodedValue::Byte(-1));
        assert_eq!(parse(&[0x04, 0xff]).unwrap(), EncodedValue::Int(-1)); // 1-byte int
        assert_eq!(parse(&[0x24, 0x00, 0x80]).unwrap(), EncodedValue::Int(-32768));
        assert_eq!(parse(&[0x24, 0xff, 0x7f]).unwrap(), EncodedValue::Int(32767));
        assert_eq!(parse(&[0x03 | 0x20, 0xff, 0xff]).unwrap(), EncodedValue::Char(0xffff));
        assert_eq!(parse(&[0xe6, 1, 0, 0, 0, 0, 0, 0, 0x80]).unwrap(), EncodedValue::Long(i64::MIN + 1));
    }

    #[test]
    fn floats_right_zero_extend() {
        // 1.0f = 0x3f800000, stored as its two high-order bytes (little-endian: 80 3f).
        assert_eq!(parse(&[0x30, 0x80, 0x3f]).unwrap(), EncodedValue::Float(1.0));
        // 2.0 = 0x4000000000000000, stored as one byte 0x40.
        assert_eq!(parse(&[0x11, 0x40]).unwrap(), EncodedValue::Double(2.0));
    }

    #[test]
    fn composite() {
        // array [null, true, string@3]
        let v = parse(&[0x1c, 3, 0x1e, 0x3f, 0x17, 3]).unwrap();
        assert_eq!(
            v,
            EncodedValue::Array(vec![EncodedValue::Null, EncodedValue::Boolean(true), EncodedValue::String(3)])
        );
        // annotation type@5 { name@1 = int 7 }
        let v = parse(&[0x1d, 5, 1, 1, 0x04, 7]).unwrap();
        assert_eq!(
            v,
            EncodedValue::Annotation(EncodedAnnotation { type_idx: 5, elements: vec![(1, EncodedValue::Int(7))] })
        );
    }

    #[test]
    fn rejects_bad() {
        assert!(parse(&[0x20, 0, 0]).is_err()); // byte with size 2
        assert!(parse(&[0x5f]).is_err()); // boolean arg 2
        assert!(parse(&[0x01]).is_err()); // unknown type
        assert!(parse(&[0x64, 0, 0]).is_err()); // int size 4 bytes but truncated
        let deep = [0x1c, 1].repeat(100);
        assert!(parse(&deep).is_err());
    }
}
