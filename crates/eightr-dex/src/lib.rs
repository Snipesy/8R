//! Zero-copy DEX reader.
//!
//! Every read is bounds-checked: malformed input produces a [`DexError`] carrying the byte
//! offset, never a panic. The fuzz-style tests in `tests/` hold the crate to that.

pub mod class;
pub mod code;
mod cursor;
pub mod debug;
mod dex;
pub mod error;
pub mod header;
pub mod insn;
pub mod mutf8;
pub mod value;

pub use dex::{adler32, item_type, Dex, FieldId, MapItem, MethodHandle, MethodId, ProtoId};
pub use error::{DexError, ErrorKind, Result};
