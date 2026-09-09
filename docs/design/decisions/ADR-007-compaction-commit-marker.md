# ADR-007: Compaction uses a commit marker and a three-step, always-resumable swap

## Status
Accepted

## Context

`docs/design/CONSTRAINTS.md` requires compaction: because storage never
overwrites anything, the log only grows, including every superseded
version and every tombstone. `Store::compact()` needed to rewrite the log
to contain only live data — without ever mutating an existing segment in
place, and without ever leaving storage in a state where a crash could
lose or corrupt committed data.

Two things made this harder than a typical "write new file, rename over
old" swap:

1. This project runs its whole toolchain against Windows, where a
   directory (or a file) cannot be deleted or renamed while any process
   still holds an open handle to it — the `Store`'s own `Log` holds
   exactly such handles on its current segment files for as long as the
   `Store` is alive.
2. A naive "delete old segments, then move new ones in" ordering has a
   real, exploitable failure window: if old and new segments can end up
   sharing the same filenames (which they do — both a fresh `Log` and a
   compacted one number their segments starting at 0), a crash between
   deleting the old files and finishing the move leaves a resumed cleanup
   unable to tell "old, meant to be deleted" apart from "new, meant to be
   kept." An earlier draft of this design had exactly this bug, and it
   was caught by a differential test before shipping (see Consequences).

## Decision

`compact()`:

1. Builds the fully compacted log in a temp directory
   (`.compact-tmp`), from the in-memory index's current live entries
   (latest non-tombstone value per key). If a crash happens here, nothing
   about the real store has been touched yet.
2. Writes a commit marker file (`.compaction-ready`). This is the single
   point of no return: once it exists, recovery is obligated to *finish*
   using the compacted data, not to trust the old segments — even if not
   one byte of the swap has actually happened yet.
3. Releases the `Store`'s own handles on the current segment files
   (required before step 4 can touch them on Windows) by swapping in a
   throwaway placeholder `Log` over a scratch directory for the duration
   of the swap.
4. Performs the actual swap in two further sub-steps, each individually
   resumable:
   - Move every current `seg-*.log` file into a backup directory
     (`.compact-old-backup`) — a no-op if that backup already exists,
     meaning this step already ran on a prior, interrupted attempt.
   - Move every file out of `.compact-tmp` into the real directory —
     naturally idempotent, since an already-moved file is simply no
     longer present in the temp directory to move again.
5. Once both scratch directories are empty, removes them and the commit
   marker — only *now* is the old, superseded data actually deleted.

`Store::open` runs the recovery half of this protocol before doing
anything else: if the commit marker exists, finish the swap (from
whatever step it was interrupted at); if not, discard any leftover temp
directory and let the untouched original segments stand.

## The bug this design caught

The first draft compacted by "delete every `seg-*.log` directly under the
store, then move every file from `.compact-tmp` into place" — no backup
step. A test named `a_crash_after_full_cleanup_except_the_marker_is_still_idempotent`
(a fabricated-but-plausible state: a stray commit marker with the
compaction already fully finished) exposed it immediately: because the
"has step 1 already run?" check was `!tmp_dir.exists() `, and in that
state `tmp_dir` genuinely didn't exist, the recovery code concluded step 1
hadn't happened yet and moved the **already-correct, already-compacted**
current segments into the backup directory — which then got deleted at
the end, destroying the only valid copy of the data. The fix (this ADR's
final design): check for *both* scratch directories being absent as the
"truly nothing to do" case, and otherwise always route through the
backup-then-move-then-cleanup sequence — which is what makes every
intermediate crash point safely resumable in the first place.

## Consequences

- Six dedicated tests simulate a crash at each distinguishable point in
  the protocol (before the marker, right after the marker, mid-backup,
  mid-move, after move but before backup cleanup, and the
  everything-done-but-the-marker edge case that caught the bug above) —
  `crates/storage/src/store.rs`'s `store::tests` module. A property test
  (`tests/compaction_property.rs`) additionally proves compaction never
  changes a store's externally observable `get`/`scan` results for
  arbitrary put/delete sequences.
- **Known limitation, stated plainly**: `compact()` discards *all*
  non-latest versions unconditionally, including ones an outstanding
  `Snapshot` might still be reading from. Compacting while an older
  snapshot is in use will silently make `get_at`/`scan_at` against it
  return incomplete results. Snapshot-aware compaction (retaining
  versions a live snapshot still needs) is real future work, not
  something this design pretends to already handle.
- Measured, not assumed: `benches/compaction.rs` shows `compact()`'s own
  cost is dominated by the fixed swap overhead (directory operations)
  rather than by how much history it processes — the compaction cost for
  a small live key set stays roughly flat whether it followed 100 or
  5,000 churned writes, because it's the swap machinery, not the record
  count, that dominates at these scales.
