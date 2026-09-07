---
status: open
phase: 2
---

# 010 — B+Tree leaf sibling pointers + range scans; LSM-tree comparison ADR

`BTree::scan_all` (ticket 005) is a full recursive in-order traversal:
correct, but O(n) in total live entries rather than O(log n + k) for a
scan bounded to a key range, because leaf nodes don't point to their right
sibling.

## Scope
- Add a sibling pointer to leaf pages (needs a small page-format extension
  or a reserved slot — decide and record as an ADR update).
- `BTree::scan_range(start, end)`: descend once to the starting leaf, then
  walk sibling pointers instead of re-descending from the root.
- Once this exists, write the B+Tree vs. LSM-tree comparison ADR that
  ticket 005 deferred (`docs/design/decisions/ADR-000-template.md`
  format), now grounded in this project's own measured B+Tree numbers
  (`benches/btree.rs`) rather than only generic tradeoffs.

Not started.
