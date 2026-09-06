---
status: open
phase: 2
---

# 004 — Page format + buffer manager

The full storage architecture in `docs/design/STORAGE.md` calls for
`Application -> Logical objects -> Pages -> Buffer manager -> Append-only
log -> Segments -> Host`. This slice went straight from Store to Log,
skipping pages and a buffer manager (index is rebuilt into memory instead).

## Scope
- Fixed-size page format: header (checksum, version metadata), record
  layout within a page.
- Buffer manager: caching, eviction (start with clock or LRU), pinning,
  dirty-page tracking, with append-only persistence semantics preserved
  (a "dirty" page still becomes a new immutable version on flush, never an
  in-place overwrite of a previously-flushed page).

Not started. Required before ticket 005 (on-disk B+Tree) can be built on
real pages rather than living entirely in memory.
