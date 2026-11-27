//! Panic-catching boundary ensuring no Rust unwind crosses the C ABI.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::diagnostics::{clear_last_error, set_last_error};
use crate::status::OryqStatus;

/// Executes `f` inside `catch_unwind`, clearing thread-local diagnostics on entry and converting
/// any panic into [`OryqStatus::ErrInternalPanic`].
pub fn ffi_guard<F>(f: F) -> OryqStatus
where
    F: FnOnce() -> Result<OryqStatus, (OryqStatus, String)>,
{
    clear_last_error();
    ffi_guard_inner(f)
}

/// Executes a cleanup/destructor closure inside `catch_unwind` WITHOUT clearing `LAST_ERROR`
/// on entry, so error-path cleanup (`oryq_cursor_close`, `oryq_read_txn_close`,
/// `oryq_write_txn_rollback`) preserves the caller's prior diagnostic message.
pub fn ffi_guard_preserve_last_error<F>(f: F) -> OryqStatus
where
    F: FnOnce() -> Result<OryqStatus, (OryqStatus, String)>,
{
    ffi_guard_inner(f)
}

fn ffi_guard_inner<F>(f: F) -> OryqStatus
where
    F: FnOnce() -> Result<OryqStatus, (OryqStatus, String)>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(status)) => status,
        Ok(Err((status, msg))) => {
            set_last_error(msg);
            status
        }
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                format!("internal panic: {s}")
            } else if let Some(s) = payload.downcast_ref::<String>() {
                format!("internal panic: {s}")
            } else {
                "internal panic (non-string payload)".to_owned()
            };
            set_last_error(msg);
            OryqStatus::ErrInternalPanic
        }
    }
}
