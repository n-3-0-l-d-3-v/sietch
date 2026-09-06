---
status: open
phase: 2
---

# 008 — Group commit / batched fsync

`crates/storage/benches/append_throughput.rs` measures ~1ms per `put`,
dominated by an `fsync` on every single record
(`crates/storage/src/segment.rs::Segment::append`). This is the honest,
measured cost of "durable by default"; it is a real throughput ceiling for
any workload that needs more than ~1000 writes/sec from a single client.

## Scope
- Batch multiple pending records into one `fsync` call (group commit),
  with a bounded latency budget (e.g. flush after N records or T
  milliseconds, whichever first).
- Benchmark before/after on `store_put` throughput; document the latency
  vs. throughput trade-off explicitly (a batched write is not durable until
  its group's fsync completes, which is a real difference callers must be
  able to reason about).

Not started. Do this only after ticket 007 (transactions) if transaction
boundaries turn out to be the natural batching unit — check before
building a separate, disconnected batching mechanism.
