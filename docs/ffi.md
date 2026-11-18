# OryqDB Subsystem Specification: C-Compatible Application Binary Interface (`include/oryq.h`)

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document specifies OryqDB's stable C-compatible Application Binary Interface (`crates/oryq-ffi` and `include/oryq.h`), enabling C, C++, Python, Go, Node.js, and .NET applications to embed OryqDB as a native shared (`liboryq.so`, `oryq.dll`, `liboryq.dylib`) or static (`liboryq.a`, `oryq.lib`) library without a Rust toolchain at runtime.

## 2. Architecture

`crates/oryq-ffi` is a thin, zero-logic adapter over `crates/oryq-core`:
* Native Rust applications consume `oryq-core` directly without passing through `oryq-ffi`.
* Foreign-language bindings call `extern "C"` functions prefixed with `oryq_`.
* Every FFI function executes inside an `ffi_guard` macro backed by `std::panic::catch_unwind`, ensuring no Rust panic ever unwinds across the C ABI stack frame.
* To preserve `OryqDoc`'s $O(\log k)$ zero-copy performance across FFI, `include/oryq.h` exposes **both** UTF-8 JSON convenience functions and **direct cursor-scoped zero-copy binary field accessors** (`oryq_cursor_get_utf8`, `oryq_cursor_get_i64`, `oryq_cursor_get_f64`, `oryq_cursor_get_bool`).

## 3. Public Interfaces (`include/oryq.h`)

```c
// Illustrative C Header Specification (Design Only — include/oryq.h)
#ifndef ORYQ_H
#define ORYQ_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ORYQ_ABI_VERSION_MAJOR 1
#define ORYQ_ABI_VERSION_MINOR 0

typedef struct oryq_db_t     oryq_db_t;
typedef struct oryq_txn_t    oryq_txn_t;
typedef struct oryq_cursor_t oryq_cursor_t;
typedef struct oryq_doc_t    oryq_doc_t;

typedef enum {
    ORYQ_OK                         = 0,
    ORYQ_CURSOR_DONE                = 1,
    ORYQ_ERR_INVALID_ARGUMENT       = 2,
    ORYQ_ERR_IO                     = 3,
    ORYQ_ERR_CORRUPTION             = 4,
    ORYQ_ERR_NOT_FOUND              = 5,
    ORYQ_ERR_DUPLICATE_KEY          = 6,
    ORYQ_ERR_LOCKED_BY_PROCESS      = 7,
    ORYQ_ERR_SNAPSHOT_TOO_OLD       = 8,
    ORYQ_ERR_QUERY_MEMORY_LIMIT     = 9,
    ORYQ_ERR_PARALLEL_ARRAY_INDEX   = 10,
    ORYQ_ERR_UNSUPPORTED_FEATURE    = 11,
    ORYQ_ERR_TYPE_MISMATCH          = 12,
    ORYQ_ERR_BUSY                   = 13,
    ORYQ_ERR_COMMIT_OUTCOME_UNKNOWN = 14,
    ORYQ_ERR_WAL_LIMIT              = 15,
    ORYQ_ERR_TOO_MANY_READERS       = 16,
    ORYQ_ERR_TRANSACTION_MEMORY    = 17,
    ORYQ_ERR_NUMERIC_OVERFLOW      = 18,
    ORYQ_ERR_NUMERIC_PRECISION     = 19,
    ORYQ_ERR_INVALID_QUERY         = 20,
    ORYQ_ERR_DOCUMENT_TOO_LARGE    = 21,
    ORYQ_ERR_INDEX_KEY_TOO_LARGE   = 22,
    ORYQ_ERR_INVALID_ID_TYPE       = 23,
    ORYQ_ERR_CONFLICTING_PATH      = 24,
    ORYQ_ERR_INDEX_NOT_FOUND       = 25,
    ORYQ_ERR_INTERNAL_PANIC         = 255
} oryq_status;

/* Lifecycle & Diagnostics */
const char *oryq_version(void);
uint32_t    oryq_abi_version(void);
oryq_status oryq_open(const char *path_utf8, oryq_db_t **out_db);
oryq_status oryq_open_with_options(const char *path_utf8, const char *options_json, size_t options_len, oryq_db_t **out_db);
void        oryq_close(oryq_db_t *db);
const char *oryq_last_error_message(const oryq_db_t *db);

/* Transactions */
oryq_status oryq_txn_begin_read(const oryq_db_t *db, oryq_txn_t **out_txn);
oryq_status oryq_txn_begin_write(const oryq_db_t *db, oryq_txn_t **out_txn);
oryq_status oryq_txn_commit(oryq_txn_t *txn);
oryq_status oryq_txn_rollback(oryq_txn_t *txn);
oryq_status oryq_txn_close(oryq_txn_t *txn);

/* Collection Lifecycle */
oryq_status oryq_collection_create(oryq_txn_t *txn, const char *name);
oryq_status oryq_collection_drop(oryq_txn_t *txn, const char *name);

/* JSON Document Operations */
oryq_status oryq_insert_json(oryq_txn_t *txn, const char *collection, const char *json_utf8, size_t json_len);
oryq_status oryq_update_json(oryq_txn_t *txn, const char *collection, const char *filter_json, size_t filter_len, const char *update_json, size_t update_len, uint64_t *out_modified_count);
oryq_status oryq_delete_json(oryq_txn_t *txn, const char *collection, const char *filter_json, size_t filter_len, uint64_t *out_deleted_count);

/* Streaming Cursors & Zero-Copy OryqDoc Field Accessors */
oryq_status oryq_find_json(oryq_txn_t *txn, const char *collection, const char *filter_json, size_t filter_len, oryq_cursor_t **out_cursor);
oryq_status oryq_aggregate_json(oryq_txn_t *txn, const char *collection, const char *pipeline_json, size_t pipeline_len, oryq_cursor_t **out_cursor);
oryq_status oryq_cursor_next(oryq_cursor_t *cursor);
void        oryq_cursor_close(oryq_cursor_t *cursor);

/* Ephemeral Zero-Copy Getters (Valid until next oryq_cursor_next or oryq_cursor_close) */
oryq_status oryq_cursor_get_utf8(const oryq_cursor_t *cursor, const char *dotted_path, size_t path_len, const char **out_utf8_ptr, size_t *out_utf8_len);
oryq_status oryq_cursor_get_i64(const oryq_cursor_t *cursor, const char *dotted_path, size_t path_len, int64_t *out_val);
oryq_status oryq_cursor_get_f64(const oryq_cursor_t *cursor, const char *dotted_path, size_t path_len, double *out_val);
oryq_status oryq_cursor_get_bool(const oryq_cursor_t *cursor, const char *dotted_path, size_t path_len, bool *out_val);
oryq_status oryq_cursor_get_raw_oryqdoc(const oryq_cursor_t *cursor, const uint8_t **out_bytes_ptr, size_t *out_bytes_len);

/* Owned Document & String Deallocation */
oryq_status oryq_cursor_current_to_json(const oryq_cursor_t *cursor, char **out_json_cstr, size_t *out_json_len);
oryq_status oryq_cursor_clone_doc(const oryq_cursor_t *cursor, oryq_doc_t **out_doc);
void        oryq_doc_free(oryq_doc_t *doc);
void        oryq_string_free(char *str);

#ifdef __cplusplus
}
#endif

#endif /* ORYQ_H */
```

