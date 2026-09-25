//! Modified UTF-8 as used by dex `string_data_item`s.
//!
//! Differences from UTF-8: U+0000 is encoded as `C0 80`, and supplementary characters are
//! encoded as a surrogate pair of two 3-byte sequences. Only 1-, 2-, and 3-byte forms exist.

/// Decodes MUTF-8 to UTF-16 code units. This is lossless: unpaired surrogates, which are
/// legal in dex strings (and used by obfuscators), are preserved.
pub fn to_utf16(bytes: &[u8]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        if b0 & 0x80 == 0 {
            if b0 == 0 {
                return None;
            }
            out.push(u16::from(b0));
            i += 1;
        } else if b0 & 0xe0 == 0xc0 {
            let b1 = *bytes.get(i + 1)?;
            if b1 & 0xc0 != 0x80 {
                return None;
            }
            out.push((u16::from(b0 & 0x1f) << 6) | u16::from(b1 & 0x3f));
            i += 2;
        } else if b0 & 0xf0 == 0xe0 {
            let b1 = *bytes.get(i + 1)?;
            let b2 = *bytes.get(i + 2)?;
            if b1 & 0xc0 != 0x80 || b2 & 0xc0 != 0x80 {
                return None;
            }
            out.push((u16::from(b0 & 0x0f) << 12) | (u16::from(b1 & 0x3f) << 6) | u16::from(b2 & 0x3f));
            i += 3;
        } else {
            return None;
        }
    }
    Some(out)
}

/// Decodes MUTF-8 to a Rust `String`. Unpaired surrogates become U+FFFD; use [`to_utf16`]
/// when exactness matters.
pub fn decode(bytes: &[u8]) -> Option<String> {
    // Fast path: plain ASCII without NUL is identical in MUTF-8 and UTF-8.
    if bytes.iter().all(|&b| b != 0 && b < 0x80) {
        return Some(String::from_utf8(bytes.to_vec()).expect("ascii"));
    }
    Some(String::from_utf16_lossy(&to_utf16(bytes)?))
}

/// Encodes a string as MUTF-8.
pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for unit in s.encode_utf16() {
        match unit {
            0x0001..=0x007f => out.push(unit as u8),
            0x0000 | 0x0080..=0x07ff => {
                out.push(0xc0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                out.push(0xe0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii() {
        assert_eq!(decode(b"Lcom/example/Main;").unwrap(), "Lcom/example/Main;");
    }

    #[test]
    fn nul_is_two_bytes() {
        assert_eq!(encode("a\0b"), vec![b'a', 0xc0, 0x80, b'b']);
        assert_eq!(decode(&[b'a', 0xc0, 0x80, b'b']).unwrap(), "a\0b");
        assert_eq!(decode(&[b'a', 0x00]), None);
    }

    #[test]
    fn supplementary_is_surrogate_pair() {
        let s = "x\u{1F600}y";
        let enc = encode(s);
        assert_eq!(enc.len(), 1 + 3 + 3 + 1);
        assert_eq!(decode(&enc).unwrap(), s);
    }

    #[test]
    fn two_and_three_byte() {
        for s in ["é", "ß", "中文", "\u{ffff}", "\u{800}"] {
            assert_eq!(decode(&encode(s)).unwrap(), s);
        }
    }

    #[test]
    fn unpaired_surrogate_is_preserved_in_utf16() {
        let bytes = [0xed, 0xa0, 0x80]; // U+D800 alone
        assert_eq!(to_utf16(&bytes).unwrap(), vec![0xd800]);
        assert_eq!(decode(&bytes).unwrap(), "\u{fffd}");
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(decode(&[0xc0]), None);
        assert_eq!(decode(&[0xe0, 0x80]), None);
        assert_eq!(decode(&[0xf0, 0x9f, 0x98, 0x80]), None); // 4-byte UTF-8 is not MUTF-8
        assert_eq!(decode(&[0xc3, 0x28]), None);
    }
}
