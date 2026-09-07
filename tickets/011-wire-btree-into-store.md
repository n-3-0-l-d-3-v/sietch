---
status: done
phase: 2
---

# 011 — Wire the B+Tree in as Store's real persistent index

`Store` (tickets 001–003) rebuilds an in-memory `BTreeMap` index by
replaying the entire log on every open. This ticket built `IndexedStore`
(`crates/storage/src/indexed_store.rs`) to use the on-disk `BTree`
(ticket 005) instead — and its full history is the honest record of how
that turned out.

## The full arc
1. First version (over `LogPageStore`): ~12x *slower* to reopen than
   plain `Store` at 10,000 entries. Root-caused to nested full-page replay
   — see ADR-004.
2. `HeapPageStore` (ticket 012): fixed that specific defect. Data replayed
   at open time dropped ~111x (254.8x the main log -> 2.3x). Absolute
   reopen latency improved substantially but didn't yet clearly beat plain
   `Store` — see ADR-005.
3. Checkpoint batching (this ticket, final round): batching the
   reconciliation sentinel's write instead of updating it on every single
   operation removed the remaining dominant cost. **Re-measured,
   reproducibly, `IndexedStore::open` is now faster than plain
   `Store::open` at 10,000+ entries (1.12x at 10,000; 1.37x at 30,000),
   with the advantage growing with history size** — see ADR-006.

## Acceptance criteria
- [x] `IndexedStore`: put/get/delete/scan backed by the on-disk `BTree`
      over `HeapPageStore`, durability log as source of truth.
- [x] Crash-consistent reconciliation, including automatic checkpoint
      batching — proven correct regardless of where a crash lands
      relative to a checkpoint boundary
      (`automatic_checkpointing_bounds_reconciliation_to_the_interval_not_the_whole_history`,
      `all_values_are_correct_regardless_of_where_a_crash_lands_relative_to_a_checkpoint`),
      and by exhaustive byte-offset crash injection
      (`tests/indexed_store_crash_recovery.rs`, unaffected by the
      batching change).
- [x] Differential property test proving identical behavior to plain
      `Store` for the same operations, unaffected by the batching change.
- [x] The decisive benchmark, run three times across this ticket's full
      arc, reported honestly each time — including two rounds that did
      not yet show the intended win — until the actual, reproducible
      crossover was measured.

## Deliberately still out of scope (tracked separately, not silently dropped)
- Multi-version reads (`Snapshot`/`get_at`/`scan_at`) are not implemented
  on `IndexedStore` — not yet even designed. `IndexedStore` is a faster
  drop-in for `Store`'s latest-value operations only.
- `checkpoint_interval`'s default (128) is a reasonable starting guess,
  not a tuned constant.
