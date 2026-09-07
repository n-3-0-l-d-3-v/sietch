//! Benchmarks the B+Tree's insert and point-lookup cost as a function of
//! tree size, against a `MemPageStore` (isolating the tree/buffer-pool
//! algorithm's own cost) and against the real `LogPageStore` (which pays
//! the same fsync-per-dirty-page price already measured in
//! `buffer_pool.rs`).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use storage::page_store::{LogPageStore, MemPageStore};
use storage::BTree;
use tempfile::tempdir;

fn bench_insert_in_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("btree_insert_mem");
    for count in [100usize, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_batched(
                || BTree::open(MemPageStore::new(), 256).unwrap(),
                |mut tree| {
                    for i in 0..count {
                        tree.insert(
                            format!("k{i:08}").into_bytes(),
                            format!("v{i}").into_bytes(),
                        )
                        .unwrap();
                    }
                    black_box(&tree);
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_point_get_in_memory(c: &mut Criterion) {
    let mut tree = BTree::open(MemPageStore::new(), 256).unwrap();
    for i in 0..10_000 {
        tree.insert(
            format!("k{i:08}").into_bytes(),
            format!("v{i}").into_bytes(),
        )
        .unwrap();
    }
    c.bench_function("btree_get_hit_mem_10k_entries", |b| {
        b.iter(|| black_box(tree.get(b"k00005000").unwrap()));
    });
}

fn bench_insert_durable(c: &mut Criterion) {
    c.bench_function("btree_insert_100_logpagestore", |b| {
        b.iter_batched(
            || {
                let dir = tempdir().unwrap();
                let store = LogPageStore::open(dir.path()).unwrap();
                (dir, BTree::open(store, 32).unwrap())
            },
            |(dir, mut tree)| {
                for i in 0..100 {
                    tree.insert(
                        format!("k{i:08}").into_bytes(),
                        format!("v{i}").into_bytes(),
                    )
                    .unwrap();
                }
                black_box(&dir);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_insert_in_memory,
    bench_point_get_in_memory,
    bench_insert_durable
);
criterion_main!(benches);
