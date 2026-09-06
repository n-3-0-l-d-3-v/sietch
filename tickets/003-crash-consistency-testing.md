---
status: done
phase: 2
---

# 003 — Crash-consistency and property-based testing

Per `docs/DEFINITION_OF_DONE.md`: simulate write interruption and
corruption directly, don't just unit-test the happy path.

## Acceptance criteria
- [x] Integration test simulating a torn write mid-append (garbage bytes
      appended to a real segment file after the process "restarts").
- [x] Integration test proving writes after recovery are durable and
      correctly ordered relative to pre-crash writes.
- [x] Exhaustive crash-injection test: truncate the real segment file at
      *every* possible byte offset and reopen the store at each cut point,
      asserting no key ever comes back with a corrupted or partially
      applied value. This is the test that must never regress.
- [x] Property-based tests: arbitrary key/value byte strings round-trip
      through record encode/decode; a truncated encoded record never
      decodes as complete; random put/delete sequences replayed through a
      fresh `Store` match a naive in-memory reference model exactly.
- [x] Criterion benchmarks for put throughput, reopen/recovery cost vs.
      history size, and point-read latency — with the fsync-per-write
      latency trade-off explicitly measured and documented (see
      `docs/design/STORAGE.md`), not hidden.
