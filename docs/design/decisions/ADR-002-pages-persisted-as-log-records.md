# ADR-002: Pages are persisted as ordinary versioned records in the existing log

## Status
Accepted

## Context

`docs/design/STORAGE.md`'s architecture calls for `Pages -> Buffer manager
-> Append-only log -> Segments`. Ticket 004 needed to decide how a fixed-
size page actually becomes durable: does it get its own dedicated file
format and its own segment/recovery machinery, or does it reuse the
`Log`/`Store` machinery already built (and already proven under exhaustive
crash injection) for plain key/value records?

## Decision

A page is persisted by treating its page id (an 8-byte big-endian key) and
its encoded 4096-byte body (the value) as an ordinary `Store` entry.
`LogPageStore` is a thin adapter: `write_page` is `store.put(id, bytes)`,
`read_page` is `store.get(id)`. Writing a new version of a page is
therefore, structurally, identical to writing a new version of any other
key — it appends a new immutable record; it never overwrites the previous
page version's bytes.

## Alternatives Considered

1. **A dedicated page file with its own fixed-slot layout** (pages written
   at `page_id * PAGE_SIZE` offsets in a single file, in place). This is
   how many real databases do it, but it requires either violating the
   no-overwrite constraint outright, or building an entirely separate
   versioning/GC scheme for pages independent from the one already built
   for the log. Rejected: it would duplicate crash-recovery logic instead
   of reusing code that is already tested against byte-offset-exhaustive
   crash injection (`crash_recovery.rs`).
2. **A second, page-specific append-only log format** with its own
   segment/rollover/recovery code, parallel to `log.rs`/`segment.rs`.
   Rejected for the same reason: two independently-maintained crash-safety
   implementations is strictly worse than one, and the existing one imposes
   no assumption that would make it unsuitable for fixed-size binary
   payloads.
3. **Reuse `Store` via a thin `PageStore` adapter** (chosen). One append-
   only log, one recovery implementation, one crash-injection test suite
   that now also covers pages (`page_crash_recovery.rs`) essentially for
   free.

## Consequences

- **Cost, measured, not assumed:** every page write still pays the
  underlying log's per-record `fsync` (`buffer_pool_eviction_churn_dirty_pages_logpagestore`
  in `benches/buffer_pool.rs` shows ~1ms per dirty page flushed — the same
  ceiling already documented for plain `put` in `docs/design/STORAGE.md`).
  Ticket 008 (group commit) will help both paths identically, for the same
  reason this reuse was worth it.
- **No page-level free-space reclamation yet:** because every page version
  is a full new `Store` record, a page that is updated 1000 times has 1000
  full 4096-byte copies sitting in the log until compaction (ticket 006)
  runs. This is the same growth-without-compaction trade already called
  out for keys; pages make it numerically worse (4096 bytes per version
  instead of a few bytes), which raises the priority of ticket 006 once a
  real workload (the B+Tree in ticket 005) starts writing pages
  frequently.
- **`PageStore` is a trait, not a concrete type**, specifically so the
  buffer pool's own logic (eviction, pinning, dirty tracking) could be
  tested against `MemPageStore` (`buffer.rs`'s unit tests, and the
  property tests in `page_and_buffer_property.rs`) without needing a real
  log/filesystem for every case — the storage-abstraction pattern the
  project's design doc calls for directly (crash simulator and production
  backend sharing one contract).
