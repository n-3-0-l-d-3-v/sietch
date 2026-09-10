//! The Vault's primitive interface: PUT / GET / DELETE / SCAN / SNAPSHOT,
//! per `docs/design/CONSTRAINTS.md` ("start with these before any
//! relational layer exists"). Built entirely on `Log` — every version of
//! every key is an immutable record; nothing is ever mutated in place.
//!
//! Compaction (ticket 006) is the one operation here that touches more
//! than "append a record": see `compact()` and
//! `docs/design/decisions/ADR-007-compaction-commit-marker.md` for how it
//! stays crash-safe without ever mutating a segment in place.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::log::{Log, LogError, LogOp};
use crate::record::{Record, RecordType};

/// One write to include in a batch — see `Store::apply_batch` (ticket 008
/// — group commit).
pub type WriteOp = LogOp;

const COMPACT_TMP_DIR: &str = ".compact-tmp";
const COMPACT_READY_MARKER: &str = ".compaction-ready";
const COMPACT_OLD_BACKUP_DIR: &str = ".compact-old-backup";
const COMPACT_PLACEHOLDER_DIR: &str = ".compact-placeholder";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Log(#[from] LogError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// What a completed compaction did, so callers can observe it rather than
/// trust it blindly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionReport {
    pub live_keys: usize,
    pub segments_before: usize,
    pub segments_after: usize,
}

#[derive(Debug, Clone)]
struct VersionEntry {
    seq: u64,
    /// `None` is a tombstone (the key was deleted as of this version).
    value: Option<Vec<u8>>,
}

/// A point-in-time read handle: a snapshot sees every version committed
/// strictly before it was taken, and nothing committed afterward — this is
/// what "SNAPSHOT" means for this store (multi-version, not a copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Snapshot {
    as_of_seq: u64,
}

impl Snapshot {
    /// Exposed for `txn::Transaction`'s conflict check — the sequence
    /// number this snapshot was taken at.
    pub(crate) fn as_of_seq(&self) -> u64 {
        self.as_of_seq
    }
}

/// A held reference to a `Snapshot` that keeps `compact()` from discarding
/// versions it still needs (ticket 013). Dropping the guard releases the
/// hold; a `Store` with no held guards compacts exactly as before (only
/// each key's current value survives).
pub struct SnapshotGuard {
    snapshot: Snapshot,
    registry: Arc<Mutex<BTreeMap<u64, usize>>>,
}

impl SnapshotGuard {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        if let Some(count) = registry.get_mut(&self.snapshot.as_of_seq) {
            *count -= 1;
            if *count == 0 {
                registry.remove(&self.snapshot.as_of_seq);
            }
        }
    }
}

pub struct Store {
    dir: PathBuf,
    log: Log,
    index: BTreeMap<Vec<u8>, Vec<VersionEntry>>,
    /// True if the most recent `open` had to discard a torn tail — exposed
    /// so callers/tests can assert recovery behavior rather than just
    /// trusting it silently happened.
    pub recovered_from_torn_tail: bool,
    /// `as_of_seq -> number of live SnapshotGuards holding it`. `compact()`
    /// consults this to find the oldest snapshot it must not invalidate.
    held_snapshots: Arc<Mutex<BTreeMap<u64, usize>>>,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        finish_or_discard_pending_compaction(&dir)?;

