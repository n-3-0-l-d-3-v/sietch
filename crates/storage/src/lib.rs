//! The Vault's storage core: an append-only, checksummed record log with
//! crash recovery. Nothing in this crate ever overwrites an existing,
//! committed byte — see `docs/design/CONSTRAINTS.md` and
//! `docs/design/decisions/ADR-001-torn-tail-truncation.md` for the one
//! carefully-scoped exception (discarding an uncommitted torn tail after a
//! crash, which is recovery, not mutation).

pub mod btree;
pub mod buffer;
pub mod heap_page_store;
pub mod indexed_store;
pub mod log;
pub mod page;
pub mod page_store;
pub mod record;
pub mod segment;
pub mod store;

pub use btree::{BTree, BTreeError};
pub use buffer::{BufferError, BufferPool};
pub use heap_page_store::{HeapPageStore, HeapPageStoreError};
pub use indexed_store::{IndexedStore, IndexedStoreError};
pub use log::{Log, LogError, LogOp, OpenReport};
pub use page::{Page, PageError, PageType, SlotId, PAGE_SIZE};
pub use page_store::{LogPageStore, LogPageStoreError, MemPageStore, PageId, PageStore};
pub use record::{Record, RecordType};
pub use store::{Snapshot, Store, StoreError, WriteOp};
