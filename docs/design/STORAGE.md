# THE VAULT — Storage Specification (Phase 2)

## Architecture (current)

```text
Application (vaultc / Store API)
        |
   Store (PUT/GET/DELETE/SCAN/SNAPSHOT, in-memory multi-version index)
        |
     Log (multi-segment append-only log, recovery, rollover)
        |
   Segment (single append-only file, checksummed records)
        |
   Host filesystem

   -- parallel, page-oriented path over the same Log/Store --

     BTree (real disk-oriented index: splits, multi-level growth)
        |
BufferPool (clock eviction, pinning, dirty tracking)
        |
   PageStore trait  --  MemPageStore (tests)  /  LogPageStore (real)
        |
     Page (4096-byte slotted page, checksummed)
        |
   Store  (a page write is just a Put record keyed by page id)
```

This is deliberately built up in slices matching the full architecture in
the project's design doc (`Application -> Logical objects -> Pages ->
Buffer manager -> Append-only log -> Segments -> Host storage`). The
disk-oriented B+Tree index now exists and is real (verified at 20,000
inserts / 3+ tree levels), but it is not yet wired in as the KV `Store`'s
actual index — see ticket 011. The KV `Store`'s index is still rebuilt
into memory on every open; that trade is explicit, not hidden (see "What
this slice does not claim" below).

## The constraint, concretely

**Persistent storage may never overwrite an existing, committed byte.**
Every logical write is a new, immutable, checksummed record appended to the
active segment. A logical update to a key does not touch any prior record
for that key — it appends a new version. A deletion appends a tombstone
record, not a byte-level erasure.

## Record format

See `crates/storage/src/record.rs` module docs for the exact byte layout.
Every record carries a CRC32 checksum over its own header + payload, so
corruption is *detected*, not just assumed absent.

## Crash recovery

On `Log::open`, every segment is replayed from its first byte. Decoding
stops at the first `Incomplete` (truncated) or `ChecksumMismatch` record —
whichever comes first — and everything before that point is the recovered,
committed state. See `docs/design/decisions/ADR-001-torn-tail-truncation.md`
for exactly what happens to the bytes after that point and why it does not
violate the no-overwrite constraint.

`crates/storage/tests/crash_recovery.rs` proves this by literally truncating
a real segment file at *every* possible byte offset and reopening the store
at each cut point: whatever key is visible afterward always has the correct
value, never a partially-written or corrupted one.

## Versioning and snapshots

Every record carries a monotonically increasing `seq` (assigned by `Log`,
persisted, and rebuilt from the log on reopen). The in-memory index keeps
every version of every key (`Vec<VersionEntry>`), not just the latest. A
`Snapshot` is just a captured `seq` value; `get_at`/`scan_at` walk backward
through a key's version list to find the version visible as of that
`seq`. This is genuine multi-version concurrency control in miniature —
not a full transaction system yet (no write-write conflict detection,
single-writer only in this slice — see ticket 007).

## Pages and the buffer manager

A `Page` (`crates/storage/src/page.rs`) is a fixed 4096-byte slotted page:
a 16-byte header (magic, page type, checksum, slot count, free-space
pointers) followed by a slot directory that grows forward and record bytes
that grow backward from the end of the page — the classic layout, chosen
because ticket 005's B+Tree nodes need variable-length keys/pointers
addressed by slot, not just fixed-width fields.

The `BufferPool` (`crates/storage/src/buffer.rs`) caches pages, pins them
while in use, tracks a dirty bit, and evicts with clock (second-chance)
replacement — a pinned page is never a valid eviction victim, proven by a
property test across arbitrary operation sequences
(`tests/page_and_buffer_property.rs`), not just hand-picked examples.

Persistence is behind a `PageStore` trait with two implementations:
`MemPageStore` (in-memory, used to test the pool's own eviction/pinning
logic in isolation) and `LogPageStore` (the real backend — see
`docs/design/decisions/ADR-002-pages-persisted-as-log-records.md` for why
a page write is, underneath, just an ordinary versioned `Store` record
keyed by page id). That means page writes inherit the exact same crash
guarantees as everything else in this store, verified directly in
`tests/page_crash_recovery.rs`.

## The B+Tree index

`BTree` (`crates/storage/src/btree.rs`) is a real disk-oriented B+Tree
built entirely on `Page`/`BufferPool`. Its meta page (a fixed, reserved
page id) records the current root, so the tree survives a process
restart. Every mutation decodes a node's entries, mutates a sorted `Vec`,
and rebuilds the page from scratch — see
`docs/design/decisions/ADR-003-btree-page-rebuild-strategy.md` for why,
and for the measured per-insert cost that trade produces
(`benches/btree.rs`). Node splits (both leaf and internal) propagate up
correctly through multiple levels — proven at 20,000 inserts producing a
3+ level tree with every key still retrievable — and the tree's behavior
matches a reference `std::collections::BTreeMap` under both a large
randomized differential test and a proptest property test over arbitrary
insert sequences with upserts.

`delete` (ticket 009) is done too: it propagates "this node became
completely empty" up through the tree — promoting a sibling into a
vacated leftmost slot, or dropping a reference outright, all the way up
to shrinking the root when it collapses to a single child. Proven at
scale (2,000 inserts collapsed back to 10 keys, tree still fully correct)
and against a reference `BTreeMap` for arbitrary interleaved insert/delete
sequences. It deliberately does *not* do full minimum-occupancy
rebalancing (redistributing from or merging with a sibling when a node is
under-full but not empty) — see
`docs/design/decisions/ADR-008-btree-deletion-without-rebalancing.md` for
why that's a fill-factor cost, not a correctness one.

Bounded range scans (ticket 010) are done too: leaf pages carry a
right-sibling pointer (reserved slot 0, threaded correctly through
splits), and `scan_range(start, end)` descends once to the starting leaf
and walks the chain instead of re-descending from the root. Measured, not
assumed: `benches/btree.rs` shows `scan_range` costing a flat ~20–24µs
regardless of tree size (1,000 to 50,000 entries) while `scan_all` over
the same trees grows from ~85µs to ~5.4ms — O(log n + k) vs. O(n), with
real numbers. The B+Tree-vs-LSM-tree comparison ticket 005 deferred is
now written, grounded in this project's own measurements:
`docs/design/decisions/ADR-009-btree-vs-lsm-tree.md`.

## IndexedStore — regression, fix, fix again, then a real measured win

Ticket 011 wired `BTree` in as an alternative to `Store`'s index
(`IndexedStore`, `crates/storage/src/indexed_store.rs`), on the hypothesis
that reopening would be fast once the index is already durable on disk
instead of rebuilt from a full log replay every time. Getting there took
three honestly-reported rounds:

1. **First version disproved the hypothesis**: `IndexedStore::open` was
   ~12x *slower* than plain `Store::open` at 10,000 entries. Root cause
   (`docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md`):
   persisting B+Tree pages through a generic `Store` (ADR-002) meant
   opening the index paid its own full-log-replay, against a log 254.8x
   larger than the original because of page-rebuild write amplification
   (ADR-003).
2. **`HeapPageStore` (ticket 012) fixed that root cause**
   (`docs/design/decisions/ADR-005-heap-page-store-fixes-the-reopen-regression.md`):
   pages in a flat heap file never scanned at open, locations in a small
   separately-replayed log. Data replayed at open dropped ~111x. Absolute
   latency improved a lot but didn't yet clearly beat plain `Store`.
3. **Checkpoint batching closed the remaining gap**
   (`docs/design/decisions/ADR-006-checkpoint-batching.md`): the
   reconciliation sentinel was being written and flushed after *every*
   operation — doubling B+Tree writes relative to `Store`'s single
   in-memory insert. Batching it (default: every 128 operations, with an
   explicit `checkpoint()`/`flush()` for a graceful shutdown) removed that
   cost. **Re-measured and reproduced twice: `IndexedStore::open` is now
   faster than plain `Store::open` at 10,000+ entries (1.12x at 10,000,
   1.37x at 30,000), with the advantage growing with history size.**

Composing independently-correct pieces first produced a real regression,
then a real architectural fix, then a real remaining-cost fix, then a
real, reproducible win — exactly the kind of finding this project's own
research question ("where does complexity move?") exists to surface,
reported honestly at every step rather than only once it looked good.

## Compaction

`Store::compact()` rewrites the log to hold exactly one live record per
current key (latest value only; tombstoned keys dropped entirely),
reclaiming space from superseded versions and deletions — the direct
answer to "because nothing can be overwritten, storage grows forever
without a compaction policy." It never mutates an existing segment: the
replacement log is built completely in a temp directory, committed via a
marker file, and only then swapped in through a three-step,
always-resumable-from-any-crash-point protocol — see
`docs/design/decisions/ADR-007-compaction-commit-marker.md`, which also
documents a real bug (an early design could have destroyed the only valid
copy of the data during a specific crash-timing edge case) that a test
caught before it shipped.

**Known limitation**: compaction discards *all* non-latest versions
unconditionally, including ones an outstanding `Snapshot` might still be
reading — using `get_at`/`scan_at` against a snapshot taken before a
compaction will silently return incomplete results. Snapshot-aware
compaction is ticket 013, not yet started. Compaction currently applies to
plain `Store` only, not `IndexedStore` or the B+Tree's pages — page-level
reclamation is separate future work.

## What this slice does not claim

- `IndexedStore` only speeds up latest-value operations (put/get/delete/
  scan); multi-version reads (`Snapshot`/`get_at`/`scan_at`) are not
  implemented on it at all, and plain `Store` remains the only option for
  those. `checkpoint_interval`'s default (128) is a reasonable starting
  guess, not a tuned constant.
- Plain `Store`'s index is still rebuilt into memory on every open by
  replaying the whole log; that's the exact scaling problem `IndexedStore`
  now measurably fixes (see above) for the latest-value operations it
  supports — reach for `IndexedStore` over `Store` once history size
  matters and multi-version reads aren't needed.
- No page-level compaction yet: every past version of every B+Tree page
  (4096 bytes each) still accumulates forever in `HeapPageStore`'s heap
  file, and a deleted slot's bytes are never reclaimed within a page
  either.
- Single-writer only; no locking or multi-process coordination.
- `fsync` per record (and therefore per dirty page flush) makes every
  write durable but limits throughput — measured at ~1ms/put for keys
  (`append_throughput.rs`) and ~1ms per dirty page evicted
  (`buffer_pool.rs`'s `buffer_pool_eviction_churn_dirty_pages_logpagestore`),
  both dominated by fsync latency, not CPU. Group-commit / batched fsync
  is a natural future optimization, deliberately not done yet — ticket 008.
