//! Isolated OS-specific positioned I/O, file identity, locking, and directory sync adapters.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{
    file_identity, read_exact_at, sync_parent_dir, try_lock_exclusive, try_lock_shared, unlock,
    write_all_at,
};

#[cfg(windows)]
pub use windows::{
    file_identity, read_exact_at, sync_parent_dir, try_lock_exclusive, try_lock_shared, unlock,
    write_all_at,
};
