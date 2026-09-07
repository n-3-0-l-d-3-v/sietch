//! `IndexedStore`: a KV store whose *current* state lives in a persistent,
//! on-disk `BTree` (ticket 005) instead of being rebuilt into an
//! in-memory index by replaying the whole log on every open (what plain
//! `Store`, tickets 001–003, still does).
//!
//! Layout: `<dir>/log` holds the durability log (the source of truth for
//! every write, unchanged from `Store`); `<dir>/index` holds the B+Tree's
//! own pages (via `LogPageStore`, itself just another `Store` instance —
//! composition, not a new persistence mechanism). This is a new on-disk
//! layout, not backward compatible with a plain `Store`'s directory.
//!
//! **Read this before assuming this is faster than `Store`**: it isn't,
//! yet — see
//! `docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md`
//! for a measured 12x reopen-time regression and its root cause.

use std::path::PathBuf;

use crate::btree::{BTree, BTreeError};
use crate::log::{Log, LogError};
use crate::page_store::{LogPageStore, LogPageStoreError};
use crate::record::{Record, RecordType};

const DEFAULT_POOL_CAPACITY: usize = 256;

/// A live key's current value, or the fact that it's a tombstone/missing
/// resolved away — `scan`'s external return shape.
type ScanResult = Result<Vec<(Vec<u8>, Vec<u8>)>, IndexedStoreError>;

/// One byte of namespace prefix on every index key, so the reserved
/// metadata key can never collide with a real (arbitrary-byte) user key.
const NS_USER: u8 = 1;
const NS_META: u8 = 0;
const LAST_INDEXED_SEQ_KEY: [u8; 1] = [0];

#[derive(Debug, thiserror::Error)]
pub enum IndexedStoreError {
    #[error(transparent)]
    Log(#[from] LogError),
    #[error(transparent)]
    Index(#[from] BTreeError<LogPageStoreError>),
    #[error(transparent)]
    PageStore(#[from] LogPageStoreError),
}

pub struct IndexedStore {
    log: Log,
    index: BTree<LogPageStore>,
    /// True if, on open, the index was behind the log (an unclean
    /// shutdown between a log write committing and the corresponding
    /// index write committing) and had to be caught up by replaying the
    /// log's tail. Exposed so tests/callers can assert this happened
    /// rather than trusting it silently.
    pub reconciled_records: usize,
    pub recovered_from_torn_tail: bool,
}

fn user_key(key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + key.len());
    out.push(NS_USER);
    out.extend_from_slice(key);
    out
}

fn meta_key(suffix: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + suffix.len());
    out.push(NS_META);
    out.extend_from_slice(suffix);
    out
}

/// Encodes the index's value for a key: a tombstone has no payload, a
/// live value is tagged and carries its bytes. Also carries the seq it
/// was written at, so a partially-applied reconciliation can be detected
/// (defensive; not currently relied on beyond the sentinel key).
fn encode_index_value(value: Option<&[u8]>) -> Vec<u8> {
    match value {
        None => vec![0u8],
        Some(v) => {
            let mut out = Vec::with_capacity(1 + v.len());
            out.push(1u8);
            out.extend_from_slice(v);
            out
        }
    }
}

fn decode_index_value(bytes: &[u8]) -> Option<Vec<u8>> {
    match bytes.first() {
        Some(0) => None,
        Some(1) => Some(bytes[1..].to_vec()),
        _ => unreachable!("corrupt index value tag"),
    }
}

impl IndexedStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, IndexedStoreError> {
        Self::open_with_pool_capacity(dir, DEFAULT_POOL_CAPACITY)
    }

    pub fn open_with_pool_capacity(
        dir: impl Into<PathBuf>,
        pool_capacity: usize,
    ) -> Result<Self, IndexedStoreError> {
        let dir = dir.into();
        let (log, log_report) = Log::open(dir.join("log"))?;
        let page_store = LogPageStore::open(dir.join("index"))?;
        let mut index = BTree::open(page_store, pool_capacity)?;

        let last_indexed_seq = read_last_indexed_seq(&mut index)?;
        let mut reconciled_records = 0;
        for record in &log_report.records {
            if record.seq >= last_indexed_seq {
                apply_record_to_index(&mut index, record)?;
                reconciled_records += 1;
            }
        }
        if reconciled_records > 0 {
            index.flush()?;
        }

        Ok(Self {
            log,
            index,
            reconciled_records,
            recovered_from_torn_tail: log_report.recovered_from_torn_tail,
        })
    }