## 4. Internal Data Structures

* **`oryq_db_t`:** Owning handle to `Arc<DatabaseInner>`; diagnostics are thread-local, never a mutable per-database borrowed buffer.
* **`oryq_txn_t`:** Owned transaction state with `Arc<DatabaseInner>`, snapshot token/writer guard and private workspace. Implement owned internal transaction/cursor types; never extend borrowed Rust lifetimes to `'static` by transmute. Track active child cursors and an Active/Committed/RolledBack/Faulted state.
* **`oryq_cursor_t`:** Holds an owning reference to transaction state, its page guards and bounded scratch. Register/unregister the child count at cursor creation/destruction; every access validates transaction and cursor state.
* **`oryq_doc_t`:** Heap-allocated `Box<OryqDocBuf>` wrapping a reference-counted immutable `OryqDoc` byte buffer (`Arc<[u8]>`).

### 4.1 Handle State, Completion & Output Rules

`oryq_close(db)` releases that caller's database handle; live transaction/cursor owners retain the engine and file lock. No further call may use that freed database pointer. `oryq_txn_commit`, `oryq_txn_rollback`, and `oryq_txn_close` return `ORYQ_ERR_BUSY` without changing anything when child cursors exist. Close cursors first. Mutating transaction operations also return ORYQ_ERR_BUSY while child cursors exist, preserving the immutable read projection used by those cursors; database/transaction handle movement never moves a thread-affine borrowed mutex guard. A successful commit (write) or end (read) moves Active to Committed; rollback moves Active to RolledBack. Neither frees the transaction handle: `oryq_txn_close` frees it, rolling back an Active transaction. Any commit I/O failure moves it to Faulted and follows the uncertain-outcome contract in transactions.md; validation failures leave Active with statement changes rolled back. Closed/freed handles are caller errors and cannot be detected safely by dereferencing opaque pointers.

Cursor state is BeforeFirst / Positioned / Done / Failed. Getters require Positioned; next invalidates every previous borrowed pointer even when it returns DONE or an error. Repeated next after Done returns DONE. Materialized raw documents and overflow fields borrow cursor-owned scratch until that same boundary. A getter must not relocate or invalidate previously returned scratch pointers within a step: prepare/cache the needed storage before exposing pointers, or retain all step allocations until next. Database close cannot invalidate a live cursor; transaction completion cannot proceed until cursor close. All handles of a transaction share one external serialization requirement, including concurrent transaction and child-cursor calls.

