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

    /// Same idea, but for arbitrary interleavings of insert and delete
    /// (ticket 009) — the tree's `delete` return value, `get`, and
    /// `scan_all` must all agree with a plain `BTreeMap` used the same
    /// way, for any sequence.
    #[test]
    fn tree_matches_reference_btreemap_for_interleaved_insert_and_delete(
        ops in prop::collection::vec((arb_key(), arb_value(), any::<bool>()), 1..300)
    ) {
        let mut tree = BTree::open(MemPageStore::new(), 8).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

        for (k, v, is_delete) in &ops {
            if *is_delete {
                let expected_found = model.remove(k).is_some();
                let actual_found = tree.delete(k).unwrap();
                prop_assert_eq!(actual_found, expected_found, "delete return value mismatch for key {:?}", k);
            } else {
                tree.insert(k.clone(), v.clone()).unwrap();
                model.insert(k.clone(), v.clone());
            }
        }

        for (k, expected) in &model {
            let actual = tree.get(k).unwrap();
            prop_assert_eq!(actual.as_ref(), Some(expected));
        }
        // Every key not in the model must genuinely be gone from the tree.
        for k in (0u8..20).map(|b| vec![b]) {
            if !model.contains_key(&k) {
                prop_assert_eq!(tree.get(&k).unwrap(), None);
            }
        }

        let scanned = tree.scan_all().unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> = model.into_iter().collect();
        prop_assert_eq!(scanned, expected);
    }

    /// `scan_range` (ticket 010, sibling-pointer-based) must agree with
    /// `scan_all` filtered to the same half-open bounds, for arbitrary
    /// insert sequences and arbitrary bounds — proving the sibling-chain
    /// traversal doesn't diverge from the already-proven full traversal.
    #[test]
    fn scan_range_matches_scan_all_filtered_to_the_same_bounds(
        ops in prop::collection::vec((arb_key(), arb_value()), 1..200),
        start in 0u8..20,
        len in 0u8..20,
    ) {
        let mut tree = BTree::open(MemPageStore::new(), 8).unwrap();
        for (k, v) in &ops {
            tree.insert(k.clone(), v.clone()).unwrap();
        }

        let end = start.saturating_add(len);
        let start_key = vec![start];
        let end_key = vec![end];

        let ranged = tree.scan_range(&start_key, &end_key).unwrap();
        let all = tree.scan_all().unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> =
            all.into_iter().filter(|(k, _)| k.as_slice() >= start_key.as_slice() && k.as_slice() < end_key.as_slice()).collect();
        prop_assert_eq!(ranged, expected);
    }
}
