//! Property-based differential testing for the B+Tree: for any sequence
//! of inserts (random keys, random order, deliberately including
//! duplicates to exercise upsert), the tree's final state must match a
//! plain `BTreeMap` used as the reference model — the same differential-
//! testing approach used for the KV `Store` and the Machine's scheduler.

use std::collections::BTreeMap;

use proptest::prelude::*;
use storage::page_store::MemPageStore;
use storage::BTree;

fn arb_key() -> impl Strategy<Value = Vec<u8>> {
    // Small key domain (single byte 0..20) so duplicate keys / upserts are
    // actually exercised often, not just theoretically possible.
    (0u8..20).prop_map(|b| vec![b])
}

fn arb_value() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..16)
}

proptest! {
    #[test]
    fn tree_matches_reference_btreemap_for_arbitrary_insert_sequences(
        ops in prop::collection::vec((arb_key(), arb_value()), 1..200)
    ) {
        let mut tree = BTree::open(MemPageStore::new(), 8).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

        for (k, v) in &ops {
            tree.insert(k.clone(), v.clone()).unwrap();
            model.insert(k.clone(), v.clone());
        }

        for (k, expected) in &model {
            let actual = tree.get(k).unwrap();
            prop_assert_eq!(actual.as_ref(), Some(expected));
        }

        let scanned = tree.scan_all().unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> = model.into_iter().collect();
        prop_assert_eq!(scanned, expected);
    }
}
