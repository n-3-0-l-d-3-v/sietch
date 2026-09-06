//! The Vault's storage core: an append-only, checksummed record log with
//! crash recovery. Nothing in this crate ever overwrites an existing,
//! committed byte — see `docs/design/CONSTRAINTS.md` and
//! `docs/design/decisions/ADR-001-torn-tail-truncation.md` for the one
//! carefully-scoped exception (discarding an uncommitted torn tail after a
//! crash, which is recovery, not mutation).

pub mod log;
pub mod record;
pub mod segment;
pub mod store;

pub use log::{Log, LogError, OpenReport};
pub use record::{Record, RecordType};
pub use store::{Snapshot, Store, StoreError};
