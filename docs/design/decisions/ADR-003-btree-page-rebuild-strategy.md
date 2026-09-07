# ADR-003: B+Tree nodes are decoded, mutated as a Vec, and rebuilt wholesale

## Status
Accepted

## Context

`Page` (ticket 004) supports appending a new slot and marking a slot
deleted, but not inserting a slot at an arbitrary sorted position or
shifting existing slots. A B+Tree node, however, must keep its entries
sorted by key at all times so search and iteration work. Ticket 005 needed
a strategy for keeping a node's on-page entries sorted despite `Page`'s
append-only slot API.

## Decision

Every B+Tree mutation (`insert`, and eventually node merges) works
against a fully decoded, in-memory, sorted `Vec` of that node's entries
(`read_leaf_entries` / `read_internal_entries`). The mutation is applied to
the `Vec`. The node's `Page` is then rebuilt from scratch
(`build_leaf`/`build_internal`): a brand-new `Page` is created and every
entry re-inserted, in the correct sorted order, as a fresh slot. If the
rebuilt page doesn't fit (`PageError::Full`), that's exactly the split
signal — the entry list is divided and two fresh pages are built from the
two halves.

## Alternatives Considered

1. **Give `Page` an in-place "insert at sorted position" operation** that
   shifts existing slot-directory entries and record bytes to make room.
   Rejected for this ticket: it couples `Page` (a generic, B+Tree-agnostic
   primitive used by any future page-oriented structure) to sorted-key
   semantics that are specific to this one use case, and the shifting
   logic is exactly the kind of fiddly, easy-to-get-subtly-wrong code this
   project's testing philosophy (property tests, differential tests
   against a reference model) is best aimed at when it's *necessary* — and
   here it isn't, because a whole-page rebuild is simpler, provably
   correct by construction (a freshly built page from a sorted list is
   sorted), and the cost is bounded.
2. **Decode-mutate-rebuild the whole node on every touch** (chosen).

## Consequences

- **Measured cost, not assumed:** `crates/storage/benches/btree.rs` shows
  per-insert cost growing mildly with tree depth (roughly 7µs at 100
  entries, 22µs at 1,000, 23µs at 10,000) — consistent with "rebuild every
  node touched along the root-to-leaf path," where each rebuild's cost is
  bounded by a page's capacity, not by tree size. This is real, honest
  overhead compared to an in-place slot-shift, and it is the price paid
  for reusing `Page`'s simple, general-purpose API instead of building a
  B+Tree-specific one.
- **Correctness gained for free:** because a page is always rebuilt from a
  known-sorted `Vec`, "is this page's slot order sorted by key" is true by
  construction and never needs its own invariant check or test — the
  differential test against `std::collections::BTreeMap`
  (`tests/btree_property.rs`) only has to check the tree's externally
  observable behavior, not an internal page-layout invariant.
- **Deliberately out of scope for ticket 005 (tracked as follow-up
  tickets, not silently dropped):**
  - No deletion or node merging/rebalancing yet — `BTree` only grows.
    Ticket 009.
  - No leaf sibling pointers, so `scan_all` is a full O(n) recursive
    traversal rather than an O(log n + k) leaf-chain walk for a bounded
    range. Ticket 010.
  - `BTree` is not yet wired in as `Store`'s actual index — `Store` still
    rebuilds an in-memory index by replaying the whole log on open. Ticket
    011 will make `BTree` the real persistent index, which is the payoff
    this ticket was building toward.
