//! Stable C ABI status codes mapping [`oryq_core::ErrorCategory`] (`docs/ffi.md` §3).

use oryq_core::{ErrorCategory, ErrorEnvelope};

/// Stable C-compatible status code returned by `oryq_*` FFI functions.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OryqStatus {
    Ok = 0,
    CursorDone = 1,
    ErrInvalidArgument = 2,
    ErrIo = 3,
    ErrCorruption = 4,
    ErrNotFound = 5,
    ErrDuplicateKey = 6,
    ErrLockedByProcess = 7,
    ErrSnapshotTooOld = 8,
    ErrQueryMemoryLimit = 9,
    ErrParallelArrayIndex = 10,
    ErrUnsupportedFeature = 11,
    ErrTypeMismatch = 12,
    ErrBusy = 13,
    ErrCommitOutcomeUnknown = 14,
    ErrWalLimit = 15,
    ErrTooManyReaders = 16,
    ErrTransactionMemory = 17,
    ErrNumericOverflow = 18,
    ErrNumericPrecision = 19,
    ErrInvalidQuery = 20,
    ErrDocumentTooLarge = 21,
    ErrIndexKeyTooLarge = 22,
    ErrInvalidIdType = 23,
    ErrConflictingPath = 24,
    ErrIndexNotFound = 25,
    ErrInternalPanic = 255,
}

impl OryqStatus {
    pub fn from_error(err: &ErrorEnvelope) -> Self {
        let msg = err.message();
        match err.category() {
            ErrorCategory::InvalidArgument => {
                if msg.contains("parallel arrays") {
                    Self::ErrParallelArrayIndex
                } else if msg.contains("conflicting") && msg.contains("path") {
                    Self::ErrConflictingPath
                } else if msg.contains("_id") && (msg.contains("type") || msg.contains("invalid")) {
                    Self::ErrInvalidIdType
                } else {
                    Self::ErrInvalidArgument
                }
            }
            ErrorCategory::InvalidQuery => Self::ErrInvalidQuery,
            ErrorCategory::NotFound => {
                if msg.contains("index") {
                    Self::ErrIndexNotFound
                } else {
                    Self::ErrNotFound
                }
            }
            ErrorCategory::DuplicateKey => Self::ErrDuplicateKey,
            ErrorCategory::TypeMismatch => Self::ErrTypeMismatch,
            ErrorCategory::Overflow => {
                if msg.contains("index key") || msg.contains("max_index_key") {
                    Self::ErrIndexKeyTooLarge
                } else if msg.contains("precision") {
                    Self::ErrNumericPrecision
                } else {
                    Self::ErrNumericOverflow
                }
            }
            ErrorCategory::QuotaExceeded => {
                if msg.contains("document") && msg.contains("max_document_bytes") {
                    Self::ErrDocumentTooLarge
                } else if msg.contains("wal") || msg.contains("WAL") || msg.contains("version") {
                    Self::ErrWalLimit
                } else if msg.contains("transaction") {
                    Self::ErrTransactionMemory
                } else {
                    Self::ErrQueryMemoryLimit
                }
            }
            ErrorCategory::Busy => {
                if msg.contains("max_readers") || msg.contains("reader snapshot table full") {
                    Self::ErrTooManyReaders
                } else {
                    Self::ErrBusy
                }
            }
            ErrorCategory::Cancelled => Self::ErrBusy,
            ErrorCategory::LockedByProcess => Self::ErrLockedByProcess,
            ErrorCategory::SnapshotTooOld => Self::ErrSnapshotTooOld,
            ErrorCategory::UnsupportedFeature => Self::ErrUnsupportedFeature,
            ErrorCategory::Io => Self::ErrIo,
            ErrorCategory::Corruption => Self::ErrCorruption,
            ErrorCategory::CommitOutcomeUnknown => Self::ErrCommitOutcomeUnknown,
            ErrorCategory::Internal => Self::ErrInternalPanic,
        }
    }
}
