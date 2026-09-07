---
status: open
phase: 2
---

# 011 — Wire the B+Tree in as Store's real persistent index

`Store` (tickets 001–003) rebuilds an in-memory `BTreeMap` index by
replaying the entire log on every open. This ticket built `IndexedStore`
(`crates/storage/src/indexed_store.rs`) to use the on-disk `BTree`
(ticket 005) instead, on the hypothesis that reopening would then be fast.

**The benchmark this ticket required disproved that hypothesis** — see
`docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md`
for the full analysis. `IndexedStore::open` is ~12x *slower* than plain
`Store::open` at 10,000 entries, because persisting B+Tree pages through a
generic `Store` (ADR-002's composition) means opening the index pays a
*second* full-log-replay — against a log that's 200x+ larger than the
original due to whole-page-rebuild write amplification (ADR-003).

## What was actually delivered (and is correct, kept, tested)
- [x] `IndexedStore`: put/get/delete/scan backed by the on-disk `BTree`,
      with the durability log kept as the source of truth.
- [x] Crash-consistent reconciliation: if the index falls behind the log
      (an unclean shutdown between a log write committing and the
      corresponding index write committing), reopening replays only the
      unindexed tail, not the whole log — proven both by a targeted unit
      test and by exhaustive byte-offset crash injection
      (`tests/indexed_store_crash_recovery.rs`).
- [x] Differential property test proving `IndexedStore` behaves identically
      to plain `Store` for the same operations
      (`tests/indexed_store_property.rs`).
- [x] The decisive benchmark (`benches/indexed_vs_plain_reopen.rs`) — run
      and reported honestly, including the negative result.

## What remains before this ticket can close
- [ ] Fix the actual bottleneck identified in ADR-004 — see ticket 012.
- [ ] Re-run `benches/indexed_vs_plain_reopen.rs` and confirm
      `IndexedStore::open` is actually faster than `Store::open` at scale
      before claiming the original goal is met.
- [ ] Multi-version reads (`Snapshot`/`get_at`/`scan_at`) are still not
      implemented on `IndexedStore` at all — deferred, not yet even
      designed.
