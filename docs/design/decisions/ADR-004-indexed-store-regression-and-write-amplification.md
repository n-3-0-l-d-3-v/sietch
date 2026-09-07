# ADR-004: Naively wiring the B+Tree in as Store's index made reopen *slower*, not faster — and here's why

## Status
Accepted (as a documented negative result — see Consequences)

## Context

Ticket 011 set out to replace `Store`'s full-log-replay-into-memory index
with the persistent, on-disk `BTree` from ticket 005, on the hypothesis
that reopening a store with a large history would then be fast (the index
is already durable; nothing needs rebuilding) instead of paying an
O(history size) cost on every open.

`IndexedStore` was built exactly that way: `<dir>/log` for the durability
log (unchanged from `Store`), `<dir>/index` for the B+Tree's pages via
`LogPageStore` (itself just another `Store` instance, composed — see
ADR-002). The benchmark ticket 011 explicitly called for
(`benches/indexed_vs_plain_reopen.rs`) was run to *prove* the improvement
rather than assume it.

**The benchmark disproved the hypothesis.** At 10,000 entries, plain
`Store::open` took ~23ms; `IndexedStore::open`, after a *clean* shutdown
requiring zero reconciliation, took ~290ms — over 12x **slower**, not
faster.

## Root cause

Diagnosed directly by measuring on-disk log sizes after 1,000 `put`s:

```
main KV log size:         28,780 bytes
index's internal log size: 7,334,250 bytes   (254.8x larger)
```

Two compounding effects, both direct consequences of earlier, individually
reasonable decisions:

1. **Write amplification from the page-rebuild strategy (ADR-003).**
   Every B+Tree node touched by an insert is fully rebuilt and persisted
   as a **whole new 4096-byte page**, even when the logical change is a
   few bytes. A single `IndexedStore::put` touches the root-to-leaf path
   (typically 1–3 pages at this scale) *plus* a second `index.insert` for
   the reconciliation sentinel key (its own root-to-leaf path) — so one
   logical KV write can produce several page-sized log records.
2. **Nested full-replay (ADR-002's composition, at scale).** `LogPageStore`
   persists pages through a generic `Store`, and `Store::open` *always*
   fully replays its entire log to rebuild its in-memory index — the exact
   cost ticket 011 was trying to eliminate at the outer layer. Composing
   `BTree -> LogPageStore -> Store` therefore doesn't eliminate that cost,
   it **relocates it one level down and multiplies the data volume by the
   page-rebuild amplification factor above.**

Net effect: `IndexedStore::open` pays the *same* replay-the-whole-log
architecture as `Store::open`, just against a log that both correctly and
inescapably in this design ends up 200x+ larger for the same logical
history.

## Decision

1. **Keep `IndexedStore`** — it is correct (crash-injection tests at every
   byte offset, a differential property test against `Store` proving
   identical behavior) and the reconciliation mechanism (catching up an
   index that's behind the log after an unclean shutdown, replaying only
   the unindexed tail) is real, tested, useful infrastructure regardless
   of this result.
2. **Do not claim ticket 011 achieved its goal.** The wiring works; the
   performance hypothesis it was built to prove was tested and falsified.
   Ticket 011 stays open with this finding recorded, rather than being
   marked done on the strength of "it compiles and the tests pass" while
   its own explicit benchmark requirement contradicts the claim.
3. **Open ticket 012**: fix the actual bottleneck (a page-location index
   that doesn't require replaying full page contents to open — e.g. a
   compact, separately-persisted `page_id -> log offset` map, or giving
   `PageStore` its own lightweight recovery format instead of reusing
   generic `Store`) before re-attempting the "prove reopen is fast"
   benchmark.

## Consequences

- **This is exactly the project's own stated research question, answered
  empirically**: "What is the cost of eliminating physical mutation from
  persistent storage, and where does that complexity move?" Here, the
  complexity moved somewhere worse than where it started — composing two
  independently-correct, independently-tested immutable-log-backed
  abstractions naively multiplied a cost rather than amortizing it. That
  is a real, load-bearing finding for this project's design record, not a
  failure to hide.
- **Every other component `IndexedStore` builds on stays exactly as
  validated**: `BTree`'s correctness (ADR-003), `LogPageStore`'s crash
  safety (ADR-002), `Log`'s recovery guarantees (ADR-001) are all
  unaffected — this ADR is about a *composition* cost, not a correctness
  defect in any of them.
- Future work building on pages (ticket 006's compaction, ticket 009's
  B+Tree deletion) should budget for this write-amplification factor when
  estimating log growth, not just the "no compaction yet" growth already
  documented in `docs/design/STORAGE.md`.
