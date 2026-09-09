---
status: done
phase: 2
---

# 009 — B+Tree deletion, node merging, and rebalancing

`crates/storage/src/btree.rs` only grew: `insert` handled node splits and
root growth, but there was no `delete`. See
`docs/design/decisions/ADR-008-btree-deletion-without-rebalancing.md` for
the scope decision this ticket made (empty-node propagation, not full
minimum-occupancy rebalancing) and why.

## Acceptance criteria
- [x] `BTree::delete(key)`: removes an entry from a leaf, rebuilding the
      page (same decode-mutate-rebuild strategy as insert), returning
      whether the key was found.
- [x] Emptiness propagation: a leaf or internal node that becomes
      completely empty is detected and handled by its parent (promoting a
      sibling into the leftmost slot, or dropping the reference outright),
      propagating further if the parent itself empties out as a result.
- [x] Root shrinking: when the root becomes an internal node with only one
      child, it's replaced by that child — proven at scale (2,000 inserts
      collapsed back down to 10 remaining keys, tree still fully correct).
- [x] Property tests: interleaved random insert/delete sequences match a
      reference `BTreeMap` exactly, both as a hand-written 400-operation
      differential test and as a proptest property test over arbitrary
      sequences (`tests/btree_property.rs`).
- [x] Crash-injection test proving deletes committed before a crash stay
      deleted after reopening through the real `LogPageStore` backend
      (`tests/btree_crash_recovery.rs`).

## Explicitly out of scope (see ADR-008)
- Full minimum-occupancy rebalancing (redistributing from or merging with
  a sibling when a node is under-full but not empty) — the tree can end
  up sparser than a textbook B+Tree after heavy deletion. Correctness is
  unaffected; only fill factor is. A well-scoped future ticket if a real
  workload ever shows this costing something measurable.
- Page-level reclamation of an emptied child's now-unreferenced page —
  same page-level GC gap as the rest of this phase (ticket 006's
  compaction doesn't reach pages either).
