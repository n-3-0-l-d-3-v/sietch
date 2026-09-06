---
status: open
phase: 2
---

# 006 — Compaction

Per `docs/design/CONSTRAINTS.md`: because nothing can be overwritten,
storage grows forever without a compaction/reclamation policy. This slice
has no compaction at all — every put, delete, and superseded version stays
in the log forever.

## Scope
- Distinguish, explicitly and in code: immutable data, obsolete logical
  versions, and physically reclaimable storage (per project philosophy
  section 16).
- Compaction reads old segments and writes a new, compacted representation
  (e.g. only the latest version of each live key) — it must never mutate
  an old segment in place, only ever produce new segments.
- A policy for when old segments become safe to physically delete (no
  snapshot still references them).
- Benchmark: storage overhead before/after compaction as a function of
  update churn, documenting where the "cost of eliminating mutation"
  (project research question, section 74) actually landed.

Not started. This is the ticket that will make the research question in
`docs/design/CONSTRAINTS.md` ("where does complexity move?") concrete and
measured rather than theoretical.
