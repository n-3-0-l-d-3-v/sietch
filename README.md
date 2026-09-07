# SIETCH — THE VAULT

> Immutable, append-only persistent storage. No byte is ever overwritten.

## Why "SIETCH"

A hidden Fremen stronghold built around one non-negotiable discipline: guard a precious resource (water), never waste a drop, never let anything already stored be casually touched again. That discipline — nothing already committed gets spent carelessly, everything is accounted for permanently — is the no-overwrite storage constraint in one word.

Part of **[ARRAKIS](https://github.com/n-3-0-l-d-3-v/arrakis)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 2 — ACTIVE.** Tickets 001–005 are done: a checksummed, append-only,
multi-segment log with crash recovery that survives corruption injected at
*every* byte offset of a real file (`crates/storage/tests/crash_recovery.rs`);
PUT/GET/DELETE/SCAN/SNAPSHOT primitives with genuine multi-version reads,
exposed through the `vaultc` CLI; a 4096-byte slotted page format plus a
buffer pool (clock eviction, pinning, dirty tracking); and a real
disk-oriented B+Tree index (`crates/storage/src/btree.rs`) with node
splitting and multi-level growth, verified at 20,000 inserts (3+ tree
levels) and differentially tested against `std::collections::BTreeMap`.

**Ticket 011 produced a negative result worth reading, not a completion.**
Wiring the B+Tree in as `Store`'s index (`IndexedStore`) was built,
crash-tested, and differentially proven correct — but the benchmark it
required showed reopening is ~12x *slower* than plain `Store`, not faster,
because persisting pages through a generic `Store` means the index pays
its own full-log-replay against a log inflated 200x+ by page-rebuild write
amplification. Root cause and decision recorded in
[ADR-004](docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md);
the actual fix is ticket 012.

See [docs/design/STORAGE.md](docs/design/STORAGE.md) for the architecture
and [tickets/](tickets/) for what's still open — the ticket 012 fix,
B+Tree deletion/rebalancing, leaf sibling links for bounded range scans,
compaction, transactions/concurrency, and group commit (tickets 006–012)
— before this phase closes.

## The constraint

Persistent storage may never overwrite an existing byte. A logical update produces a new version; a deletion produces a tombstone; compaction produces a new representation without mutating the old one.

## What the constraint forces

Versioning, MVCC, append-only logs, page/segment design, and a real compaction and reclamation policy.

## Research question

> What is the cost of eliminating physical mutation from persistent storage, and where does that complexity move?

## Sibling repositories

- [mentat](https://github.com/n-3-0-l-d-3-v/mentat) — THE MACHINE (COMPLETE)
- [chakobsa](https://github.com/n-3-0-l-d-3-v/chakobsa) — THE LANGUAGE (QUEUED)
- [muaddib](https://github.com/n-3-0-l-d-3-v/muaddib) — THE KERNEL (QUEUED)
- [choam](https://github.com/n-3-0-l-d-3-v/choam) — THE DATABASE (QUEUED)
- [distrans](https://github.com/n-3-0-l-d-3-v/distrans) — THE WIRE (QUEUED)
- [landsraad](https://github.com/n-3-0-l-d-3-v/landsraad) — THE COLONY (QUEUED)
- [ghola](https://github.com/n-3-0-l-d-3-v/ghola) — THE HISTORY (QUEUED)
- [shai-hulud](https://github.com/n-3-0-l-d-3-v/shai-hulud) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