        let (log, report) = Log::open(&dir)?;
        let mut index: BTreeMap<Vec<u8>, Vec<VersionEntry>> = BTreeMap::new();
        for record in report.records {
            let value = match record.record_type {
                crate::record::RecordType::Put => Some(record.value),
                crate::record::RecordType::Delete => None,
            };
            index.entry(record.key).or_default().push(VersionEntry {
                seq: record.seq,
                value,
            });
        }
        Ok(Self {
            dir,
            log,
            index,
            recovered_from_torn_tail: report.recovered_from_torn_tail,
            held_snapshots: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Takes a snapshot and holds it open against future compactions:
    /// while this guard (or any other guard on the same or an older
    /// snapshot) is alive, `compact()` preserves every version a held
    /// snapshot could still need, not just each key's current value. Drop
    /// the guard when done with it — an unreleased guard permanently caps
    /// how much compaction can reclaim.
    pub fn hold_snapshot(&self) -> SnapshotGuard {
        let snapshot = self.snapshot();
        let mut registry = self.held_snapshots.lock().unwrap();
        *registry.entry(snapshot.as_of_seq).or_insert(0) += 1;
        SnapshotGuard {
            snapshot,
            registry: self.held_snapshots.clone(),
        }
    }

    fn oldest_held_snapshot_seq(&self) -> Option<u64> {
        self.held_snapshots.lock().unwrap().keys().next().copied()
    }

    /// Rewrites the log to hold only the versions still reachable: each
    /// key's current value always survives, and if any `SnapshotGuard` is
    /// held, every version at or after the *oldest* held snapshot's
    /// sequence number survives too, plus (for each key) the one version
    /// that snapshot itself would read — the latest version strictly
    /// before it. A key with nothing to retain (fully tombstoned, and
    /// created no earlier than the oldest held snapshot so nothing could
    /// have seen it as live) is dropped entirely, same as before.
    ///
    /// Surviving records keep their **original** sequence numbers (via
    /// `Log::append_records_verbatim`) rather than being renumbered —
    /// renumbering would silently scramble every held snapshot's
    /// before/after ordering. Never mutates an existing segment: the
    /// replacement log is built completely in a temporary directory,
    /// committed via a marker file, and only then swapped in — see
    /// `docs/design/decisions/ADR-007-compaction-commit-marker.md` for why
    /// this is crash-safe at every point, and
    /// `docs/design/decisions/ADR-012-snapshot-aware-compaction.md` for
    /// this ticket's retention design.
    pub fn compact(&mut self) -> Result<CompactionReport, StoreError> {
        let segments_before = self.log.segment_count();
        let retain_from = self.oldest_held_snapshot_seq();

        let mut records: Vec<Record> = Vec::new();
        let mut live_keys = 0usize;
        for (key, versions) in self.index.iter() {
            let retained: Vec<&VersionEntry> = match retain_from {
                None => versions
                    .last()
                    .into_iter()
                    .filter(|v| v.value.is_some())
                    .collect(),
                Some(threshold) => {
                    let boundary = versions.iter().rev().find(|v| v.seq < threshold);
                    let tail = versions.iter().filter(|v| v.seq >= threshold);
                    boundary.into_iter().chain(tail).collect()
                }
            };
            if retained.is_empty() {
                continue;
            }
            if retained.last().unwrap().value.is_some() {
                live_keys += 1;
            }
            for v in retained {
                records.push(match &v.value {
                    Some(val) => Record::put(v.seq, key.clone(), val.clone()),
                    None => Record::delete(v.seq, key.clone()),
                });
            }
        }
        records.sort_by_key(|r| r.seq);

        let tmp_dir = self.dir.join(COMPACT_TMP_DIR);
        if tmp_dir.exists() {
            fs::remove_dir_all(&tmp_dir)?;
        }
        fs::create_dir_all(&tmp_dir)?;
        {
            let (mut tmp_log, _) = Log::open(&tmp_dir)?;
            tmp_log.append_records_verbatim(&records)?;
            // `tmp_log` drops here: every record it holds was already
            // fsync'd on append, and dropping releases its file handles,
            // which the swap below needs (Windows will not let us delete
            // or rename files that are still open).
        }

        // Commit point: once this marker exists, recovery must finish
        // the swap using `tmp_dir` rather than trusting the old segments,
        // even if we crash before a single old file is touched.
        fs::write(self.dir.join(COMPACT_READY_MARKER), b"")?;

        // Release this Store's own handles on the old segment files
        // before the swap touches them (required on Windows) by
        // swapping in a throwaway placeholder Log over a scratch
        // directory for the duration of the swap.
        let placeholder_dir = self.dir.join(COMPACT_PLACEHOLDER_DIR);
        if placeholder_dir.exists() {
            fs::remove_dir_all(&placeholder_dir)?;
        }
        fs::create_dir_all(&placeholder_dir)?;
        let (placeholder_log, _) = Log::open(&placeholder_dir)?;
        drop(std::mem::replace(&mut self.log, placeholder_log));

        finish_pending_compaction(&self.dir)?;

        let (log, _report) = Log::open(&self.dir)?;
        let segments_after = log.segment_count();
        self.log = log; // drops the placeholder Log, releasing its handles
        fs::remove_dir_all(&placeholder_dir)?;

        Ok(CompactionReport {
            live_keys,
            segments_before,
            segments_after,
        })
    }

    pub fn put(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), StoreError> {
        let key = key.into();
        let record = self.log.append_put(key.clone(), value.into())?;
        self.index.entry(key).or_default().push(VersionEntry {
            seq: record.seq,
            value: Some(record.value),
        });
        Ok(())
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Result<(), StoreError> {
        let key = key.into();
        let record = self.log.append_delete(key.clone())?;
        self.index.entry(key).or_default().push(VersionEntry {
            seq: record.seq,
            value: None,
        });
        Ok(())
    }

    /// Applies every op in `ops` with a single trailing `fsync` instead of
    /// one per op (ticket 008 — group commit). Semantically identical to
    /// calling `put`/`delete` once per op in order — only the durability
    /// cost changes. See `docs/design/decisions/ADR-010-group-commit.md`
    /// for the measured throughput/latency trade this makes, and why it's
    /// an explicit batch API rather than a background timer: a caller
    /// always controls exactly which writes share a durability point.
    pub fn apply_batch(&mut self, ops: Vec<WriteOp>) -> Result<(), StoreError> {
        let keys: Vec<Vec<u8>> = ops
            .iter()
            .map(|op| match op {
                LogOp::Put(k, _) => k.clone(),
                LogOp::Delete(k) => k.clone(),
            })
            .collect();
        let records = self.log.append_batch(ops)?;
        for (key, record) in keys.into_iter().zip(records) {
            let value = match record.record_type {
                RecordType::Put => Some(record.value),
                RecordType::Delete => None,
            };
            self.index.entry(key).or_default().push(VersionEntry {
                seq: record.seq,
                value,
            });
        }
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.index
            .get(key)
            .and_then(|versions| versions.last())
            .and_then(|v| v.value.clone())
    }

    /// Reads `key` as it existed at `snapshot` — the latest version
    /// committed strictly before the snapshot was taken.
    pub fn get_at(&self, key: &[u8], snapshot: Snapshot) -> Option<Vec<u8>> {
        let versions = self.index.get(key)?;
        versions
            .iter()
            .rev()
            .find(|v| v.seq < snapshot.as_of_seq)
            .and_then(|v| v.value.clone())
    }

    /// Every live (non-tombstone) key currently starting with `prefix`,
    /// with its current value, in key order.
    pub fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, versions)| {
                versions
                    .last()
                    .and_then(|v| v.value.clone())
                    .map(|val| (k.clone(), val))
            })
            .collect()
    }

