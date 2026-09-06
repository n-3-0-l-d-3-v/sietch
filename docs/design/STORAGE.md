# THE VAULT — Storage Specification (Phase 2, slice 1)

## Architecture (this slice)

```text
Application (vaultc / Store API)
        |
   Store (PUT/GET/DELETE/SCAN/SNAPSHOT, in-memory multi-version index)
        |
     Log (multi-segment append-only log, recovery, rollover)
        |
   Segment (single append-only file, checksummed records)
        |
   Host filesystem
```

This is deliberately the first slice of the full architecture in the
project's design doc (`Application -> Logical objects -> Pages -> Buffer
manager -> Append-only log -> Segments -> Host storage`). Pages, a buffer
manager, and a disk-oriented B+Tree index are **not yet built** — see
tickets 005/006. The in-memory index here is a stand-in that is correct but
does not scale to data larger than memory; that trade is explicit, not
hidden.

## The constraint, concretely

**Persistent storage may never overwrite an existing, committed byte.**
Every logical write is a new, immutable, checksummed record appended to the
active segment. A logical update to a key does not touch any prior record
for that key — it appends a new version. A deletion appends a tombstone
record, not a byte-level erasure.

## Record format

See `crates/storage/src/record.rs` module docs for the exact byte layout.
Every record carries a CRC32 checksum over its own header + payload, so
corruption is *detected*, not just assumed absent.

## Crash recovery

On `Log::open`, every segment is replayed from its first byte. Decoding
stops at the first `Incomplete` (truncated) or `ChecksumMismatch` record —
whichever comes first — and everything before that point is the recovered,
committed state. See `docs/design/decisions/ADR-001-torn-tail-truncation.md`
for exactly what happens to the bytes after that point and why it does not
violate the no-overwrite constraint.

`crates/storage/tests/crash_recovery.rs` proves this by literally truncating
a real segment file at *every* possible byte offset and reopening the store
at each cut point: whatever key is visible afterward always has the correct
value, never a partially-written or corrupted one.

## Versioning and snapshots

Every record carries a monotonically increasing `seq` (assigned by `Log`,
persisted, and rebuilt from the log on reopen). The in-memory index keeps
every version of every key (`Vec<VersionEntry>`), not just the latest. A
`Snapshot` is just a captured `seq` value; `get_at`/`scan_at` walk backward
through a key's version list to find the version visible as of that
`seq`. This is genuine multi-version concurrency control in miniature —
not a full transaction system yet (no write-write conflict detection,
single-writer only in this slice — see ticket 007).

## What this slice does not claim

- No on-disk index (B+Tree) yet — the index is rebuilt into memory on every
  open by replaying the whole log. Fine for now; will not scale past the
  point where the log no longer fits comfortably in memory-rebuild time.
  Measured in `crates/storage/benches/append_throughput.rs`
  (`store_reopen_recovery`).
- No compaction yet. Because nothing is ever overwritten, the log only
  grows — including superseded versions and tombstones. Ticket 006.
- Single-writer only; no locking or multi-process coordination.
- `fsync` per record makes every write durable but limits throughput
  (measured at ~1ms/put on this machine, dominated by fsync latency, not
  CPU). Group-commit / batched fsync is a natural future optimization,
  deliberately not done yet — see ticket 008.
