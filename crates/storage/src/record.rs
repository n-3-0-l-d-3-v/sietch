//! On-disk record format for the append-only log. Every logical write
//! (`Put` or `Delete`) becomes one immutable, checksummed record; nothing
//! in this format supports rewriting a record in place.
//!
//! Layout (21-byte header, little-endian, followed by key then value bytes):
//!
//! ```text
//! byte 0      : record_type (0 = Put, 1 = Delete)
//! bytes 1..9  : seq            (u64)  — monotonic, assigned by the log
//! bytes 9..13 : key_len        (u32)
//! bytes 13..17: value_len      (u32)  — always 0 for Delete
//! bytes 17..21: crc32          (u32)  — over [record_type, seq, key_len,
//!               value_len, key bytes, value bytes]
//! bytes 21..21+key_len         : key
//! bytes 21+key_len..           : value
//! ```

pub const HEADER_LEN: usize = 21;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordType {
    Put,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub record_type: RecordType,
    pub seq: u64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

impl Record {
    pub fn put(seq: u64, key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            record_type: RecordType::Put,
            seq,
            key,
            value,
        }
    }

    pub fn delete(seq: u64, key: Vec<u8>) -> Self {
        Self {
            record_type: RecordType::Delete,
            seq,
            key,
            value: Vec::new(),
        }
    }

    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.key.len() + self.value.len()
    }

    fn checksum_input(record_type: u8, seq: u64, key: &[u8], value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 8 + 4 + 4 + key.len() + value.len());
        buf.push(record_type);
        buf.extend_from_slice(&seq.to_le_bytes());
        buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(value);
        buf
    }

    pub fn encode(&self) -> Vec<u8> {
        let record_type = match self.record_type {
            RecordType::Put => 0u8,
            RecordType::Delete => 1u8,
        };
        let checksum_input = Self::checksum_input(record_type, self.seq, &self.key, &self.value);
        let crc = crc32fast::hash(&checksum_input);

        let mut out = Vec::with_capacity(self.encoded_len());
        out.push(record_type);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&(self.key.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.value.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&self.key);
        out.extend_from_slice(&self.value);
        out
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeOutcome {
    /// A complete, checksum-valid record, and the total number of bytes it
    /// occupied in the buffer.
    Ok(Record, usize),
    /// Not enough bytes in the buffer to know yet — the caller should read
    /// more data (or, at end of stream, treat this as a torn write).
    Incomplete,
    /// Enough bytes were present but the checksum did not match — data
    /// corruption or a torn write that happened to land on a header
    /// boundary. Always treated as "nothing valid past this point."
    ChecksumMismatch,
}

/// Attempts to decode one record from the start of `buf`. Never panics on
/// malformed or truncated input — every failure mode is a `DecodeOutcome`
/// variant the caller (the segment reader / recovery logic) must handle
/// explicitly.
pub fn decode(buf: &[u8]) -> DecodeOutcome {
    if buf.len() < HEADER_LEN {
        return DecodeOutcome::Incomplete;
    }
    let record_type_byte = buf[0];
    let seq = u64::from_le_bytes(buf[1..9].try_into().unwrap());
    let key_len = u32::from_le_bytes(buf[9..13].try_into().unwrap()) as usize;
    let value_len = u32::from_le_bytes(buf[13..17].try_into().unwrap()) as usize;
    let stored_crc = u32::from_le_bytes(buf[17..21].try_into().unwrap());

    let total_len = HEADER_LEN + key_len + value_len;
    if buf.len() < total_len {
        return DecodeOutcome::Incomplete;
    }

    let key = buf[HEADER_LEN..HEADER_LEN + key_len].to_vec();
    let value = buf[HEADER_LEN + key_len..total_len].to_vec();

    let checksum_input = Record::checksum_input(record_type_byte, seq, &key, &value);
    let computed_crc = crc32fast::hash(&checksum_input);
    if computed_crc != stored_crc {
        return DecodeOutcome::ChecksumMismatch;
    }

    let record_type = match record_type_byte {
        0 => RecordType::Put,
        1 => RecordType::Delete,
        _ => return DecodeOutcome::ChecksumMismatch, // unknown type: treat as corrupt, not a panic
    };

    DecodeOutcome::Ok(
        Record {
            record_type,
            seq,
            key,
            value,
        },
        total_len,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_roundtrips() {
        let r = Record::put(7, b"key".to_vec(), b"value".to_vec());
        let bytes = r.encode();
        match decode(&bytes) {
            DecodeOutcome::Ok(decoded, len) => {
                assert_eq!(decoded, r);
                assert_eq!(len, bytes.len());
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn delete_roundtrips() {
        let r = Record::delete(9, b"gone".to_vec());
        let bytes = r.encode();
        match decode(&bytes) {
            DecodeOutcome::Ok(decoded, _) => assert_eq!(decoded, r),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn truncated_header_is_incomplete() {
        let r = Record::put(1, b"k".to_vec(), b"v".to_vec());
        let bytes = r.encode();
        assert_eq!(decode(&bytes[..5]), DecodeOutcome::Incomplete);
    }

    #[test]
    fn truncated_payload_is_incomplete() {
        let r = Record::put(1, b"key".to_vec(), b"value".to_vec());
        let bytes = r.encode();
        assert_eq!(decode(&bytes[..HEADER_LEN + 1]), DecodeOutcome::Incomplete);
    }

    #[test]
    fn corrupted_byte_is_checksum_mismatch() {
        let r = Record::put(1, b"key".to_vec(), b"value".to_vec());
        let mut bytes = r.encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(decode(&bytes), DecodeOutcome::ChecksumMismatch);
    }
}
