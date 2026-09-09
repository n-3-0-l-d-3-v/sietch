---
status: done
phase: 2
---

# 010 — B+Tree leaf sibling pointers + range scans; LSM-tree comparison ADR

`BTree::scan_all` was a full recursive in-order traversal: correct, but
O(n) in total live entries rather than O(log n + k) for a scan bounded to
a key range.

## Acceptance criteria
- [x] Leaf sibling pointers: slot 0 of every leaf page is reserved for its
      right-sibling page id, using the same `key_len == u16::MAX` sentinel
      trick `InternalEntry`'s leftmost entry already used. Threaded
      correctly through splits (the new right sibling is spliced into the
      existing chain) and left untouched by deletes.
- [x] `BTree::scan_range(start, end)`: descends once to the leaf that
      would contain `start`, then walks sibling pointers instead of
      re-descending from the root.
- [x] **Measured, not assumed**: `benches/btree.rs`'s
      `btree_range_scan_vs_full_scan` shows `scan_range` costing a flat
      ~20–24µs regardless of tree size (1,000 to 50,000 entries), while
      `scan_all` over the same trees grows from ~85µs to ~5.4ms — a
      textbook O(log n + k) vs. O(n) demonstration, with real numbers.
- [x] Correctness: a property test proving `scan_range` matches
      `scan_all` filtered to the same bounds for arbitrary insert
      sequences and arbitrary bounds, plus hand-written tests covering
      half-open bounds, multi-leaf traversal (3,000 entries forcing
      several splits), and post-delete correctness.
- [x] The B+Tree vs. LSM-tree comparison ADR ticket 005 deferred —
      `docs/design/decisions/ADR-009-btree-vs-lsm-tree.md` — grounded in
      this project's own measured numbers (this ticket's range-scan
      benchmark, ADR-003's insert-cost numbers, ADR-007's compaction
      cost) rather than only generic tradeoffs.
