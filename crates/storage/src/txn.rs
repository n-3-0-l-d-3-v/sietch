//! Multi-operation transactions with Snapshot Isolation on top of `Store`
//! (ticket 007), for the concurrent-client case `docs/design/CONSTRAINTS.md`
//! calls for: multiple threads reading and writing the same store at once
//! while the store keeps its consistency guarantees, including deliberately
//! forcing conflicts and retries rather than assuming they won't happen.
//!
//! `Store` itself stays single-writer and un-synchronized — that contract
//! doesn't change. `TransactionalStore` wraps it in a `Mutex` and is the
//! thing multiple threads actually share (`Clone` + `Arc` internally, so
//! every clone refers to the same underlying store).
//!
//! See `docs/design/decisions/ADR-011-transactions-snapshot-isolation.md`
//! for why this reuses `Store::snapshot`/`get_at`/`apply_batch` instead of
//! building a separate transaction log, and why conflict detection is
//! first-committer-wins rather than a lock-based scheme.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::log::LogOp;
use crate::store::{Snapshot, SnapshotGuard, Store, StoreError, WriteOp};

#[derive(Debug, thiserror::Error)]
pub enum TxnError {
    #[error("write-write conflict on key {0:?}: committed by another transaction after this one's snapshot was taken")]
    Conflict(Vec<u8>),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A handle to a `Store` shared by multiple threads. Cheap to `Clone` —
/// every clone locks the same underlying store.
#[derive(Clone)]
pub struct TransactionalStore {
    inner: Arc<Mutex<Store>>,
}

impl TransactionalStore {
    pub fn open(dir: impl Into<std::path::PathBuf>) -> Result<Self, StoreError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(Store::open(dir)?)),
        })
    }

    /// Starts a new transaction reading from a snapshot taken right now.
    /// The transaction sees exactly the state as of this call, and nothing
    /// committed by any other transaction afterward — that's what Snapshot
    /// Isolation means here. The snapshot is held (ticket 013's
    /// `SnapshotGuard`) for the transaction's whole lifetime, so a
    /// concurrent `Store::compact()` can never invalidate an in-flight
    /// transaction's reads.
    pub fn begin(&self) -> Transaction {
        let guard = self.inner.lock().unwrap().hold_snapshot();
        let snapshot = guard.snapshot();
        Transaction {
            store: self.inner.clone(),
            snapshot,
            _snapshot_guard: guard,
            writes: BTreeMap::new(),
        }
    }

    /// A single-operation convenience matching plain `Store::get`, for
    /// callers that don't need a transaction at all.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().get(key)
    }

    /// Runs `Store::compact()` against the underlying store. Any
    /// transaction currently open on this `TransactionalStore` is holding
    /// its own snapshot (see `begin()`), so compaction never invalidates
    /// an in-flight transaction's reads.
    pub fn compact(&self) -> Result<crate::store::CompactionReport, StoreError> {
        self.inner.lock().unwrap().compact()
    }
}

