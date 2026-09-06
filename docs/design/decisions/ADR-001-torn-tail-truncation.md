# ADR-001: Torn-tail truncation on recovery is not a constraint violation

## Status
Accepted

## Context

The Vault's absolute constraint is: persistent storage may never overwrite
an existing byte. But `Segment::open_for_append` calls `file.set_len(valid_len)`
on open, which — if the physical file is longer than `valid_len` — discards
trailing bytes. Is that "overwriting"?

An unclean shutdown (process killed mid-`write`/`fsync`) can leave a
partially-written record at the end of a segment: a record whose header
says it needs N more bytes than are actually present, or whose checksum
does not match because only part of it landed on disk. No caller was ever
told this record committed — `Segment::append` only returns success after
`sync_data()` completes, and a torn record by definition means that call
never returned (or the whole process died before/during it).

## Decision

On `Log::open`, the last segment is truncated to `valid_len` — the byte
offset of the last complete, checksum-valid record — before any new record
is appended. Every earlier segment must have no torn tail at all (an
interior segment with one is `LogError::SealedSegmentCorrupted`, treated as
real corruption, not recovery).

This is **not** a violation of "never overwrite a committed byte", because:

1. A byte only becomes "committed" once its record is written *and*
   `fsync`'d and the call returns `Ok` to the caller.
2. A torn tail is, by construction, the *absence* of a completed write —
   there is no committed record there to overwrite. Truncating it discards
   bytes that never represented a decodable, checksum-valid unit of data in
   the first place.
3. The project's own design doc makes this distinction explicitly (section
   14, Crash Consistency): "the system must be able to restart and
   determine ... what must be discarded." Discarding an incomplete tail is
   the specified behavior, not an exception to it.

## Alternatives Considered

1. **Never truncate; skip over the gap on every future scan.** Rejected:
   requires persisting a "valid watermark" somewhere durable (itself another
   append-only write with its own torn-write problem), and every future
   `recover()` call would need to re-discover the same gap by re-scanning
   from the start — no simpler, and adds a second source of truth that can
   drift from the segment's actual bytes.
2. **Refuse to reopen a segment with any torn tail; require manual
   operator intervention.** Rejected as needlessly hostile for a
   single-writer, single-process store — real databases (Postgres, SQLite,
   Kafka) all auto-recover past a torn WAL/segment tail exactly this way.
3. **Truncate the last segment's torn tail on open; treat any other
   segment's torn tail as fatal corruption** (chosen). Matches how a
   correctly-functioning single-writer log actually fails: only the
   currently-being-written segment can ever have a torn tail; an older,
   sealed segment should never change after it was sealed, so a torn tail
   there means something else touched the file after the fact — a real
   error worth surfacing loudly (`LogError::SealedSegmentCorrupted`) rather
   than silently "recovering" past.

## Consequences

- `crates/storage/tests/crash_recovery.rs::repeated_crash_injection_never_produces_an_invalid_committed_state`
  is the test that must never regress: truncating the active segment at
  every byte offset and reopening must never expose a corrupted or
  partially-applied value for any key.
- If a future phase adds multi-writer or multi-process access, this ADR's
  "only the last segment may have a torn tail" assumption needs revisiting
  — it currently relies on single-writer, in-order appends.
