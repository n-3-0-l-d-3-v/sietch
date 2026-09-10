# ADR-011: Transactions via Snapshot Isolation, reusing the existing MVCC index

## Status
Accepted

## Context

`Store` has always been single-writer, single-operation: every `put`/
`delete` commits immediately, with no way to group several writes into
one atomic unit, and no notion of "this read and that later write are
part of the same logical operation." `docs/design/CONSTRAINTS.md` calls
for real concurrency — multiple clients reading and writing at once,
deliberately forcing conflicts, retries, and stale-snapshot reads rather
than assuming a toy single-threaded demo is sufficient.

`Store` already has almost everything Snapshot Isolation needs, because
of decisions made for unrelated reasons: `Snapshot`/`get_at`/`scan_at`
(built for point-in-time reads generally, ADR predates this one) give
every key a full version history keyed by sequence number, and
`apply_batch` (ADR-010, ticket 008) gives a way to commit a whole set of
writes atomically and durably with a single `fsync`. The gap was purely
the transaction *protocol* around these: buffering a set of reads/writes,
deciding whether they're allowed to commit, and making that decision
correctly proof against multiple real threads racing each other.

## Decision

**Snapshot Isolation, first-committer-wins, on top of the existing
`Store`, not a redesign of it.**

- `Store` itself is untouched and stays single-writer/un-synchronized —
  changing that would have meant re-deriving crash-safety and
  MVCC-correctness properties this project already spent several tickets
  proving.
- `TransactionalStore` (`crates/storage/src/txn.rs`) wraps a `Store` in
  `Arc<Mutex<Store>>`. It's the thing multiple threads actually hold —
  cloning it clones the `Arc`, not the store.
- `Transaction::begin()` takes a `Store::snapshot()` immediately and
  buffers every subsequent `put`/`delete` purely in memory
  (`BTreeMap<Vec<u8>, Option<Vec<u8>>>` — `None` is a buffered delete).
  `Transaction::get()` checks its own buffer first, then falls back to
  `store.get_at(key, snapshot)` — so a transaction always sees its own
  uncommitted writes plus exactly the state as of when it began, and
  nothing any other transaction commits in between.
- `Transaction::commit()` locks the store once, then for every key the
  transaction wrote checks `Store::latest_seq(key) >= snapshot.as_of_seq`
  — has anyone committed a newer version of this key since I started?
  If any key conflicts, the **whole transaction** aborts (nothing is
  applied) and returns `TxnError::Conflict`. If none conflict, every
  buffered write is applied via one `apply_batch` call — a multi-key
  transaction gets exactly one `fsync`, for free, from ticket 008's work.

This is first-committer-wins, not first-*validator*-wins or a locking
scheme: a transaction never blocks another transaction's reads or writes
while it's open (there are no read locks, and no write locks are taken
until the single, quick, mutex-guarded commit check). The cost is that a
transaction can do arbitrary work against its snapshot and only find out
at commit time that it must retry — which is the standard SI trade-off,
not an oversight; a client that wants a different trade-off (blocking,
lower abort rate under hot contention) would need a different scheme.

**Explicitly not implemented**: full serializability (this permits write
skew — two transactions that each read a value the other writes and each
individually stay within an invariant can jointly violate it — a known,
accepted SI limitation, not a bug), and multi-process/cross-machine
coordination (`Mutex` only works within one process; `Store`'s own
"single-writer" note in `docs/design/STORAGE.md` still applies at the
process level).

## Measured result

`crates/storage/tests/concurrency.rs` runs real OS threads against a
shared `TransactionalStore`:

- **`concurrent_transactions_never_lose_an_update_to_a_shared_counter`**:
  8 threads × 25 increments each to one shared counter key, every
  increment done as read-modify-write-retry-on-conflict. Final value is
  exactly 200 — no increment is ever silently lost to a race, which is
  the specific failure mode write-write conflict detection exists to
  prevent (without it, two threads reading the same stale value and both
  writing `current + 1` would lose one increment).
- **`a_readers_snapshot_is_unaffected_by_concurrent_writer_threads`**: a
  reader's snapshot, taken before 4 writer threads race to update 50
  shared keys, is checked against all 50 keys after the writers finish —
  every read still returns the pre-writer value, proving snapshot
  isolation holds under genuine thread contention, not just in a
  single-threaded unit test.
- **`concurrent_transactions_on_disjoint_keys_all_succeed`**: 8 threads
  each owning a distinct key never conflict with each other, confirming
  the conflict check is scoped to actually-overlapping keys, not
  something coarser (e.g. a whole-store version counter, which would
  have made every commit conflict with every other).

`crates/storage/tests/txn_property.rs` adds proptest coverage: arbitrary
sequential transaction sequences match a naive in-memory model exactly,
and a snapshot taken at an arbitrary point in an arbitrary sequence of
later-committed transactions never observes any of them.

## Consequences

- Ticket 007 closes for its core scope (multi-operation transactions,
  Snapshot Isolation, write-write conflict detection, a real concurrent-
  client test harness).
- `choam` (Phase 6, the database layer) can build on `TransactionalStore`
  directly rather than re-deriving MVCC transaction semantics from
  scratch — this was explicitly called out as the reason to do this
  ticket now rather than defer it further.
- Full serializability and multi-process coordination remain open
  questions if a future workload needs them — tracked as known scope
  boundaries here rather than silently assumed equivalent to what's
  built.
- `IndexedStore` has no transactional wrapper yet; `TransactionalStore`
  only wraps plain `Store`. Extending it to `IndexedStore` is a natural
  follow-up, not yet started.
