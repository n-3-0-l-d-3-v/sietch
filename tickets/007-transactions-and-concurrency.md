---
status: done
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
- [x] Multi-operation transactions (begin/commit/abort) with Snapshot
      Isolation: `TransactionalStore`/`Transaction` in
      `crates/storage/src/txn.rs`, built on the existing `Snapshot`/
      `get_at`/`apply_batch` primitives rather than a new mechanism. See
      `docs/design/decisions/ADR-011-transactions-snapshot-isolation.md`.
- [x] Concurrent-client test harness: `crates/storage/tests/concurrency.rs`
      runs real OS threads against one shared store — a lost-update test
      (8 threads × 25 retry-on-conflict increments to a shared counter,
      final value exactly 200), a snapshot-isolation-under-contention test
      (a reader's snapshot is unaffected by 4 concurrently racing writer
      threads), and a disjoint-keys-never-conflict test.
- [x] Write-write conflict detection, first-committer-wins:
      `Transaction::commit` checks every written key's latest committed
      sequence number against the transaction's snapshot; any conflict
      aborts the whole transaction (nothing partial is ever applied).

Deliberately out of scope, tracked as open follow-ups rather than
silently assumed done: full serializability (write skew is possible
under SI, as is standard), multi-process coordination (the `Mutex` in
`TransactionalStore` only coordinates threads within one process), and
a transactional wrapper for `IndexedStore` (only plain `Store` has one).

This is the on-ramp ticket for Phase 6 (`choam`), which needs real
transactions and MVCC on top of this exact storage engine — `choam` can
build on `TransactionalStore` directly.
