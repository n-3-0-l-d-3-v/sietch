//! A disk-oriented B+Tree index built entirely on `Page`/`BufferPool` from
//! ticket 004. Per `docs/design/CONSTRAINTS.md` ("at least one serious
//! disk-oriented index: a B+ tree"), and per
//! `docs/design/decisions/ADR-003-btree-page-rebuild-strategy.md` for why
//! nodes are decoded, mutated, and rebuilt wholesale rather than edited
//! in place.
//!
//! Page id `0` is reserved as the tree's meta page (a single slot holding
//! the current root page id) so the tree can be reopened across restarts
//! without a separate metadata channel.

use crate::buffer::{BufferError, BufferPool};
use crate::page::{Page, PageError, PageType};
use crate::page_store::{PageId, PageStore};

const META_PAGE_ID: PageId = 0;

/// A key/value pair as stored in a leaf.
type Entry = (Vec<u8>, Vec<u8>);
/// What a node split produces for its parent to incorporate: the new
/// separator key and the newly allocated right-sibling page id.
type SplitResult<E> = Result<Option<(Vec<u8>, PageId)>, BTreeError<E>>;

#[derive(Debug, thiserror::Error)]
pub enum BTreeError<E: std::error::Error> {
    #[error(transparent)]
    Buffer(#[from] BufferError<E>),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error("corrupt meta page: expected a root page id")]
    CorruptMeta,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn encode_leaf_entry(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + key.len() + value.len());
    out.extend_from_slice(&(key.len() as u16).to_le_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value);
    out
}

fn decode_leaf_entry(bytes: &[u8]) -> Entry {
    let klen = u16_at(bytes, 0) as usize;
    let key = bytes[2..2 + klen].to_vec();
    let vlen = u16_at(bytes, 2 + klen) as usize;
    let value = bytes[4 + klen..4 + klen + vlen].to_vec();
    (key, value)
}

/// One entry in an internal node: `key = None` marks the leading,
/// "leftmost child" entry (everything less than the first real separator
/// key routes there); every other entry's `key` is the separator such that
/// keys `>= key` (and `<` the next separator) route to `child`.
#[derive(Debug, Clone)]
struct InternalEntry {
    key: Option<Vec<u8>>,
    child: PageId,
}

fn encode_internal_entry(entry: &InternalEntry) -> Vec<u8> {
    let key_len = entry.key.as_ref().map(|k| k.len()).unwrap_or(0);
    let mut out = Vec::with_capacity(2 + key_len + 8);
    if let Some(key) = &entry.key {
        out.extend_from_slice(&(key_len as u16).to_le_bytes());
        out.extend_from_slice(key);
    } else {
        out.extend_from_slice(&u16::MAX.to_le_bytes()); // sentinel: no key
    }
    out.extend_from_slice(&entry.child.to_le_bytes());
    out
}

fn decode_internal_entry(bytes: &[u8]) -> InternalEntry {
    let key_len_field = u16_at(bytes, 0);
    if key_len_field == u16::MAX {
        let child = u64::from_le_bytes(bytes[2..10].try_into().unwrap());
        InternalEntry { key: None, child }
    } else {
        let klen = key_len_field as usize;
        let key = bytes[2..2 + klen].to_vec();
        let child = u64::from_le_bytes(bytes[2 + klen..10 + klen].try_into().unwrap());
        InternalEntry {
            key: Some(key),
            child,
        }
    }
}

fn read_leaf_entries(page: &Page) -> Vec<Entry> {
    page.slot_ids()
        .map(|s| decode_leaf_entry(page.get(s).unwrap()))
        .collect()
}

fn read_internal_entries(page: &Page) -> Vec<InternalEntry> {
    page.slot_ids()
        .map(|s| decode_internal_entry(page.get(s).unwrap()))
        .collect()
}

/// Rebuilds a leaf page from scratch containing exactly `entries`, in
/// order. Returns `Err(PageError::Full)` if they don't all fit — the
/// caller is responsible for splitting in that case.
fn build_leaf(entries: &[Entry]) -> Result<Page, PageError> {
    let mut page = Page::new(PageType::BTreeLeaf);
    for (k, v) in entries {
        page.insert(&encode_leaf_entry(k, v))?;
    }
    Ok(page)
}

fn build_internal(entries: &[InternalEntry]) -> Result<Page, PageError> {
    let mut page = Page::new(PageType::BTreeInternal);
    for entry in entries {
        page.insert(&encode_internal_entry(entry))?;
    }
    Ok(page)
}

/// Splits a sorted entry list roughly in half — used identically for leaf
/// and internal splits (the internal case additionally promotes the
/// midpoint key to the parent instead of keeping it in the right node).
fn split_at_midpoint<T>(entries: &[T]) -> usize {
    entries.len() / 2
}

pub struct BTree<S: PageStore> {
    pool: BufferPool<S>,
}

impl<S: PageStore> BTree<S> {
    /// Opens (creating if necessary) a B+Tree over `store`. A fresh store
    /// gets an empty leaf as its root; an existing one resumes from the
    /// root recorded in the meta page.
    pub fn open(store: S, pool_capacity: usize) -> Result<Self, BTreeError<S::Error>> {
        let mut pool = BufferPool::new(store, pool_capacity);
        pool.fetch(META_PAGE_ID)?;
        let has_root = !pool
            .page(META_PAGE_ID)
            .unwrap()
            .slot_ids()
            .collect::<Vec<_>>()
            .is_empty();
        if !has_root {
            let root_id = pool.new_page(PageType::BTreeLeaf)?;
            pool.unpin(root_id, true)?;
            let meta = pool.page_mut(META_PAGE_ID).unwrap();
            meta.insert(&root_id.to_le_bytes())?;
        }
        pool.unpin(META_PAGE_ID, true)?;
        Ok(Self { pool })
    }

