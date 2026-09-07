//! The ticket 012 fix for the regression documented in
//! `docs/design/decisions/ADR-004-indexed-store-regression-and-write-amplification.md`:
//! `LogPageStore` persists whole pages through a generic `Store`, so
//! opening it means fully replaying every historical page version just to
//! find the latest one. `HeapPageStore` instead splits page storage into
//! two independent pieces with very different recovery costs:
//!
//! - a flat, append-only **heap file** holding raw page bytes, which is
//!   never scanned or decoded at open time — only its length is checked
//!   (an O(1) `stat`, truncated to the nearest whole page if a crash left
//!   a torn partial page at the end);
//! - a small **location log** (page id -> heap offset) using the same
//!   `Log`/`Record` machinery as everywhere else in this crate, but with
//!   tiny fixed-size records instead of 4096-byte page bodies — so
//!   replaying it at open time costs O(number of page writes), not O(page
//!   bytes ever written).
//!
//! Reading a page means one O(1) lookup in the (already-replayed, in
//! memory) location map, then a direct seek + read from the heap file —
//! no log replay involved on the read path at all.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::log::{Log, LogError};
use crate::page::{Page, PageError, PAGE_SIZE};
use crate::page_store::{PageId, PageStore};

const HEAP_FILE_NAME: &str = "heap.dat";

#[derive(Debug, thiserror::Error)]
pub enum HeapPageStoreError {
    #[error(transparent)]
    Log(#[from] LogError),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

pub struct HeapPageStore {
    heap_file: File,
    heap_len: u64,
    location_log: Log,
    locations: HashMap<PageId, u64>, // page id -> byte offset in the heap file
    next_page_id: PageId,
}

fn encode_location(offset: u64) -> Vec<u8> {
    offset.to_le_bytes().to_vec()
}

fn decode_location(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(
        bytes
            .try_into()
            .expect("location record must be exactly 8 bytes"),
    )
}

impl HeapPageStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, HeapPageStoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;

        let heap_path = heap_path(&dir);
        let heap_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&heap_path)?;
        let mut heap_len = heap_file.metadata()?.len();
        let torn_bytes = heap_len % PAGE_SIZE as u64;
        if torn_bytes != 0 {
            // A crash mid-page-write left a partial page at the end of the
            // heap file. No location record can possibly reference it
            // (the location write always happens strictly after the heap
            // write's fsync completes — see `write_page`), so discarding
            // it is recovery, not mutation of anything committed, exactly
            // the same reasoning as ADR-001's torn-tail truncation.
            heap_len -= torn_bytes;
            heap_file.set_len(heap_len)?;
        }

        let (location_log, report) = Log::open(dir.join("locations"))?;
        let mut locations = HashMap::new();
        let mut max_id: Option<PageId> = None;
        for record in &report.records {
            let id = PageId::from_le_bytes(
                record
                    .key
                    .as_slice()
                    .try_into()
                    .expect("page id key must be 8 bytes"),
            );
            locations.insert(id, decode_location(&record.value));
            max_id = Some(max_id.map_or(id, |m| m.max(id)));
        }

        Ok(Self {
            heap_file,
            heap_len,
            location_log,
            locations,
            next_page_id: max_id.map(|m| m + 1).unwrap_or(1),
        })
    }
}

fn heap_path(dir: &Path) -> PathBuf {
    dir.join(HEAP_FILE_NAME)
}

impl PageStore for HeapPageStore {
    type Error = HeapPageStoreError;

