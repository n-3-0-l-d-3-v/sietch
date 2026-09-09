# ADR-009: B+Tree over LSM-tree for this project's index, grounded in measured numbers

## Status
Accepted

## Context

`docs/design/CONSTRAINTS.md` asked for "at least one serious disk-oriented
index," naming a B+Tree specifically and calling an LSM-tree optional
future work. Ticket 005 built the B+Tree first on that basis and deferred
this comparison until there was a second, real data point to compare
against — which now exists: `Store`'s own append-only log (tickets
001–003) is, in miniature, exactly the write path of an LSM-tree's
memtable-to-SSTable flow (sequential appends, no in-place updates,
periodic compaction of superseded versions). So this project already
contains a working example of both families' core write strategy, not
just the B+Tree.

## The tradeoff, grounded in this project's own measurements

**Writes.** An LSM-style structure's whole premise is that sequential
appends are cheap and random writes are expensive — `Log::append_put`
(tickets 001–003) already demonstrates the cheap side of that: ~1ms/put
dominated by `fsync`, not seek cost, because it never does a random
write. The B+Tree pays more per write: `benches/btree.rs` shows insert
cost growing from ~7µs to ~23µs per operation as the tree grows from 100
to 10,000 entries (`docs/design/decisions/ADR-003-btree-page-rebuild-strategy.md`),
because a B+Tree write touches and persists every page along a
root-to-leaf path, not just an append.

**Reads.** This is where the B+Tree wins decisively, and ticket 010 just
measured exactly how much: a bounded range scan
(`benches/btree.rs`'s `btree_range_scan_vs_full_scan`) costs ~20–24µs
*regardless of tree size* (1,000 to 50,000 entries) once leaf sibling
pointers exist, because it's O(log n + k) — descend once, then walk
siblings. An LSM-tree's read path, by contrast, has to check the memtable
and potentially every SSTable level (even with bloom filters narrowing
candidates), and a range query has to merge across all of them in sorted
order — a real, structural cost a single B+Tree with sibling links simply
doesn't have to pay, because there is exactly one physical location for
any given point in the key space at any time.

**Compaction.** Both structures need to reclaim space from superseded
versions, but the *purpose* differs. `Store::compact()` (ticket 006,
ADR-007) is a wholesale rewrite from scratch — cheap here because a KV
log's compaction discards almost everything (only the latest version per
key survives) and there's no notion of "levels." An LSM-tree's compaction
is an ongoing, level-merging background process specifically because it
must keep read amplification bounded across many co-existing SSTable
generations — a structural concern this project's single-generation
B+Tree/log designs don't have in the first place.

## Decision

Keep the B+Tree as this project's disk-oriented index, as originally
specified. Do not build an LSM-tree as a second, competing index
implementation — the project's own `Log`/`Store` write path already
demonstrates an LSM-tree's core write strategy (sequential append, no
in-place update, wholesale compaction) well enough to reason about the
tradeoff without a second full implementation existing purely to be
compared against.

## Consequences

- This project's overall design already leans toward the LSM side for
  raw writes (`Log`) and the B+Tree side for reads that need range
  locality (`BTree`/`IndexedStore`) — which is itself an interesting,
  honest data point: the "right" choice depends on which operation a
  given component is actually optimizing for, and this project ended up
  needing both, in different layers, rather than picking one universally.
- If a future phase needs genuinely higher sustained write throughput
  than the B+Tree's per-operation page-touching cost allows (beyond what
  ticket 008's group commit can claw back), building a real LSM-tree
  becomes the concretely-justified next step — this ADR is the record of
  why it wasn't needed yet, not a permanent decision against it.
