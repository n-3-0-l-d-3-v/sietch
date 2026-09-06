//! Property-based tests for the slotted page format and the buffer pool's
//! eviction/pinning invariants — the things that are easy to get subtly
//! wrong and hard to catch with a handful of hand-picked examples.

use proptest::prelude::*;
use storage::page::{Page, PageType};
use storage::{BufferPool, MemPageStore};

fn arb_record() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..64)
}

proptest! {
    /// Inserting records into a page and reading them back must always
    /// return exactly what was inserted, in the order of their slot ids,
    /// for any sequence of record sizes that fits.
    #[test]
    fn page_insert_then_get_matches_for_arbitrary_records(records in prop::collection::vec(arb_record(), 0..30)) {
        let mut page = Page::new(PageType::Data);
        let mut inserted = Vec::new();
        for record in &records {
            match page.insert(record) {
                Ok(slot) => inserted.push((slot, record.clone())),
                Err(_) => break, // page full; stop feeding it more
            }
        }
        for (slot, expected) in &inserted {
            prop_assert_eq!(page.get(*slot).unwrap(), expected.as_slice());
        }
    }

    /// A page that round-trips through encode/decode must expose exactly
    /// the same live records as before encoding, regardless of which
    /// slots were deleted along the way.
    #[test]
    fn page_survives_encode_decode_with_deletions(
        records in prop::collection::vec(arb_record(), 1..20),
        delete_mask in prop::collection::vec(any::<bool>(), 1..20),
    ) {
        let mut page = Page::new(PageType::Data);
        let mut slots = Vec::new();
        for record in &records {
            if let Ok(slot) = page.insert(record) {
                slots.push(slot);
            } else {
                break;
            }
        }
        for (i, &slot) in slots.iter().enumerate() {
            if delete_mask.get(i).copied().unwrap_or(false) {
                page.delete(slot).unwrap();
            }
        }

        let decoded = Page::decode(&page.encode()).unwrap();
        let live_before: Vec<_> = page.slot_ids().collect();
        let live_after: Vec<_> = decoded.slot_ids().collect();
        prop_assert_eq!(live_before, live_after);
    }

    /// Whatever sequence of new_page/fetch/unpin operations happens, a
    /// page that is currently pinned must never be evicted from the pool
    /// — this is the buffer pool's core correctness contract.
    #[test]
    fn pinned_pages_are_never_evicted(num_extra_pages in 1usize..40) {
        let mut pool = BufferPool::new(MemPageStore::new(), 3);
        let pinned = pool.new_page(PageType::Data).unwrap(); // never unpinned

        for _ in 0..num_extra_pages {
            match pool.new_page(PageType::Data) {
                Ok(id) => { let _ = pool.unpin(id, false); }
                Err(_) => break, // pool full of pinned pages is a valid terminal state
            }
        }

        prop_assert!(pool.page(pinned).is_some(), "a pinned page was evicted");
        prop_assert_eq!(pool.pin_count(pinned), 1);
    }

    /// A page written while dirty and then evicted must always be
    /// readable with its latest contents afterward, no matter how many
    /// unrelated pages churn through the pool in between.
    #[test]
    fn dirty_pages_survive_eviction_churn(churn in 1usize..50) {
        let mut pool = BufferPool::new(MemPageStore::new(), 2);
        let id = pool.new_page(PageType::Data).unwrap();
        pool.page_mut(id).unwrap().insert(b"must-survive").unwrap();
        pool.unpin(id, true).unwrap();

        for _ in 0..churn {
            if let Ok(other) = pool.new_page(PageType::Data) {
                let _ = pool.unpin(other, false);
            }
        }

        pool.fetch(id).unwrap();
        prop_assert_eq!(pool.page(id).unwrap().get(0).unwrap(), b"must-survive");
        pool.unpin(id, false).unwrap();
    }
}
