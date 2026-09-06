# impossible-vault — THE VAULT

> Immutable, append-only persistent storage. No byte is ever overwritten.

Part of **[The Impossible Computer](https://github.com/n-3-0-l-d-3-v/impossible-computer)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 2 — ACTIVE.** Slice 1 (tickets 001–003) is real and tested: a
checksummed, append-only, multi-segment log with crash recovery that
survives corruption injected at *every* byte offset of a real file (see
`crates/storage/tests/crash_recovery.rs`), and PUT/GET/DELETE/SCAN/SNAPSHOT
primitives with genuine multi-version reads, exposed through the `vaultc`
CLI. See [docs/design/STORAGE.md](docs/design/STORAGE.md) for the
architecture and [tickets/](tickets/) for what's still open — an on-disk
B+Tree index, a buffer manager, compaction, transactions/concurrency, and
group commit (tickets 004–008) — before this phase closes.

## The constraint

Persistent storage may never overwrite an existing byte. A logical update produces a new version; a deletion produces a tombstone; compaction produces a new representation without mutating the old one.

## What the constraint forces

Versioning, MVCC, append-only logs, page/segment design, and a real compaction and reclamation policy.

## Research question

> What is the cost of eliminating physical mutation from persistent storage, and where does that complexity move?

## Sibling repositories

- [impossible-machine](https://github.com/n-3-0-l-d-3-v/impossible-machine) — THE MACHINE (COMPLETE)
- [impossible-language](https://github.com/n-3-0-l-d-3-v/impossible-language) — THE LANGUAGE (QUEUED)
- [impossible-kernel](https://github.com/n-3-0-l-d-3-v/impossible-kernel) — THE KERNEL (QUEUED)
- [impossible-database](https://github.com/n-3-0-l-d-3-v/impossible-database) — THE DATABASE (QUEUED)
- [impossible-wire](https://github.com/n-3-0-l-d-3-v/impossible-wire) — THE WIRE (QUEUED)
- [impossible-colony](https://github.com/n-3-0-l-d-3-v/impossible-colony) — THE COLONY (QUEUED)
- [impossible-history](https://github.com/n-3-0-l-d-3-v/impossible-history) — THE HISTORY (QUEUED)
- [impossible-artifact](https://github.com/n-3-0-l-d-3-v/impossible-artifact) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
