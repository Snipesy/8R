use crate::cursor::Cursor;
use crate::error::{ErrorKind, Result};
use crate::insn::{self, Decoded};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeItem {
    /// File offset of the code_item.
    pub off: u32,
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    pub debug_info_off: u32,
    pub insns: Vec<u16>,
    pub tries: Vec<TryItem>,
    /// Handlers keyed by their byte offset within the encoded_catch_handler_list, which is
    /// what `TryItem::handler_off` refers to. Sorted by offset.
    pub handlers: Vec<(u16, CatchHandler)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryItem {
    pub start_addr: u32,
    pub insn_count: u16,
    pub handler_off: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchHandler {
    /// (type_idx, handler address)
    pub catches: Vec<(u32, u32)>,
    pub catch_all: Option<u32>,
}

impl CodeItem {
    pub(crate) fn parse(data: &[u8], off: u32) -> Result<CodeItem> {
        let mut c = Cursor::new(data, off as usize);
        if !off.is_multiple_of(4) {
            return Err(c.err(ErrorKind::Malformed("code_item not 4-byte aligned")));
        }
        let registers_size = c.u16()?;
        let ins_size = c.u16()?;
        let outs_size = c.u16()?;
        let tries_size = c.u16()?;
        let debug_info_off = c.u32()?;
        let insns_size = c.u32()? as usize;
        let raw = c.bytes(insns_size.checked_mul(2).ok_or_else(|| c.err(ErrorKind::Malformed("insns_size overflow")))?)?;
        let insns = raw.as_chunks::<2>().0.iter().map(|&b| u16::from_le_bytes(b)).collect();
        if ins_size > registers_size {
            return Err(c.err(ErrorKind::Malformed("ins_size > registers_size")));
        }
        let mut tries = Vec::new();
        let mut handlers = Vec::new();
        if tries_size > 0 {
            if insns_size % 2 == 1 {
                c.u16()?; // padding
            }
            for _ in 0..tries_size {
                tries.push(TryItem { start_addr: c.u32()?, insn_count: c.u16()?, handler_off: c.u16()? });
            }
            let list_start = c.pos();
            let count = c.uleb128()?;
            for _ in 0..count {
                let rel = u16::try_from(c.pos() - list_start)
                    .map_err(|_| c.err(ErrorKind::Malformed("catch handler list too large")))?;
                let size = c.sleb128()?;
                let mut catches = Vec::new();
                for _ in 0..size.unsigned_abs() {
                    catches.push((c.uleb128()?, c.uleb128()?));
                }
                let catch_all = if size <= 0 { Some(c.uleb128()?) } else { None };
                handlers.push((rel, CatchHandler { catches, catch_all }));
            }
            for t in &tries {
                if handlers.binary_search_by_key(&t.handler_off, |(o, _)| *o).is_err() {
                    return Err(c.err(ErrorKind::Malformed("try_item handler_off matches no handler")));
                }
            }
        }
        Ok(CodeItem { off, registers_size, ins_size, outs_size, debug_info_off, insns, tries, handlers })
    }

    pub fn decode(&self) -> Result<Vec<Decoded>> {
        insn::decode_all(&self.insns, self.off as usize + 16)
    }

    pub fn handler(&self, handler_off: u16) -> Option<&CatchHandler> {
        self.handlers
            .binary_search_by_key(&handler_off, |(o, _)| *o)
            .ok()
            .map(|i| &self.handlers[i].1)
    }
}
