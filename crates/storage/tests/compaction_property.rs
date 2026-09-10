//! Property-based test for compaction: for arbitrary put/delete
//! sequences, compacting must never change the store's externally
//! observable live state — only its on-disk footprint.

use std::collections::HashMap;

use proptest::prelude::*;
use storage::Store;
use tempfile::tempdir;

proptest! {
    // Real fsync'd disk I/O per operation (and compaction itself does
    // more I/O on top) — bounded to keep this fast, the same way
    // `indexed_store_property.rs` bounds its own I/O-heavy case count.
    #![proptest_config(ProptestConfig::with_cases(20))]
    #[test]
    fn compaction_never_changes_observable_state(
        ops in prop::collection::vec((0..10u8, 0..10u8, any::<bool>()), 1..80)
    ) {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut model: HashMap<String, Option<String>> = HashMap::new();

        for (k, v, is_delete) in &ops {
            let key = format!("k{k}");
            if *is_delete {
                store.delete(key.clone()).unwrap();
                model.insert(key, None);
            } else {
                let value = format!("v{v}");
                store.put(key.clone(), value.clone()).unwrap();
                model.insert(key, Some(value));
            }
        }

        // Snapshot live state before compaction.
        let before: Vec<(Vec<u8>, Vec<u8>)> = store.scan(b"k");

        store.compact().unwrap();

        let after: Vec<(Vec<u8>, Vec<u8>)> = store.scan(b"k");
        prop_assert_eq!(&before, &after, "compaction changed the live scan result");

        for (key, expected) in &model {
            let actual = store.get(key.as_bytes()).map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(&actual, expected, "compaction changed get() for {}", key);
        }
    }

    /// Ticket 013: compacting while a snapshot is held must not change
    /// what that snapshot's `get_at` returns, for arbitrary put/delete
    /// sequences interleaved with the point the snapshot was taken.
    #[test]
    fn compacting_with_a_held_snapshot_never_changes_that_snapshots_reads(
        before in prop::collection::vec((0..6u8, 0..6u8, any::<bool>()), 1..30),
        after in prop::collection::vec((0..6u8, 0..6u8, any::<bool>()), 0..30),
    ) {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut model_at_snapshot: HashMap<String, Option<String>> = HashMap::new();

        for (k, v, is_delete) in &before {
            let key = format!("k{k}");
            if *is_delete {
                store.delete(key.clone()).unwrap();
                model_at_snapshot.insert(key, None);
            } else {
                let value = format!("v{v}");
                store.put(key.clone(), value.clone()).unwrap();
                model_at_snapshot.insert(key, Some(value));
            }
        }

        let guard = store.hold_snapshot();
        let snapshot = guard.snapshot();
        let expected = model_at_snapshot.clone();

        for (k, v, is_delete) in &after {
            let key = format!("k{k}");
            if *is_delete {
                store.delete(key).unwrap();
            } else {
                store.put(key, format!("v{v}")).unwrap();
            }
        }

        store.compact().unwrap();

        for (key, expected_value) in &expected {
            let actual = store
                .get_at(key.as_bytes(), snapshot)
                .map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(&actual, expected_value, "held snapshot's get_at changed for {} after compaction", key);
        }
    }
}
