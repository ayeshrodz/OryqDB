//! Positioned I/O, file identity registry, and isolated platform adapters.

pub mod platform;
pub mod registry;
pub mod vfs;

pub use registry::{FileIdentity, FileLease, FileRegistry};
pub use vfs::{FileHandle, OpenMode, OsVfs, StorageIo};
