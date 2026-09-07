---
status: open
phase: 2
---

# 011 — Wire the B+Tree in as Store's real persistent index

`Store` (tickets 001–003) rebuilds an in-memory `BTreeMap` index by
replaying the entire log on every open. This ticket built `IndexedStore`
(`crates/storage/src/indexed_store.rs`) to use the on-disk `BTree`
(ticket 005) instead.

**History**: the first version (over `LogPageStore`) was measured ~12x
*slower* to reopen than plain `Store` — see ADR-004. Ticket 012's
`HeapPageStore` fixed that specific root cause (~111x reduction in data
replayed at open time — ADR-005), but absolute reopen latency still
doesn't clearly beat plain `Store` within the tested range (100–30,000
entries). This ticket stays open, narrowed to the remaining cause.

## What was delivered and is correct, kept, tested
- [x] `IndexedStore`: put/get/delete/scan backed by the on-disk `BTree`
      over `HeapPageStore`, with the durability log as source of truth.
- [x] Crash-consistent reconciliation (unindexed log tail replayed on
      reopen after an unclean shutdown), proven by targeted tests and
      exhaustive byte-offset crash injection.
- [x] Differential property test proving identical behavior to plain
      `Store` for the same operations.
- [x] Two full rounds of the decisive benchmark, run and reported
      honestly both times (ADR-004, ADR-005), including a negative result
      the first time.

## What remains before this ticket can close
- [ ] Reduce writes-per-operation: `apply_and_advance` currently does two
      B+Tree inserts per `put`/`delete` (the user key, and the
      reconciliation sentinel key) — identified in ADR-005 as the likely
      dominant remaining cost, now that nested full-replay is fixed. Batch
      or otherwise avoid updating the sentinel on every single operation.
- [ ] Re-run `benches/indexed_vs_plain_reopen.rs` after that change and
      confirm `IndexedStore::open` is actually faster than `Store::open`
      at scale before claiming this ticket's original goal is met.
- [ ] Multi-version reads (`Snapshot`/`get_at`/`scan_at`) are still not
      implemented on `IndexedStore` at all — deferred, not yet even
      designed.