    /// Same as `scan`, but as of a snapshot rather than the live state.
    pub fn scan_at(&self, prefix: &[u8], snapshot: Snapshot) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, versions)| {
                versions
                    .iter()
                    .rev()
                    .find(|v| v.seq < snapshot.as_of_seq)
                    .and_then(|v| v.value.clone())
                    .map(|val| (k.clone(), val))
            })
            .collect()
    }

    /// Captures the current point in the log's history. Every write that
    /// happens after this call is invisible to reads made through the
    /// returned `Snapshot`.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            as_of_seq: self.log.next_seq(),
        }
    }

    /// The sequence number of `key`'s most recently committed version, if
    /// it has one — the write-write conflict check `txn::Transaction`
    /// needs at commit time (has anyone committed a newer version of this
    /// key since my snapshot was taken?), exposed here rather than
    /// duplicating `Store`'s index layout in `txn.rs`.
    pub(crate) fn latest_seq(&self, key: &[u8]) -> Option<u64> {
        self.index
            .get(key)
            .and_then(|versions| versions.last())
            .map(|v| v.seq)
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Called from `Store::open`: if a previous `compact()` crashed after its
/// commit marker was written but before the swap finished, finish the
/// swap now. If the marker is absent, no compaction was ever committed —
/// discard any leftover scratch directories and let the untouched
/// original segments stand.
fn finish_or_discard_pending_compaction(dir: &Path) -> Result<(), StoreError> {
    let marker = dir.join(COMPACT_READY_MARKER);
    if marker.exists() {
        finish_pending_compaction(dir)?;
    } else {
        remove_dir_if_exists(&dir.join(COMPACT_TMP_DIR))?;
    }
    remove_dir_if_exists(&dir.join(COMPACT_PLACEHOLDER_DIR))?;
    Ok(())
}

/// Performs the actual swap in three steps, each individually resumable
/// from a crash at any point, because after step 1 there is never a
/// moment where "old" and "new" segment files share the same directory
/// under the same names (which would make a resumed cleanup unable to
/// tell them apart — see
/// `docs/design/decisions/ADR-007-compaction-commit-marker.md`):
///
/// 1. Move every current `seg-*.log` file directly under `dir` into a
///    backup directory (no-op if that backup already exists — meaning
///    step 1 already completed on a prior, interrupted attempt).
/// 2. Move every file out of the compacted-log temp directory into
///    `dir` (naturally idempotent: already-moved files are simply no
///    longer present in the temp directory to move again).
/// 3. Once both scratch directories are empty, remove them and the
///    commit marker — only now is the old data actually deleted.
///
/// If neither the temp directory nor the backup directory exist, there is
/// nothing left to migrate (a real `compact()` call always removes both
/// together with the marker at the very end) — step 1 must *not* run in
/// that case, or it would mistake the already-correct current segments
/// for "old" ones and destroy them on cleanup. This only clears a stray
/// marker.
fn finish_pending_compaction(dir: &Path) -> Result<(), StoreError> {
    let tmp_dir = dir.join(COMPACT_TMP_DIR);
    let backup_dir = dir.join(COMPACT_OLD_BACKUP_DIR);

    if !tmp_dir.exists() && !backup_dir.exists() {
        remove_if_exists(&dir.join(COMPACT_READY_MARKER))?;
        return Ok(());
    }

    if !backup_dir.exists() {
        fs::create_dir_all(&backup_dir)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("seg-") && name_str.ends_with(".log") {
                fs::rename(entry.path(), backup_dir.join(&name))?;
            }
        }
    }

    if tmp_dir.exists() {
        for entry in fs::read_dir(&tmp_dir)? {
            let entry = entry?;
            let dest = dir.join(entry.file_name());
            fs::rename(entry.path(), dest)?;
        }
    }

    remove_dir_if_exists(&tmp_dir)?;
    remove_dir_if_exists(&backup_dir)?;
    remove_if_exists(&dir.join(COMPACT_READY_MARKER))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_then_get_round_trips() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
    }

    #[test]
    fn delete_makes_get_return_none() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get(b"a"), None);
    }

    #[test]
    fn apply_batch_has_the_same_effect_as_the_same_ops_applied_one_at_a_time() {
        let dir_batched = tempdir().unwrap();
        let mut batched = Store::open(dir_batched.path()).unwrap();
        batched
            .apply_batch(vec![
                WriteOp::Put(b"a".to_vec(), b"1".to_vec()),
                WriteOp::Put(b"b".to_vec(), b"2".to_vec()),
                WriteOp::Delete(b"a".to_vec()),
            ])
            .unwrap();

        let dir_sequential = tempdir().unwrap();
        let mut sequential = Store::open(dir_sequential.path()).unwrap();
        sequential.put("a", "1").unwrap();
        sequential.put("b", "2").unwrap();
        sequential.delete("a").unwrap();

        assert_eq!(batched.get(b"a"), sequential.get(b"a"));
        assert_eq!(batched.get(b"b"), sequential.get(b"b"));
        assert_eq!(batched.get(b"a"), None);
        assert_eq!(batched.get(b"b"), Some(b"2".to_vec()));
    }

    #[test]
    fn apply_batch_of_empty_ops_is_a_no_op() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.apply_batch(vec![]).unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
    }

    #[test]
    fn apply_batch_survives_reopen_exactly_like_sequential_writes() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store
                .apply_batch(vec![
                    WriteOp::Put(b"x".to_vec(), b"1".to_vec()),
                    WriteOp::Put(b"y".to_vec(), b"2".to_vec()),
                    WriteOp::Delete(b"x".to_vec()),
                ])
                .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"x"), None);
        assert_eq!(store.get(b"y"), Some(b"2".to_vec()));
        assert!(!store.recovered_from_torn_tail);
    }

    #[test]
    fn apply_batch_is_visible_to_scan_and_snapshot_like_individual_writes() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("user:1", "alice").unwrap();
        let snap = store.snapshot();

        store
            .apply_batch(vec![
                WriteOp::Put(b"user:2".to_vec(), b"bob".to_vec()),
                WriteOp::Put(b"user:3".to_vec(), b"carol".to_vec()),
            ])
            .unwrap();

        // The snapshot taken before the batch must not see it.
        assert_eq!(
            store.scan_at(b"user:", snap),
            vec![(b"user:1".to_vec(), b"alice".to_vec())]
        );
        // A fresh read sees every record the batch wrote, in order.
        assert_eq!(
            store.scan(b"user:"),
            vec![
                (b"user:1".to_vec(), b"alice".to_vec()),
                (b"user:2".to_vec(), b"bob".to_vec()),
                (b"user:3".to_vec(), b"carol".to_vec()),
            ]
        );
    }

    #[test]
    fn snapshot_isolates_reads_from_later_writes() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        let snap = store.snapshot();
        store.put("a", "2").unwrap();
        store.delete("a").unwrap();

        assert_eq!(store.get_at(b"a", snap), Some(b"1".to_vec()));
        assert_eq!(store.get(b"a"), None);
    }

    #[test]
    fn scan_returns_live_keys_with_prefix_in_order() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("user:1", "alice").unwrap();
        store.put("user:2", "bob").unwrap();
        store.put("order:1", "widget").unwrap();
        store.delete("user:2").unwrap();

        let results = store.scan(b"user:");
        assert_eq!(results, vec![(b"user:1".to_vec(), b"alice".to_vec())]);
    }

    #[test]
    fn reopening_after_writes_recovers_full_state() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.put("b", "2").unwrap();
            store.delete("a").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), None);
        assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
        assert!(!store.recovered_from_torn_tail);
    }

    #[test]
    fn compact_preserves_live_values_and_drops_tombstoned_keys() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.put("a", "2").unwrap(); // superseded version, should be dropped
        store.put("b", "keep").unwrap();
        store.put("c", "gone").unwrap();
        store.delete("c").unwrap(); // tombstoned, should be dropped entirely

        let report = store.compact().unwrap();
        assert_eq!(report.live_keys, 2); // "a" and "b"

        assert_eq!(store.get(b"a"), Some(b"2".to_vec()));
        assert_eq!(store.get(b"b"), Some(b"keep".to_vec()));
        assert_eq!(store.get(b"c"), None);
    }

    #[test]
    fn compacting_while_a_snapshot_is_held_does_not_break_its_reads() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.put("b", "keep").unwrap();

        let guard = store.hold_snapshot();
        let snapshot = guard.snapshot();

        // Everything below happens strictly after the snapshot was taken.
        store.put("a", "2").unwrap();
        store.put("c", "new-after-snapshot").unwrap();
        store.delete("b").unwrap();

        store.compact().unwrap();

        // The held snapshot must still see exactly what it saw before
        // compaction touched anything.
        assert_eq!(store.get_at(b"a", snapshot), Some(b"1".to_vec()));
        assert_eq!(store.get_at(b"b", snapshot), Some(b"keep".to_vec()));
        assert_eq!(store.get_at(b"c", snapshot), None);

        // The live (unsnapshotted) view reflects everything, as normal.
        assert_eq!(store.get(b"a"), Some(b"2".to_vec()));
        assert_eq!(store.get(b"b"), None);
        assert_eq!(store.get(b"c"), Some(b"new-after-snapshot".to_vec()));
    }

    #[test]
    fn a_key_deleted_entirely_before_the_held_snapshot_stays_dropped_by_compaction() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.delete("a").unwrap();

        // The snapshot is taken after "a" was already deleted — it never
        // saw "a" as live, so compaction owes it nothing for that key.
        let guard = store.hold_snapshot();
        let snapshot = guard.snapshot();

        store.compact().unwrap();

        assert_eq!(store.get_at(b"a", snapshot), None);
        assert_eq!(store.get(b"a"), None);
    }

    #[test]
    fn releasing_a_snapshot_guard_lets_compaction_reclaim_its_versions_again() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();

        {
            let _guard = store.hold_snapshot();
            store.put("a", "2").unwrap();
            // _guard drops here, releasing the hold before compact() runs.
        }

        store.put("a", "3").unwrap();
        let report = store.compact().unwrap();
        assert_eq!(report.live_keys, 1);
        assert_eq!(store.get(b"a"), Some(b"3".to_vec()));

        // Reopening rebuilds the index straight from the compacted log —
        // if any now-unneeded superseded version had survived, this would
        // still return the right answer, but the disk footprint wouldn't
        // have shrunk (covered by `compaction_actually_reduces_stored_history`).
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), Some(b"3".to_vec()));
    }

    #[test]
    fn compacted_state_survives_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            for i in 0..20u32 {
                store.put(format!("k{i}"), format!("v{i}")).unwrap();
            }
            for i in 0..10u32 {
                store.delete(format!("k{i}")).unwrap();
            }
            store.compact().unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        for i in 0..10u32 {
            assert_eq!(
                store.get(format!("k{i}").as_bytes()),
                None,
                "k{i} should have stayed deleted after compaction"
            );
        }
        for i in 10..20u32 {
            assert_eq!(
                store.get(format!("k{i}").as_bytes()),
                Some(format!("v{i}").into_bytes())
            );
        }
    }

    #[test]
    fn compaction_actually_reduces_stored_history() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        // Many overwrites of the same small key set: lots of superseded
        // history that compaction should be able to discard.
        for round in 0..200u32 {
            store.put("k", format!("v{round}")).unwrap();
        }
        let before: u64 = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        store.compact().unwrap();
        let after: u64 = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(
            after < before,
            "compaction should shrink on-disk size (before={before}, after={after})"
        );
        assert_eq!(store.get(b"k"), Some(b"v199".to_vec()));
    }

    #[test]
    fn you_can_keep_writing_after_compaction() {
        let dir = tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put("a", "1").unwrap();
        store.compact().unwrap();
        store.put("b", "2").unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
        assert_eq!(store.get(b"b"), Some(b"2".to_vec()));

        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
        assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
    }

    #[test]
    fn a_crash_before_the_commit_marker_leaves_the_original_state_intact() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.put("b", "2").unwrap();
        }
        // Simulate a crash mid-compaction: the tmp log got written, but
        // the commit marker never did.
        let tmp_dir = dir.path().join(COMPACT_TMP_DIR);
        fs::create_dir_all(&tmp_dir).unwrap();
        {
            let (mut tmp_log, _) = Log::open(&tmp_dir).unwrap();
            tmp_log
                .append_put(b"a".to_vec(), b"WRONG".to_vec())
                .unwrap();
        }
        assert!(!dir.path().join(COMPACT_READY_MARKER).exists());

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.get(b"a"),
            Some(b"1".to_vec()),
            "uncommitted compaction attempt must not affect recovered state"
        );
        assert_eq!(store.get(b"b"), Some(b"2".to_vec()));
        assert!(
            !tmp_dir.exists(),
            "leftover uncommitted tmp dir should be cleaned up on open"
        );
    }

    #[test]
    fn a_crash_after_the_commit_marker_finishes_the_swap_on_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.put("b", "2").unwrap();
        }
        // Simulate a crash right after the commit marker was written:
        // build a valid compacted tmp log and write the marker, but never
        // run the actual swap.
        let tmp_dir = dir.path().join(COMPACT_TMP_DIR);
        fs::create_dir_all(&tmp_dir).unwrap();
        {
            let (mut tmp_log, _) = Log::open(&tmp_dir).unwrap();
            tmp_log
                .append_put(b"a".to_vec(), b"COMPACTED".to_vec())
                .unwrap();
        }
        fs::write(dir.path().join(COMPACT_READY_MARKER), b"").unwrap();

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.get(b"a"),
            Some(b"COMPACTED".to_vec()),
            "a committed-but-unswapped compaction must be finished on next open"
        );
        assert_eq!(
            store.get(b"b"),
            None,
            "the compacted log is authoritative once committed, even if it dropped a key"
        );
        assert!(!dir.path().join(COMPACT_READY_MARKER).exists());
        assert!(!tmp_dir.exists());
        assert!(!dir.path().join(COMPACT_OLD_BACKUP_DIR).exists());
    }

    #[test]
    fn a_crash_mid_swap_after_backup_but_before_move_still_finishes_correctly() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
        }
        let tmp_dir = dir.path().join(COMPACT_TMP_DIR);
        fs::create_dir_all(&tmp_dir).unwrap();
        {
            let (mut tmp_log, _) = Log::open(&tmp_dir).unwrap();
            tmp_log
                .append_put(b"a".to_vec(), b"COMPACTED".to_vec())
                .unwrap();
        }
        fs::write(dir.path().join(COMPACT_READY_MARKER), b"").unwrap();

        // Simulate the swap having completed step 1 (old segments backed
        // up) but crashing before step 2 (moving the new ones in).
        let backup_dir = dir.path().join(COMPACT_OLD_BACKUP_DIR);
        fs::create_dir_all(&backup_dir).unwrap();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("seg-") && name_str.ends_with(".log") {
                fs::rename(entry.path(), backup_dir.join(&name)).unwrap();
            }
        }

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), Some(b"COMPACTED".to_vec()));
        assert!(!backup_dir.exists());
        assert!(!tmp_dir.exists());
    }

    #[test]
    fn a_crash_after_move_but_before_backup_cleanup_still_finishes_correctly() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
        }
        fs::write(dir.path().join(COMPACT_READY_MARKER), b"").unwrap();

        // Simulate: old segments already backed up, new ones already
        // moved into place — only the final cleanup (removing the
        // now-obsolete backup) never ran.
        let backup_dir = dir.path().join(COMPACT_OLD_BACKUP_DIR);
        fs::create_dir_all(&backup_dir).unwrap();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("seg-") && name_str.ends_with(".log") {
                fs::rename(entry.path(), backup_dir.join(&name)).unwrap();
            }
        }
        {
            let (mut new_log, _) = Log::open(dir.path()).unwrap();
            new_log
                .append_put(b"a".to_vec(), b"COMPACTED".to_vec())
                .unwrap();
        }

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), Some(b"COMPACTED".to_vec()));
        assert!(
            !backup_dir.exists(),
            "obsolete backup must be cleaned up on the finishing open"
        );
        assert!(!dir.path().join(COMPACT_READY_MARKER).exists());
    }

    #[test]
    fn a_crash_after_full_cleanup_except_the_marker_is_still_idempotent() {
        let dir = tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.put("a", "1").unwrap();
            store.compact().unwrap();
        }
        // Simulate: the real compact() already finished everything except
        // that the process died before removing its own marker (an
        // artificial state — compact() itself removes tmp/backup/marker
        // together at the end — but recovery must tolerate a stray
        // marker with nothing left to actually finish).
        fs::write(dir.path().join(COMPACT_READY_MARKER), b"").unwrap();

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get(b"a"), Some(b"1".to_vec()));
        assert!(!dir.path().join(COMPACT_READY_MARKER).exists());
    }
}
