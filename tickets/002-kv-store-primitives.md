---
status: done
phase: 2
---

# 002 — PUT / GET / DELETE / SCAN / SNAPSHOT primitives

Per `docs/design/CONSTRAINTS.md`: start with these primitives before any
relational layer exists. Built entirely on `Log` — see `crates/storage/src/store.rs`.

## Acceptance criteria
- [x] `put`/`get`/`delete` round-trip correctly, including across a
      process restart (full log replay).
- [x] `scan(prefix)` returns only live (non-tombstone) keys with the given
      prefix, in key order, with their current values.
- [x] `snapshot()` + `get_at`/`scan_at` provide multi-version point-in-time
      reads: a snapshot taken before a write/delete is unaffected by it.
- [x] `vaultc` CLI exposes put/get/delete/scan/status over the same API.
