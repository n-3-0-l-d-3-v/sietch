---
status: open
phase: 2
---

# 009 — B+Tree deletion, node merging, and rebalancing

`crates/storage/src/btree.rs` only grows: `insert` handles node splits and
root growth, but there is no `delete`, so there is no corresponding
merge/redistribute/shrink-the-root logic either.

## Scope
- `BTree::delete(key)`: remove an entry from a leaf, rebuilding the page
  (same decode-mutate-rebuild strategy as insert — see ADR-003).
- Underflow handling: when a node drops below a minimum occupancy after a
  delete, either redistribute entries from a sibling or merge with one,
  propagating the change up (mirroring how insert propagates a split up).
- Root shrinking: when the root becomes an internal node with only one
  child, replace it with that child.
- Property test: interleaved random insert/delete sequences must match a
  reference `BTreeMap` exactly, the same way `tests/btree_property.rs`
  already does for insert-only sequences.

Not started.
