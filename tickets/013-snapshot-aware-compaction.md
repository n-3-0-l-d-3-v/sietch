---
status: open
phase: 2
---

# 013 — Snapshot-aware compaction

`Store::compact()` (ticket 006) discards every non-latest version of every
key unconditionally. If a `Snapshot` taken before compaction is still in
use, `get_at`/`scan_at` calls against it will silently return incomplete
or wrong results afterward — the version they need may no longer exist.

## Scope
- `Store` needs to track outstanding snapshots (or at least the oldest
  `as_of_seq` still referenced) so `compact()` can know which versions are
  still reachable from a live snapshot and must be preserved.
- Decide the API: does a `Snapshot` need an explicit `drop`/`release` so
  `Store` knows when it's no longer needed, or does `Store` just keep
  versions back to some configurable retention horizon?
- Property test: compacting while a snapshot is held must not change what
  that snapshot's `get_at`/`scan_at` returns, for arbitrary put/delete
  sequences interleaved with snapshot creation — the same differential
  approach as `tests/compaction_property.rs`, extended to cover snapshots.

Not started.
