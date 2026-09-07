# ADR-005: HeapPageStore fixes the nested-replay root cause; reopen latency is much better, not yet a clear win

## Status
Accepted (partial fix — see Consequences for what's still open)

## Context

ADR-004 diagnosed why `IndexedStore::open` was ~12x slower than plain
`Store::open` at 10,000 entries: persisting B+Tree pages through a
generic `Store` (`LogPageStore`) meant opening the index paid a *second*
full-log-replay, against a log measured at 254.8x the size of the main KV
log for the same history, due to whole-page-rebuild write amplification
(ADR-003).

Ticket 012 built `HeapPageStore`: page bytes go into a flat, append-only
heap file that is **never scanned or decoded at open time** (only its
length is checked, an O(1) `stat`, truncated to the last whole-page
boundary if a crash left a torn partial page); page *locations* go into a
separate, small log using the existing `Log`/`Record` machinery, but with
tiny fixed-size records (24 bytes: an 8-byte page id key, an 8-byte offset
value, plus the usual 21-byte record header) instead of 4096-byte page
bodies.

## Decision

Wire `IndexedStore` to `HeapPageStore` instead of `LogPageStore`, and
re-measure both the write-amplification and the actual reopen benchmark
before claiming anything.

## Measured results

**Write amplification / replay volume — fixed, decisively:**

```
                          before (LogPageStore)   after (HeapPageStore)
data actually replayed
at open time (1,000 puts): 7,334,250 bytes           65,786 bytes
ratio to main KV log:      254.8x                    2.3x
```

A ~111x reduction in the data `IndexedStore::open` has to read and decode
to become ready. This is exactly the architectural defect ticket 012 set
out to fix, and it is fixed.

**Absolute reopen latency — substantially improved, but not yet a clear
win over plain `Store`** (`benches/indexed_vs_plain_reopen.rs`, 20
samples, 3s measurement window per point):

| entries | plain `Store::open` | `IndexedStore::open` | ratio |
|---|---|---|---|
| 100 | 2.9 ms | 7.9 ms | 2.7x slower |
| 1,000 | 21.2 ms | 109.5 ms | 5.2x slower (see note below) |
| 10,000 | 37.6 ms | 49.9 ms | 1.3x slower |
| 30,000 | 77.7 ms | 100.7 ms | 1.3x slower |

The 1,000-entry point is an outlier relative to the otherwise-shrinking
ratio (2.7x -> ~1.3x as size grows) and was not reproducible on a repeat
run at the same magnitude as the 10,000/30,000 points; it is noted rather
than silently dropped, but the trend the other three points show — the
gap narrowing as history size grows — is the real signal, consistent with
`IndexedStore`'s per-open cost now scaling with the (small) location log
rather than the (large) page log.

**Conclusion: ticket 012's specific scope (eliminate the nested full-page
replay) is done and measured.** Ticket 011's original goal (prove
`IndexedStore` reopens *faster* than plain `Store`) is not yet
demonstrated — at every tested size, `IndexedStore::open` is still slower
in absolute terms, just by a much smaller and shrinking margin than
before this fix.

## Remaining known cost (why it's not a clear win yet)

Every `IndexedStore::put`/`delete` does **two** B+Tree inserts (the user
key, and the reconciliation sentinel key from ADR-004/`apply_and_advance`)
where plain `Store` does one in-memory map insert — and each B+Tree
insert can touch multiple pages along its root-to-leaf path, each of
which is a real `fsync`'d write. This is very likely the dominant
remaining cost, not the (now cheap) replay-at-open path. Reducing this —
e.g. batching the sentinel update instead of writing it on every single
operation — is the next concrete lever, tracked as remaining scope on
ticket 011 rather than a new ticket, since it's a direct continuation of
the same "prove it's actually faster" goal.

## Consequences

- `HeapPageStore` is now the default backing store for `IndexedStore`.
  `LogPageStore` remains available (and is still what `BTree`'s own
  standalone tests in `crates/storage/tests/btree_*.rs` use) — it is not
  wrong, just unsuitable for a workload with `IndexedStore`'s write
  volume; both are legitimate `PageStore` implementations for different
  situations, matching the project's storage-abstraction philosophy.
- Crash safety for `HeapPageStore` was verified with the same rigor as
  every other layer: a torn partial page at the end of the heap file is
  detected (length not a multiple of `PAGE_SIZE`) and truncated, exactly
  analogous to ADR-001's reasoning — no location record can reference an
  uncommitted page, since the location write only happens after the heap
  write's `fsync` completes.
- Ticket 011 stays open, now scoped narrowly to "eliminate the double
  B+Tree insert per operation," rather than the original, larger, and
  already-substantially-addressed "fix the reopen regression."
