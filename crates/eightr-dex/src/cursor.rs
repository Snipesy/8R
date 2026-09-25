//! Bounds-checked little-endian reader. Nothing in this crate indexes a slice directly;
//! every read goes through here so malformed input yields an error, never a panic.

use crate::error::{DexError, ErrorKind, Result};

#[derive(Clone)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Cursor { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn err(&self, kind: ErrorKind) -> DexError {
        DexError::new(self.pos, kind)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| self.err(ErrorKind::Truncated { need: n }))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn uleb128(&mut self) -> Result<u32> {
        let start = self.pos;
        let mut result: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            let chunk = u32::from(b & 0x7f);
            if i == 4 && chunk > 0x0f {
                return Err(DexError::new(start, ErrorKind::BadLeb128));
            }
            result |= chunk << (7 * i);
            if b & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err(DexError::new(start, ErrorKind::BadLeb128))
    }

    pub fn sleb128(&mut self) -> Result<i32> {
        let start = self.pos;
        let mut result: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            result |= u32::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                let shift = 7 * (i + 1);
                if shift < 32 {
                    // Sign-extend from bit (shift - 1).
                    let s = 32 - shift;
                    return Ok(((result << s) as i32) >> s);
                }
                return Ok(result as i32);
            }
        }
        Err(DexError::new(start, ErrorKind::BadLeb128))
    }

    /// `uleb128p1`: the encoded value minus one, so `0` decodes to `None` (NO_INDEX).
    pub fn uleb128p1(&mut self) -> Result<Option<u32>> {
        Ok(self.uleb128()?.checked_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uleb(b: &[u8]) -> Result<u32> {
        Cursor::new(b, 0).uleb128()
    }
    fn sleb(b: &[u8]) -> Result<i32> {
        Cursor::new(b, 0).sleb128()
    }

    // Vectors from the dex format spec, "LEB128" section.
    #[test]
    fn spec_vectors() {
        assert_eq!(sleb(&[0x00]).unwrap(), 0);
        assert_eq!(uleb(&[0x00]).unwrap(), 0);
        assert_eq!(Cursor::new(&[0x00], 0).uleb128p1().unwrap(), None);
        assert_eq!(sleb(&[0x01]).unwrap(), 1);
        assert_eq!(uleb(&[0x01]).unwrap(), 1);
        assert_eq!(Cursor::new(&[0x01], 0).uleb128p1().unwrap(), Some(0));
        assert_eq!(sleb(&[0x7f]).unwrap(), -1);
        assert_eq!(uleb(&[0x7f]).unwrap(), 127);
        assert_eq!(Cursor::new(&[0x7f], 0).uleb128p1().unwrap(), Some(126));
        assert_eq!(sleb(&[0x80, 0x7f]).unwrap(), -128);
        assert_eq!(uleb(&[0x80, 0x7f]).unwrap(), 16256);
        assert_eq!(Cursor::new(&[0x80, 0x7f], 0).uleb128p1().unwrap(), Some(16255));
    }

    #[test]
    fn extremes() {
        assert_eq!(uleb(&[0xff, 0xff, 0xff, 0xff, 0x0f]).unwrap(), u32::MAX);
        assert_eq!(sleb(&[0x80, 0x80, 0x80, 0x80, 0x78]).unwrap(), i32::MIN);
        assert_eq!(sleb(&[0xff, 0xff, 0xff, 0xff, 0x07]).unwrap(), i32::MAX);
    }

    #[test]
    fn rejects_overlong_and_truncated() {
        assert_eq!(uleb(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]).unwrap_err().kind, ErrorKind::BadLeb128);
        assert_eq!(uleb(&[0xff, 0xff, 0xff, 0xff, 0x1f]).unwrap_err().kind, ErrorKind::BadLeb128);
        assert!(matches!(uleb(&[0x80]).unwrap_err().kind, ErrorKind::Truncated { .. }));
    }

    #[test]
    fn fixed_width() {
        let mut c = Cursor::new(&[1, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12], 0);
        assert_eq!(c.u8().unwrap(), 1);
        assert_eq!(c.u16().unwrap(), 0x1234);
        assert_eq!(c.u32().unwrap(), 0x12345678);
        assert!(c.u8().is_err());
    }
}
