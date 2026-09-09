# ADR-008: B+Tree deletion propagates emptiness, not minimum-occupancy rebalancing

## Status
Accepted

## Context

Ticket 009 needed `BTree::delete`. A textbook B+Tree delete redistributes
entries from a sibling, or merges with one, whenever a node drops below
some minimum occupancy threshold (commonly half-full) — keeping every
node reasonably dense. Implementing that correctly requires reading a
node's *siblings* (not just its parent) during delete, deciding
redistribute-vs-merge, and propagating a merge's resulting "one fewer
child" up through the tree exactly the way a split propagates "one more
child" up during insert.

## Decision

`delete` propagates a narrower signal than "underflow": whether a node
became **completely empty**. A leaf that loses its last entry, or an
internal node that loses its last non-leftmost entry while its leftmost
entry has nothing left to promote, reports "empty" to its parent; the
parent then either promotes a sibling into the emptied leftmost slot or
drops the reference to the emptied child outright, and may itself become
empty as a result, propagating further — all the way up to shrinking the
root when it collapses to a single child.

This guarantees the one invariant that matters for correctness: **no node
is ever left with zero live entries**, which would otherwise be a
structurally broken tree (an internal node needs at least its leftmost
pointer to route through; a completely empty leaf serves no purpose but
isn't itself invalid the way an empty *internal* node would be). It does
**not** guarantee any minimum fill factor beyond "not empty" — a node left
with one or two entries after a run of deletes stays exactly that sparse.

## Alternatives Considered

1. **Full minimum-occupancy rebalancing** (redistribute from a sibling,
   or merge two under-full nodes). Textbook-correct and the eventual
   right answer for a production B+Tree, but requires the delete path to
   fetch and potentially rewrite a *sibling* page in addition to the
   node and its parent — real added complexity for a scope this ticket
   didn't need to reach in order to deliver a correct, tested `delete`.
2. **Empty-node propagation only** (chosen). Strictly simpler: every
   propagation decision an internal node makes is local to itself and the
   one child that just reported emptying — no sibling lookups. Correct by
   the invariant above; suboptimal only in fill factor, and only after
   substantial deletion, which is exactly the honestly-documented
   limitation this ADR records rather than hides.

## Consequences

- **Tree can become sparser than a textbook B+Tree** after heavy deletion
  — more pages than strictly necessary for the live key count, each
  holding just one or two entries. This is a real, admitted cost, not
  silently absorbed; it compounds with the "no compaction for pages yet"
  limitation already documented in `docs/design/STORAGE.md` (an emptied
  child page's bytes are never reclaimed either — same page-level GC gap
  ticket 006's compaction didn't touch).
- **Correctness fully covered anyway**: `root_shrinks_after_deletes_collapse_it_to_a_single_child`
  proves the tree keeps working correctly through heavy deletion (2,000
  inserts down to 10 remaining keys), and
  `interleaved_insert_and_delete_matches_a_reference_btreemap` (both a
  hand-written 400-operation test and a proptest property test over
  arbitrary sequences) proves behavior matches `std::collections::BTreeMap`
  exactly regardless of how sparse the tree gets.
- Proper minimum-occupancy rebalancing, if ever needed (e.g. once a real
  workload shows the sparseness actually costs something measurable), is
  a well-scoped, isolated follow-up: the "does this child need
  attention?" signal already exists in `delete_recursive`'s return value,
  it would just need to become richer than a boolean.
