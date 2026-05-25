//! UTF-8/16 encode/decode, case folding, and character classification.
//!
//! Mirrors `utf.c` from the C SQLite source.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

/// Error type for unicode operations.
#[derive(Debug, thiserror::Error)]
pub enum UnicodeError {
    #[error("invalid UTF-8 sequence")]
    InvalidUtf8,
    #[error("invalid UTF-16 sequence")]
    InvalidUtf16,
    #[error("buffer too small")]
    BufferTooSmall,
}

/// Decode a single UTF-8 codepoint from `bytes`, returning the codepoint and
/// the number of bytes consumed.
pub fn decode_utf8(bytes: &[u8]) -> Result<(char, usize), UnicodeError> {
    let s = std::str::from_utf8(bytes).map_err(|_| UnicodeError::InvalidUtf8)?;
    let mut chars = s.chars();
    let c = chars.next().ok_or(UnicodeError::InvalidUtf8)?;
    Ok((c, c.len_utf8()))
}

/// Encode a single Unicode codepoint to UTF-16 LE, writing into `buf`.
/// Returns the number of bytes written (2 or 4).
pub fn encode_utf16le(c: char, buf: &mut [u8]) -> Result<usize, UnicodeError> {
    let mut tmp = [0u16; 2];
    let encoded = c.encode_utf16(&mut tmp);
    let byte_len = encoded.len() * 2;
    if buf.len() < byte_len {
        return Err(UnicodeError::BufferTooSmall);
    }
    for (i, &unit) in encoded.iter().enumerate() {
        let b = unit.to_le_bytes();
        buf[i * 2] = b[0];
        buf[i * 2 + 1] = b[1];
    }
    Ok(byte_len)
}

/// Convert a UTF-8 string to UTF-16 LE bytes.
pub fn utf8_to_utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

/// Convert UTF-16 LE bytes to a Rust `String`.
pub fn utf16le_to_string(bytes: &[u8]) -> Result<String, UnicodeError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(UnicodeError::InvalidUtf16);
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| UnicodeError::InvalidUtf16)
}

/// SQLite-compatible case-insensitive comparison (ASCII only, mirrors
/// `sqlite3StrICmp`).
pub fn str_icmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();
    for (x, y) in a.iter().zip(b.iter()) {
        let cx = x.to_ascii_uppercase();
        let cy = y.to_ascii_uppercase();
        if cx != cy {
            return cx.cmp(&cy);
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_utf8_utf16le() {
        let s = "Hello, 世界!";
        let bytes = utf8_to_utf16le(s);
        let back = utf16le_to_string(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn decode_ascii() {
        let (c, n) = decode_utf8(b"A").unwrap();
        assert_eq!(c, 'A');
        assert_eq!(n, 1);
    }

    #[test]
    fn case_insensitive_cmp() {
        use std::cmp::Ordering;
        assert_eq!(str_icmp("ABC", "abc"), Ordering::Equal);
        assert_eq!(str_icmp("abc", "abd"), Ordering::Less);
    }
}
