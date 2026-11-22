# OryqDB C ABI v1.1 Developer Guide (`include/oryq.h`)

The `oryq-ffi` crate exposes a panic-safe (`catch_unwind`), stable C99/C++11-compatible ABI (`ORYQ_ABI_VERSION_MAJOR = 1`, `ORYQ_ABI_VERSION_MINOR = 1`) defined in [include/oryq.h](../../include/oryq.h).

## 1. Building & Linking

```bash
cargo build --release -p oryq-ffi
```

- **Linux:** Link with `-Iinclude -Ltarget/release -loryq -Wl,-rpath,target/release -lm` (`liboryq.so` or static `liboryq.a`).
- **macOS:** Link with `-Iinclude -Ltarget/release -loryq -Wl,-rpath,target/release` (`liboryq.dylib` or static `liboryq.a`).
- **Windows:** Link against `target/release/oryq.dll.lib` with `oryq.dll` in your executable directory or `PATH`.

## 2. Handle Ownership & Lifecycle Rules

1. **Strict parent-child close order:**
   - Close every `OryqCursor*` (`oryq_cursor_close`) before closing its parent `OryqReadTxn*` (`oryq_read_txn_close`).
   - Close all active `OryqReadTxn*` and commit/rollback all active `OryqWriteTxn*` before calling `oryq_db_close(db)`.
   - `oryq_db_close` tracks active child handles and returns `ORYQ_ERR_BUSY` if any child transaction/cursor is still open.
2. **Write transaction consumption:**
   - Both `oryq_write_txn_commit(wtx)` and `oryq_write_txn_rollback(wtx)` consume and free `wtx`. Never reuse `wtx` after either call.
3. **String memory discipline:**
   - `oryq_cursor_next_json(cur, &json_str)` allocates a NUL-terminated UTF-8 string on the Rust heap; callers **MUST** release it with `oryq_free_string(json_str)` (never `free()`).
   - `oryq_version()` and `oryq_last_error_message()` return borrowed pointers (`oryq_version` is static; `oryq_last_error_message` is thread-local and valid until the next FFI call on the same thread). **NEVER** pass them to `oryq_free_string`.
4. **Zero-copy cursor slices:**
   - `oryq_cursor_get_utf8(cur, field, &ptr, &len)` returns a borrowed pointer into the cursor's current document buffer. It is **NOT NUL-terminated** — always respect `len`, and copy the bytes if needed beyond the next `oryq_cursor_next` / `oryq_cursor_next_json` / `oryq_cursor_close` call.

## 3. Error Handling & Diagnostics

Every fallible C ABI function returns `oryq_status`:
- `ORYQ_OK (0)`: Success.
- `ORYQ_CURSOR_DONE (1)`: Normal end of cursor iteration.
- `ORYQ_ERR_* (2..=25, 255)`: Error; call `oryq_last_error_message()` on the same thread for a human-readable diagnostic string.
- Rust panics never unwind across the FFI boundary; any unexpected panic is caught and returned as `ORYQ_ERR_INTERNAL_PANIC (255)`.

## 4. Building Bindings in Other Languages

- **Python (`ctypes`):** Official package in [bindings/python/oryqdb/__init__.py](../../bindings/python/oryqdb/__init__.py).
- **C99 / C++17:** Reference examples in [bindings/c/quickstart.c](../../bindings/c/quickstart.c) and [bindings/cpp/quickstart.cpp](../../bindings/cpp/quickstart.cpp).
- **Go (`cgo`):** `#cgo CFLAGS: -I../../include` and `#cgo LDFLAGS: -L../../target/release -loryq`, calling `C.oryq_db_open`, `C.oryq_cursor_next`, and `C.GoStringN(ptr, C.int(len))` for zero-copy UTF-8 extraction.
- **Node.js (`koffi` / `node-ffi-napi` / N-API):** Bind the exact prototypes in `include/oryq.h`, always freeing `oryq_cursor_next_json` outputs with `oryq_free_string`.
