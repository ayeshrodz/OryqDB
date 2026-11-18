#![forbid(unsafe_code)]

//! `oryq-ffi` C ABI crate scaffold (scheduled for `W11`).

/// Returns the workspace version string exposed through `oryq-core`.
pub const fn ffi_scaffold_version() -> &'static str {
    oryq_core::version()
}
