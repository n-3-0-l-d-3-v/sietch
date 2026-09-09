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
}
