//! Crash-consistency for the B+Tree over its real backend: `LogPageStore`
//! writes are just log records, so the tree's meta page, root, and every
//! node it creates must all survive a torn write the same way plain pages
//! already do — this proves it end to end through the tree API instead of
//! just at the page layer.

use std::fs::OpenOptions;
use std::io::Write;

use storage::page_store::LogPageStore;
use storage::BTree;
use tempfile::tempdir;

#[test]
fn tree_state_before_a_crash_is_fully_recovered_after_reopening() {
    let dir = tempdir().unwrap();
    {
        let store = LogPageStore::open(dir.path()).unwrap();
        let mut tree = BTree::open(store, 32).unwrap();
        for i in 0..300u32 {
            tree.insert(
                format!("key{i:04}").into_bytes(),
                format!("value{i}").into_bytes(),
            )
            .unwrap();
        }
        tree.flush().unwrap();
    }

    // Simulate a crash: torn bytes appended after the last committed
    // record in the segment.
    let seg_path = dir.path().join("seg-0000000000000000.log");
    if seg_path.exists() {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0u8; 30]).unwrap();
    }

    let store = LogPageStore::open(dir.path()).unwrap();
    let mut tree = BTree::open(store, 32).unwrap();
    for i in 0..300u32 {
        let expected = format!("value{i}").into_bytes();
        assert_eq!(
            tree.get(format!("key{i:04}").as_bytes()).unwrap(),
            Some(expected),
            "lost key{i:04} after crash"
        );
    }
}

#[test]
fn tree_survives_reopening_mid_growth_and_keeps_inserting_correctly() {
    let dir = tempdir().unwrap();
    {
        let store = LogPageStore::open(dir.path()).unwrap();
        let mut tree = BTree::open(store, 16).unwrap();
        for i in 0..100u32 {
            tree.insert(format!("a{i:04}").into_bytes(), b"x".to_vec())
                .unwrap();
        }
        tree.flush().unwrap();
    }
    {
        let store = LogPageStore::open(dir.path()).unwrap();
        let mut tree = BTree::open(store, 16).unwrap();
        for i in 0..100u32 {
            tree.insert(format!("b{i:04}").into_bytes(), b"y".to_vec())
                .unwrap();
        }
        tree.flush().unwrap();
    }
    let store = LogPageStore::open(dir.path()).unwrap();
    let mut tree = BTree::open(store, 16).unwrap();
    for i in 0..100u32 {
        assert_eq!(
            tree.get(format!("a{i:04}").as_bytes()).unwrap(),
            Some(b"x".to_vec())
        );
        assert_eq!(
            tree.get(format!("b{i:04}").as_bytes()).unwrap(),
            Some(b"y".to_vec())
        );
    }
}

#[test]
fn deletes_committed_before_a_crash_stay_deleted_after_reopening() {
    let dir = tempdir().unwrap();
    {
        let store = LogPageStore::open(dir.path()).unwrap();
        let mut tree = BTree::open(store, 32).unwrap();
        for i in 0..200u32 {
            tree.insert(
                format!("key{i:04}").into_bytes(),
                format!("value{i}").into_bytes(),
            )
            .unwrap();
        }
        for i in 0..100u32 {
            tree.delete(format!("key{i:04}").as_bytes()).unwrap();
        }
        tree.flush().unwrap();
    }

    let seg_path = dir.path().join("seg-0000000000000000.log");
    if seg_path.exists() {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(&[0u8; 30]).unwrap();
    }

    let store = LogPageStore::open(dir.path()).unwrap();
    let mut tree = BTree::open(store, 32).unwrap();
    for i in 0..100u32 {
        assert_eq!(
            tree.get(format!("key{i:04}").as_bytes()).unwrap(),
            None,
            "key{i:04} should have stayed deleted after a crash"
        );
    }
    for i in 100..200u32 {
        assert_eq!(
            tree.get(format!("key{i:04}").as_bytes()).unwrap(),
            Some(format!("value{i}").into_bytes())
        );
    }
}
