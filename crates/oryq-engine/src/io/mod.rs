//! Positioned I/O, file identity registry, and isolated platform adapters.

pub mod memory;
pub mod platform;
pub mod registry;
pub mod vfs;

pub use memory::MemoryVfs;
pub use registry::{FileIdentity, FileLease, FileRegistry};
pub use vfs::{FileHandle, OpenMode, OsVfs, StorageIo};
