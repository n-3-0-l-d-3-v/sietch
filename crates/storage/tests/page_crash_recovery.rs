//! Crash-consistency for the page layer specifically: a page write is just
//! another record in the same append-only log, so it must survive the
//! exact same torn-write injection that `crash_recovery.rs` already proves
//! for plain key/value records.

use std::fs::OpenOptions;
use std::io::Write;

use storage::page::{Page, PageType};
use storage::page_store::{LogPageStore, PageStore};
use tempfile::tempdir;

#[test]
fn a_torn_write_after_a_committed_page_does_not_corrupt_it() {
    let dir = tempdir().unwrap();
    let id;
    {
        let mut store = LogPageStore::open(dir.path()).unwrap();
        id = store.allocate_page_id();
        let mut page = Page::new(PageType::Data);
        page.insert(b"committed-before-crash").unwrap();
        store.write_page(id, &page).unwrap();
    }

    // Simulate a crash mid-write on the underlying segment file.
    let seg_path = dir.path().join("seg-0000000000000000.log");
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0xAA; 50]).unwrap();
    }

    let store = LogPageStore::open(dir.path()).unwrap();
    let page = store.read_page(id).unwrap().unwrap();
    assert_eq!(page.get(0).unwrap(), b"committed-before-crash");
}

#[test]
fn only_the_latest_committed_version_of_a_page_survives_a_crash_between_writes() {
    let dir = tempdir().unwrap();
    let id;
    {
        let mut store = LogPageStore::open(dir.path()).unwrap();
        id = store.allocate_page_id();
        let mut v1 = Page::new(PageType::Data);
        v1.insert(b"v1").unwrap();
        store.write_page(id, &v1).unwrap();

        let mut v2 = Page::new(PageType::Data);
        v2.insert(b"v2").unwrap();
        store.write_page(id, &v2).unwrap();
    }
    // Crash right after v2 committed: torn bytes appended afterward must
    // not resurrect v1 or corrupt v2.
    let seg_path = dir.path().join("seg-0000000000000000.log");
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0u8; 5]).unwrap();
    }

    let store = LogPageStore::open(dir.path()).unwrap();
    let page = store.read_page(id).unwrap().unwrap();
    assert_eq!(page.get(0).unwrap(), b"v2");
}
