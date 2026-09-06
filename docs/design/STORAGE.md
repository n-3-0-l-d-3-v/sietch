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
Buffer manager -> Append-only log -> Segments -> Host storage`). A
disk-oriented B+Tree index is **not yet built** — see ticket 005, which
will be the first real consumer of the page layer. The KV `Store`'s index
is still rebuilt into memory on every open; that trade is explicit, not
hidden (see "What this slice does not claim" below).

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

## What this slice does not claim

- No on-disk index (B+Tree) yet — both the KV `Store`'s index and, so far,
  nothing above the page layer use it; pages exist but nothing is built on
  top of them yet. The KV index is rebuilt into memory on every open by
  replaying the whole log; fine for now, will not scale past the point
  where the log no longer fits comfortably in memory-rebuild time.
  Measured in `crates/storage/benches/append_throughput.rs`
  (`store_reopen_recovery`). Ticket 005.
- No compaction yet, for keys or pages. Because nothing is ever
  overwritten, the log only grows — including superseded versions,
  tombstones, and every past version of every page (4096 bytes each,
  strictly worse than a plain key's growth). Ticket 006.
- No in-page compaction either: a deleted slot's bytes are never reclaimed
  within a page.
- Single-writer only; no locking or multi-process coordination.
- `fsync` per record (and therefore per dirty page flush) makes every
  write durable but limits throughput — measured at ~1ms/put for keys
  (`append_throughput.rs`) and ~1ms per dirty page evicted
  (`buffer_pool.rs`'s `buffer_pool_eviction_churn_dirty_pages_logpagestore`),
  both dominated by fsync latency, not CPU. Group-commit / batched fsync
  is a natural future optimization, deliberately not done yet — ticket 008.
