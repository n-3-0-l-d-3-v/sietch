---
status: done
phase: 2
---

# 004 — Page format + buffer manager

The full storage architecture in `docs/design/STORAGE.md` calls for
`Application -> Logical objects -> Pages -> Buffer manager -> Append-only
log -> Segments -> Host`. This ticket built the Pages and Buffer manager
layers on top of the existing log — see
`docs/design/decisions/ADR-002-pages-persisted-as-log-records.md` for how
persistence was wired up without duplicating crash-recovery logic.

## Acceptance criteria
- [x] Fixed-size (4096-byte) slotted page format: header with checksum and
      version metadata (page type), slot directory, variable-length record
      layout within a page (`crates/storage/src/page.rs`).
- [x] Buffer manager: caching, clock (second-chance) eviction, reference-
      counted pinning, dirty-page tracking (`crates/storage/src/buffer.rs`).
- [x] Append-only persistence semantics preserved: a "dirty" page becomes a
      new immutable version on flush, never an in-place overwrite of a
      previously-flushed page — proven by
      `page_store::tests::writing_a_new_version_of_a_page_does_not_lose_history_in_the_log`
      and the crash-injection tests in `tests/page_crash_recovery.rs`.
- [x] `PageStore` trait with two implementations: `MemPageStore` (in-memory,
      for testing the buffer pool's own logic in isolation) and
      `LogPageStore` (real, crash-safe, built on the existing `Store`) —
      the storage-abstraction pattern (test double vs. production backend
      sharing one contract) the project's design doc calls for directly.
- [x] Property-based tests: pinned pages are never evicted under any
      operation sequence; dirty pages survive arbitrary eviction churn;
      pages round-trip through encode/decode with arbitrary insert/delete
      sequences (`tests/page_and_buffer_property.rs`) — this is also what
      caught a real header byte-offset bug (checksum field overlapping
      `num_slots`) before it shipped.
- [x] Benchmarks: cache-hit fetch cost vs. eviction-under-pressure cost vs.
      dirty-page-eviction cost (which pays the same fsync-per-write price
      already documented for plain key/value puts) — `benches/buffer_pool.rs`.

## Known limitation (tracked, not silently ignored)
- No in-page compaction: a deleted slot's bytes are never reclaimed within
  a page, only marked invalid. Revisit if/when ticket 005's B+Tree node
  splits and merges make this matter in practice.
