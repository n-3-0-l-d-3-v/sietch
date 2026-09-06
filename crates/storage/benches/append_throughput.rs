//! Benchmarks append throughput and point-read latency against a
//! conventional expectation: append-only writes should be cheap (sequential
//! I/O + fsync per record); reads pay for the in-memory index rebuild cost
//! at open time, which this also measures as a function of history size.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use storage::Store;
use tempfile::tempdir;

fn bench_put_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_put");
    for count in [100usize, 1_000] {
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_batched(
                || {
                    let dir = tempdir().unwrap();
                    let store = Store::open(dir.path()).unwrap();
                    (dir, store)
                },
                |(dir, mut store)| {
                    for i in 0..count {
                        store.put(format!("k{i}"), format!("v{i}")).unwrap();
                    }
                    black_box(&dir);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_reopen_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_reopen_recovery");
    for count in [100usize, 1_000, 10_000] {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            for i in 0..count {
                store.put(format!("k{i}"), format!("v{i}")).unwrap();
            }
        }
        group.bench_with_input(BenchmarkId::from_parameter(count), &dir, |b, dir| {
            b.iter(|| {
                black_box(Store::open(dir.path()).unwrap());
            });
        });
    }
    group.finish();
}

fn bench_point_get(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    for i in 0..10_000 {
        store.put(format!("k{i}"), format!("v{i}")).unwrap();
    }
    c.bench_function("store_get_hit", |b| {
        b.iter(|| black_box(store.get(b"k5000")));
    });
}

criterion_group!(
    benches,
    bench_put_throughput,
    bench_reopen_recovery,
    bench_point_get
);
criterion_main!(benches);
