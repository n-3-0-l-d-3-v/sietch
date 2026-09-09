//! Measures compaction's own cost against the size of history it
//! processes, and how much on-disk space it reclaims for a churny
//! (frequently-overwritten small key set) workload — the concrete,
//! measured answer to this project's own research question ("where does
//! complexity move?") for the no-overwrite storage constraint.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use storage::Store;
use tempfile::tempdir;

fn bench_compact_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("compact");
    for count in [100usize, 1_000, 5_000] {
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_batched(
                || {
                    let dir = tempdir().unwrap();
                    let mut store = Store::open(dir.path()).unwrap();
                    // Churn: repeatedly overwrite a small key set so
                    // there is real superseded history to reclaim.
                    for i in 0..count {
                        store.put(format!("k{}", i % 20), format!("v{i}")).unwrap();
                    }
                    (dir, store)
                },
                |(dir, mut store)| {
                    black_box(store.compact().unwrap());
                    black_box(&dir);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_compaction_space_reclaimed(c: &mut Criterion) {
    c.bench_function("compact_space_reclaimed_report_only", |b| {
        b.iter_batched(
            || {
                let dir = tempdir().unwrap();
                let mut store = Store::open(dir.path()).unwrap();
                for i in 0..2000u32 {
                    store.put("hot-key", format!("v{i}")).unwrap();
                }
                (dir, store)
            },
            |(dir, mut store)| {
                let before: u64 = std::fs::read_dir(dir.path())
                    .unwrap()
                    .map(|e| e.unwrap().metadata().unwrap().len())
                    .sum();
                store.compact().unwrap();
                let after: u64 = std::fs::read_dir(dir.path())
                    .unwrap()
                    .map(|e| e.unwrap().metadata().unwrap().len())
                    .sum();
                black_box((before, after));
            },
            criterion::BatchSize::LargeInput,
        );
    });
}

criterion_group!(
    benches,
    bench_compact_cost,
    bench_compaction_space_reclaimed
);
criterion_main!(benches);
