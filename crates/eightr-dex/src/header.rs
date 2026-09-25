use crate::cursor::Cursor;
use crate::error::{DexError, ErrorKind, Result};

pub const HEADER_SIZE: u32 = 0x70;
pub const ENDIAN_CONSTANT: u32 = 0x1234_5678;
pub const REVERSE_ENDIAN_CONSTANT: u32 = 0x7856_3412;
pub const NO_INDEX: u32 = 0xffff_ffff;

/// Versions this reader accepts. 041 (the multi-dex container format) changes offset
/// semantics and is rejected until it is implemented and tested against real output.
pub const SUPPORTED_VERSIONS: &[u32] = &[35, 37, 38, 39, 40];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub checksum: u32,
    pub signature: [u8; 20],
    pub file_size: u32,
    pub header_size: u32,
    pub link_size: u32,
    pub link_off: u32,
    pub map_off: u32,
    pub string_ids: Section,
    pub type_ids: Section,
    pub proto_ids: Section,
    pub field_ids: Section,
    pub method_ids: Section,
    pub class_defs: Section,
    pub data: Section,
}

/// A (count, offset) pair from the header or the map list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Section {
    pub size: u32,
    pub off: u32,
}

/// Parses the three-digit version out of `dex\n0NN\0`.
pub fn parse_magic(magic: &[u8]) -> Result<u32> {
    let bad = || DexError::new(0, ErrorKind::BadMagic);
    if magic.len() != 8 || &magic[..4] != b"dex\n" || magic[7] != 0 {
        return Err(bad());
    }
    let digits = &magic[4..7];
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(bad());
    }
    Ok(digits.iter().fold(0, |acc, d| acc * 10 + u32::from(d - b'0')))
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Header> {
        let mut c = Cursor::new(data, 0);
        let version = parse_magic(c.bytes(8)?)?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(DexError::new(4, ErrorKind::UnsupportedVersion(version)));
        }
        let checksum = c.u32()?;
        let signature: [u8; 20] = c.bytes(20)?.try_into().expect("20 bytes");
        let file_size = c.u32()?;
        let header_size = c.u32()?;
        let endian = c.u32()?;
        if endian != ENDIAN_CONSTANT {
            return Err(DexError::new(0x28, ErrorKind::UnsupportedEndian(endian)));
        }
        let section = |c: &mut Cursor| -> Result<Section> {
            Ok(Section { size: c.u32()?, off: c.u32()? })
        };
        let link = section(&mut c)?;
        let map_off = c.u32()?;
        let header = Header {
            version,
            checksum,
            signature,
            file_size,
            header_size,
            link_size: link.size,
            link_off: link.off,
            map_off,
            string_ids: section(&mut c)?,
            type_ids: section(&mut c)?,
            proto_ids: section(&mut c)?,
            field_ids: section(&mut c)?,
            method_ids: section(&mut c)?,
            class_defs: section(&mut c)?,
            data: section(&mut c)?,
        };
        if header_size != HEADER_SIZE {
            return Err(DexError::new(0x24, ErrorKind::Malformed("header_size is not 0x70")));
        }
        if file_size as usize > data.len() || file_size < HEADER_SIZE {
            return Err(DexError::new(0x20, ErrorKind::Malformed("file_size exceeds input")));
        }
        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic() {
        assert_eq!(parse_magic(b"dex\n035\0").unwrap(), 35);
        assert_eq!(parse_magic(b"dex\n041\0").unwrap(), 41);
        assert!(parse_magic(b"dey\n035\0").is_err());
        assert!(parse_magic(b"dex\n0a5\0").is_err());
        assert!(parse_magic(b"dex\n035\n").is_err());
    }

    fn minimal(version: &[u8; 3]) -> Vec<u8> {
        let mut d = vec![0u8; 0x70];
        d[..4].copy_from_slice(b"dex\n");
        d[4..7].copy_from_slice(version);
        d[0x20..0x24].copy_from_slice(&0x70u32.to_le_bytes());
        d[0x24..0x28].copy_from_slice(&0x70u32.to_le_bytes());
        d[0x28..0x2c].copy_from_slice(&ENDIAN_CONSTANT.to_le_bytes());
        d
    }

    #[test]
    fn parses_minimal_header() {
        let h = Header::parse(&minimal(b"039")).unwrap();
        assert_eq!(h.version, 39);
        assert_eq!(h.file_size, 0x70);
    }

    #[test]
    fn rejects_container_version_and_reverse_endian() {
        assert_eq!(
            Header::parse(&minimal(b"041")).unwrap_err().kind,
            ErrorKind::UnsupportedVersion(41)
        );
        let mut d = minimal(b"035");
        d[0x28..0x2c].copy_from_slice(&REVERSE_ENDIAN_CONSTANT.to_le_bytes());
        assert!(matches!(Header::parse(&d).unwrap_err().kind, ErrorKind::UnsupportedEndian(_)));
    }

    #[test]
    fn rejects_truncation_and_bad_sizes() {
        let d = minimal(b"035");
        assert!(matches!(Header::parse(&d[..0x40]).unwrap_err().kind, ErrorKind::Truncated { .. }));
        let mut d = minimal(b"035");
        d[0x20..0x24].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(Header::parse(&d).is_err());
    }
}
