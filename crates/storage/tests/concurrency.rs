//! Ticket 007's concurrent-client harness: real OS threads hammering one
//! shared `TransactionalStore`, checking the properties Snapshot Isolation
//! promises actually hold under contention rather than only in single-
//! threaded unit tests. See `docs/design/decisions/ADR-011-transactions-snapshot-isolation.md`.

use std::sync::Barrier;
use std::thread;

use storage::TransactionalStore;
use tempfile::tempdir;

/// The classic lost-update test: many threads incrementing the same
/// counter, each retrying its own transaction on conflict. Under Snapshot
/// Isolation with write-write conflict detection, no increment can ever be
/// silently lost — either a transaction commits its increment, or it
/// conflicts and must retry, but it never sees a stale value silently
/// accepted as current.
#[test]
fn concurrent_transactions_never_lose_an_update_to_a_shared_counter() {
    let dir = tempdir().unwrap();
    let store = TransactionalStore::open(dir.path()).unwrap();
    {
        let mut txn = store.begin();
        txn.put("counter", "0");
        txn.commit().unwrap();
    }

    const THREADS: usize = 8;
    const INCREMENTS_PER_THREAD: usize = 25;

    let barrier = std::sync::Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..INCREMENTS_PER_THREAD {
                    loop {
                        let mut txn = store.begin();
                        let current: u64 = String::from_utf8(txn.get(b"counter").unwrap())
                            .unwrap()
                            .parse()
                            .unwrap();
                        txn.put("counter", (current + 1).to_string());
                        if txn.commit().is_ok() {
                            break;
                        }
                        // Conflict: someone else committed first. Retry
                        // against a fresh snapshot, exactly as a real
                        // client under Snapshot Isolation must.
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let final_value: u64 = String::from_utf8(store.get(b"counter").unwrap())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(final_value, (THREADS * INCREMENTS_PER_THREAD) as u64);
}

/// A transaction's reads must be pinned to the moment it began, even while
/// other threads are actively committing conflicting and non-conflicting
/// writes concurrently.
#[test]
fn a_readers_snapshot_is_unaffected_by_concurrent_writer_threads() {
    let dir = tempdir().unwrap();
    let store = TransactionalStore::open(dir.path()).unwrap();
    {
        let mut txn = store.begin();
        for i in 0..50 {
            txn.put(format!("k{i}"), "before");
        }
        txn.commit().unwrap();
    }

    let reader = store.begin();
    for i in 0..50 {
        assert_eq!(
            reader.get(format!("k{i}").as_bytes()),
            Some(b"before".to_vec())
        );
    }

    const THREADS: usize = 4;
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let store = store.clone();
            thread::spawn(move || {
                for i in 0..50 {
                    let mut txn = store.begin();
                    txn.put(format!("k{i}"), format!("after-{t}"));
                    // Contention on the same 50 keys means most of these
                    // will conflict with each other; that's fine and
                    // expected — this test only cares that the reader
                    // never observes any of the writes that *do* land.
                    let _ = txn.commit();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // Sanity: the writers actually changed something (otherwise this test
    // would trivially pass without exercising anything).
    let changed =
        (0..50).any(|i| store.get(format!("k{i}").as_bytes()) != Some(b"before".to_vec()));
    assert!(
        changed,
        "test setup bug: no writer thread's commit ever landed"
    );

    for i in 0..50 {
        assert_eq!(
            reader.get(format!("k{i}").as_bytes()),
            Some(b"before".to_vec()),
            "a snapshot taken before the writer threads ran must never see their writes"
        );
    }
}

/// Disjoint-key transactions from different threads must never spuriously
/// conflict with each other — only genuine same-key write-write conflicts
/// should ever cause a commit to fail.
#[test]
fn concurrent_transactions_on_disjoint_keys_all_succeed() {
    let dir = tempdir().unwrap();
    let store = TransactionalStore::open(dir.path()).unwrap();

    const THREADS: usize = 8;
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let store = store.clone();
            thread::spawn(move || {
                let mut txn = store.begin();
                txn.put(format!("owned-by-{t}"), "value");
                txn.commit().unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    for t in 0..THREADS {
        assert_eq!(
            store.get(format!("owned-by-{t}").as_bytes()),
            Some(b"value".to_vec())
        );
    }
}
