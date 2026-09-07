---
status: done
phase: 2
---

# 005 — Disk-oriented B+Tree index

`docs/design/CONSTRAINTS.md` calls for "at least one serious disk-oriented
index: a B+ tree." See
`docs/design/decisions/ADR-003-btree-page-rebuild-strategy.md` for the core
implementation decision (decode-mutate-rebuild instead of in-place slot
shifting) and its measured cost.

## Acceptance criteria
- [x] B+Tree pages built on ticket 004's page format
      (`crates/storage/src/btree.rs`), persisted via `PageStore` — a node
      "modification" is a fresh page version through the log, never an
      in-place overwrite of a previous one.
- [x] Real node splitting with multi-level growth: 20,000 inserts produce
      a tree at least 3 levels deep (leaf + 2 internal levels), verified
      via a `depth()` diagnostic, with every key still retrievable
      afterward (`tests::many_inserts_force_leaf_and_internal_splits_and_all_keys_survive`).
      This exercises both the leaf-split and the (more subtle)
      internal-node-split path, not just leaves.
- [x] Point lookups (`get`) and a full sorted traversal (`scan_all`) served
      from the on-disk tree.
- [x] Differential testing against `std::collections::BTreeMap` as the
      reference model, both as a hand-written test (800 randomly-ordered
      keys) and as a property test over arbitrary insert sequences with
      duplicate keys / upserts (`tests/btree_property.rs`).
- [x] Crash-injection tests over the real `LogPageStore` backend, proving
      the tree's meta page, root, and every node it creates survive a torn
      write the same way plain pages already do
      (`tests/btree_crash_recovery.rs`).
- [x] Benchmarks for insert cost vs. tree size (both in-memory and over
      the real durable backend) and point-lookup cost
      (`benches/btree.rs`).

## Explicitly deferred (tracked as new tickets, not silently dropped)
- **B+Tree vs. LSM-tree comparison ADR** — the project doc treats LSM as
  optional future work (section 44), and this ticket's job was to ship
  *a* serious disk-oriented index per the constraint, which a B+Tree
  satisfies on its own. A comparison ADR is meaningful once there is a
  second index to compare against, not before — see ticket 010.
- Deletion / node merging / rebalancing — ticket 009.
- Leaf sibling pointers for O(log n + k) bounded range scans instead of
  full O(n) traversal — ticket 010.
- Wiring `BTree` in as `Store`'s actual persistent index, replacing the
  full-log-replay in-memory index — ticket 011. This is the ticket that
  actually delivers on "point lookups... served from the on-disk tree
  instead of a full in-memory replay" for the KV `Store` itself; ticket
  005 built and proved the tree in isolation first.
