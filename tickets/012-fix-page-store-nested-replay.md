---
status: done
phase: 2
---

# 012 — Eliminate LogPageStore's nested full-replay bottleneck

See `docs/design/decisions/ADR-005-heap-page-store-fixes-the-reopen-regression.md`
for the full measured result.

## Acceptance criteria
- [x] `HeapPageStore`: page bytes in a flat, append-only heap file that is
      never scanned/decoded at open time (an O(1) length check, truncated
      to the last whole-page boundary if a crash left a torn partial
      page); page locations in a separate, small log with tiny fixed-size
      records instead of 4096-byte page bodies
      (`crates/storage/src/heap_page_store.rs`).
- [x] Crash-safety proven with the same rigor as every other layer: a
      torn partial page at the end of the heap file is detected and
      truncated; no location record can reference an uncommitted page
      (the location write happens strictly after the heap write's fsync).
- [x] `IndexedStore` rewired to use `HeapPageStore`; the full existing
      crash-injection and differential-property test suites for
      `IndexedStore` still pass unchanged, proving the swap didn't change
      behavior, only the storage mechanics underneath it.
- [x] Re-ran `benches/indexed_vs_plain_reopen.rs` and the write-
      amplification diagnostic from ADR-004: data replayed at open time
      dropped from 254.8x the main log to 2.3x — a ~111x reduction. This
      is the specific bottleneck this ticket targeted, and it's fixed and
      measured, not asserted.

## Honest scope note
This ticket fixed the nested-full-replay architecture defect. It did
**not** fully close the broader question ticket 011 raised (is
`IndexedStore` actually faster to reopen than plain `Store`?) — absolute
reopen latency improved substantially but doesn't yet clearly beat plain
`Store` at the sizes tested. That remaining gap has a different, narrower
cause (two B+Tree inserts per logical write instead of one) and is
tracked as ticket 011's continuing scope, not reopened here.
