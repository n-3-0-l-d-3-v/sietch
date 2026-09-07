---
status: open
phase: 2
---

# 012 — Eliminate LogPageStore's nested full-replay bottleneck

See `docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md`
for the full measured finding this ticket exists to fix: `LogPageStore`
persists pages through a generic `Store`, and `Store::open` always fully
replays its entire log to rebuild an in-memory index — which is the exact
cost ticket 011 was trying to eliminate at the `IndexedStore` layer, just
relocated one level down and multiplied by page-rebuild write
amplification (ADR-003). Net result: `IndexedStore::open` was ~12x
*slower* than plain `Store::open` at 10,000 entries.

## Scope
- Give page storage its own lightweight recovery path instead of reusing
  generic `Store`'s full-replay-into-`BTreeMap` index — e.g. a compact,
  separately persisted `page_id -> (segment, offset)` location table that
  can itself be appended to incrementally, so opening doesn't require
  reading every historical page version, only the latest location per
  page id.
- Alternative worth evaluating instead: batch multiple page writes from a
  single logical operation (e.g. one `BTree::insert` call, which may touch
  several pages plus the reconciliation sentinel) into fewer, larger log
  records, reducing both the record count and the fsync count ticket 008
  (group commit) is separately concerned with.
- Whatever the fix, re-run `benches/indexed_vs_plain_reopen.rs` — ticket
  011 does not close until that benchmark actually shows the improvement
  it was written to prove.

Not started. This is the direct, honest follow-up to a negative
benchmark result, not speculative work.
