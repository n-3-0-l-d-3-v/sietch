//! The `PageStore` trait is the buffer manager's only dependency on how
//! pages actually become durable — matching the project's OOP guidance
//! (storage backends as swappable implementations behind one contract,
//! not a hardwired concrete type). `MemPageStore` lets the buffer pool's
//! own logic (eviction, pinning, dirty tracking) be tested in isolation
//! from disk I/O; `LogPageStore` is the real, crash-safe backend, built
//! entirely on the same append-only `Store` used for plain key/value data.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;

use crate::page::{Page, PageError, PAGE_SIZE};
use crate::store::{Store, StoreError};

pub type PageId = u64;

pub trait PageStore {
    type Error: std::error::Error;

    fn read_page(&self, id: PageId) -> Result<Option<Page>, Self::Error>;
    fn write_page(&mut self, id: PageId, page: &Page) -> Result<(), Self::Error>;
    /// Reserves and returns a fresh page id. Ids are never reused.
    fn allocate_page_id(&mut self) -> PageId;
}

/// An in-memory page store with no persistence at all — used to test the
/// buffer pool's eviction/pinning/dirty-tracking logic without touching a
/// filesystem, and as the "test double" half of the storage-abstraction
/// pattern the project calls for (crash simulator vs. production backend
/// sharing one contract).
#[derive(Debug, Default)]
pub struct MemPageStore {
    pages: HashMap<PageId, Page>,
    next_id: PageId,
}

impl MemPageStore {
    pub fn new() -> Self {
        Self {
            pages: HashMap::new(),
            next_id: 1,
        }
    }
}

impl PageStore for MemPageStore {
    type Error = Infallible;

    fn read_page(&self, id: PageId) -> Result<Option<Page>, Self::Error> {
        Ok(self.pages.get(&id).cloned())
    }

    fn write_page(&mut self, id: PageId, page: &Page) -> Result<(), Self::Error> {
        self.pages.insert(id, page.clone());
        Ok(())
    }

    fn allocate_page_id(&mut self) -> PageId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LogPageStoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error("page {0} has a corrupt length ({1} bytes, expected {PAGE_SIZE})")]
    WrongLength(PageId, usize),
}

/// Persists pages as immutable versioned records in the same append-only
/// log used for plain key/value data: writing a page never overwrites the
/// previous version, it appends a new one. Recovery (via `Store::open`)
/// replays the whole log, so a crash mid-page-write behaves exactly like
/// `crash_recovery.rs` already proves for ordinary keys — the last
/// complete, checksummed record wins, nothing partially written is ever
/// visible.
pub struct LogPageStore {
    store: Store,
    next_id: PageId,
}

impl LogPageStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, LogPageStoreError> {
        let store = Store::open(dir)?;
        let max_id = store
            .scan(&[])
            .into_iter()
            .filter_map(|(k, _)| {
                if k.len() == 8 {
                    Some(u64::from_be_bytes(k.try_into().unwrap()))
                } else {
                    None
                }
            })
            .max();
        Ok(Self {
            store,
            next_id: max_id.map(|m| m + 1).unwrap_or(1),
        })
    }

    fn key(id: PageId) -> Vec<u8> {
        id.to_be_bytes().to_vec()
    }
}

impl PageStore for LogPageStore {
    type Error = LogPageStoreError;

    fn read_page(&self, id: PageId) -> Result<Option<Page>, Self::Error> {
        match self.store.get(&Self::key(id)) {
            None => Ok(None),
            Some(bytes) => {
                if bytes.len() != PAGE_SIZE {
                    return Err(LogPageStoreError::WrongLength(id, bytes.len()));
                }
                Ok(Some(Page::decode(&bytes)?))
            }
        }
    }

    fn write_page(&mut self, id: PageId, page: &Page) -> Result<(), Self::Error> {
        self.store.put(Self::key(id), page.encode().to_vec())?;
        Ok(())
    }

    fn allocate_page_id(&mut self) -> PageId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageType;
    use tempfile::tempdir;

    #[test]
    fn mem_store_round_trips() {
        let mut store = MemPageStore::new();
        let id = store.allocate_page_id();
        let mut page = Page::new(PageType::Data);
        page.insert(b"hi").unwrap();
        store.write_page(id, &page).unwrap();
        let read_back = store.read_page(id).unwrap().unwrap();
        assert_eq!(read_back.get(0).unwrap(), b"hi");
    }

    #[test]
    fn log_store_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let id;
        {
            let mut store = LogPageStore::open(dir.path()).unwrap();
            id = store.allocate_page_id();
            let mut page = Page::new(PageType::Data);
            page.insert(b"durable").unwrap();
            store.write_page(id, &page).unwrap();
        }
        let store = LogPageStore::open(dir.path()).unwrap();
        let page = store.read_page(id).unwrap().unwrap();
        assert_eq!(page.get(0).unwrap(), b"durable");
    }

    #[test]
    fn log_store_next_id_survives_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut store = LogPageStore::open(dir.path()).unwrap();
            for _ in 0..5 {
                let id = store.allocate_page_id();
                store.write_page(id, &Page::new(PageType::Data)).unwrap();
            }
        }
        let mut store = LogPageStore::open(dir.path()).unwrap();
        assert_eq!(store.allocate_page_id(), 6);
    }

    #[test]
    fn writing_a_new_version_of_a_page_does_not_lose_history_in_the_log() {
        // Because the underlying log is append-only, overwriting a page
        // logically just appends a newer version; reading always returns
        // the latest, but the old bytes are still physically present
        // until compaction (ticket 006) — this test locks in the "latest
        // wins" read behavior that ticket 006 must preserve.
        let dir = tempdir().unwrap();
        let mut store = LogPageStore::open(dir.path()).unwrap();
        let id = store.allocate_page_id();
        let mut v1 = Page::new(PageType::Data);
        v1.insert(b"version-1").unwrap();
        store.write_page(id, &v1).unwrap();

        let mut v2 = Page::new(PageType::Data);
        v2.insert(b"version-2").unwrap();
        store.write_page(id, &v2).unwrap();

        let latest = store.read_page(id).unwrap().unwrap();
        assert_eq!(latest.get(0).unwrap(), b"version-2");
    }
}
