//! The benchmark ticket 011 exists to justify: does using a persistent
//! B+Tree index actually fix `Store`'s reopen-time scaling problem, or did
//! we just move the cost around? Compares `Store::open` (always replays
//! the full log into a fresh in-memory index) against
//! `IndexedStore::open` after a *clean* shutdown (should need zero
//! reconciliation, since the index was already caught up) across growing
//! history sizes.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use storage::{IndexedStore, Store};
use tempfile::tempdir;

fn bench_plain_store_reopen(c: &mut Criterion) {
    let mut group = c.benchmark_group("reopen_after_clean_shutdown");
    for count in [100usize, 1_000, 10_000, 30_000] {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            for i in 0..count {
                store.put(format!("k{i}"), format!("v{i}")).unwrap();
            }
        }
        group.bench_with_input(BenchmarkId::new("plain_store", count), &dir, |b, dir| {
            b.iter(|| {
                black_box(Store::open(dir.path()).unwrap());
            });
        });
    }
    group.finish();
}

fn bench_indexed_store_reopen(c: &mut Criterion) {
    let mut group = c.benchmark_group("reopen_after_clean_shutdown");
    for count in [100usize, 1_000, 10_000, 30_000] {
        let dir = tempdir().unwrap();
        {
            let mut store = IndexedStore::open(dir.path()).unwrap();
            for i in 0..count {
                store.put(format!("k{i}"), format!("v{i}")).unwrap();
            }
            // A graceful shutdown checkpoints explicitly (checkpointing is
            // otherwise only automatic every `checkpoint_interval` ops).
            store.checkpoint().unwrap();
        }
        group.bench_with_input(BenchmarkId::new("indexed_store", count), &dir, |b, dir| {
            b.iter(|| {
                let store = IndexedStore::open(dir.path()).unwrap();
                assert_eq!(
                    store.reconciled_records, 0,
                    "benchmark invariant: clean shutdown needs no catch-up"
                );
                black_box(store);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_plain_store_reopen,
    bench_indexed_store_reopen
);
criterion_main!(benches);
