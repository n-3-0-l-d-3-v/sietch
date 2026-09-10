//! Property tests for `txn::Transaction`: sequential transaction commits
//! must match a naive in-memory reference model exactly, and a
//! transaction's snapshot must never observe writes committed after it
//! began, for arbitrary interleavings of "when the snapshot was taken."

use std::collections::HashMap;

use proptest::prelude::*;
use storage::TransactionalStore;
use tempfile::tempdir;

proptest! {
    /// A sequence of single-key transactions, each committed before the
    /// next begins (no real concurrency here — that's covered by
    /// tests/concurrency.rs's thread-based tests), must produce exactly
    /// the state a naive sequential model would.
    #[test]
    fn sequential_transaction_commits_match_a_naive_model(
        ops in prop::collection::vec((0..5u8, 0..5u8, any::<bool>()), 1..40)
    ) {
        let dir = tempdir().unwrap();
        let store = TransactionalStore::open(dir.path()).unwrap();
        let mut model: HashMap<String, Option<String>> = HashMap::new();

        for (k, v, is_delete) in &ops {
            let key = format!("k{k}");
            let mut txn = store.begin();
            if *is_delete {
                txn.delete(key.clone());
                model.insert(key, None);
            } else {
                let value = format!("v{v}");
                txn.put(key.clone(), value.clone());
                model.insert(key, Some(value));
            }
            txn.commit().unwrap();
        }

        for (key, expected) in &model {
            let actual = store.get(key.as_bytes()).map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(&actual, expected, "mismatch for key {}", key);
        }
    }

    /// A snapshot taken partway through a sequence of committed
    /// transactions must reflect exactly the state at that point — every
    /// key as the model had it right after the snapshot-point transaction,
    /// regardless of what any later transaction in the sequence commits.
    #[test]
    fn a_snapshot_never_sees_transactions_committed_after_it_began(
        before in prop::collection::vec((0..5u8, 0..5u8), 1..15),
        after in prop::collection::vec((0..5u8, 0..5u8), 1..15),
    ) {
        let dir = tempdir().unwrap();
        let store = TransactionalStore::open(dir.path()).unwrap();
        let mut model: HashMap<String, String> = HashMap::new();

        for (k, v) in &before {
            let key = format!("k{k}");
            let value = format!("v{v}");
            let mut txn = store.begin();
            txn.put(key.clone(), value.clone());
            txn.commit().unwrap();
            model.insert(key, value);
        }

        // Take the snapshot here — its reads must be pinned to `model` as
        // it stands right now, for the rest of this test.
        let reader = store.begin();
        let expected_at_snapshot = model.clone();

        for (k, v) in &after {
            let key = format!("k{k}");
            let value = format!("v{v}");
            let mut txn = store.begin();
            txn.put(key.clone(), value.clone());
            txn.commit().unwrap();
            model.insert(key, value);
        }

        for (key, expected) in &expected_at_snapshot {
            let actual = reader.get(key.as_bytes()).map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(Some(expected.clone()), actual, "snapshot saw a write committed after it began, for key {}", key);
        }
    }
}
