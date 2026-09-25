//! `debug_info_item` decoding: runs the DBG_* state machine to produce the position table
//! and local-variable events.

use crate::cursor::Cursor;
use crate::error::Result;

const DBG_FIRST_SPECIAL: u8 = 0x0a;
const DBG_LINE_BASE: i32 = -4;
const DBG_LINE_RANGE: i32 = 15;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DebugInfo {
    pub line_start: u32,
    /// String indices of parameter names (not including `this`); `None` = no name.
    pub parameter_names: Vec<Option<u32>>,
    pub positions: Vec<Position>,
    pub locals: Vec<LocalEvent>,
    /// Address of DBG_SET_PROLOGUE_END, if any.
    pub prologue_end: Option<u32>,
    pub epilogue_begin: Vec<u32>,
    /// DBG_SET_FILE events: (address, source file string index).
    pub file_changes: Vec<(u32, Option<u32>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub addr: u32,
    /// Line numbers are unsigned in the spec but the state machine can transiently go
    /// negative; R8's pc-encoding never emits negatives, so a negative here is surfaced
    /// verbatim rather than clamped.
    pub line: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEvent {
    Start { addr: u32, reg: u32, name: Option<u32>, ty: Option<u32>, sig: Option<u32> },
    End { addr: u32, reg: u32 },
    Restart { addr: u32, reg: u32 },
}

impl DebugInfo {
    pub(crate) fn parse(data: &[u8], off: u32) -> Result<DebugInfo> {
        let mut c = Cursor::new(data, off as usize);
        let line_start = c.uleb128()?;
        let params = c.uleb128()?;
        let mut info = DebugInfo { line_start, ..Default::default() };
        for _ in 0..params {
            info.parameter_names.push(c.uleb128p1()?);
        }
        let mut addr: u32 = 0;
        let mut line: i64 = i64::from(line_start);
        loop {
            let op = c.u8()?;
            match op {
                0x00 => break,
                0x01 => addr = addr.wrapping_add(c.uleb128()?),
                0x02 => line += i64::from(c.sleb128()?),
                0x03 => info.locals.push(LocalEvent::Start {
                    addr,
                    reg: c.uleb128()?,
                    name: c.uleb128p1()?,
                    ty: c.uleb128p1()?,
                    sig: None,
                }),
                0x04 => info.locals.push(LocalEvent::Start {
                    addr,
                    reg: c.uleb128()?,
                    name: c.uleb128p1()?,
                    ty: c.uleb128p1()?,
                    sig: c.uleb128p1()?,
                }),
                0x05 => info.locals.push(LocalEvent::End { addr, reg: c.uleb128()? }),
                0x06 => info.locals.push(LocalEvent::Restart { addr, reg: c.uleb128()? }),
                0x07 => info.prologue_end = Some(addr),
                0x08 => info.epilogue_begin.push(addr),
                0x09 => info.file_changes.push((addr, c.uleb128p1()?)),
                special => {
                    let adjusted = i32::from(special - DBG_FIRST_SPECIAL);
                    line += i64::from(DBG_LINE_BASE + adjusted % DBG_LINE_RANGE);
                    addr = addr.wrapping_add((adjusted / DBG_LINE_RANGE) as u32);
                    info.positions.push(Position { addr, line });
                }
            }
        }
        Ok(info)
    }

    /// The line for a given pc: the last position entry at or before it.
    pub fn line_at(&self, pc: u32) -> Option<i64> {
        self.positions.iter().take_while(|p| p.addr <= pc).last().map(|p| p.line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn special(line_delta: i32, addr_delta: i32) -> u8 {
        (DBG_FIRST_SPECIAL as i32 + (line_delta - DBG_LINE_BASE) + DBG_LINE_RANGE * addr_delta) as u8
    }

    #[test]
    fn state_machine() {
        let bytes = [
            10, // line_start
            2,  // parameters_size
            0,  // param 0: no name
            5,  // param 1: string 4
            0x07,              // prologue end @0
            special(0, 0),     // line 10 @0
            special(2, 3),     // line 12 @3
            0x02, 0x7b,        // advance_line -5 => 7
            0x01, 0x04,        // advance_pc 4 => 7
            special(0, 0),     // line 7 @7
            0x03, 1, 3, 4,     // start_local v1 name=2 type=3 @7
            0x05, 1,           // end_local v1
            0x09, 0,           // set_file NO_INDEX
            0x00,
        ];
        let d = DebugInfo::parse(&bytes, 0).unwrap();
        assert_eq!(d.line_start, 10);
        assert_eq!(d.parameter_names, vec![None, Some(4)]);
        assert_eq!(d.prologue_end, Some(0));
        assert_eq!(
            d.positions,
            vec![Position { addr: 0, line: 10 }, Position { addr: 3, line: 12 }, Position { addr: 7, line: 7 }]
        );
        assert_eq!(
            d.locals,
            vec![
                LocalEvent::Start { addr: 7, reg: 1, name: Some(2), ty: Some(3), sig: None },
                LocalEvent::End { addr: 7, reg: 1 },
            ]
        );
        assert_eq!(d.file_changes, vec![(7, None)]);
        assert_eq!(d.line_at(0), Some(10));
        assert_eq!(d.line_at(5), Some(12));
        assert_eq!(d.line_at(100), Some(7));
    }

    #[test]
    fn special_opcode_bounds() {
        // 0x0a: line -4, addr +0. 0xff: adjusted 245 => line +(245%15 - 4) = +1, addr +16.
        let d = DebugInfo::parse(&[20, 0, 0x0a, 0xff, 0x00], 0).unwrap();
        assert_eq!(d.positions, vec![Position { addr: 0, line: 16 }, Position { addr: 16, line: 17 }]);
    }

    #[test]
    fn truncated() {
        assert!(DebugInfo::parse(&[1, 0, 0x0a], 0).is_err());
    }
}
