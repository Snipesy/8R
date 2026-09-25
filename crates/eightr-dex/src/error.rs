use std::fmt;

/// Every error carries the byte offset (within the dex file) where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexError {
    pub offset: usize,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Needed `need` bytes but the file ended.
    Truncated { need: usize },
    BadMagic,
    UnsupportedVersion(u32),
    /// Byte-swapped dex files are legal per spec but never produced in practice.
    UnsupportedEndian(u32),
    /// An index into an id table is out of range.
    IndexOutOfRange { table: &'static str, index: u32, size: u32 },
    /// A LEB128 value ran past 5 bytes or overflowed 32 bits.
    BadLeb128,
    BadMutf8,
    Malformed(&'static str),
}

impl DexError {
    pub fn new(offset: usize, kind: ErrorKind) -> Self {
        DexError { offset, kind }
    }
}

impl fmt::Display for DexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Truncated { need } => write!(f, "truncated: needed {need} bytes")?,
            ErrorKind::BadMagic => write!(f, "not a dex file (bad magic)")?,
            ErrorKind::UnsupportedVersion(v) => write!(f, "unsupported dex version {v:03}")?,
            ErrorKind::UnsupportedEndian(t) => write!(f, "unsupported endian tag {t:#010x}")?,
            ErrorKind::IndexOutOfRange { table, index, size } => {
                write!(f, "{table} index {index} out of range (size {size})")?
            }
            ErrorKind::BadLeb128 => write!(f, "invalid LEB128 value")?,
            ErrorKind::BadMutf8 => write!(f, "invalid MUTF-8 string data")?,
            ErrorKind::Malformed(what) => write!(f, "malformed: {what}")?,
        }
        write!(f, " at offset {:#x}", self.offset)
    }
}

impl std::error::Error for DexError {}

pub type Result<T> = std::result::Result<T, DexError>;
