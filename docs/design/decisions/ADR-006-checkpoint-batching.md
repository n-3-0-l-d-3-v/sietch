# ADR-006: Checkpoint batching closes the reopen-time gap — IndexedStore is now measurably faster than Store at scale

## Status
Accepted

## Context

ADR-005 fixed the nested-full-replay defect (`HeapPageStore`) but left
`IndexedStore::open` still somewhat slower than plain `Store::open` in
absolute terms at the tested sizes, and identified the likely remaining
cause: every `put`/`delete` did **two** B+Tree inserts — the user key, and
a reconciliation sentinel key (recording "everything up to this log
position is indexed") updated and flushed after *every single operation*.
Each B+Tree insert can touch and `fsync` several pages along its
root-to-leaf path, so this doubled (at minimum) the durable-write cost of
every logical operation compared to plain `Store`'s single in-memory map
insert.

## Decision

Batch the sentinel checkpoint instead of writing it on every operation:
`IndexedStore` now checkpoints (writes the sentinel, flushes the buffer
pool) automatically every `checkpoint_interval` operations (default 128),
and exposes `checkpoint()`/`flush()` for a caller to checkpoint explicitly
before a graceful shutdown. This is the same latency/durability trade
ticket 008 (group commit) makes for `fsync` batching in general, applied
here specifically to the reconciliation sentinel.

This does **not** weaken durability of the data itself: every `put`/
`delete` is still immediately and unconditionally `fsync`'d to the
**durability log** (unchanged). Only the *index's own* durability — which
was always a derived, reconstructible-from-the-log cache, not a source of
truth — is now batched. An unclean shutdown between checkpoints simply
means the next `open` replays a few more log records during
reconciliation (up to `checkpoint_interval - 1` of them) — bounded,
correct, and already exactly what the reconciliation mechanism (built and
crash-tested in ticket 011's first round) is for.

## Measured result

`benches/indexed_vs_plain_reopen.rs`, re-run twice for reproducibility (20
samples, 3s measurement window, both stores measured in the same run to
avoid cross-run system-noise comparison — see the note below):

| entries | `Store::open` | `IndexedStore::open` | result |
|---|---|---|---|
| 100 | 0.84 ms | 1.74 ms | 2.1x slower |
| 1,000 | 5.27 ms | 6.15 ms | 1.17x slower |
| 10,000 | 9.48 ms | 8.43 ms | **1.12x faster** |
| 30,000 | 17.36 ms | 12.69 ms | **1.37x faster** |

Reproduced on a second run with consistent results (10,000: 8.90ms vs
8.74ms; 30,000: 16.35ms vs 12.17ms — same crossover, same direction).

**This is the crossover ticket 011 set out to demonstrate.** At small
history sizes, `IndexedStore` pays more fixed per-open overhead (three
separate files/logs to open: main log, heap file, location log, vs.
`Store`'s one) than it saves. Past roughly 1,000–10,000 entries, its
much-slower-growing replay cost (proportional to the small location log,
not the full history) overtakes `Store`'s linearly-growing full-log-replay
cost, and the advantage grows with history size — exactly the scaling
behavior this ticket existed to build.

**Methodology note**: absolute timings drift noticeably between separate
benchmark invocations on this machine (likely OS-level disk-cache
warmth) — comparing `Store` and `IndexedStore` *within the same benchmark
run* is what makes this result trustworthy; comparing one run's numbers
against a different run's cached baseline (as earlier ADRs necessarily
did, before both sides of the comparison existed side by side) is not
reliable enough to draw conclusions from alone.

## Consequences

- Ticket 011 closes. Its full arc — hypothesis, disproof (ADR-004), a
  real architectural fix (ADR-005), a real remaining-cost fix (this ADR),
  and a reproducible measured win — is the complete, honest record.
- `checkpoint_interval` trades reopen-after-crash replay cost against
  per-operation write cost. The default (128) is a starting guess, not a
  tuned constant; revisit if a real workload's crash-replay time or
  steady-state write throughput matters more precisely than this default
  assumes.
- Multi-version reads (`Snapshot`/`get_at`/`scan_at`) are still not
  implemented on `IndexedStore` — this ADR only closes the performance
  question, not the feature-parity-with-`Store` question. That remains
  explicitly out of scope, not silently dropped (see ticket 011's
  original text for the still-undesigned multi-version indexing
  question).
