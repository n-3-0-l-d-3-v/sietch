//! The append-only log: a directory of segment files. New records go to
//! the active (last) segment; segments roll over once they pass
//! `segment_max_bytes`. Recovery replays every segment in order at open
//! time and truncates only a torn tail on the very last segment (an
//! interior segment with a torn tail would mean an earlier segment was
//! corrupted after being sealed, which is reported as an error, not
//! silently patched over).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::record::Record;
use crate::segment::{self, Segment};

const DEFAULT_SEGMENT_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SEGMENT_PREFIX: &str = "seg-";
const SEGMENT_SUFFIX: &str = ".log";

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("segment {0} (not the last segment) has a torn tail — on-disk corruption of a previously sealed segment")]
    SealedSegmentCorrupted(u64),
}

pub struct Log {
    dir: PathBuf,
    segment_max_bytes: u64,
    sealed: Vec<(u64, PathBuf)>,
    active_id: u64,
    active: Segment,
    next_seq: u64,
}

fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{id:016}{SEGMENT_SUFFIX}"))
}

fn list_segment_ids(dir: &Path) -> io::Result<Vec<u64>> {
    let mut ids = Vec::new();
    if !dir.exists() {
        return Ok(ids);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name
            .strip_prefix(SEGMENT_PREFIX)
            .and_then(|s| s.strip_suffix(SEGMENT_SUFFIX))
        {
            if let Ok(id) = rest.parse::<u64>() {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

pub struct OpenReport {
    /// Every valid record recovered, across all segments, in commit order.
    pub records: Vec<Record>,
    /// True if the active (last) segment had a torn tail that was
    /// truncated during recovery.
    pub recovered_from_torn_tail: bool,
}

impl Log {
    /// Opens (creating if necessary) the log directory at `dir`, replaying
    /// every segment to rebuild the record stream and truncating a torn
    /// tail on the last segment only.
    pub fn open(dir: impl Into<PathBuf>) -> Result<(Self, OpenReport), LogError> {
        Self::open_with_segment_size(dir, DEFAULT_SEGMENT_MAX_BYTES)
    }

    pub fn open_with_segment_size(
        dir: impl Into<PathBuf>,
        segment_max_bytes: u64,
    ) -> Result<(Self, OpenReport), LogError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let ids = list_segment_ids(&dir)?;

        let mut all_records = Vec::new();
        let mut sealed = Vec::new();
        let mut recovered_from_torn_tail = false;
        let mut max_seq_seen: Option<u64> = None;

        let last_id = ids.last().copied();

        for &id in ids.iter() {
            let path = segment_path(&dir, id);
            let recovered = segment::recover(&path)?;
            let is_last = Some(id) == last_id;

            if recovered.had_torn_tail && !is_last {
                return Err(LogError::SealedSegmentCorrupted(id));
            }
            if recovered.had_torn_tail {
                recovered_from_torn_tail = true;
            }
            for r in &recovered.records {
                max_seq_seen = Some(max_seq_seen.map_or(r.seq, |m: u64| m.max(r.seq)));
            }
            all_records.extend(recovered.records);

            if is_last {
                let active = Segment::open_for_append(path, recovered.valid_len)?;
                let next_seq = max_seq_seen.map(|s| s + 1).unwrap_or(0);
                return Ok((
                    Self {
                        dir,
                        segment_max_bytes,
                        sealed,
                        active_id: id,
                        active,
                        next_seq,
                    },
                    OpenReport {
                        records: all_records,
                        recovered_from_torn_tail,
                    },
                ));
            }
            sealed.push((id, path));
        }

        // No segments existed yet: create the first one.
        let path = segment_path(&dir, 0);
        let active = Segment::open_for_append(path, 0)?;
        Ok((
            Self {
                dir,
                segment_max_bytes,
                sealed,
                active_id: 0,
                active,
                next_seq: 0,
            },
            OpenReport {
                records: all_records,
                recovered_from_torn_tail,
            },
        ))
    }

    fn roll_over_if_needed(&mut self, incoming_len: u64) -> Result<(), LogError> {
        if self.active.is_empty() || self.active.len() + incoming_len <= self.segment_max_bytes {
            return Ok(());
        }
        self.sealed
            .push((self.active_id, self.active.path().to_path_buf()));
        self.active_id += 1;
        let path = segment_path(&self.dir, self.active_id);
        self.active = Segment::open_for_append(path, 0)?;
        Ok(())
    }

    pub fn append_put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<Record, LogError> {
        let record = Record::put(self.next_seq, key, value);
        self.append(record)
    }

    pub fn append_delete(&mut self, key: Vec<u8>) -> Result<Record, LogError> {
        let record = Record::delete(self.next_seq, key);
        self.append(record)
    }

    fn append(&mut self, record: Record) -> Result<Record, LogError> {
        let encoded_len = record.encoded_len() as u64;
        self.roll_over_if_needed(encoded_len)?;
        self.active.append(&record)?;
        self.next_seq = record.seq + 1;
        Ok(record)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn segment_count(&self) -> usize {
        self.sealed.len() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn appends_and_reopens_preserve_all_records() {
        let dir = tempdir().unwrap();
        {
            let (mut log, _report) = Log::open(dir.path()).unwrap();
            log.append_put(b"a".to_vec(), b"1".to_vec()).unwrap();
            log.append_put(b"b".to_vec(), b"2".to_vec()).unwrap();
            log.append_delete(b"a".to_vec()).unwrap();
        }
        let (_log, report) = Log::open(dir.path()).unwrap();
        assert_eq!(report.records.len(), 3);
        assert!(!report.recovered_from_torn_tail);
    }

    #[test]
    fn rolls_over_to_a_new_segment_once_the_size_limit_is_exceeded() {
        let dir = tempdir().unwrap();
        let (mut log, _) = Log::open_with_segment_size(dir.path(), 200).unwrap();
        for i in 0..50u32 {
            log.append_put(format!("key{i}").into_bytes(), vec![0u8; 20])
                .unwrap();
        }
        assert!(log.segment_count() > 1);

        let (_log2, report) = Log::open_with_segment_size(dir.path(), 200).unwrap();
        assert_eq!(report.records.len(), 50);
    }

    #[test]
    fn sequence_numbers_are_monotonic_and_survive_reopen() {
        let dir = tempdir().unwrap();
        {
            let (mut log, _) = Log::open(dir.path()).unwrap();
            for i in 0..10u32 {
                log.append_put(format!("k{i}").into_bytes(), vec![])
                    .unwrap();
            }
            assert_eq!(log.next_seq(), 10);
        }
        let (log2, report) = Log::open(dir.path()).unwrap();
        assert_eq!(log2.next_seq(), 10);
        for (i, r) in report.records.iter().enumerate() {
            assert_eq!(r.seq, i as u64);
        }
    }
}
