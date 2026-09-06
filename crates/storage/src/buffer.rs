//! A fixed-capacity buffer pool with clock (second-chance) eviction,
//! reference-counted pinning, and dirty-page tracking. Persistence is
//! delegated entirely to a `PageStore` implementation — the pool itself
//! knows nothing about logs, segments, or crash recovery, only about which
//! pages are cached, pinned, and dirty.

use std::collections::HashMap;

use crate::page::{Page, PageType};
use crate::page_store::{PageId, PageStore};

#[derive(Debug, thiserror::Error)]
pub enum BufferError<E: std::error::Error> {
    #[error("no evictable frame available: all {0} frames are pinned")]
    PoolExhausted(usize),
    #[error("page {0} is not currently pinned")]
    NotPinned(PageId),
    #[error(transparent)]
    Store(E),
}

struct Frame {
    page_id: PageId,
    page: Page,
    pin_count: u32,
    dirty: bool,
    ref_bit: bool,
}

/// A fixed-size cache of pages sitting in front of a `PageStore`. Callers
/// `fetch` a page (pinning it), mutate it in place through
/// `page_mut`/`page`, then `unpin` it — marking it dirty if they changed
/// it. A dirty page is only ever made durable by writing a whole new
/// version of it through the `PageStore`, never by mutating bytes already
/// on disk.
pub struct BufferPool<S: PageStore> {
    store: S,
    capacity: usize,
    frames: Vec<Frame>,
    page_table: HashMap<PageId, usize>, // page_id -> frame index
    clock_hand: usize,
}

