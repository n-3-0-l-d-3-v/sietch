---
status: open
phase: 2
---

# 007 — Transactions and concurrent-client testing

Current `Store` is single-writer, single-transaction (every `put`/`delete`
commits immediately, no multi-operation atomicity, no conflict detection).
`docs/design/CONSTRAINTS.md` calls for real concurrency: multiple clients
reading/writing simultaneously while the store maintains its consistency
guarantees, deliberately forcing conflicts, crashes, retries, and stale
snapshots.

## Scope
- Multi-operation transactions (begin/commit/abort) with at least Read
  Committed, ideally Snapshot Isolation (the `Snapshot` type already here
  is a building block).
- Concurrent-client test harness: N threads/processes hammering the same
  store, asserting the documented isolation level actually holds.
- Write-write conflict detection under Snapshot Isolation (first-committer-
  wins or similar).

Not started. This is the natural on-ramp to Phase 6 (`choam`),
which needs real transactions and MVCC on top of this exact storage engine.