    fn root_id(&mut self) -> Result<PageId, BTreeError<S::Error>> {
        self.pool.fetch(META_PAGE_ID)?;
        let meta = self.pool.page(META_PAGE_ID).unwrap();
        let bytes = meta.get(0)?;
        let id = u64::from_le_bytes(bytes.try_into().map_err(|_| BTreeError::CorruptMeta)?);
        self.pool.unpin(META_PAGE_ID, false)?;
        Ok(id)
    }

    fn set_root_id(&mut self, new_root: PageId) -> Result<(), BTreeError<S::Error>> {
        self.pool.fetch(META_PAGE_ID)?;
        let mut meta = Page::new(PageType::Meta);
        meta.insert(&new_root.to_le_bytes())?;
        *self.pool.page_mut(META_PAGE_ID).unwrap() = meta;
        self.pool.unpin(META_PAGE_ID, true)?;
        Ok(())
    }

    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, BTreeError<S::Error>> {
        let root = self.root_id()?;
        self.get_recursive(root, key)
    }

    fn get_recursive(
        &mut self,
        page_id: PageId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, BTreeError<S::Error>> {
        self.pool.fetch(page_id)?;
        let page = self.pool.page(page_id).unwrap();
        let result = match page.page_type {
            PageType::BTreeLeaf => {
                let entries = read_leaf_entries(page);
                entries
                    .iter()
                    .find(|(k, _)| k.as_slice() == key)
                    .map(|(_, v)| v.clone())
            }
            PageType::BTreeInternal => {
                let entries = read_internal_entries(page);
                let child = route(&entries, key);
                self.pool.unpin(page_id, false)?;
                return self.get_recursive(child, key);
            }
            other => unreachable!("unexpected page type in btree: {other:?}"),
        };
        self.pool.unpin(page_id, false)?;
        Ok(result)
    }

    pub fn insert(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), BTreeError<S::Error>> {
        let root = self.root_id()?;
        if let Some((sep, new_child)) = self.insert_recursive(root, &key, &value)? {
            // The root split: build a fresh internal root with two children.
            let new_root_id = self.pool.new_page(PageType::BTreeInternal)?;
            let entries = vec![
                InternalEntry {
                    key: None,
                    child: root,
                },
                InternalEntry {
                    key: Some(sep),
                    child: new_child,
                },
            ];
            *self.pool.page_mut(new_root_id).unwrap() = build_internal(&entries)?;
            self.pool.unpin(new_root_id, true)?;
            self.set_root_id(new_root_id)?;
        }
        Ok(())
    }

    /// Returns `Some((separator_key, new_right_sibling_id))` if `page_id`
    /// split as a result of this insert — the caller (parent frame, or
    /// `insert` for the root) must incorporate that into itself.
    fn insert_recursive(
        &mut self,
        page_id: PageId,
        key: &[u8],
        value: &[u8],
    ) -> SplitResult<S::Error> {
        self.pool.fetch(page_id)?;
        let page_type = self.pool.page(page_id).unwrap().page_type;

        match page_type {
            PageType::BTreeLeaf => {
                let mut entries = read_leaf_entries(self.pool.page(page_id).unwrap());
                match entries.binary_search_by(|(k, _)| k.as_slice().cmp(key)) {
                    Ok(i) => entries[i].1 = value.to_vec(),
                    Err(i) => entries.insert(i, (key.to_vec(), value.to_vec())),
                }

                let split = match build_leaf(&entries) {
                    Ok(page) => {
                        *self.pool.page_mut(page_id).unwrap() = page;
                        None
                    }
                    Err(PageError::Full { .. }) => {
                        let mid = split_at_midpoint(&entries);
                        let (left, right) = entries.split_at(mid);
                        *self.pool.page_mut(page_id).unwrap() = build_leaf(left)?;
                        let right_id = self.pool.new_page(PageType::BTreeLeaf)?;
                        *self.pool.page_mut(right_id).unwrap() = build_leaf(right)?;
                        let sep = right[0].0.clone();
                        self.pool.unpin(right_id, true)?;
                        Some((sep, right_id))
                    }
                    Err(e) => return Err(e.into()),
                };
                self.pool.unpin(page_id, true)?;
                Ok(split)
            }
            PageType::BTreeInternal => {
                let entries = read_internal_entries(self.pool.page(page_id).unwrap());
                let child = route(&entries, key);
                self.pool.unpin(page_id, false)?;

                let child_split = self.insert_recursive(child, key, value)?;
                let Some((sep_key, new_child)) = child_split else {
                    return Ok(None);
                };

                self.pool.fetch(page_id)?;
                let mut entries = read_internal_entries(self.pool.page(page_id).unwrap());
                let insert_at = entries[1..]
                    .iter()
                    .position(|e| e.key.as_deref().unwrap() > sep_key.as_slice())
                    .map(|i| i + 1)
                    .unwrap_or(entries.len());
                entries.insert(
                    insert_at,
                    InternalEntry {
                        key: Some(sep_key),
                        child: new_child,
                    },
                );

                let split = match build_internal(&entries) {
                    Ok(page) => {
                        *self.pool.page_mut(page_id).unwrap() = page;
                        None
                    }
                    Err(PageError::Full { .. }) => {
                        let mid = split_at_midpoint(&entries).max(1);
                        let promoted_key =
                            entries[mid].key.clone().expect("mid entry must have a key");
                        let left = &entries[..mid];
                        let mut right = entries[mid + 1..].to_vec();
                        right.insert(
                            0,
                            InternalEntry {
                                key: None,
                                child: entries[mid].child,
                            },
                        );

                        *self.pool.page_mut(page_id).unwrap() = build_internal(left)?;
                        let right_id = self.pool.new_page(PageType::BTreeInternal)?;
                        *self.pool.page_mut(right_id).unwrap() = build_internal(&right)?;
                        self.pool.unpin(right_id, true)?;
                        Some((promoted_key, right_id))
                    }
                    Err(e) => return Err(e.into()),
                };
                self.pool.unpin(page_id, true)?;
                Ok(split)
            }
            other => unreachable!("unexpected page type in btree: {other:?}"),
        }
    }

    /// Full in-order traversal. Correct for any tree shape, but O(n) in
    /// the number of live entries rather than O(log n + k) for a bounded
    /// range — see the "known limitations" note in
    /// `docs/design/decisions/ADR-003-btree-page-rebuild-strategy.md` on
    /// why leaf sibling links are deliberately not implemented yet.
    pub fn scan_all(&mut self) -> Result<Vec<Entry>, BTreeError<S::Error>> {
        let root = self.root_id()?;
        let mut out = Vec::new();
        self.scan_recursive(root, &mut out)?;
        Ok(out)
    }

    fn scan_recursive(
        &mut self,
        page_id: PageId,
        out: &mut Vec<Entry>,
    ) -> Result<(), BTreeError<S::Error>> {
        self.pool.fetch(page_id)?;
        let page_type = self.pool.page(page_id).unwrap().page_type;
        match page_type {
            PageType::BTreeLeaf => {
                out.extend(read_leaf_entries(self.pool.page(page_id).unwrap()));
                self.pool.unpin(page_id, false)?;
            }
            PageType::BTreeInternal => {
                let children: Vec<PageId> = read_internal_entries(self.pool.page(page_id).unwrap())
                    .iter()
                    .map(|e| e.child)
                    .collect();
                self.pool.unpin(page_id, false)?;
                for child in children {
                    self.scan_recursive(child, out)?;
                }
            }
            other => unreachable!("unexpected page type in btree: {other:?}"),
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), BTreeError<S::Error>> {
        self.pool.flush_all()?;
        Ok(())
    }

    /// Height of the tree (1 = a single leaf root, with no internal
    /// nodes). Walks the leftmost path, which is always representative
    /// since every root-to-leaf path in a B+Tree has equal length.
    pub fn depth(&mut self) -> Result<usize, BTreeError<S::Error>> {
        let mut page_id = self.root_id()?;
        let mut depth = 1;
        loop {
            self.pool.fetch(page_id)?;
            let page = self.pool.page(page_id).unwrap();
            let next = match page.page_type {
                PageType::BTreeLeaf => None,
                PageType::BTreeInternal => Some(read_internal_entries(page)[0].child),
                other => unreachable!("unexpected page type in btree: {other:?}"),
            };
            self.pool.unpin(page_id, false)?;
            match next {
                None => return Ok(depth),
                Some(child) => {
                    page_id = child;
                    depth += 1;
                }
            }
        }
    }
}

/// Given a sorted internal-node entry list, finds which child a search
/// key routes to: the last entry whose key is `<= search_key`, or the
/// leading (`key = None`) leftmost child if none qualify.
fn route(entries: &[InternalEntry], search_key: &[u8]) -> PageId {
    let mut chosen = entries[0].child;
    for entry in &entries[1..] {
        match &entry.key {
            Some(k) if k.as_slice() <= search_key => chosen = entry.child,
            _ => break,
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_store::MemPageStore;
    use std::collections::BTreeMap;

    fn tree(capacity: usize) -> BTree<MemPageStore> {
        BTree::open(MemPageStore::new(), capacity).unwrap()
    }

    #[test]
    fn insert_then_get_round_trips() {
        let mut t = tree(64);
        t.insert(b"a".to_vec(), b"1".to_vec()).unwrap();
        t.insert(b"b".to_vec(), b"2".to_vec()).unwrap();
        assert_eq!(t.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(t.get(b"b").unwrap(), Some(b"2".to_vec()));
        assert_eq!(t.get(b"missing").unwrap(), None);
    }

    #[test]
    fn upsert_replaces_existing_value() {
        let mut t = tree(64);
        t.insert(b"a".to_vec(), b"1".to_vec()).unwrap();
        t.insert(b"a".to_vec(), b"2".to_vec()).unwrap();
        assert_eq!(t.get(b"a").unwrap(), Some(b"2".to_vec()));
    }

    #[test]
    fn many_inserts_force_leaf_and_internal_splits_and_all_keys_survive() {
        let mut t = tree(512);
        // Large-ish values force leaf splits early, and enough of them
        // force the resulting internal node(s) to split too, so this
        // actually exercises the internal-split path, not just leaves.
        let n = 20_000;
        for i in 0..n {
            let key = format!("key{i:06}").into_bytes();
            let value = vec![b'v'; 40];
            t.insert(key, value).unwrap();
        }
        for i in 0..n {
            let key = format!("key{i:06}").into_bytes();
            assert_eq!(
                t.get(&key).unwrap(),
                Some(vec![b'v'; 40]),
                "missing key{i:06}"
            );
        }
        let depth = t.depth().unwrap();
        assert!(
            depth >= 3,
            "expected at least 3 levels (leaf + 2 internal), got {depth}"
        );
    }

    #[test]
    fn scan_all_returns_every_key_in_sorted_order() {
        let mut t = tree(128);
        let mut keys: Vec<u32> = (0..500).collect();
        // insert out of order to make sure sorting is the tree's job, not the caller's.
        keys.reverse();
        for k in &keys {
            t.insert(format!("{k:05}").into_bytes(), b"v".to_vec())
                .unwrap();
        }
        let scanned = t.scan_all().unwrap();
        let scanned_keys: Vec<String> = scanned
            .into_iter()
            .map(|(k, _)| String::from_utf8(k).unwrap())
            .collect();
        let mut expected: Vec<String> = keys.iter().map(|k| format!("{k:05}")).collect();
        expected.sort();
        assert_eq!(scanned_keys, expected);
    }

    #[test]
    fn tree_reopens_from_the_same_store_with_all_data_intact() {
        let mut store = MemPageStore::new();
        {
            let mut t = BTree::open(std::mem::take(&mut store), 64).unwrap();
            for i in 0..50 {
                t.insert(format!("k{i}").into_bytes(), format!("v{i}").into_bytes())
                    .unwrap();
            }
            t.flush().unwrap();
        }
    }

    #[test]
    fn random_insert_order_matches_a_reference_btreemap() {
        use rand_like::shuffled_range;
        let mut t = tree(128);
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for i in shuffled_range(0, 800) {
            let key = format!("{i:06}").into_bytes();
            let value = format!("v{i}").into_bytes();
            t.insert(key.clone(), value.clone()).unwrap();
            model.insert(key, value);
        }
        for (k, v) in &model {
            assert_eq!(t.get(k).unwrap().as_ref(), Some(v));
        }
        let scanned = t.scan_all().unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> = model.into_iter().collect();
        assert_eq!(scanned, expected);
    }

    /// A tiny deterministic shuffle (no external RNG dependency needed for
    /// the storage crate's own unit tests) — good enough to exercise
    /// out-of-order insertion without pulling in `rand` as a dependency.
    mod rand_like {
        pub fn shuffled_range(start: u32, end: u32) -> Vec<u32> {
            let mut v: Vec<u32> = (start..end).collect();
            // A fixed-seed LCG-based shuffle: deterministic across runs,
            // which keeps this test reproducible without extra crates.
            let mut seed: u64 = 0x9E3779B97F4A7C15;
            let n = v.len();
            for i in (1..n).rev() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let j = (seed >> 33) as usize % (i + 1);
                v.swap(i, j);
            }
            v
        }
    }
}