impl<S: PageStore> BufferPool<S> {
    pub fn new(store: S, capacity: usize) -> Self {
        assert!(capacity > 0, "buffer pool capacity must be at least 1");
        Self {
            store,
            capacity,
            frames: Vec::with_capacity(capacity),
            page_table: HashMap::new(),
            clock_hand: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Allocates a brand-new page (via the underlying store) and pins it
    /// in the pool, ready for the caller to populate.
    pub fn new_page(&mut self, page_type: PageType) -> Result<PageId, BufferError<S::Error>> {
        let id = self.store.allocate_page_id();
        let page = Page::new(page_type);
        let frame_idx = self.find_frame_for(id)?;
        self.frames[frame_idx] = Frame {
            page_id: id,
            page,
            pin_count: 1,
            dirty: true,
            ref_bit: true,
        };
        self.page_table.insert(id, frame_idx);
        Ok(id)
    }

    /// Pins `id` in the pool, loading it from the store if not already
    /// cached. Must be paired with `unpin`.
    pub fn fetch(&mut self, id: PageId) -> Result<(), BufferError<S::Error>> {
        if let Some(&idx) = self.page_table.get(&id) {
            self.frames[idx].pin_count += 1;
            self.frames[idx].ref_bit = true;
            return Ok(());
        }
        let page = self.store.read_page(id).map_err(BufferError::Store)?;
        let page = page.unwrap_or_else(|| Page::new(PageType::Data));
        let frame_idx = self.find_frame_for(id)?;
        self.frames[frame_idx] = Frame {
            page_id: id,
            page,
            pin_count: 1,
            dirty: false,
            ref_bit: true,
        };
        self.page_table.insert(id, frame_idx);
        Ok(())
    }

    pub fn page(&self, id: PageId) -> Option<&Page> {
        self.page_table.get(&id).map(|&idx| &self.frames[idx].page)
    }

    pub fn page_mut(&mut self, id: PageId) -> Option<&mut Page> {
        self.page_table
            .get(&id)
            .map(|&idx| &mut self.frames[idx].page)
    }

    /// Unpins a previously fetched page. `dirtied` should be true if the
    /// caller mutated the page since fetching it — once dirty, a page
    /// stays dirty until it is flushed, even across multiple pin/unpin
    /// cycles.
    pub fn unpin(&mut self, id: PageId, dirtied: bool) -> Result<(), BufferError<S::Error>> {
        let idx = *self.page_table.get(&id).ok_or(BufferError::NotPinned(id))?;
        let frame = &mut self.frames[idx];
        if frame.pin_count == 0 {
            return Err(BufferError::NotPinned(id));
        }
        frame.pin_count -= 1;
        frame.dirty |= dirtied;
        Ok(())
    }

    /// Writes a dirty page's current contents through to the store as a
    /// new immutable version, then clears its dirty bit.
    pub fn flush(&mut self, id: PageId) -> Result<(), BufferError<S::Error>> {
        let idx = *self.page_table.get(&id).ok_or(BufferError::NotPinned(id))?;
        if self.frames[idx].dirty {
            self.store
                .write_page(id, &self.frames[idx].page)
                .map_err(BufferError::Store)?;
            self.frames[idx].dirty = false;
        }
        Ok(())
    }

    pub fn flush_all(&mut self) -> Result<(), BufferError<S::Error>> {
        let ids: Vec<PageId> = self.frames.iter().map(|f| f.page_id).collect();
        for id in ids {
            self.flush(id)?;
        }
        Ok(())
    }

    pub fn is_dirty(&self, id: PageId) -> bool {
        self.page_table
            .get(&id)
            .map(|&idx| self.frames[idx].dirty)
            .unwrap_or(false)
    }

    pub fn pin_count(&self, id: PageId) -> u32 {
        self.page_table
            .get(&id)
            .map(|&idx| self.frames[idx].pin_count)
            .unwrap_or(0)
    }

    /// Finds a frame to hold a new page: grows the pool if under capacity,
    /// otherwise runs clock eviction to find (and flush, if dirty) a
    /// victim. The returned index is a "hole" the caller immediately
    /// overwrites with the new frame's contents.
    fn find_frame_for(&mut self, incoming_id: PageId) -> Result<usize, BufferError<S::Error>> {
        if self.frames.len() < self.capacity {
            self.frames.push(Frame {
                page_id: incoming_id,
                page: Page::new(PageType::Data),
                pin_count: 0,
                dirty: false,
                ref_bit: false,
            });
            return Ok(self.frames.len() - 1);
        }

        let mut scanned = 0;
        loop {
            if scanned > 2 * self.capacity {
                return Err(BufferError::PoolExhausted(self.capacity));
            }
            let idx = self.clock_hand;
            self.clock_hand = (self.clock_hand + 1) % self.capacity;
            scanned += 1;

            if self.frames[idx].pin_count > 0 {
                continue;
            }
            if self.frames[idx].ref_bit {
                self.frames[idx].ref_bit = false;
                continue;
            }

            // Victim found: flush if dirty, then evict from the page table.
            if self.frames[idx].dirty {
                self.store
                    .write_page(self.frames[idx].page_id, &self.frames[idx].page)
                    .map_err(BufferError::Store)?;
            }
            self.page_table.remove(&self.frames[idx].page_id);
            return Ok(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_store::MemPageStore;

    fn new_pool(capacity: usize) -> BufferPool<MemPageStore> {
        BufferPool::new(MemPageStore::new(), capacity)
    }

    #[test]
    fn new_page_is_pinned_and_dirty() {
        let mut pool = new_pool(4);
        let id = pool.new_page(PageType::Data).unwrap();
        assert_eq!(pool.pin_count(id), 1);
        assert!(pool.is_dirty(id));
    }

    #[test]
    fn fetch_after_flush_reads_back_written_data() {
        let mut pool = new_pool(4);
        let id = pool.new_page(PageType::Data).unwrap();
        pool.page_mut(id).unwrap().insert(b"payload").unwrap();
        pool.unpin(id, true).unwrap();
        pool.flush(id).unwrap();
        assert!(!pool.is_dirty(id));

        // Evict everything, then fetch fresh from the store.
        for _ in 0..8 {
            let id = pool.new_page(PageType::Data).unwrap();
            pool.unpin(id, false).unwrap();
        }
        pool.fetch(id).unwrap();
        assert_eq!(pool.page(id).unwrap().get(0).unwrap(), b"payload");
    }

    #[test]
    fn clock_eviction_never_evicts_a_pinned_page() {
        let mut pool = new_pool(2);
        let pinned = pool.new_page(PageType::Data).unwrap(); // stays pinned
        let evictable = pool.new_page(PageType::Data).unwrap();
        pool.unpin(evictable, false).unwrap();

        // Allocating more pages must never evict `pinned`.
        for _ in 0..10 {
            let id = pool.new_page(PageType::Data).unwrap();
            pool.unpin(id, false).unwrap();
        }
        assert!(
            pool.page(pinned).is_some(),
            "pinned page must never be evicted"
        );
    }

    #[test]
    fn pool_exhausted_when_everything_is_pinned() {
        let mut pool = new_pool(2);
        pool.new_page(PageType::Data).unwrap();
        pool.new_page(PageType::Data).unwrap();
        let err = pool.new_page(PageType::Data).unwrap_err();
        matches!(err, BufferError::PoolExhausted(2));
    }

    #[test]
    fn dirty_page_is_flushed_on_eviction() {
        let mut pool = new_pool(1);
        let id = pool.new_page(PageType::Data).unwrap();
        pool.page_mut(id).unwrap().insert(b"must-survive").unwrap();
        pool.unpin(id, true).unwrap();

        // Force eviction of `id` by allocating a second page in a
        // capacity-1 pool.
        let id2 = pool.new_page(PageType::Data).unwrap();
        pool.unpin(id2, false).unwrap();

        pool.fetch(id).unwrap();
        assert_eq!(pool.page(id).unwrap().get(0).unwrap(), b"must-survive");
    }

    #[test]
    fn unpin_without_fetch_is_an_error() {
        let mut pool = new_pool(2);
        let err = pool.unpin(999, false).unwrap_err();
        matches!(err, BufferError::NotPinned(999));
    }
}
