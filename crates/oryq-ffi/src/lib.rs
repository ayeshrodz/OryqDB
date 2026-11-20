//! `oryq-ffi`: Panic-safe C Application Binary Interface (`include/oryq.h`) for OryqDB.
//!
//! This is the ONLY crate in the OryqDB workspace permitted to contain `unsafe` code,
//! strictly isolated to the FFI boundary with `// SAFETY:` invariants and `catch_unwind`
//! panic guards (`docs/ffi.md`).

pub mod api;
pub mod diagnostics;
pub mod guard;
pub mod handles;
pub mod status;

pub use api::*;
pub use handles::{OryqCursor, OryqDb, OryqReadTxn, OryqWriteTxn};
pub use status::OryqStatus;