For status functions, initialize valid output addresses to NULL/0 before work; a required output address must be non-NULL. Length-bearing input may use `(NULL,0)` to mean empty bytes (which may subsequently fail JSON validation), but NULL with positive length is invalid. NUL-terminated collection/path arguments require valid readable UTF-8 through NUL; counted buffers require readable storage for the supplied length and lengths no greater than `isize::MAX`. Destructors accept NULL as no-op. No promise covers dangling, forged, double-freed or undersized foreign pointers. Error enum values above are stable; map all validation variants explicitly, never renumber existing values. Unknown future codes must be treated as generic errors by callers.

Collection creation is explicit through `oryq_collection_create(txn,name)` and deletion through `oryq_collection_drop(txn,name)` (declared above); insert never silently creates a misspelled collection. Optional aggregation symbols exist in M6 and return `UnsupportedFeature` until M8. The C test creates its collection, verifies both parent destruction orders, BUSY completion, failed sync, panic fallback, diagnostics after failed open, and output initialization.

## 5. Invariants

1. **No Unwinding Across FFI:** Every `extern "C"` entrypoint catches Rust unwinding panics. Status functions return `ORYQ_ERR_INTERNAL_PANIC`; void destructors store a diagnostic and return, pointer getters return NULL, and integer version getters return 0. Fatal allocation/process aborts are outside this guarantee. A panic during mutation faults the transaction/database and requires reopen.
2. **Null-Pointer & Length Defence:** Passing `NULL` for any required pointer argument returns `ORYQ_ERR_INVALID_ARGUMENT` (`2`) without dereferencing the pointer.
3. **Ephemeral Slice Lifetime Contract:** Pointers written to `*out_utf8_ptr` by `oryq_cursor_get_utf8()` point directly into the cursor's currently pinned buffer-pool page (or normalized delta patch) without null termination (length is explicitly returned in `*out_utf8_len`). These pointers are valid strictly until the next call to `oryq_cursor_next()` or `oryq_cursor_close()` on that cursor handle.
4. **Feature-Flag Symbol Invariance:** Every symbol declared in `include/oryq.h` is always exported by `liboryq` regardless of which compile-time Cargo features (`fts`, `vec`, `multi-process`) are enabled.

## 6. Error Handling

* Every fallible C function returns an `oryq_status` enum value (`ORYQ_OK == 0` on success, `ORYQ_CURSOR_DONE == 1` at end of cursor iteration).
* Diagnostics use one thread-local `CString`. `oryq_last_error_message(db)` ignores `db` and accepts NULL, allowing failed-open diagnostics. Its pointer is valid until the next non-diagnostic ABI call on the same thread; other threads cannot invalidate it. Callers copy the string to retain it.

## 7. Concurrency Behavior

| Handle Type | Rust Trait Equivalent | Multi-Thread Sharing Rule for C Callers |
| :--- | :--- | :--- |
| `oryq_db_t *` | `Send + Sync` | Safe to share and invoke concurrently across multiple OS threads. |
| `oryq_txn_t *` | `Send + !Sync` | May be moved between threads, but must not be invoked concurrently by two threads without an external mutex. |
| `oryq_cursor_t *` | `Send + !Sync` | Single-thread active access only; bound to its creating `oryq_txn_t *`. |
| `oryq_doc_t *` | `Send + Sync` | Immutable owned document buffer; safe for concurrent read access until `oryq_doc_free()`. |

## 8. Performance Characteristics

* **`oryq_cursor_get_*` Latency:** Zero heap allocations and zero UTF-8 re-encoding; performs an $O(\log k)$ binary search directly on the pinned `OryqDocView` in the Rust buffer pool.
* **Handle Safety Overhead:** Holding an `Arc<DatabaseInner>` clone inside `oryq_txn_t` and `oryq_cursor_t` costs one atomic increment on transaction creation and prevents C use-after-free crashes if `oryq_close(db)` is called while a handle is still open.

## 9. Known Limitations

* Strings returned by `oryq_cursor_get_utf8()` are UTF-8 byte slices `(const char *ptr, size_t len)` and are **not** null-terminated inside the page buffer. Callers requiring null-terminated C strings must either pass `len` to length-aware functions (`printf("%.*s", (int)len, ptr)`) or call `oryq_cursor_current_to_json()`.

## 10. Testing Methodology

* **Compiled Native C Integration Suite (`tests/c_abi/test_oryq_c_api.c`):** Compiled with `cc -Wall -Wextra -Werror` and executed under Valgrind / AddressSanitizer against `liboryq`, exercising database creation, transactions, JSON insertion, zero-copy cursor field reading, updates, deletions, reopen persistence, null-pointer rejection, and panic containment.
* **C ABI Fuzzing (`fuzz/fuzz_targets/fuzz_c_abi.rs`):** Feeds arbitrary non-UTF-8 byte buffers, zero/max lengths, and invalid JSON filters into all `oryq_*` entrypoints.