/// A buffered set of reads/writes against a snapshot of the store. Nothing
/// a transaction does is visible to anyone else — including its own
/// `Store`'s other readers — until `commit()` succeeds.
pub struct Transaction {
    store: Arc<Mutex<Store>>,
    snapshot: Snapshot,
    /// Keeps `Store::compact()` from discarding versions this
    /// transaction's snapshot might still need — held for as long as the
    /// transaction lives, released automatically on commit or abort.
    _snapshot_guard: SnapshotGuard,
    /// `None` is a buffered delete; `Some` a buffered put. Buffered
    /// writes are applied in key order at commit time via a single
    /// `apply_batch` call (ticket 008), so a multi-key transaction is one
    /// `fsync`, not one per key.
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl Transaction {
    /// Reads `key` as this transaction would see it: its own uncommitted
    /// write if it has one, otherwise the value as of this transaction's
    /// snapshot — never a write some other transaction committed after
    /// this one began.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        if let Some(buffered) = self.writes.get(key) {
            return buffered.clone();
        }
        self.store.lock().unwrap().get_at(key, self.snapshot)
    }

    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.writes.insert(key.into(), Some(value.into()));
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) {
        self.writes.insert(key.into(), None);
    }

    /// Commits the transaction: first-committer-wins conflict detection,
    /// then (only if every key is conflict-free) one `apply_batch` call
    /// applying every buffered write atomically and durably. A conflict
    /// aborts the *whole* transaction — nothing it wrote is applied,
    /// matching Snapshot Isolation's all-or-nothing commit.
    ///
    /// A conflict is: some other transaction committed a newer version of
    /// a key this transaction also wrote, after this transaction's
    /// snapshot was taken. This transaction's own reads are never the
    /// source of a conflict — only its writes are checked, which is what
    /// makes this "write-write" conflict detection rather than a stricter
    /// (and much more abort-prone) serializable check.
    pub fn commit(self) -> Result<(), TxnError> {
        if self.writes.is_empty() {
            return Ok(());
        }
        let mut store = self.store.lock().unwrap();
        for key in self.writes.keys() {
            if let Some(latest_seq) = store.latest_seq(key) {
                if latest_seq >= self.snapshot.as_of_seq() {
                    return Err(TxnError::Conflict(key.clone()));
                }
            }
        }
        let ops: Vec<WriteOp> = self
            .writes
            .into_iter()
            .map(|(key, value)| match value {
                Some(v) => LogOp::Put(key, v),
                None => LogOp::Delete(key),
            })
            .collect();
        store.apply_batch(ops)?;
        Ok(())
    }

    /// Discards every buffered write. Since nothing is applied to the
    /// store until `commit()`, this is just dropping the transaction —
    /// provided as an explicit, readable alternative to letting it go out
    /// of scope.
    pub fn abort(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_transactions_writes_are_invisible_to_others_until_commit() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();

        let mut txn = ts.begin();
        txn.put("a", "1");
        assert_eq!(
            ts.get(b"a"),
            None,
            "uncommitted write must not be visible outside the transaction"
        );
        assert_eq!(
            txn.get(b"a"),
            Some(b"1".to_vec()),
            "a transaction sees its own uncommitted write"
        );

        txn.commit().unwrap();
        assert_eq!(ts.get(b"a"), Some(b"1".to_vec()));
    }

    #[test]
    fn a_transactions_reads_are_pinned_to_its_snapshot() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        {
            let mut txn = ts.begin();
            txn.put("a", "1");
            txn.commit().unwrap();
        }

        let reader = ts.begin();
        assert_eq!(reader.get(b"a"), Some(b"1".to_vec()));

        let mut writer = ts.begin();
        writer.put("a", "2");
        writer.commit().unwrap();

        // The reader's snapshot was taken before the second commit.
        assert_eq!(reader.get(b"a"), Some(b"1".to_vec()));
        assert_eq!(ts.get(b"a"), Some(b"2".to_vec()));
    }

    #[test]
    fn concurrent_writers_to_the_same_key_the_second_committer_conflicts() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        {
            let mut txn = ts.begin();
            txn.put("a", "0");
            txn.commit().unwrap();
        }

        let mut first = ts.begin();
        let mut second = ts.begin();
        first.put("a", "1");
        second.put("a", "2");

        first.commit().unwrap();
        let result = second.commit();
        assert!(matches!(result, Err(TxnError::Conflict(key)) if key == b"a"));

        // The losing transaction's write must not have applied at all.
        assert_eq!(ts.get(b"a"), Some(b"1".to_vec()));
    }

    #[test]
    fn a_conflict_on_one_key_aborts_the_whole_transaction_not_just_that_key() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        {
            let mut txn = ts.begin();
            txn.put("a", "0");
            txn.commit().unwrap();
        }

        let mut first = ts.begin();
        let mut second = ts.begin();
        first.put("a", "1");
        first.commit().unwrap();

        second.put("a", "conflicting");
        second.put("b", "should not be applied either");
        assert!(second.commit().is_err());

        assert_eq!(
            ts.get(b"b"),
            None,
            "an aborted transaction must not apply any of its writes"
        );
    }

    #[test]
    fn disjoint_keys_never_conflict_even_with_overlapping_snapshots() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();

        let mut first = ts.begin();
        let mut second = ts.begin();
        first.put("a", "1");
        second.put("b", "2");

        first.commit().unwrap();
        second.commit().unwrap();

        assert_eq!(ts.get(b"a"), Some(b"1".to_vec()));
        assert_eq!(ts.get(b"b"), Some(b"2".to_vec()));
    }

    #[test]
    fn compacting_while_a_transaction_is_open_does_not_break_its_reads() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        {
            let mut txn = ts.begin();
            txn.put("a", "1");
            txn.commit().unwrap();
        }

        // This transaction's snapshot must survive the compaction below.
        let reader = ts.begin();
        assert_eq!(reader.get(b"a"), Some(b"1".to_vec()));

        {
            let mut writer = ts.begin();
            writer.put("a", "2");
            writer.commit().unwrap();
        }

        ts.compact().unwrap();

        assert_eq!(
            reader.get(b"a"),
            Some(b"1".to_vec()),
            "compaction while a transaction is open must not change what it reads"
        );
        assert_eq!(ts.get(b"a"), Some(b"2".to_vec()));
    }

    #[test]
    fn aborting_a_transaction_applies_nothing() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        let mut txn = ts.begin();
        txn.put("a", "1");
        txn.abort();
        assert_eq!(ts.get(b"a"), None);
    }

    #[test]
    fn committing_a_transaction_with_no_writes_is_a_harmless_no_op() {
        let dir = tempdir().unwrap();
        let ts = TransactionalStore::open(dir.path()).unwrap();
        let txn = ts.begin();
        assert!(txn.commit().is_ok());
    }
}