    fn read_page(&self, id: PageId) -> Result<Option<Page>, Self::Error> {
        let Some(&offset) = self.locations.get(&id) else {
            return Ok(None);
        };
        let mut buf = vec![0u8; PAGE_SIZE];
        let mut file = self.heap_file.try_clone()?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf)?;
        Ok(Some(Page::decode(&buf)?))
    }

    fn write_page(&mut self, id: PageId, page: &Page) -> Result<(), Self::Error> {
        let offset = self.heap_len;
        let bytes = page.encode();
        self.heap_file.seek(SeekFrom::Start(offset))?;
        self.heap_file.write_all(&bytes)?;
        self.heap_file.sync_data()?;
        self.heap_len += PAGE_SIZE as u64;

        self.location_log
            .append_put(id.to_le_bytes().to_vec(), encode_location(offset))?;
        self.locations.insert(id, offset);
        Ok(())
    }

    fn allocate_page_id(&mut self) -> PageId {
        let id = self.next_page_id;
        self.next_page_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageType;
    use tempfile::tempdir;

    #[test]
    fn round_trips_a_page() {
        let dir = tempdir().unwrap();
        let mut store = HeapPageStore::open(dir.path()).unwrap();
        let id = store.allocate_page_id();
        let mut page = Page::new(PageType::Data);
        page.insert(b"hello").unwrap();
        store.write_page(id, &page).unwrap();
        let read_back = store.read_page(id).unwrap().unwrap();
        assert_eq!(read_back.get(0).unwrap(), b"hello");
    }

    #[test]
    fn missing_page_returns_none_not_an_error() {
        let dir = tempdir().unwrap();
        let store = HeapPageStore::open(dir.path()).unwrap();
        assert!(store.read_page(999).unwrap().is_none());
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempdir().unwrap();
        let id;
        {
            let mut store = HeapPageStore::open(dir.path()).unwrap();
            id = store.allocate_page_id();
            let mut page = Page::new(PageType::Data);
            page.insert(b"durable").unwrap();
            store.write_page(id, &page).unwrap();
        }
        let store = HeapPageStore::open(dir.path()).unwrap();
        assert_eq!(
            store.read_page(id).unwrap().unwrap().get(0).unwrap(),
            b"durable"
        );
    }

    #[test]
    fn newer_version_of_a_page_wins_after_reopen() {
        let dir = tempdir().unwrap();
        let id;
        {
            let mut store = HeapPageStore::open(dir.path()).unwrap();
            id = store.allocate_page_id();
            let mut v1 = Page::new(PageType::Data);
            v1.insert(b"v1").unwrap();
            store.write_page(id, &v1).unwrap();
            let mut v2 = Page::new(PageType::Data);
            v2.insert(b"v2").unwrap();
            store.write_page(id, &v2).unwrap();
        }
        let store = HeapPageStore::open(dir.path()).unwrap();
        assert_eq!(store.read_page(id).unwrap().unwrap().get(0).unwrap(), b"v2");
    }

    #[test]
    fn a_torn_partial_page_at_the_end_of_the_heap_file_is_discarded_on_reopen() {
        let dir = tempdir().unwrap();
        let id;
        {
            let mut store = HeapPageStore::open(dir.path()).unwrap();
            id = store.allocate_page_id();
            let mut page = Page::new(PageType::Data);
            page.insert(b"committed").unwrap();
            store.write_page(id, &page).unwrap();
        }
        // Simulate a crash mid-write of a second page: only part of a
        // full PAGE_SIZE write landed.
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(heap_path(dir.path()))
                .unwrap();
            f.write_all(&[0xAB; 100]).unwrap();
        }
        let store = HeapPageStore::open(dir.path()).unwrap();
        // The committed page is untouched...
        assert_eq!(
            store.read_page(id).unwrap().unwrap().get(0).unwrap(),
            b"committed"
        );
        // ...and the heap file was truncated back to a whole-page boundary.
        let len = std::fs::metadata(heap_path(dir.path())).unwrap().len();
        assert_eq!(len % PAGE_SIZE as u64, 0);
        assert_eq!(len, PAGE_SIZE as u64);
    }

    #[test]
    fn next_page_id_survives_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut store = HeapPageStore::open(dir.path()).unwrap();
            for _ in 0..5 {
                let id = store.allocate_page_id();
                store.write_page(id, &Page::new(PageType::Data)).unwrap();
            }
        }
        let mut store = HeapPageStore::open(dir.path()).unwrap();
        assert_eq!(store.allocate_page_id(), 6);
    }
}
