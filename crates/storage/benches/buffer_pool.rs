//! Benchmarks the buffer pool's cache-hit path (should be cheap: a hash
//! lookup and a pin-count bump) against its eviction path (should be more
//! expensive: a clock scan, and a real disk write if the victim is dirty),
//! so the cost of "no free lunch" caching is measured, not assumed.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use storage::page::PageType;
use storage::{BufferPool, LogPageStore, MemPageStore};
use tempfile::tempdir;

fn bench_cache_hit_fetch(c: &mut Criterion) {
    let mut pool = BufferPool::new(MemPageStore::new(), 64);
    let id = pool.new_page(PageType::Data).unwrap();
    pool.unpin(id, false).unwrap();
    c.bench_function("buffer_pool_cache_hit_fetch", |b| {
        b.iter(|| {
            pool.fetch(id).unwrap();
            pool.unpin(id, false).unwrap();
            black_box(id);
        });
    });
}

fn bench_eviction_under_pressure(c: &mut Criterion) {
    // A deliberately tiny pool (capacity 4) against a much larger working
    // set forces constant eviction, so every fetch pays the clock-scan +
    // possible-flush cost.
    c.bench_function("buffer_pool_eviction_churn_clean_pages", |b| {
        b.iter_batched(
            || {
                let mut pool = BufferPool::new(MemPageStore::new(), 4);
                let ids: Vec<_> = (0..20)
                    .map(|_| {
                        let id = pool.new_page(PageType::Data).unwrap();
                        pool.unpin(id, false).unwrap();
                        id
                    })
                    .collect();
                (pool, ids)
            },
            |(mut pool, ids)| {
                for &id in &ids {
                    pool.fetch(id).unwrap();
                    pool.unpin(id, false).unwrap();
                }
                black_box(&pool);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_eviction_flushes_dirty_pages_to_disk(c: &mut Criterion) {
    c.bench_function("buffer_pool_eviction_churn_dirty_pages_logpagestore", |b| {
        b.iter_batched(
            || {
                let dir = tempdir().unwrap();
                let store = LogPageStore::open(dir.path()).unwrap();
                let pool = BufferPool::new(store, 4);
                (dir, pool)
            },
            |(dir, mut pool)| {
                for _ in 0..20 {
                    let id = pool.new_page(PageType::Data).unwrap();
                    pool.page_mut(id).unwrap().insert(b"dirty-payload").unwrap();
                    pool.unpin(id, true).unwrap();
                }
                black_box(&dir);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_cache_hit_fetch, bench_eviction_under_pressure, bench_eviction_flushes_dirty_pages_to_disk);
criterion_main!(benches);
