//! Property-based differential test: `IndexedStore` must behave exactly
//! like plain `Store` (tickets 001–003, the already-proven implementation)
//! for the same sequence of put/delete operations — the persistent B+Tree
//! index is an optimization, not a behavior change, and this is what
//! proves it.

use std::collections::HashMap;

use proptest::prelude::*;
use storage::{IndexedStore, Store};
use tempfile::tempdir;

proptest! {
    // Each case does real fsync'd disk I/O (twice per op, for the log and
    // the index) for both stores under test, so this is deliberately
    // bounded to far fewer cases than the in-memory property tests
    // elsewhere in this crate — it's still exercising real randomized
    // sequences, just not thousands of them.
    #![proptest_config(ProptestConfig::with_cases(20))]
    #[test]
    fn indexed_store_matches_plain_store_for_the_same_operations(
        ops in prop::collection::vec((0..8u8, 0..8u8, any::<bool>()), 1..60)
    ) {
        let plain_dir = tempdir().unwrap();
        let indexed_dir = tempdir().unwrap();
        let mut plain = Store::open(plain_dir.path()).unwrap();
        let mut indexed = IndexedStore::open(indexed_dir.path()).unwrap();
        let mut model: HashMap<String, Option<String>> = HashMap::new();

        for (k, v, is_delete) in &ops {
            let key = format!("k{k}");
            if *is_delete {
                plain.delete(key.clone()).unwrap();
                indexed.delete(key.clone()).unwrap();
                model.insert(key, None);
            } else {
                let value = format!("v{v}");
                plain.put(key.clone(), value.clone()).unwrap();
                indexed.put(key.clone(), value.clone()).unwrap();
                model.insert(key, Some(value));
            }
        }

        for (key, expected) in &model {
            let plain_val = plain.get(key.as_bytes()).map(|v| String::from_utf8(v).unwrap());
            let indexed_val = indexed.get(key.as_bytes()).unwrap().map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(&plain_val, expected, "plain Store diverged from model for {}", key);
            prop_assert_eq!(&indexed_val, expected, "IndexedStore diverged from model for {}", key);
        }

        let mut plain_scan = plain.scan(b"k");
        plain_scan.sort();
        let mut indexed_scan = indexed.scan(b"k").unwrap();
        indexed_scan.sort();
        prop_assert_eq!(plain_scan, indexed_scan, "scan results diverged between Store and IndexedStore");
    }
}