    pub fn put(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), IndexedStoreError> {
        let key = key.into();
        let record = self.log.append_put(key.clone(), value.into())?;
        self.apply_and_advance(&record)?;
        Ok(())
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Result<(), IndexedStoreError> {
        let key = key.into();
        let record = self.log.append_delete(key.clone())?;
        self.apply_and_advance(&record)?;
        Ok(())
    }

    fn apply_and_advance(&mut self, record: &Record) -> Result<(), IndexedStoreError> {
        apply_record_to_index(&mut self.index, record)?;
        self.index.insert(
            meta_key(&LAST_INDEXED_SEQ_KEY).to_vec(),
            (record.seq + 1).to_le_bytes().to_vec(),
        )?;
        self.index.flush()?;
        Ok(())
    }

    /// A single B+Tree lookup — no log involvement, no full-history
    /// replay. This is the payoff this ticket exists for.
    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, IndexedStoreError> {
        let raw = self.index.get(&user_key(key))?;
        Ok(raw.and_then(|bytes| decode_index_value(&bytes)))
    }

    /// Every live key with the given prefix, current value, in key order.
    /// Implemented as a full traversal + filter (ticket 010's leaf
    /// sibling links would make this bounded instead of O(n) — see
    /// `docs/design/STORAGE.md`).
    pub fn scan(&mut self, prefix: &[u8]) -> ScanResult {
        let all = self.index.scan_all()?;
        let mut out = Vec::new();
        for (k, v) in all {
            if k.first() != Some(&NS_USER) {
                continue; // skip internal metadata entries
            }
            let real_key = &k[1..];
            if !real_key.starts_with(prefix) {
                continue;
            }
            if let Some(value) = decode_index_value(&v) {
                out.push((real_key.to_vec(), value));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub fn flush(&mut self) -> Result<(), IndexedStoreError> {
        self.index.flush()?;
        Ok(())
    }
}

fn read_last_indexed_seq(index: &mut BTree<LogPageStore>) -> Result<u64, IndexedStoreError> {
    match index.get(&meta_key(&LAST_INDEXED_SEQ_KEY))? {
        None => Ok(0),
        Some(bytes) => Ok(u64::from_le_bytes(bytes.try_into().unwrap())),
    }
}

fn apply_record_to_index(
    index: &mut BTree<LogPageStore>,
    record: &Record,
) -> Result<(), IndexedStoreError> {
    let value = match record.record_type {
        RecordType::Put => Some(record.value.as_slice()),
        RecordType::Delete => None,
    };
    index.insert(user_key(&record.key), encode_index_value(value))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_then_get_round_trips() {
        let dir = tempdir().unwrap();
        let mut store = IndexedStore::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        assert_eq!(store.get(b"a").unwrap(), Some(b"1".to_vec()));
    }

    #[test]
    fn delete_makes_get_return_none() {
        let dir = tempdir().unwrap();
        let mut store = IndexedStore::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get(b"a").unwrap(), None);
    }

    #[test]
    fn scan_returns_live_keys_with_prefix_in_order() {
        let dir = tempdir().unwrap();
        let mut store = IndexedStore::open(dir.path()).unwrap();
        store.put("user:2", "bob").unwrap();
        store.put("user:1", "alice").unwrap();
        store.put("order:1", "widget").unwrap();
        store.delete("user:2").unwrap();

        let results = store.scan(b"user:").unwrap();
        assert_eq!(results, vec![(b"user:1".to_vec(), b"alice".to_vec())]);
    }

    #[test]
    fn reopening_after_a_clean_shutdown_needs_no_reconciliation() {
        let dir = tempdir().unwrap();
        {
            let mut store = IndexedStore::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.put("b", "2").unwrap();
        }
        let store = IndexedStore::open(dir.path()).unwrap();
        assert_eq!(
            store.reconciled_records, 0,
            "a clean shutdown should need no catch-up replay"
        );
    }

    #[test]
    fn reopening_reconciles_state_that_committed_to_the_log_but_not_the_index() {
        // Simulates a crash between the log write committing and the
        // index write committing: append directly to the log, bypassing
        // `put`'s index update entirely.
        let dir = tempdir().unwrap();
        {
            let mut store = IndexedStore::open(dir.path()).unwrap();
            store.put("a", "1").unwrap(); // fully applied: log + index
            let record = store.log.append_put(b"b".to_vec(), b"2".to_vec()).unwrap();
            let _ = record; // log committed "b" -> "2"; index was never told
        }

        let mut store = IndexedStore::open(dir.path()).unwrap();
        assert_eq!(
            store.reconciled_records, 1,
            "exactly the one unindexed record should be replayed"
        );
        assert_eq!(store.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(
            store.get(b"b").unwrap(),
            Some(b"2".to_vec()),
            "record committed to the log but not the index must be recovered"
        );
    }

    #[test]
    fn reconciliation_replays_only_the_unindexed_tail_not_the_whole_log() {
        let dir = tempdir().unwrap();
        {
            let mut store = IndexedStore::open(dir.path()).unwrap();
            for i in 0..50u32 {
                store.put(format!("k{i}"), format!("v{i}")).unwrap();
            }
            // Two more records committed to the log only.
            store
                .log
                .append_put(b"unindexed1".to_vec(), b"x".to_vec())
                .unwrap();
            store
                .log
                .append_put(b"unindexed2".to_vec(), b"y".to_vec())
                .unwrap();
        }
        let store = IndexedStore::open(dir.path()).unwrap();
        assert_eq!(
            store.reconciled_records, 2,
            "only the two records the index never saw should be replayed, not all 52"
        );
    }
}
