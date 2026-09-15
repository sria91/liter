//! Record (row) encoding and decoding for Liter-rs.
//!
//! Mirrors `record.c`. Implements the SQLite record format as specified in
//! <https://sqlite.org/fileformat2.html#record_format>.
//!
//! A record is a sequence of typed values prefixed by a header containing
//! varint-encoded type codes.

/// Error type for record operations.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("buffer too short")]
    BufferTooShort,
    #[error("invalid serial type: {0}")]
    InvalidSerialType(u64),
    #[error("varint overflow")]
    VarintOverflow,
}

pub type RecordResult<T> = Result<T, RecordError>;

/// A typed SQLite value, mirroring the VDBE `Mem` type.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(Vec<u8>), // UTF-8 bytes
    Blob(Vec<u8>),
    ZeroBlob(i64), // zero-filled blob of the given length
}

/// Decode a variable-length integer (varint) from `buf`.
/// Returns the decoded value and the number of bytes consumed (1–9).
pub fn decode_varint(buf: &[u8]) -> RecordResult<(u64, usize)> {
    let mut result = 0u64;
    for (i, &byte) in buf.iter().enumerate().take(9) {
        if i == 8 {
            // 9th byte: all 8 bits are data.
            result = (result << 8) | (byte as u64);
            return Ok((result, 9));
        }
        result = (result << 7) | ((byte & 0x7F) as u64);
        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
    }
    Err(RecordError::BufferTooShort)
}

/// Encode a value as a varint into `buf`. Returns the number of bytes written.
pub fn encode_varint(mut v: u64, buf: &mut [u8]) -> RecordResult<usize> {
    if buf.len() < 9 {
        return Err(RecordError::BufferTooShort);
    }
    if v <= 0x7F {
        buf[0] = v as u8;
        return Ok(1);
    }
    // 9-byte varint: the SQLite spec gives the 9th byte all 8 bits.
    // Values >= 2^56 require 9 bytes.
    if v >= (1u64 << 56) {
        // Byte 8 (last) gets the low 8 bits verbatim.
        buf[8] = (v & 0xFF) as u8;
        let mut remaining = v >> 8;
        // Bytes 0–7 each get 7 bits, all with continuation bit set.
        for i in (0..8).rev() {
            buf[i] = ((remaining & 0x7F) | 0x80) as u8;
            remaining >>= 7;
        }
        return Ok(9);
    }
    // Standard 2–8 byte case: every byte contributes 7 bits.
    let mut tmp = [0u8; 8];
    let mut n = 0usize;
    while v > 0 {
        tmp[n] = (v & 0x7F) as u8;
        v >>= 7;
        n += 1;
    }
    // Reverse and set continuation bits on all but the last byte.
    for i in 0..n {
        buf[i] = tmp[n - 1 - i] | if i + 1 < n { 0x80 } else { 0x00 };
    }
    Ok(n)
}

/// Decode a single value from `payload` using the given serial type.
pub fn decode_value(serial_type: u64, payload: &[u8]) -> RecordResult<(Value, usize)> {
    match serial_type {
        0 => Ok((Value::Null, 0)),
        1 => {
            if payload.is_empty() {
                return Err(RecordError::BufferTooShort);
            }
            Ok((Value::Int(payload[0] as i8 as i64), 1))
        }
        2 => {
            if payload.len() < 2 {
                return Err(RecordError::BufferTooShort);
            }
            let v = i16::from_be_bytes([payload[0], payload[1]]) as i64;
            Ok((Value::Int(v), 2))
        }
        3 => {
            if payload.len() < 3 {
                return Err(RecordError::BufferTooShort);
            }
            let fill = if payload[0] & 0x80 != 0 { 0xFF } else { 0x00 };
            let v = i32::from_be_bytes([fill, payload[0], payload[1], payload[2]]) as i64;
            Ok((Value::Int(v), 3))
        }
        4 => {
            if payload.len() < 4 {
                return Err(RecordError::BufferTooShort);
            }
            let v = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as i64;
            Ok((Value::Int(v), 4))
        }
        5 => {
            if payload.len() < 6 {
                return Err(RecordError::BufferTooShort);
            }
            let fill = if payload[0] & 0x80 != 0 { 0xFF } else { 0x00 };
            let mut arr = [fill; 8];
            arr[2..8].copy_from_slice(&payload[0..6]);
            let v = i64::from_be_bytes(arr);
            Ok((Value::Int(v), 6))
        }
        6 => {
            if payload.len() < 8 {
                return Err(RecordError::BufferTooShort);
            }
            let v = i64::from_be_bytes(payload[0..8].try_into().unwrap());
            Ok((Value::Int(v), 8))
        }
        7 => {
            if payload.len() < 8 {
                return Err(RecordError::BufferTooShort);
            }
            let v = f64::from_be_bytes(payload[0..8].try_into().unwrap());
            Ok((Value::Real(v), 8))
        }
        8 => Ok((Value::Int(0), 0)),
        9 => Ok((Value::Int(1), 0)),
        n if n >= 12 && n % 2 == 0 => {
            let len = ((n - 12) / 2) as usize;
            if payload.len() < len {
                return Err(RecordError::BufferTooShort);
            }
            Ok((Value::Blob(payload[..len].to_vec()), len))
        }
        n if n >= 13 && n % 2 == 1 => {
            let len = ((n - 13) / 2) as usize;
            if payload.len() < len {
                return Err(RecordError::BufferTooShort);
            }
            Ok((Value::Text(payload[..len].to_vec()), len))
        }
        other => Err(RecordError::InvalidSerialType(other)),
    }
}

