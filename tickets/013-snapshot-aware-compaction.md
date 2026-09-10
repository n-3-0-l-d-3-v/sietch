---
status: done
phase: 2
---

# 013 — Snapshot-aware compaction

`Store::compact()` (ticket 006) discards every non-latest version of every
key unconditionally. If a `Snapshot` taken before compaction is still in
use, `get_at`/`scan_at` calls against it will silently return incomplete
or wrong results afterward — the version they need may no longer exist.

## Scope
- [x] `Store` tracks outstanding snapshots via `Store::hold_snapshot() ->
      SnapshotGuard` — a refcounted registry (`as_of_seq -> count`) a
      caller opts into explicitly; `compact()` consults only the oldest
      currently-held seq. A `Store` with no held guards compacts exactly
      as before, with unchanged reclaim ratio.
- [x] API decision: **explicit hold/release** (`SnapshotGuard`, released
      on `Drop`) rather than an implicit configurable retention horizon —
      makes the cost of keeping a snapshot alive (what compaction can no
      longer reclaim) visible and attributable to whoever holds a guard.
      See `docs/design/decisions/ADR-012-snapshot-aware-compaction.md`.
- [x] Property test: `tests/compaction_property.rs`'s
      `compacting_with_a_held_snapshot_never_changes_that_snapshots_reads`
      — arbitrary put/delete sequences before and after a held snapshot,
      compaction run in between, checked against a reference model for
      every key, the same differential approach as the existing
      compaction property test.

Found and fixed one more defect along the way, not originally scoped but
a direct consequence of the same rewrite: compaction previously
reassigned every surviving record a **fresh** sequence number (via the
ordinary `Log::append_put` path), which silently scrambled any snapshot's
before/after ordering on every compaction, snapshot held or not. Fixed by
`Log::append_records_verbatim`, which preserves each surviving record's
original `seq` — used unconditionally now, not only when a snapshot is
held.

Also wired `TransactionalStore`/`Transaction` (ticket 007) to hold a
`SnapshotGuard` for the whole life of an open transaction, so a
transaction's reads are protected from a concurrent `compact()` call —
this was flagged as an open gap while writing ADR-012 and closed in the
same pass rather than left for later.

This closes Phase 2 (THE VAULT)'s ticket backlog.
