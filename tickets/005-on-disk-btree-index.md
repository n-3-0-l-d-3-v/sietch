---
status: open
phase: 2
---

# 005 — Disk-oriented B+Tree index

`docs/design/CONSTRAINTS.md` calls for "at least one serious disk-oriented
index: a B+ tree." The current index is an in-memory `BTreeMap` rebuilt
from a full log replay on every open — correct, but doesn't scale past the
point where replay time or memory become a problem (see the
`store_reopen_recovery` benchmark in `crates/storage/benches/append_throughput.rs`,
which already shows replay cost growing with history size).

## Scope
- B+Tree pages built on ticket 004's page format, stored via the append-
  only log (a "modification" is a new page version, old pages become
  reclaimable only through ticket 006's compaction).
- Point lookups and range scans served from the on-disk tree instead of a
  full in-memory replay.
- An ADR comparing B+Tree vs. LSM-tree for this workload, per
  `docs/design/decisions/ADR-000-template.md`.

Not started.
