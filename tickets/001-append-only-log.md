---
status: done
phase: 2
---

# 001 — Append-only segment log with crash recovery

Record format with checksums, single-segment file abstraction, and a
multi-segment `Log` with recovery-time torn-tail truncation. See
`docs/design/STORAGE.md` and `docs/design/decisions/ADR-001-torn-tail-truncation.md`.

## Acceptance criteria
- [x] Fixed record header (type, seq, key_len, value_len, crc32) with
      checksum covering the whole record; corrupted bytes detected, not
      silently accepted.
- [x] Segment recovery scans from byte 0 and stops cleanly at the first
      incomplete or checksum-mismatched record — never panics on malformed
      input.
- [x] Multi-segment rollover once a size threshold is exceeded; segments
      replay in order on reopen.
- [x] Sequence numbers are monotonic and survive reopen (rebuilt from the
      max seq seen across all recovered records).
- [x] A torn tail on the active segment is truncated on reopen; a torn
      tail on any earlier (sealed) segment is treated as fatal corruption,
      not silently patched.
