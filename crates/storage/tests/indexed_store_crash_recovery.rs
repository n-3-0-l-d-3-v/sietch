//! Crash-consistency for `IndexedStore`, over real byte-level corruption
//! injected into its log segment (not just the "skip the index update"
//! simulation in the unit tests) — the same exhaustive-cut-point technique
//! `crash_recovery.rs` already applies to plain `Store`.

use std::fs::OpenOptions;
use std::io::Write;

use storage::IndexedStore;
use tempfile::tempdir;

#[test]
fn a_torn_log_write_is_recovered_and_the_index_stays_consistent_with_it() {
    let dir = tempdir().unwrap();
    {
        let mut store = IndexedStore::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.put("b", "2").unwrap();
    }
    let seg_path = dir.path().join("log").join("seg-0000000000000000.log");
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3]).unwrap();
    }

    let mut store = IndexedStore::open(dir.path()).unwrap();
    assert!(store.recovered_from_torn_tail);
    assert_eq!(store.get(b"a").unwrap(), Some(b"1".to_vec()));
    assert_eq!(store.get(b"b").unwrap(), Some(b"2".to_vec()));
}

#[test]
fn repeated_crash_injection_at_every_byte_offset_never_desyncs_the_index_from_the_log() {
    let dir = tempdir().unwrap();
    let seg_path = dir.path().join("log").join("seg-0000000000000000.log");

    {
        let mut store = IndexedStore::open(dir.path()).unwrap();
        for i in 0..20u32 {
            store.put(format!("k{i}"), format!("v{i}")).unwrap();
        }
    }

    let full_len = std::fs::metadata(&seg_path).unwrap().len();
    for cut in 1..full_len {
        let full_bytes = std::fs::read(&seg_path).unwrap();
        std::fs::write(&seg_path, &full_bytes[..cut as usize]).unwrap();

        let mut store = IndexedStore::open(dir.path()).unwrap();
        for i in 0..20u32 {
            let key = format!("k{i}");
            if let Some(v) = store.get(key.as_bytes()).unwrap() {
                assert_eq!(
                    v,
                    format!("v{i}").into_bytes(),
                    "corrupted value recovered for {key} at cut {cut}"
                );
            }
        }
        std::fs::write(&seg_path, &full_bytes).unwrap();
    }
}
