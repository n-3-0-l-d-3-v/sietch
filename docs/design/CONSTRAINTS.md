# Constraints — THE VAULT

## Primary constraint

Persistent storage may never overwrite an existing byte. A logical update produces a new version; a deletion produces a tombstone; compaction produces a new representation without mutating the old one.

## What it forces

Versioning, MVCC, append-only logs, page/segment design, and a real compaction and reclamation policy.

## Research question

What is the cost of eliminating physical mutation from persistent storage, and where does that complexity move?

## What is explicitly out of scope

See the root [SCOPE.md](../../SCOPE.md) for the CORE / EXTENSION / EXPERIMENT
classification that applies to this repo.
