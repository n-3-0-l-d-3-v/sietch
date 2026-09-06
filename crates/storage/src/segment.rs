//! A single append-only segment file. A segment is never opened for
//! read-write-in-place; it is either being appended to (the *active*
//! segment) or read sequentially (recovery, iteration, an older sealed
//! segment).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::record::{decode, DecodeOutcome, Record};

#[derive(Debug)]
pub struct RecoveredSegment {
    pub records: Vec<Record>,
    /// Byte offset up to which the segment contains valid, checksummed
    /// records. Anything beyond this in the physical file is an
    /// uncommitted, torn tail from an unclean shutdown.
    pub valid_len: u64,
    /// True if the segment contained any bytes past `valid_len` — i.e. a
    /// crash truly did leave a torn write behind, worth reporting to an
    /// operator even though recovery handled it safely.
    pub had_torn_tail: bool,
}

/// Reads every record from `path` from the beginning, stopping at the
/// first sign of truncation or corruption. This is the whole crash-safety
/// contract: whatever comes back in `records` is exactly what recovery
/// considers committed, and nothing else is ever trusted.
pub fn recover(path: &Path) -> io::Result<RecoveredSegment> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let mut records = Vec::new();
    let mut offset = 0usize;
    while let DecodeOutcome::Ok(record, len) = decode(&buf[offset..]) {
        records.push(record);
        offset += len;
    }

    let valid_len = offset as u64;
    let had_torn_tail = offset < buf.len();
    Ok(RecoveredSegment {
        records,
        valid_len,
        had_torn_tail,
    })
}

/// A segment open for appending. Recovery must run (via `recover`) and any
/// torn tail must be truncated *before* constructing this, so that new
/// appends always land immediately after the last valid record — never
/// inside or before it.
pub struct Segment {
    path: PathBuf,
    file: File,
    len: u64,
}

impl Segment {
    /// Opens `path` for appending, truncating it to `valid_len` first (the
    /// only truncation this codebase ever performs, and only of bytes that
    /// were never a complete, checksum-valid record — see
    /// `docs/design/decisions/ADR-001-torn-tail-truncation.md`).
    pub fn open_for_append(path: PathBuf, valid_len: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.set_len(valid_len)?;
        let mut file = file;
        file.seek(SeekFrom::Start(valid_len))?;
        Ok(Self {
            path,
            file,
            len: valid_len,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends one record and fsyncs it before returning — a record is
    /// never reported as written until it is durable.
    pub fn append(&mut self, record: &Record) -> io::Result<u64> {
        let bytes = record.encode();
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        let offset = self.len;
        self.len += bytes.len() as u64;
        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;
    use tempfile::tempdir;

    #[test]
    fn recovers_all_records_from_a_clean_segment() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("seg-0.log");
        {
            let mut seg = Segment::open_for_append(path.clone(), 0).unwrap();
            seg.append(&Record::put(1, b"a".to_vec(), b"1".to_vec()))
                .unwrap();
            seg.append(&Record::put(2, b"b".to_vec(), b"2".to_vec()))
                .unwrap();
            seg.append(&Record::delete(3, b"a".to_vec())).unwrap();
        }
        let recovered = recover(&path).unwrap();
        assert_eq!(recovered.records.len(), 3);
        assert!(!recovered.had_torn_tail);
    }

    #[test]
    fn recovers_up_to_a_torn_tail_and_reports_it() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("seg-0.log");
        {
            let mut seg = Segment::open_for_append(path.clone(), 0).unwrap();
            seg.append(&Record::put(1, b"a".to_vec(), b"1".to_vec()))
                .unwrap();
        }
        // simulate a crash mid-write: append a truncated record by hand.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0u8; 10]).unwrap(); // shorter than a full header
        }
        let recovered = recover(&path).unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert!(recovered.had_torn_tail);

        // Reopening for append at valid_len must discard exactly the torn
        // bytes and nothing that was previously valid.
        let mut seg = Segment::open_for_append(path.clone(), recovered.valid_len).unwrap();
        seg.append(&Record::put(2, b"b".to_vec(), b"2".to_vec()))
            .unwrap();
        let recovered_again = recover(&path).unwrap();
        assert_eq!(recovered_again.records.len(), 2);
        assert!(!recovered_again.had_torn_tail);
    }

    #[test]
    fn recovers_up_to_corruption_in_the_middle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("seg-0.log");
        let mut valid_len;
        {
            let mut seg = Segment::open_for_append(path.clone(), 0).unwrap();
            seg.append(&Record::put(1, b"a".to_vec(), b"1".to_vec()))
                .unwrap();
            valid_len = seg.len();
            seg.append(&Record::put(2, b"b".to_vec(), b"2".to_vec()))
                .unwrap();
        }
        // Flip a byte inside the second record's header (its `seq` field),
        // which the checksum covers, so decode must detect the mismatch.
        {
            let mut f = OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::Start(valid_len + 1)).unwrap();
            f.write_all(&[0xFFu8]).unwrap();
        }
        let recovered = recover(&path).unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert!(recovered.had_torn_tail);
        valid_len = recovered.valid_len;
        assert_eq!(valid_len, recovered.records[0].encoded_len() as u64);
    }
}
