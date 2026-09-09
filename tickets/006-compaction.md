---
status: done
phase: 2
---

# 006 — Compaction

Per `docs/design/CONSTRAINTS.md`: because nothing can be overwritten,
storage grows forever without a compaction/reclamation policy.
`Store::compact()` (`crates/storage/src/store.rs`) fixes this — see
`docs/design/decisions/ADR-007-compaction-commit-marker.md` for the full
design and a real bug it caught before shipping.

## Acceptance criteria
- [x] Distinguishes immutable data, obsolete logical versions, and
      physically reclaimable storage in code: `compact()` reads the
      in-memory index's live entries (latest non-tombstone value per key),
      writes them as a brand-new log, and only *then* reclaims the old
      segments — never mutating an old segment in place.
- [x] Crash-safe at every point: a three-step, always-resumable swap
      (build compacted log in a temp dir -> commit marker -> backup old
      segments -> move new segments in -> clean up), proven by six
      dedicated crash-simulation tests covering every distinguishable
      interruption point, plus a property test proving compaction never
      changes a store's externally observable `get`/`scan` results.
- [x] Benchmarked: `benches/compaction.rs` measures `compact()`'s own cost
      vs. history size (dominated by fixed swap overhead, not record
      count, at the tested scales) and demonstrates real on-disk shrinkage
      for a churny workload (`compaction_actually_reduces_stored_history`).

## Known limitation (tracked as a follow-up, not silently ignored)
- Compaction discards *all* non-latest versions unconditionally, including
  ones an outstanding `Snapshot` might still be reading — see ticket 013
  for snapshot-aware compaction.
- This lands on plain `Store`, not `IndexedStore` (ticket 011). Wiring
  compaction for the B+Tree/page-backed path is separate future work,
  since it involves page-level reclamation rather than log-record
  reclamation.
