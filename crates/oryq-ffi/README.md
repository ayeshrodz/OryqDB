# `oryq-ffi`

Panic-safe C Application Binary Interface (`include/oryq.h`) for embedding OryqDB in C, C++, Python, Go, Node.js, and .NET applications.

- Every `extern "C"` entry point runs inside `guard::ffi_guard` (`std::panic::catch_unwind`).
- Thread-local diagnostics via `oryq_last_error_message()`.
- Opaque handle lifecycle tracking prevents closing a database while child transactions remain open (`ORYQ_ERR_BUSY`).
- Exposes both JSON convenience APIs and zero-copy cursor field accessors (`oryq_cursor_get_i64`).
