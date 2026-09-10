---
status: done
phase: 2
---

# 008 — Group commit / batched fsync

`crates/storage/benches/append_throughput.rs` measures ~1ms per `put`,
dominated by an `fsync` on every single record
(`crates/storage/src/segment.rs::Segment::append`). This is the honest,
measured cost of "durable by default"; it is a real throughput ceiling for
any workload that needs more than ~1000 writes/sec from a single client.

## Scope
- [x] Batch multiple pending records into one `fsync` call (group commit).
      Chosen shape: an explicit caller-driven batch API at every layer
      (`Segment::append_batch`, `Log::append_batch`, `Store::apply_batch`)
      rather than a background-timer flush-after-N-records-or-T-ms scheme —
      a caller always controls exactly which writes share a durability
      point, so nothing is held back longer than intended. See
      `docs/design/decisions/ADR-010-group-commit.md`.
- [x] Benchmark before/after on `store_put` throughput:
      `benches/append_throughput.rs`'s `store_group_commit` group measures
      sequential `put` (N fsyncs) against `apply_batch` (1 fsync) for the
      same writes in the same run — ~7.5x at 10 writes, ~57x at 100, ~260x
      at 1,000. Documented in ADR-010, including why the speedup grows
      with batch size.
- [x] Document the latency vs. throughput trade-off: `apply_batch`'s whole
      batch is durable together or not at all — an individual write inside
      it is not independently acknowledged durable until the batch's single
      `fsync` returns. Crash safety needs no new reasoning: a partially
      written batch is indistinguishable from any other torn write under
      the existing torn-tail recovery contract (ADR-001).

Built independently of ticket 007 (transactions) rather than waiting on
it: transaction boundaries would be *a* natural batching unit, but
`apply_batch` works today for any caller with a batch of writes ready
(bulk load, request handler accumulating keys, etc.) without needing
transaction semantics to exist first. Ticket 007 can adopt `apply_batch`
as its commit-time mechanism later without redesign.
