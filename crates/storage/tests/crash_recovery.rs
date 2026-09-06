//! Integration-level crash-consistency tests: real file corruption
//! injected between process "runs" of the store, per
//! `docs/DEFINITION_OF_DONE.md`'s requirement to simulate write
//! interruption and corruption, not just unit-test the decoder.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use storage::Store;
use tempfile::tempdir;

#[test]
fn crash_mid_write_loses_only_the_incomplete_record() {
    let dir = tempdir().unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.put("b", "2").unwrap();
    }

    // Simulate the process dying mid-append: garbage bytes land after the
    // last complete record, as they would from a torn write.
    let seg_path = dir.path().join("seg-0000000000000000.log");
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3]).unwrap();
    }

    let store = Store::open(dir.path()).unwrap();
    assert!(store.recovered_from_torn_tail);
    assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
    assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
}

#[test]
fn writes_after_recovery_are_durable_and_readable() {
    let dir = tempdir().unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
    }
    let seg_path = dir.path().join("seg-0000000000000000.log");
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0u8; 12]).unwrap();
    }
    {
        let mut store = Store::open(dir.path()).unwrap();
        assert!(store.recovered_from_torn_tail);
        store.put("b", "2").unwrap();
    }
    // Reopen a third time: the post-recovery write must have landed
    // cleanly (no torn tail this time) and both keys must be present.
    let store = Store::open(dir.path()).unwrap();
    assert!(!store.recovered_from_torn_tail);
    assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
    assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
}

#[test]
fn repeated_crash_injection_never_produces_an_invalid_committed_state() {
    // A run of writes interleaved with simulated crashes at every possible
    // byte offset of the segment file. After each simulated crash and
    // reopen, every key that IS visible must equal the value from some
    // prefix of the write history — the store must never fabricate,
    // corrupt, or partially apply a record.
    let dir = tempdir().unwrap();
    let seg_path = dir.path().join("seg-0000000000000000.log");

    {
        let mut store = Store::open(dir.path()).unwrap();
        for i in 0..20u32 {
            store.put(format!("k{i}"), format!("v{i}")).unwrap();
        }
    }

    let full_len = std::fs::metadata(&seg_path).unwrap().len();

    for cut in 1..full_len {
        let full_bytes = std::fs::read(&seg_path).unwrap();
        std::fs::write(&seg_path, &full_bytes[..cut as usize]).unwrap();

        let store = Store::open(dir.path()).unwrap();
        for i in 0..20u32 {
            let key = format!("k{i}");
            if let Some(v) = store.get(key.as_bytes()) {
                assert_eq!(
                    v,
                    format!("v{i}").into_bytes(),
                    "corrupted value recovered for {key} at cut {cut}"
                );
            }
        }
        // restore full file for the next cut point
        std::fs::write(&seg_path, &full_bytes).unwrap();
    }

    // truncate the underlying file handle position too, so reopening after
    // the loop sees the fully restored file.
    let mut f = OpenOptions::new().write(true).open(&seg_path).unwrap();
    f.seek(SeekFrom::End(0)).unwrap();
}