/// Decode a full record from `buf`.
pub fn decode_record(buf: &[u8]) -> RecordResult<Vec<Value>> {
    if buf.is_empty() {
        return Ok(vec![]);
    }
    // Read header size varint.
    let (header_size, consumed) = decode_varint(buf)?;
    let header_size = header_size as usize;
    if header_size > buf.len() {
        return Err(RecordError::BufferTooShort);
    }

    // Read serial types from header.
    let mut pos = consumed;
    let mut serial_types = Vec::new();
    while pos < header_size {
        let (st, n) = decode_varint(&buf[pos..])?;
        serial_types.push(st);
        pos += n;
    }

    // Decode values from payload.
    let mut values = Vec::with_capacity(serial_types.len());
    let mut payload_pos = header_size;
    for st in serial_types {
        let (v, n) = decode_value(st, &buf[payload_pos..])?;
        values.push(v);
        payload_pos += n;
    }

    Ok(values)
}

/// Encode a full record into `buf`.
pub fn encode_record(values: &[Value]) -> RecordResult<Vec<u8>> {
    let mut header = Vec::new();
    let mut payload = Vec::new();

    for val in values {
        match val {
            Value::Null => {
                let mut tmp = [0u8; 9];
                let n = encode_varint(0, &mut tmp)?;
                header.extend_from_slice(&tmp[..n]);
            }
            Value::Int(i) => {
                let i = *i;
                if i == 0 {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(8, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                } else if i == 1 {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(9, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                } else if i >= i8::MIN as i64 && i <= i8::MAX as i64 {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(1, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    payload.push(i as i8 as u8);
                } else if i >= i16::MIN as i64 && i <= i16::MAX as i64 {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(2, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    payload.extend_from_slice(&(i as i16).to_be_bytes());
                } else if (-8388608..=8388607).contains(&i) {
                    // 24-bit
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(3, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    let bytes = (i as i32).to_be_bytes();
                    payload.extend_from_slice(&bytes[1..4]);
                } else if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(4, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    payload.extend_from_slice(&(i as i32).to_be_bytes());
                } else if (-140737488355328..=140737488355327).contains(&i) {
                    // 48-bit
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(5, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    let bytes = i.to_be_bytes();
                    payload.extend_from_slice(&bytes[2..8]);
                } else {
                    let mut tmp = [0u8; 9];
                    let n = encode_varint(6, &mut tmp)?;
                    header.extend_from_slice(&tmp[..n]);
                    payload.extend_from_slice(&i.to_be_bytes());
                }
            }
            Value::Real(f) => {
                let mut tmp = [0u8; 9];
                let n = encode_varint(7, &mut tmp)?;
                header.extend_from_slice(&tmp[..n]);
                payload.extend_from_slice(&f.to_be_bytes());
            }
            Value::Blob(b) => {
                let st = (b.len() * 2 + 12) as u64;
                let mut tmp = [0u8; 9];
                let n = encode_varint(st, &mut tmp)?;
                header.extend_from_slice(&tmp[..n]);
                payload.extend_from_slice(b);
            }
            Value::Text(t) => {
                let st = (t.len() * 2 + 13) as u64;
                let mut tmp = [0u8; 9];
                let n = encode_varint(st, &mut tmp)?;
                header.extend_from_slice(&tmp[..n]);
                payload.extend_from_slice(t);
            }
            Value::ZeroBlob(_) => {
                return Err(RecordError::InvalidSerialType(0)); // Unsupported directly right now
            }
        }
    }

    // Header size includes the header_size varint itself
    let mut header_size_varint = [0u8; 9];
    let mut hs = header.len() as u64;
    let mut n = encode_varint(hs, &mut header_size_varint)?;

    // Varint encoding might push the size over, iterate if necessary
    while hs != (header.len() + n) as u64 {
        hs = (header.len() + n) as u64;
        n = encode_varint(hs, &mut header_size_varint)?;
    }

    let mut record = Vec::with_capacity(n + header.len() + payload.len());
    record.extend_from_slice(&header_size_varint[..n]);
    record.extend_from_slice(&header);
    record.extend_from_slice(&payload);

    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_all_byte_lengths() {
        // Test values that encode to 1, 2, 3, 4, 5, 6, 7, 8, and 9 bytes
        let test_values = [
            (0u64, 1),
            (127, 1),
            (128, 2),
            (16383, 2),
            (16384, 3),
            (2097151, 3),
            (2097152, 4),
            (268435455, 4),
            (268435456, 5),
            (34359738367, 5),
            (34359738368, 6),
            (4398046511103, 6),
            (4398046511104, 7),
            (562949953421311, 7),
            (562949953421312, 8),
            ((1u64 << 56) - 1, 8),
            (1u64 << 56, 9),
            (u64::MAX, 9),
        ];

        let mut buf = [0u8; 9];
        for (val, expected_len) in test_values {
            let n = encode_varint(val, &mut buf).unwrap();
            assert_eq!(
                n, expected_len,
                "Value {} expected len {}",
                val, expected_len
            );
            let (decoded, consumed) = decode_varint(&buf[..n]).unwrap();
            assert_eq!(decoded, val);
            assert_eq!(consumed, expected_len);
        }
    }

    #[test]
    fn error_display_and_value_helpers() {
        let e1 = RecordError::BufferTooShort;
        let e2 = RecordError::InvalidSerialType(42);
        let e3 = RecordError::VarintOverflow;
        assert_eq!(format!("{e1}"), "buffer too short");
        assert_eq!(format!("{e2}"), "invalid serial type: 42");
        assert_eq!(format!("{e3}"), "varint overflow");

        let v = Value::ZeroBlob(100);
        let cloned = v.clone();
        assert_eq!(v, cloned);
        assert!(format!("{v:?}").contains("ZeroBlob(100)"));
    }

    #[test]
    fn varint_roundtrip_small() {
        let mut buf = [0u8; 9];
        let n = encode_varint(42, &mut buf).unwrap();
        let (v, m) = decode_varint(&buf[..n]).unwrap();
        assert_eq!(v, 42);
        assert_eq!(n, m);
    }

    #[test]
    fn varint_roundtrip_large() {
        let mut buf = [0u8; 9];
        let orig = 0x0FFF_FFFF_FFFF_FFFFu64;
        let n = encode_varint(orig, &mut buf).unwrap();
        let (v, _) = decode_varint(&buf[..n]).unwrap();
        assert_eq!(v, orig);
    }

    #[test]
    fn varint_roundtrip_maximum() {
        let mut buf = [0u8; 9];
        let orig = u64::MAX;
        let n = encode_varint(orig, &mut buf).unwrap();
        assert_eq!(n, 9);
        let (v, m) = decode_varint(&buf[..n]).unwrap();
        assert_eq!(v, orig);
        assert_eq!(m, 9);
    }

    #[test]
    fn varint_errors() {
        let mut short_buf = [0u8; 5];
        assert!(matches!(
            encode_varint(100, &mut short_buf),
            Err(RecordError::BufferTooShort)
        ));

        // Incomplete varint with continuation bit set
        let incomplete = [0x80u8, 0x80, 0x80];
        assert!(matches!(
            decode_varint(&incomplete),
            Err(RecordError::BufferTooShort)
        ));
    }

    #[test]
    fn decode_null_value() {
        let (v, n) = decode_value(0, &[]).unwrap();
        assert_eq!(v, Value::Null);
        assert_eq!(n, 0);
    }

    #[test]
    fn decode_int_constants() {
        let (v0, n0) = decode_value(8, &[]).unwrap();
        assert_eq!(v0, Value::Int(0));
        assert_eq!(n0, 0);

        let (v1, n1) = decode_value(9, &[]).unwrap();
        assert_eq!(v1, Value::Int(1));
        assert_eq!(n1, 0);
    }

    #[test]
    fn decode_integers_all_sizes() {
        // 1 byte
        let (v, n) = decode_value(1, &[0xFE]).unwrap();
        assert_eq!(v, Value::Int(-2));
        assert_eq!(n, 1);
        assert!(matches!(
            decode_value(1, &[]),
            Err(RecordError::BufferTooShort)
        ));

        // 2 bytes
        let (v, n) = decode_value(2, &[0x01, 0x00]).unwrap();
        assert_eq!(v, Value::Int(256));
        assert_eq!(n, 2);
        assert!(matches!(
            decode_value(2, &[1]),
            Err(RecordError::BufferTooShort)
        ));

        // 3 bytes (24-bit)
        let (v, n) = decode_value(3, &[0x01, 0x02, 0x03]).unwrap();
        assert_eq!(v, Value::Int(66051));
        assert_eq!(n, 3);
        let (v_neg, _) = decode_value(3, &[0xFF, 0xFE, 0xFD]).unwrap();
        assert_eq!(v_neg, Value::Int(-259));
        assert!(matches!(
            decode_value(3, &[1, 2]),
            Err(RecordError::BufferTooShort)
        ));

        // 4 bytes (32-bit)
        let (v, n) = decode_value(4, &[0x00, 0x01, 0x00, 0x00]).unwrap();
        assert_eq!(v, Value::Int(65536));
        assert_eq!(n, 4);
        assert!(matches!(
            decode_value(4, &[1, 2, 3]),
            Err(RecordError::BufferTooShort)
        ));

        // 6 bytes (48-bit)
        let (v, n) = decode_value(5, &[0x00, 0x00, 0x01, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(v, Value::Int(16777216));
        assert_eq!(n, 6);
        let (v_neg, _) = decode_value(5, &[0xFF, 0xFF, 0xFE, 0xFD, 0xFC, 0xFB]).unwrap();
        assert_eq!(v_neg, Value::Int(-16909061));
        assert!(matches!(
            decode_value(5, &[1, 2, 3, 4, 5]),
            Err(RecordError::BufferTooShort)
        ));

        // 8 bytes (64-bit)
        let (v, n) = decode_value(6, &[0, 0, 0, 0, 1, 0, 0, 0]).unwrap();
        assert_eq!(v, Value::Int(16777216));
        assert_eq!(n, 8);
        assert!(matches!(
            decode_value(6, &[1, 2, 3, 4, 5, 6, 7]),
            Err(RecordError::BufferTooShort)
        ));
    }

    #[test]
    fn decode_real() {
        let f: f64 = std::f64::consts::PI;
        let bytes = f.to_be_bytes();
        let (v, n) = decode_value(7, &bytes).unwrap();
        assert_eq!(v, Value::Real(f));
        assert_eq!(n, 8);
        assert!(matches!(
            decode_value(7, &[1, 2, 3]),
            Err(RecordError::BufferTooShort)
        ));
    }

    #[test]
    fn decode_blob_and_text() {
        // Blob: length 3 -> serial_type = 3*2 + 12 = 18
        let (v, n) = decode_value(18, b"xyz").unwrap();
        assert_eq!(v, Value::Blob(b"xyz".to_vec()));
        assert_eq!(n, 3);
        assert!(matches!(
            decode_value(18, b"xy"),
            Err(RecordError::BufferTooShort)
        ));

        // Text: length 5 -> serial_type = 5*2 + 13 = 23
        let (v, n) = decode_value(23, b"hello").unwrap();
        assert_eq!(v, Value::Text(b"hello".to_vec()));
        assert_eq!(n, 5);
        assert!(matches!(
            decode_value(23, b"hell"),
            Err(RecordError::BufferTooShort)
        ));

        // Invalid serial types (10, 11)
        assert!(matches!(
            decode_value(10, &[]),
            Err(RecordError::InvalidSerialType(10))
        ));
        assert!(matches!(
            decode_value(11, &[]),
            Err(RecordError::InvalidSerialType(11))
        ));
    }

    #[test]
    fn encode_decode_record_roundtrip() {
        let values = vec![
            Value::Null,
            Value::Int(0),
            Value::Int(1),
            Value::Int(-10),
            Value::Int(300),
            Value::Int(100_000),
            Value::Int(10_000_000),
            Value::Int(1_000_000_000_000),
            Value::Int(i64::MIN),
            Value::Real(2.5),
            Value::Blob(vec![1, 2, 3, 4]),
            Value::Text(b"SQLite Clone Liter".to_vec()),
        ];

        let encoded = encode_record(&values).unwrap();
        let decoded = decode_record(&encoded).unwrap();
        assert_eq!(values, decoded);
    }

    #[test]
    fn decode_record_edge_cases() {
        // Empty buffer
        let empty = decode_record(&[]).unwrap();
        assert_eq!(empty, vec![]);

        // Header size larger than buffer
        let bad_header = [0x10]; // header claims 16 bytes, but buffer is only 1 byte
        assert!(matches!(
            decode_record(&bad_header),
            Err(RecordError::BufferTooShort)
        ));

        // ZeroBlob encode error
        assert!(matches!(
            encode_record(&[Value::ZeroBlob(10)]),
            Err(RecordError::InvalidSerialType(0))
        ));
    }
}
