---
status: open
phase: 2
---

# 011 — Wire the B+Tree in as Store's real persistent index

`Store` (tickets 001–003) still rebuilds an in-memory `BTreeMap` index by
replaying the entire log on every open (`store_reopen_recovery` in
`benches/append_throughput.rs` already shows this cost growing with
history size). `BTree` (ticket 005) is a real, tested, on-disk index that
exists but isn't used by `Store` yet.

## Scope
- Replace (or offer as an alternative backend behind a shared trait —
  decide and record as an ADR) `Store`'s in-memory index with `BTree` over
  `LogPageStore`.
- Multi-version reads (`Snapshot`/`get_at`/`scan_at`) need to keep working:
  either the B+Tree stores version chains as its values, or a second
  small index maps key -> version-chain head. Decide and document.
- Benchmark open/recovery time before and after, on a large history, to
  prove this actually fixes the scaling problem it's meant to fix — don't
  just assert it.
- This is a prerequisite for ticket 006 (compaction), since compaction
  needs an efficient way to know which page versions are still reachable
  from the current tree structure.

Not started. This is the ticket that turns ticket 005's B+Tree from "a
tested component that exists" into "the thing `Store` actually uses."
