# ADR-010: Group commit — batch fsync across a caller-defined set of writes

## Status
Accepted

## Context

Every durable write in this codebase — `Store::put`/`delete`, and
`Segment::append` underneath them — has always called `fsync` (via
`File::sync_data`) once per record, immediately, before returning. That is
the right default for correctness (a caller is never told a write
succeeded before it is durable) but it means N logical writes cost N
`fsync` calls, even when a caller has a whole batch of writes ready at
once and doesn't need each one acknowledged individually. `fsync` is by
far the most expensive part of a small write (a few microseconds of
in-memory work vs. a disk/controller round-trip), so this is the
dominant cost at any real write volume.

## Decision

Add an explicit batch API at each layer, rather than an implicit
background-timer-based batching scheme:

- `Segment::append_batch(&[Record])` writes every record's bytes, then
  calls `sync_data()` exactly once for the whole batch. `append()` is now
  defined in terms of `append_batch` with a one-record slice, so there is
  a single code path and no risk of the two drifting apart.
- `Log::append_batch(Vec<LogOp>)` assigns sequential sequence numbers to
  the whole batch, rolls the segment over at most once if the batch as a
  whole doesn't fit in what's left of the active segment, and delegates
  to `Segment::append_batch`. `LogOp` (`Put`/`Delete`) is the batch's unit
  of work — the same shape `Store` already used internally to decide
  which `Record` variant to build.
- `Store::apply_batch(Vec<WriteOp>)` (`WriteOp` is `LogOp` re-exported
  under the name callers of `Store` see) does the same thing `put`/
  `delete` do to the in-memory index, but for every op in the batch
  against the single `Log::append_batch` result.

A caller who wants per-write durability acknowledgment keeps calling
`put`/`delete`, unchanged, at unchanged cost. A caller who has a batch of
writes ready — a bulk load, a transaction's writes at commit time, a
request handler that accumulated several keys before returning — calls
`apply_batch` once and gets one `fsync` for the whole batch instead of
one per write. This is why it's an explicit API rather than a timer-based
"wait a few ms and coalesce whatever showed up": a caller always controls
exactly which writes share a durability point, so nothing is ever
silently held back longer than the caller intended.

Crash safety needs no new reasoning. If `append_batch` fails or the
process dies partway through writing a batch's bytes, the result on disk
is indistinguishable from any other torn write: `Segment::recover`
already stops at the first invalid/incomplete record and reports the
valid prefix, whether that prefix came from a batch or from N separate
single-record appends. A partially-written batch is simply "not
committed" for every record in it that didn't make it durably — the
existing torn-tail contract (ADR-001) fully covers this.

## Measured result

`benches/append_throughput.rs`'s `store_group_commit` group, sequential
`put` (N `fsync` calls) vs. `apply_batch` (1 `fsync` call) for the same N
writes, both variants measured in the same `cargo bench` run (see the
methodology note in ADR-006 — cross-run absolute numbers drift on this
machine, so only same-run comparisons are trusted):

| writes | sequential `put` | `apply_batch` | speedup |
|---|---|---|---|
| 10 | 17.4 ms | 2.33 ms | ~7.5x |
| 100 | 105.2 ms | 1.86 ms | ~57x |
| 1,000 | 1.25 s | 4.79 ms | ~260x |

The speedup grows with batch size because sequential `put` pays a full
`fsync` per write (roughly constant cost per call, dominating at any
size) while `apply_batch` pays a roughly constant single `fsync` plus
writes whose cost scales with total bytes, not call count. This is the
textbook group-commit result, reproduced with this project's own
implementation and its own numbers rather than assumed from general
knowledge.

## Consequences

- Ticket 008 closes. `Store::apply_batch` is the durability-batching
  primitive; nothing yet automatically groups writes that arrive as
  separate `put`/`delete` calls from independent callers (that would be
  the "background timer" design this ADR deliberately didn't choose) —
  if a future workload wants that, it needs its own ADR, because it
  reintroduces the "how long do we hold a write before flushing it"
  question this design avoided.
- `IndexedStore` does not yet have an `apply_batch` equivalent. Its
  checkpoint-batching (ADR-006) already amortizes its *sentinel* write
  across operations; batching its *data* writes the same way `Store`
  now does is a natural follow-up but is out of scope here — tracked
  separately rather than silently assumed done.
- `WriteOp` is a public re-export of `Log`'s internal `LogOp` type,
  which keeps `Store`'s public API from needing its own parallel
  `Put`/`Delete` enum for exactly the same two variants.
