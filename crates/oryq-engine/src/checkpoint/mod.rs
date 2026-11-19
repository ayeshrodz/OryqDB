//! Ordered checkpointing, WAL reset intent protocol, and sealed-file transitions
//! (`docs/transactions.md` §4.1 & §4.1.1).

pub mod copy;
pub mod reset;
pub mod seal;

pub use copy::copy_committed_pages_to_main;
pub use reset::{complete_interrupted_reset, reset_wal_after_full_checkpoint};
pub use seal::{unseal_for_first_write, write_fresh_wal_header};
