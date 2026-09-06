//! Property-based tests: random key/value byte strings and random
//! put/delete sequences must always round-trip through the log and store
//! exactly as written, and truncating a valid record stream at any byte
//! boundary must never decode past the truncation point.

use proptest::prelude::*;
use storage::record::{decode, DecodeOutcome, Record};
use storage::Store;
use tempfile::tempdir;

fn arb_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..64)
}

proptest! {
    /// Any record, for any key/value bytes (including empty), round-trips
    /// through encode/decode exactly.
    #[test]
    fn record_roundtrips_for_arbitrary_bytes(key in arb_bytes(), value in arb_bytes(), seq in any::<u64>()) {
        let r = Record::put(seq, key, value);
        let bytes = r.encode();
        match decode(&bytes) {
            DecodeOutcome::Ok(decoded, len) => {
                prop_assert_eq!(decoded, r);
                prop_assert_eq!(len, bytes.len());
            }
            other => prop_assert!(false, "expected Ok, got {:?}", other),
        }
    }

    /// Truncating an encoded record anywhere before its full length must
    /// never decode as `Ok` — a partial record must never be mistaken for
    /// a complete, valid one.
    #[test]
    fn truncated_record_never_decodes_as_ok(key in arb_bytes(), value in arb_bytes(), cut_ratio in 0.0f64..1.0) {
        let r = Record::put(1, key, value);
        let bytes = r.encode();
        if bytes.len() > 1 {
            let cut = ((bytes.len() - 1) as f64 * cut_ratio) as usize;
            if let DecodeOutcome::Ok(..) = decode(&bytes[..cut]) {
                prop_assert!(false, "truncated buffer decoded as a complete record");
            }
        }
    }

    /// A sequence of put/delete operations replayed through a fresh Store
    /// after a reopen must match an in-memory reference model exactly.
    #[test]
    fn store_matches_a_naive_in_memory_model(
        ops in prop::collection::vec((0..5u8, 0..5u8, any::<bool>()), 1..40)
    ) {
        use std::collections::HashMap;
        let dir = tempdir().unwrap();
        let mut model: HashMap<String, Option<String>> = HashMap::new();

        {
            let mut store = Store::open(dir.path()).unwrap();
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
        }

        let store = Store::open(dir.path()).unwrap();
        for (key, expected) in &model {
            let actual = store.get(key.as_bytes()).map(|v| String::from_utf8(v).unwrap());
            prop_assert_eq!(&actual, expected, "mismatch for key {}", key);
        }
    }
}
