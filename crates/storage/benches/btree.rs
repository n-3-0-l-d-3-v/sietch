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

/// The ticket 010 claim, measured: `scan_range` for a small, fixed-size
/// window should cost roughly the same regardless of total tree size
/// (O(log n) to find the start leaf, O(k) to walk it), while `scan_all`
/// over the same growing tree should cost more as the tree grows (O(n)).
fn bench_range_scan_vs_full_scan_as_tree_grows(c: &mut Criterion) {
    let mut group = c.benchmark_group("btree_range_scan_vs_full_scan");
    for count in [1_000usize, 10_000, 50_000] {
        let mut tree = BTree::open(MemPageStore::new(), 256).unwrap();
        for i in 0..count {
            tree.insert(format!("{i:08}").into_bytes(), format!("v{i}").into_bytes())
                .unwrap();
        }

        group.bench_with_input(
            BenchmarkId::new("scan_range_100_results", count),
            &(),
            |b, ()| {
                b.iter(|| black_box(tree.scan_range(b"00000100", b"00000200").unwrap()));
            },
        );
        group.bench_with_input(BenchmarkId::new("scan_all", count), &(), |b, ()| {
            b.iter(|| black_box(tree.scan_all().unwrap()));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_insert_in_memory,
    bench_point_get_in_memory,
    bench_insert_durable,
    bench_range_scan_vs_full_scan_as_tree_grows
);
criterion_main!(benches);
