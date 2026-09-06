//! The Vault's primitive interface: PUT / GET / DELETE / SCAN / SNAPSHOT,
//! per `docs/design/CONSTRAINTS.md` ("start with these before any
//! relational layer exists"). Built entirely on `Log` — every version of
//! every key is an immutable record; nothing is ever mutated in place.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::log::{Log, LogError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Log(#[from] LogError),
}

#[derive(Debug, Clone)]
struct VersionEntry {
    seq: u64,
    /// `None` is a tombstone (the key was deleted as of this version).
    value: Option<Vec<u8>>,
}

/// A point-in-time read handle: a snapshot sees every version committed
/// strictly before it was taken, and nothing committed afterward — this is
/// what "SNAPSHOT" means for this store (multi-version, not a copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Snapshot {
    as_of_seq: u64,
}

pub struct Store {
    log: Log,
    index: BTreeMap<Vec<u8>, Vec<VersionEntry>>,
    /// True if the most recent `open` had to discard a torn tail — exposed
    /// so callers/tests can assert recovery behavior rather than just
    /// trusting it silently happened.
    pub recovered_from_torn_tail: bool,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let (log, report) = Log::open(dir)?;
        let mut index: BTreeMap<Vec<u8>, Vec<VersionEntry>> = BTreeMap::new();
        for record in report.records {
            let value = match record.record_type {
                crate::record::RecordType::Put => Some(record.value),
                crate::record::RecordType::Delete => None,
            };
            index.entry(record.key).or_default().push(VersionEntry {
                seq: record.seq,
                value,
            });
        }
        Ok(Self {
            log,
            index,
            recovered_from_torn_tail: report.recovered_from_torn_tail,
        })
    }

    pub fn put(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), StoreError> {
        let key = key.into();
        let record = self.log.append_put(key.clone(), value.into())?;
        self.index.entry(key).or_default().push(VersionEntry {
            seq: record.seq,
            value: Some(record.value),
        });
        Ok(())
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Result<(), StoreError> {
        let key = key.into();
        let record = self.log.append_delete(key.clone())?;
        self.index.entry(key).or_default().push(VersionEntry {
            seq: record.seq,
            value: None,
        });
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.index
            .get(key)
            .and_then(|versions| versions.last())
            .and_then(|v| v.value.clone())
    }

    /// Reads `key` as it existed at `snapshot` — the latest version
    /// committed strictly before the snapshot was taken.
    pub fn get_at(&self, key: &[u8], snapshot: Snapshot) -> Option<Vec<u8>> {
        let versions = self.index.get(key)?;
        versions
            .iter()
            .rev()
            .find(|v| v.seq < snapshot.as_of_seq)
            .and_then(|v| v.value.clone())
    }

    /// Every live (non-tombstone) key currently starting with `prefix`,
    /// with its current value, in key order.
    pub fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, versions)| {
                versions
                    .last()
                    .and_then(|v| v.value.clone())
                    .map(|val| (k.clone(), val))
            })
            .collect()
    }

    /// Same as `scan`, but as of a snapshot rather than the live state.
    pub fn scan_at(&self, prefix: &[u8], snapshot: Snapshot) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, versions)| {
                versions
                    .iter()
                    .rev()
                    .find(|v| v.seq < snapshot.as_of_seq)
                    .and_then(|v| v.value.clone())
                    .map(|val| (k.clone(), val))
            })
            .collect()
    }

    /// Captures the current point in the log's history. Every write that
    /// happens after this call is invisible to reads made through the
    /// returned `Snapshot`.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            as_of_seq: self.log.next_seq(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_then_get_round_trips() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
    }

    #[test]
    fn delete_makes_get_return_none() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get(b"a"), None);
    }

    #[test]
    fn snapshot_isolates_reads_from_later_writes() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        let snap = store.snapshot();
        store.put("a", "2").unwrap();
        store.delete("a").unwrap();

        assert_eq!(store.get_at(b"a", snap), Some(b"1".to_vec()));
        assert_eq!(store.get(b"a"), None);
    }

    #[test]
    fn scan_returns_live_keys_with_prefix_in_order() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("user:1", "alice").unwrap();
        store.put("user:2", "bob").unwrap();
        store.put("order:1", "widget").unwrap();
        store.delete("user:2").unwrap();

        let results = store.scan(b"user:");
        assert_eq!(results, vec![(b"user:1".to_vec(), b"alice".to_vec())]);
    }

    #[test]
    fn reopening_after_writes_recovers_full_state() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.put("b", "2").unwrap();
            store.delete("a").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), None);
        assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
        assert!(!store.recovered_from_torn_tail);
    }
}
