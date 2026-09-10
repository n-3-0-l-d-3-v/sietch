# ADR-012: Snapshot-aware compaction — explicit hold, oldest-snapshot retention, seq-preserving rewrite

## Status
Accepted

## Context

`Store::compact()` (ADR-007, ticket 006) rewrote the log to keep only
each key's current value, unconditionally discarding every earlier
version. Documented as a known limitation ever since: a `Snapshot` taken
before compaction runs would have its `get_at`/`scan_at` calls silently
return wrong results afterward, because the version it needed no longer
existed on disk.

There was a second, undocumented defect discovered while fixing the
first: compaction rewrote the log through the ordinary `Log::append_put`
path, which **reassigns fresh sequential sequence numbers starting from
0** to the surviving records, in `BTreeMap` key order (not original write
order). Every surviving record's `seq` changed on every compaction. This
didn't break `get`/`scan` (which only ever look at the latest version),
but it meant a live `Snapshot`'s `as_of_seq` comparisons were being
checked against completely renumbered, key-ordered sequence numbers
after any compaction — not just "some old versions are gone," but "the
remaining seq numbers no longer mean what they meant when the snapshot
was taken," a strictly worse problem than the one this ticket set out to
fix. It also meant a `Store` kept running (not reopened) after a
compaction could produce **duplicate seq numbers** between old in-memory
`VersionEntry`s (from before compaction, never pruned from memory) and
newly-`put` records (assigned starting from the reset low counter) —
latent, unnoticed because no existing test happened to write, compact,
then write again through the same live `Store` while checking `get_at`
against an old snapshot spanning both.

## Decision

**Both problems share one fix: preserve every surviving record's
original `seq` through compaction, and retain enough versions to serve
every currently-held snapshot.**

- `Log::append_records_verbatim(&[Record])` (new) writes pre-built
  records exactly as given — the caller supplies each `seq` — instead of
  `Log` assigning fresh ones. `compact()` now uses this exclusively, so a
  surviving record's `seq` is always its original one, regardless of
  whether any snapshot is held. This is a strict correctness improvement
  even with no snapshots involved (the duplicate-seq latent bug above is
  gone too), not only the ticket's headline feature.
- `Store::hold_snapshot() -> SnapshotGuard` takes a snapshot and
  registers it in `Store`'s `held_snapshots: Arc<Mutex<BTreeMap<u64,
  usize>>>` (seq -> refcount); dropping the guard unregisters it. This
  answers the ticket's open API question in favor of **explicit
  hold/release** over an implicit retention horizon: a caller that never
  holds a snapshot gets `compact()`'s original behavior (and original
  reclaim ratio) unchanged, and the cost of snapshot-awareness — versions
  compaction can no longer reclaim — is visible and attributable to
  whoever is holding a guard, not a silent, un-tunable "always keep the
  last N seconds" policy.
- `compact()` computes only the **oldest** held snapshot's seq
  (`held_snapshots.keys().next()`, an O(log n) `BTreeMap` lookup) and
  retains, per key: every version at or after that threshold (serves
  every held snapshot, since none is older), plus — if it exists — the
  one version strictly before the threshold (the exact version the
  oldest snapshot's `get_at` would resolve to). A key whose only versions
  are older than the threshold and already tombstoned by the time the
  threshold was reached is dropped entirely, matching the no-snapshot
  case: absence from the index already means `get`/`get_at` return
  `None`, which is exactly what a pre-tombstoned key means to any
  observer.

## Measured / verified result

- `crates/storage/src/store.rs` unit tests: a held snapshot's `get_at`
  is provably unaffected by a `compact()` that runs entirely after it
  (`compacting_while_a_snapshot_is_held_does_not_break_its_reads`); a key
  deleted entirely before the snapshot was taken is still correctly
  dropped (`a_key_deleted_entirely_before_the_held_snapshot_stays_dropped_by_compaction`);
  releasing a guard lets compaction reclaim normally again
  (`releasing_a_snapshot_guard_lets_compaction_reclaim_its_versions_again`).
- `crates/storage/tests/compaction_property.rs`'s
  `compacting_with_a_held_snapshot_never_changes_that_snapshots_reads`:
  the differential test this ticket specifically asked for — arbitrary
  put/delete sequences before and after a held snapshot, compaction run
  in between, snapshot's `get_at` checked against a reference model for
  every key. Passes across 20 randomized cases per run (bounded, like
  the existing compaction property test, because compaction does real
  fsync'd disk I/O).
- `crates/storage/src/log.rs`:
  `append_records_verbatim_preserves_the_given_seqs_and_bumps_next_seq_past_the_max`
  confirms seqs survive a reopen exactly as given, non-contiguous gaps
  included.
- All 89 pre-existing storage unit tests plus the full workspace test
  suite still pass unchanged — the seq-preservation rewrite is a drop-in
  replacement for the old reassign-from-0 path.

## Consequences

- Ticket 013 closes. Phase 2 (THE VAULT)'s ticket backlog (007, 008,
  013 — the three tickets `docs/design/STORAGE.md` and past READMEs
  called out as remaining) is now fully closed.
- An unreleased `SnapshotGuard` permanently caps how much any future
  `compact()` call can reclaim for every key touched since it was taken
  — this is the explicit trade-off the hold/release API makes visible,
  not a hidden cost. A caller that holds a long-lived snapshot across
  heavy write volume should expect compaction to do correspondingly
  less.
- `TransactionalStore`/`Transaction` (ADR-011) now call `hold_snapshot()`
  in `begin()` and keep the guard for the transaction's whole lifetime —
  an open transaction's reads are protected from a concurrent
  `compact()` call on another thread, closing the gap this ADR would
  otherwise have left open.
- `IndexedStore` has no equivalent snapshot-aware compaction — it has no
  compaction at all yet (`docs/design/STORAGE.md`'s "no page-level
  compaction yet" note still applies).
