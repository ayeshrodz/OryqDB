//! Exported `extern "C"` symbols for OryqDB (`include/oryq.h`).

use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use oryq_core::{
    parse_json_value_str, to_json_string, Database, DatabaseConfig, SortOrder, Value, ValueView,
};

use crate::diagnostics::last_error_ptr;
use crate::guard::{ffi_guard, ffi_guard_preserve_last_error};
use crate::handles::{OryqCursor, OryqDb, OryqReadTxn, OryqWriteTxn};
use crate::status::OryqStatus;

/// ABI version packed as `(major << 16) | minor` (`1.2` -> `0x0001_0002`).
pub const ORYQ_ABI_VERSION: u32 = (1 << 16) | 2;

const VERSION_CSTR: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

fn read_cstr<'a>(ptr: *const c_char, name: &str) -> Result<&'a str, (OryqStatus, String)> {
    if ptr.is_null() {
        return Err((
            OryqStatus::ErrInvalidArgument,
            format!("null pointer passed for '{name}'"),
        ));
    }
    // SAFETY: Caller guarantees `ptr` points to a valid NUL-terminated C string for this call.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().map_err(|_| {
        (
            OryqStatus::ErrInvalidArgument,
            format!("invalid UTF-8 string passed for '{name}'"),
        )
    })
}

fn parse_id_json(id_json: &str) -> Result<Value, (OryqStatus, String)> {
    // Match RFC 8259 JSON whitespace only (` `, `\t`, `\n`, `\r`), never Unicode `White_Space`.
    let trimmed = id_json.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
    // Fast path for plain non-leading-zero JSON integer `_id` literals (avoids serde_json + DocBuf).
    if !trimmed.is_empty() && trimmed != "-0" {
        let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
        let valid_json_int = !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits.len() == 1 || !digits.starts_with('0'));
        if valid_json_int {
            if let Ok(n) = trimmed.parse::<i64>() {
                return Ok(Value::Int64(n));
            }
        }
    }
    // Parse strictly as a standalone JSON value (prevents JSON key-injection payloads like `1,"x":2`).
    parse_json_value_str(id_json).map_err(|e| (OryqStatus::from_error(&e), e.to_string()))
}

#[no_mangle]
pub extern "C" fn oryq_version() -> *const c_char {
    VERSION_CSTR.as_ptr().cast::<c_char>()
}

#[no_mangle]
pub extern "C" fn oryq_abi_version() -> u32 {
    ORYQ_ABI_VERSION
}

#[no_mangle]
pub extern "C" fn oryq_last_error_message() -> *const c_char {
    last_error_ptr()
}

/// Frees a heap-allocated UTF-8 string returned by `oryq_*` APIs (e.g. `oryq_cursor_next_json`).
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by OryqDB and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn oryq_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        // SAFETY: `ptr` was created by `CString::into_raw` inside this library.
        let _ = unsafe { CString::from_raw(ptr) };
    }
}

/// Opens or creates an OryqDB database file at `path_utf8` (pass `":memory:"` for an isolated,
/// zero-disk ephemeral in-memory database).
///
/// # Safety
/// `path_utf8` must be a valid NUL-terminated UTF-8 string; `out_db` must be a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_open(
    path_utf8: *const c_char,
    out_db: *mut *mut OryqDb,
) -> OryqStatus {
    ffi_guard(|| {
        if out_db.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "out_db must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_db` is verified non-null; zero-init before fallible work.
        unsafe {
            *out_db = std::ptr::null_mut();
        }
        let path_str = read_cstr(path_utf8, "path_utf8")?;
        let db = Database::open(Path::new(path_str), DatabaseConfig::default())
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let boxed = Box::new(OryqDb::new(db));
        // SAFETY: `out_db` is verified non-null and owned by the caller for writing the handle.
        unsafe {
            *out_db = Box::into_raw(boxed);
        }
        Ok(OryqStatus::Ok)
    })
}

/// Closes an open [`OryqDb`] handle. Returns `ORYQ_ERR_BUSY` if any child transaction is still open.
///
/// # Safety
/// `db` must be null or a valid handle returned by `oryq_db_open` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_close(db: *mut OryqDb) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() {
            return Ok(OryqStatus::Ok);
        }
        // SAFETY: Caller passes an owned `OryqDb` pointer from `oryq_db_open`.
        let db_ref = unsafe { &*db };
        if !db_ref.try_begin_close() {
            return Err((
                OryqStatus::ErrBusy,
                "cannot close database while child read/write transactions are still open"
                    .to_owned(),
            ));
        }
        // SAFETY: `try_begin_close` atomically verified 0 children and set CLOSING_BIT;
        // reclaim Box allocation and drop `Database`.
        let _ = unsafe { Box::from_raw(db) };
        Ok(OryqStatus::Ok)
    })
}

/// Runs a passive WAL checkpoint on `db`.
///
/// # Safety
/// `db` must be a valid open handle from `oryq_db_open`.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_checkpoint(db: *const OryqDb) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "db is null".to_owned()));
        }
        // SAFETY: `db` is valid for the duration of this call.
        let db_ref = unsafe { &*db };
        db_ref
            .db
            .checkpoint(true)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Compacts `db` in place (`VACUUM`), reclaiming free space, shrinking the `.oryq` file,
/// and removing any companion `.oryq-wal` sidecar.
///
/// # Safety
/// `db` must be a valid open handle from `oryq_db_open`.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_vacuum(db: *const OryqDb) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "db is null".to_owned()));
        }
        // SAFETY: `db` is valid for the duration of this call.
        let db_ref = unsafe { &*db };
        if db_ref.has_live_children() {
            return Err((
                OryqStatus::ErrBusy,
                "cannot VACUUM while child read/write transactions are still open".to_owned(),
            ));
        }
        db_ref
            .db
            .vacuum()
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Begins a snapshot-isolated read transaction.
///
/// # Safety
/// `db` must be valid; `out_txn` must be a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn oryq_read_txn_begin(
    db: *const OryqDb,
    out_txn: *mut *mut OryqReadTxn,
) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() || out_txn.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "db and out_txn must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_txn` is verified non-null; zero-init before fallible work.
        unsafe {
            *out_txn = std::ptr::null_mut();
        }
        // SAFETY: `db` is valid for this call.
        let db_ref = unsafe { &*db };
        let rtx =
            OryqReadTxn::begin(db_ref).map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        // SAFETY: `out_txn` is non-null and writable.
        unsafe {
            *out_txn = Box::into_raw(Box::new(rtx));
        }
        Ok(OryqStatus::Ok)
    })
}

/// Closes a read transaction handle.
///
/// # Safety
/// `txn` must be null or a valid pointer from `oryq_read_txn_begin` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn oryq_read_txn_close(txn: *mut OryqReadTxn) -> OryqStatus {
    ffi_guard_preserve_last_error(|| {
        if !txn.is_null() {
            // SAFETY: Reclaims owned `OryqReadTxn`.
            let _ = unsafe { Box::from_raw(txn) };
        }
        Ok(OryqStatus::Ok)
    })
}

/// Begins an exclusive write transaction.
///
/// # Safety
/// `db` must be valid; `out_txn` must be a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn oryq_write_txn_begin(
    db: *const OryqDb,
    out_txn: *mut *mut OryqWriteTxn,
) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() || out_txn.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "db and out_txn must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_txn` is verified non-null; zero-init before fallible work.
        unsafe {
            *out_txn = std::ptr::null_mut();
        }
        // SAFETY: `db` is valid for this call.
        let db_ref = unsafe { &*db };
        let wtx =
            OryqWriteTxn::begin(db_ref).map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        // SAFETY: `out_txn` is non-null and writable.
        unsafe {
            *out_txn = Box::into_raw(Box::new(wtx));
        }
        Ok(OryqStatus::Ok)
    })
}

/// Commits and frees a write transaction handle.
///
/// # Safety
/// `txn` must be a valid pointer from `oryq_write_txn_begin` and must not be used afterward.
#[no_mangle]
pub unsafe extern "C" fn oryq_write_txn_commit(txn: *mut OryqWriteTxn) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        // SAFETY: Takes ownership of `OryqWriteTxn` so handle is always freed.
        let mut boxed = unsafe { Box::from_raw(txn) };
        let inner = boxed.tx.take().ok_or_else(|| {
            (
                OryqStatus::ErrInvalidArgument,
                "write transaction already finalized".to_owned(),
            )
        })?;
        inner
            .commit()
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Rolls back and frees a write transaction handle.
///
/// # Safety
/// `txn` must be null or a valid pointer from `oryq_write_txn_begin` and must not be used afterward.
#[no_mangle]
pub unsafe extern "C" fn oryq_write_txn_rollback(txn: *mut OryqWriteTxn) -> OryqStatus {
    ffi_guard_preserve_last_error(|| {
        if !txn.is_null() {
            // SAFETY: Takes ownership and drops the transaction (which rolls back automatically).
            let _ = unsafe { Box::from_raw(txn) };
        }
        Ok(OryqStatus::Ok)
    })
}

/// Creates a collection inside an open write transaction.
///
/// # Safety
/// `txn` must be valid; `name` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn oryq_create_collection(
    txn: *mut OryqWriteTxn,
    name: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(name, "name")?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        wtx.create_collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Drops a collection inside an open write transaction.
///
/// # Safety
/// `txn` must be valid; `name` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn oryq_drop_collection(
    txn: *mut OryqWriteTxn,
    name: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(name, "name")?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        wtx.drop_collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Inserts a JSON document into `collection`.
///
/// # Safety
/// `txn` must be valid; `collection` and `doc_json` must be valid NUL-terminated UTF-8 strings.
#[no_mangle]
pub unsafe extern "C" fn oryq_insert_json(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    doc_json: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let json = read_cstr(doc_json, "doc_json")?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        let mut col = wtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        col.insert_json(json)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Updates a document by `_id` JSON literal using a JSON update operator document.
///
/// # Safety
/// All pointers must be valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn oryq_update_by_id_json(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    id_json: *const c_char,
    update_json: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let id_val = parse_id_json(read_cstr(id_json, "id_json")?)?;
        let upd = read_cstr(update_json, "update_json")?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        let mut col = wtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let updated = col
            .update_by_id(&id_val, upd)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        if updated {
            Ok(OryqStatus::Ok)
        } else {
            Ok(OryqStatus::ErrNotFound)
        }
    })
}

/// Deletes a document by `_id` JSON literal.
///
/// # Safety
/// All pointers must be valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn oryq_delete_by_id_json(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    id_json: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let id_val = parse_id_json(read_cstr(id_json, "id_json")?)?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        let mut col = wtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let deleted = col
            .delete_by_id(&id_val)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        if deleted {
            Ok(OryqStatus::Ok)
        } else {
            Ok(OryqStatus::ErrNotFound)
        }
    })
}

/// Executes a JSON `find()` query on a read transaction and returns an [`OryqCursor`].
///
/// # Safety
/// `txn` and `collection` and `out_cursor` must be valid; `filter_json` may be null for match-all.
#[no_mangle]
pub unsafe extern "C" fn oryq_find_json(
    txn: *const OryqReadTxn,
    collection: *const c_char,
    filter_json: *const c_char,
    out_cursor: *mut *mut OryqCursor,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() || out_cursor.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "txn and out_cursor must not be null".to_owned(),
            ));
        }
        let col_name = read_cstr(collection, "collection")?;
        let filter_opt = if filter_json.is_null() {
            None
        } else {
            Some(read_cstr(filter_json, "filter_json")?)
        };
        // SAFETY: `txn` is valid for this call.
        let rtx = unsafe { &*txn }
            .tx
            .as_ref()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn closed".to_owned()))?;
        let col = rtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let cursor = col
            .find_json(filter_opt, None, None, 0, None)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        // SAFETY: `out_cursor` is non-null and writable.
        unsafe {
            *out_cursor = Box::into_raw(Box::new(OryqCursor::new(cursor)));
        }
        Ok(OryqStatus::Ok)
    })
}

/// Advances `cursor` to the next matching document and returns its JSON string in `*out_json`.
/// Caller must free `*out_json` with `oryq_free_string`. Returns `ORYQ_CURSOR_DONE` at end.
///
/// # Safety
/// `cursor` and `out_json` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_next_json(
    cursor: *mut OryqCursor,
    out_json: *mut *mut c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_json.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor and out_json must not be null".to_owned(),
            ));
        }
        // SAFETY: `cursor` is exclusively borrowed for this call.
        let cur = unsafe { &mut *cursor };
        match cur.backing.next_row() {
            None => {
                cur.current = None;
                cur.current_score = None;
                // SAFETY: `out_json` is non-null.
                unsafe {
                    *out_json = std::ptr::null_mut();
                }
                Ok(OryqStatus::CursorDone)
            }
            Some((doc, score)) => {
                let json = to_json_string(&doc.view())
                    .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
                let cstr = CString::new(json).map_err(|_| {
                    (
                        OryqStatus::ErrInternalPanic,
                        "encoded JSON contained interior NUL".to_owned(),
                    )
                })?;
                cur.current = Some(doc);
                cur.current_score = score;
                // SAFETY: `out_json` is non-null.
                unsafe {
                    *out_json = cstr.into_raw();
                }
                Ok(OryqStatus::Ok)
            }
        }
    })
}

/// Zero-copy `i64` field accessor on the cursor's current document.
///
/// # Safety
/// `cursor`, `field`, and `out_val` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_get_i64(
    cursor: *const OryqCursor,
    field: *const c_char,
    out_val: *mut i64,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_val.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor and out_val must not be null".to_owned(),
            ));
        }
        let field_name = read_cstr(field, "field")?;
        // SAFETY: `cursor` is valid for this call.
        let cur = unsafe { &*cursor };
        let doc = cur.current.as_ref().ok_or_else(|| {
            (
                OryqStatus::ErrNotFound,
                "cursor has no active current document".to_owned(),
            )
        })?;
        match doc.view().get_field(field_name) {
            Some(ValueView::Int64(n)) => {
                // SAFETY: `out_val` is non-null.
                unsafe {
                    *out_val = n;
                }
                Ok(OryqStatus::Ok)
            }
            Some(_) => Err((
                OryqStatus::ErrTypeMismatch,
                format!("field '{field_name}' is not an Int64"),
            )),
            None => Err((
                OryqStatus::ErrNotFound,
                format!("field '{field_name}' not found"),
            )),
        }
    })
}

/// Closes and frees a cursor handle.
///
/// # Safety
/// `cursor` must be null or a valid pointer from `oryq_find_json`, `oryq_aggregate_json`,
/// `oryq_search_fts`, or `oryq_search_vector` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_close(cursor: *mut OryqCursor) -> OryqStatus {
    ffi_guard_preserve_last_error(|| {
        if !cursor.is_null() {
            // SAFETY: Reclaims owned `OryqCursor`.
            let _ = unsafe { Box::from_raw(cursor) };
        }
        Ok(OryqStatus::Ok)
    })
}

/// Advances `cursor` to the next matching document without serializing JSON.
/// Use with zero-copy `oryq_cursor_get_*` accessors. Returns `ORYQ_CURSOR_DONE` at end.
///
/// # Safety
/// `cursor` must be a valid pointer from `oryq_find_json`, `oryq_aggregate_json`,
/// `oryq_search_fts`, or `oryq_search_vector`.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_next(cursor: *mut OryqCursor) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor must not be null".to_owned(),
            ));
        }
        // SAFETY: `cursor` is exclusively borrowed for this call.
        let cur = unsafe { &mut *cursor };
        match cur.backing.next_row() {
            None => {
                cur.current = None;
                cur.current_score = None;
                Ok(OryqStatus::CursorDone)
            }
            Some((doc, score)) => {
                cur.current = Some(doc);
                cur.current_score = score;
                Ok(OryqStatus::Ok)
            }
        }
    })
}

/// Zero-copy UTF-8 string field accessor on the cursor's current document.
///
/// Sets `*out_ptr` to a borrowed UTF-8 byte pointer (NOT NUL-terminated) and `*out_len` to its
/// byte length. The returned pointer remains valid until the next `oryq_cursor_next*` or
/// `oryq_cursor_close` call on `cursor`.
///
/// # Safety
/// `cursor`, `field`, `out_ptr`, and `out_len` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_get_utf8(
    cursor: *const OryqCursor,
    field: *const c_char,
    out_ptr: *mut *const c_char,
    out_len: *mut usize,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_ptr.is_null() || out_len.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor, out_ptr, and out_len must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_ptr` and `out_len` are verified non-null; initialize before fallible work.
        unsafe {
            *out_ptr = std::ptr::null();
            *out_len = 0;
        }
        let field_name = read_cstr(field, "field")?;
        // SAFETY: `cursor` is valid for this call.
        let cur = unsafe { &*cursor };
        let doc = cur.current.as_ref().ok_or_else(|| {
            (
                OryqStatus::ErrNotFound,
                "cursor has no active current document".to_owned(),
            )
        })?;
        match doc.view().get_field(field_name) {
            Some(ValueView::String(s)) => {
                // SAFETY: `out_ptr` and `out_len` are non-null and writable.
                // `s` borrows directly from `cur.current`'s immutable `Arc<[u8]>` buffer.
                unsafe {
                    *out_ptr = s.as_ptr().cast::<c_char>();
                    *out_len = s.len();
                }
                Ok(OryqStatus::Ok)
            }
            Some(_) => Err((
                OryqStatus::ErrTypeMismatch,
                format!("field '{field_name}' is not a String"),
            )),
            None => Err((
                OryqStatus::ErrNotFound,
                format!("field '{field_name}' not found"),
            )),
        }
    })
}

/// Zero-copy `f64` field accessor on the cursor's current document.
///
/// # Safety
/// `cursor`, `field`, and `out_val` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_get_f64(
    cursor: *const OryqCursor,
    field: *const c_char,
    out_val: *mut f64,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_val.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor and out_val must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_val` is verified non-null; initialize before fallible work.
        unsafe {
            *out_val = 0.0;
        }
        let field_name = read_cstr(field, "field")?;
        // SAFETY: `cursor` is valid for this call.
        let cur = unsafe { &*cursor };
        let doc = cur.current.as_ref().ok_or_else(|| {
            (
                OryqStatus::ErrNotFound,
                "cursor has no active current document".to_owned(),
            )
        })?;
        match doc.view().get_field(field_name) {
            Some(ValueView::Float64(v)) => {
                // SAFETY: `out_val` is non-null.
                unsafe {
                    *out_val = v;
                }
                Ok(OryqStatus::Ok)
            }
            Some(_) => Err((
                OryqStatus::ErrTypeMismatch,
                format!("field '{field_name}' is not a Float64"),
            )),
            None => Err((
                OryqStatus::ErrNotFound,
                format!("field '{field_name}' not found"),
            )),
        }
    })
}

/// Zero-copy `bool` field accessor on the cursor's current document.
///
/// # Safety
/// `cursor`, `field`, and `out_val` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_get_bool(
    cursor: *const OryqCursor,
    field: *const c_char,
    out_val: *mut bool,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_val.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor and out_val must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_val` is verified non-null; initialize before fallible work.
        unsafe {
            *out_val = false;
        }
        let field_name = read_cstr(field, "field")?;
        // SAFETY: `cursor` is valid for this call.
        let cur = unsafe { &*cursor };
        let doc = cur.current.as_ref().ok_or_else(|| {
            (
                OryqStatus::ErrNotFound,
                "cursor has no active current document".to_owned(),
            )
        })?;
        match doc.view().get_field(field_name) {
            Some(ValueView::Bool(b)) => {
                // SAFETY: `out_val` is non-null.
                unsafe {
                    *out_val = b;
                }
                Ok(OryqStatus::Ok)
            }
            Some(_) => Err((
                OryqStatus::ErrTypeMismatch,
                format!("field '{field_name}' is not a Bool"),
            )),
            None => Err((
                OryqStatus::ErrNotFound,
                format!("field '{field_name}' not found"),
            )),
        }
    })
}

/// Executes a JSON aggregation pipeline on a read transaction and returns an [`OryqCursor`].
///
/// # Safety
/// `txn`, `collection`, `pipeline_json`, and `out_cursor` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_aggregate_json(
    txn: *const OryqReadTxn,
    collection: *const c_char,
    pipeline_json: *const c_char,
    out_cursor: *mut *mut OryqCursor,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() || out_cursor.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "txn and out_cursor must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_cursor` is verified non-null; initialize before fallible work.
        unsafe {
            *out_cursor = std::ptr::null_mut();
        }
        let col_name = read_cstr(collection, "collection")?;
        let pipe = read_cstr(pipeline_json, "pipeline_json")?;
        // SAFETY: `txn` is valid for this call.
        let rtx = unsafe { &*txn }
            .tx
            .as_ref()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn closed".to_owned()))?;
        let col = rtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let docs = col
            .aggregate_json(pipe)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        // SAFETY: `out_cursor` is non-null and writable.
        unsafe {
            *out_cursor = Box::into_raw(Box::new(OryqCursor::from_docs(docs)));
        }
        Ok(OryqStatus::Ok)
    })
}

/// Creates a secondary index on `field_path` inside an open write transaction.
/// `sort_order`: `1` for ascending, `-1` for descending (0 is rejected as invalid).
/// `name_or_null`: optional index name (null auto-generates a name).
///
/// # Safety
/// `txn`, `collection`, and `field_path` must be valid; `name_or_null` may be null.
#[no_mangle]
pub unsafe extern "C" fn oryq_create_index(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    field_path: *const c_char,
    sort_order: i32,
    unique: bool,
    sparse: bool,
    name_or_null: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let order = match sort_order {
            1 => SortOrder::Asc,
            -1 => SortOrder::Desc,
            other => {
                return Err((
                    OryqStatus::ErrInvalidArgument,
                    format!("sort_order must be 1 (Asc) or -1 (Desc), got {other}"),
                ));
            }
        };
        let col_name = read_cstr(collection, "collection")?;
        let path = read_cstr(field_path, "field_path")?;
        let name_opt = if name_or_null.is_null() {
            None
        } else {
            Some(read_cstr(name_or_null, "name_or_null")?)
        };
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        let mut col = wtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        col.create_index(path, order, unique, sparse, name_opt)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Drops a non-primary index by name inside an open write transaction.
///
/// # Safety
/// `txn`, `collection`, and `index_name` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_drop_index(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    index_name: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let idx_name = read_cstr(index_name, "index_name")?;
        // SAFETY: `txn` is valid for this call.
        let wtx = unsafe { &mut *txn }
            .tx
            .as_mut()
            .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
        let mut col = wtx
            .collection(col_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        let dropped = col
            .drop_index(idx_name)
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        if dropped {
            Ok(OryqStatus::Ok)
        } else {
            Err((
                OryqStatus::ErrIndexNotFound,
                format!("index '{idx_name}' not found on collection '{col_name}'"),
            ))
        }
    })
}

/// Creates a standalone sealed online backup of `db` at `dest_path_utf8`.
///
/// # Safety
/// `db` and `dest_path_utf8` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_backup_to(
    db: *const OryqDb,
    dest_path_utf8: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "db is null".to_owned()));
        }
        let dest = read_cstr(dest_path_utf8, "dest_path_utf8")?;
        // SAFETY: `db` is valid for this call.
        let db_ref = unsafe { &*db };
        db_ref
            .db
            .backup_to(Path::new(dest))
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        Ok(OryqStatus::Ok)
    })
}

/// Runs a snapshot-consistent integrity check on `db`.
/// Writes `true` to `*out_ok` if clean; returns `ORYQ_ERR_CORRUPTION` if issues are found.
///
/// # Safety
/// `db` and `out_ok` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_db_integrity_check(
    db: *const OryqDb,
    out_ok: *mut bool,
) -> OryqStatus {
    ffi_guard(|| {
        if db.is_null() || out_ok.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "db and out_ok must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_ok` is verified non-null; initialize before fallible work.
        unsafe {
            *out_ok = false;
        }
        // SAFETY: `db` is valid for this call.
        let db_ref = unsafe { &*db };
        let report = db_ref
            .db
            .integrity_check()
            .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
        // SAFETY: `out_ok` is non-null.
        unsafe {
            *out_ok = report.ok;
        }
        if report.ok {
            Ok(OryqStatus::Ok)
        } else {
            Err((
                OryqStatus::ErrCorruption,
                format!("integrity check failed: {}", report.issues.join("; ")),
            ))
        }
    })
}

/// Returns the BM25 relevance score (FTS) or distance (Vector) for the current row of a scored
/// cursor. Returns `ORYQ_ERR_NOT_FOUND` if the cursor is unscored or not positioned on a row.
///
/// # Safety
/// `cursor` and `out_score` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_get_score(
    cursor: *const OryqCursor,
    out_score: *mut f64,
) -> OryqStatus {
    ffi_guard(|| {
        if cursor.is_null() || out_score.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "cursor and out_score must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_score` is non-null; zero-initialize before fallible lookup.
        unsafe {
            *out_score = 0.0;
        }
        // SAFETY: `cursor` is valid for this call.
        let cur = unsafe { &*cursor };
        match cur.current_score {
            Some(score) => {
                // SAFETY: `out_score` is non-null.
                unsafe {
                    *out_score = score;
                }
                Ok(OryqStatus::Ok)
            }
            None => Err((
                OryqStatus::ErrNotFound,
                "cursor has no active score for the current row".to_owned(),
            )),
        }
    })
}

/// Creates an Okapi BM25 full-text search index over comma-separated `fields_csv` inside an open
/// write transaction (`--features fts`). Returns `ORYQ_ERR_UNSUPPORTED_FEATURE` when compiled
/// without the `fts` feature.
///
/// # Safety
/// `txn`, `collection`, and `fields_csv` must be valid; `name_or_null` may be null.
#[no_mangle]
pub unsafe extern "C" fn oryq_create_fts_index(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    fields_csv: *const c_char,
    name_or_null: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let csv = read_cstr(fields_csv, "fields_csv")?;
        let fields: Vec<&str> = csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if fields.is_empty() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "fields_csv must specify at least one field path".to_owned(),
            ));
        }
        let name_opt = if name_or_null.is_null() {
            None
        } else {
            Some(read_cstr(name_or_null, "name_or_null")?)
        };
        #[cfg(feature = "fts")]
        {
            // SAFETY: `txn` is valid for this call.
            let wtx = unsafe { &mut *txn }
                .tx
                .as_mut()
                .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
            let mut col = wtx
                .collection(col_name)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            col.create_fts_index(&fields, name_opt)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            Ok(OryqStatus::Ok)
        }
        #[cfg(not(feature = "fts"))]
        {
            let _ = (col_name, fields, name_opt);
            Err((
                OryqStatus::ErrUnsupportedFeature,
                "oryq-ffi compiled without 'fts' feature".to_owned(),
            ))
        }
    })
}

/// Executes an Okapi BM25 full-text search against a named FTS index (`--features fts`), returning
/// a scored [`OryqCursor`] ordered by descending BM25 score (`oryq_cursor_get_score`).
///
/// # Safety
/// `txn`, `collection`, `index_name`, `query_text`, and `out_cursor` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_search_fts(
    txn: *const OryqReadTxn,
    collection: *const c_char,
    index_name: *const c_char,
    query_text: *const c_char,
    limit: usize,
    out_cursor: *mut *mut OryqCursor,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() || out_cursor.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "txn and out_cursor must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_cursor` is verified non-null; initialize before fallible work.
        unsafe {
            *out_cursor = std::ptr::null_mut();
        }
        let col_name = read_cstr(collection, "collection")?;
        let idx_name = read_cstr(index_name, "index_name")?;
        let query = read_cstr(query_text, "query_text")?;
        #[cfg(feature = "fts")]
        {
            // SAFETY: `txn` is valid for this call.
            let rtx = unsafe { &*txn }
                .tx
                .as_ref()
                .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn closed".to_owned()))?;
            let col = rtx
                .collection(col_name)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            let hits = col
                .search_fts(idx_name, query, limit)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            // SAFETY: `out_cursor` is non-null and writable.
            unsafe {
                *out_cursor = Box::into_raw(Box::new(OryqCursor::from_scored_docs(hits)));
            }
            Ok(OryqStatus::Ok)
        }
        #[cfg(not(feature = "fts"))]
        {
            let _ = (col_name, idx_name, query, limit);
            Err((
                OryqStatus::ErrUnsupportedFeature,
                "oryq-ffi compiled without 'fts' feature".to_owned(),
            ))
        }
    })
}

/// Creates a dense vector similarity index on `field_path` inside an open write transaction
/// (`--features vec`). `metric`: `0 = Cosine`, `1 = L2`, `2 = DotProduct`.
/// `quantization`: `0 = F32`, `1 = Int8`.
///
/// # Safety
/// `txn`, `collection`, and `field_path` must be valid; `name_or_null` may be null.
#[no_mangle]
pub unsafe extern "C" fn oryq_create_vector_index(
    txn: *mut OryqWriteTxn,
    collection: *const c_char,
    field_path: *const c_char,
    dimensions: u32,
    metric: i32,
    quantization: i32,
    name_or_null: *const c_char,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() {
            return Err((OryqStatus::ErrInvalidArgument, "txn is null".to_owned()));
        }
        let col_name = read_cstr(collection, "collection")?;
        let path = read_cstr(field_path, "field_path")?;
        if dimensions == 0 {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "dimensions must be > 0".to_owned(),
            ));
        }
        if !(0..=2).contains(&metric) {
            return Err((
                OryqStatus::ErrInvalidArgument,
                format!("metric must be 0 (Cosine), 1 (L2), or 2 (DotProduct), got {metric}"),
            ));
        }
        if !(0..=1).contains(&quantization) {
            return Err((
                OryqStatus::ErrInvalidArgument,
                format!("quantization must be 0 (F32) or 1 (Int8), got {quantization}"),
            ));
        }
        let name_opt = if name_or_null.is_null() {
            None
        } else {
            Some(read_cstr(name_or_null, "name_or_null")?)
        };
        #[cfg(feature = "vec")]
        {
            let dist_metric = match metric {
                0 => oryq_core::DistanceMetric::Cosine,
                1 => oryq_core::DistanceMetric::L2,
                2 => oryq_core::DistanceMetric::DotProduct,
                _ => unreachable!(),
            };
            let quant = match quantization {
                0 => oryq_core::VectorQuantization::F32,
                1 => oryq_core::VectorQuantization::Int8,
                _ => unreachable!(),
            };
            let dims_usize = usize::try_from(dimensions).map_err(|_| {
                (
                    OryqStatus::ErrInvalidArgument,
                    format!("dimensions {dimensions} exceeds platform usize"),
                )
            })?;
            let opts = oryq_core::VectorIndexOptions {
                dimensions: dims_usize,
                metric: dist_metric,
                quantization: quant,
            };
            // SAFETY: `txn` is valid for this call.
            let wtx = unsafe { &mut *txn }
                .tx
                .as_mut()
                .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn finalized".to_owned()))?;
            let mut col = wtx
                .collection(col_name)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            col.create_vector_index(path, opts, name_opt)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            Ok(OryqStatus::Ok)
        }
        #[cfg(not(feature = "vec"))]
        {
            let _ = (col_name, path, name_opt);
            Err((
                OryqStatus::ErrUnsupportedFeature,
                "oryq-ffi compiled without 'vec' feature".to_owned(),
            ))
        }
    })
}

/// Executes a top-`top_k` nearest-neighbor vector similarity search (`--features vec`), returning
/// a scored [`OryqCursor`] ordered by ascending distance (`oryq_cursor_get_score`).
///
/// # Safety
/// `txn`, `collection`, `index_name`, `query_vec` (length `query_len`), and `out_cursor` must be
/// valid pointers.
#[no_mangle]
pub unsafe extern "C" fn oryq_search_vector(
    txn: *const OryqReadTxn,
    collection: *const c_char,
    index_name: *const c_char,
    query_vec: *const f32,
    query_len: usize,
    top_k: usize,
    out_cursor: *mut *mut OryqCursor,
) -> OryqStatus {
    ffi_guard(|| {
        if txn.is_null() || out_cursor.is_null() {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "txn and out_cursor must not be null".to_owned(),
            ));
        }
        // SAFETY: `out_cursor` is verified non-null; initialize before fallible work.
        unsafe {
            *out_cursor = std::ptr::null_mut();
        }
        let col_name = read_cstr(collection, "collection")?;
        let idx_name = read_cstr(index_name, "index_name")?;
        if query_vec.is_null() || query_len == 0 {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "query_vec must be non-null with query_len > 0".to_owned(),
            ));
        }
        if (query_vec as usize) % std::mem::align_of::<f32>() != 0 {
            return Err((
                OryqStatus::ErrInvalidArgument,
                "query_vec pointer is not 4-byte aligned for f32".to_owned(),
            ));
        }
        let max_elems = (isize::MAX as usize) / std::mem::size_of::<f32>();
        if query_len > max_elems {
            return Err((
                OryqStatus::ErrInvalidArgument,
                format!("query_len ({query_len}) exceeds platform slice byte limit"),
            ));
        }
        #[cfg(feature = "vec")]
        {
            // SAFETY: `query_vec` is verified non-null, properly aligned for `f32`, and
            // `query_len * size_of::<f32>() <= isize::MAX`.
            let query_slice = unsafe { std::slice::from_raw_parts(query_vec, query_len) };
            // SAFETY: `txn` is valid for this call.
            let rtx = unsafe { &*txn }
                .tx
                .as_ref()
                .ok_or_else(|| (OryqStatus::ErrInvalidArgument, "txn closed".to_owned()))?;
            let col = rtx
                .collection(col_name)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            let hits = col
                .search_vector(idx_name, query_slice, top_k)
                .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
            // SAFETY: `out_cursor` is non-null and writable.
            unsafe {
                *out_cursor = Box::into_raw(Box::new(OryqCursor::from_scored_docs(hits)));
            }
            Ok(OryqStatus::Ok)
        }
        #[cfg(not(feature = "vec"))]
        {
            let _ = (col_name, idx_name, top_k);
            Err((
                OryqStatus::ErrUnsupportedFeature,
                "oryq-ffi compiled without 'vec' feature".to_owned(),
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_id_json_edge_cases_match_rfc8259_json() {
        assert_eq!(parse_id_json("42").unwrap(), Value::Int64(42));
        assert_eq!(parse_id_json(" \t\r\n-42\n ").unwrap(), Value::Int64(-42));
        assert_eq!(parse_id_json("0").unwrap(), Value::Int64(0));
        assert_eq!(
            parse_id_json(&i64::MAX.to_string()).unwrap(),
            Value::Int64(i64::MAX)
        );
        assert_eq!(
            parse_id_json(&i64::MIN.to_string()).unwrap(),
            Value::Int64(i64::MIN)
        );
        // Leading zero and non-RFC-8259 Unicode whitespace must be rejected just like serde_json.
        assert!(parse_id_json("01").is_err());
        assert!(parse_id_json("\u{00A0}42").is_err());
        assert!(parse_id_json("\u{2003}42").is_err());
        // String and -0 fall through to standard JSON parser cleanly.
        assert_eq!(
            parse_id_json(r#""user-1""#).unwrap(),
            Value::String("user-1".into())
        );
        assert!(parse_id_json("-0").is_ok());
        // Key-injection payloads must be rejected cleanly by standalone JSON parsing.
        assert!(parse_id_json(r#"1, "injected": 2"#).is_err());
        assert!(parse_id_json(r#"1},{"_id":2"#).is_err());
    }

    #[test]
    fn status_mappings_and_destructor_last_error_preservation() {
        use oryq_core::{ErrorCategory, ErrorEnvelope};

        let doc_large = ErrorEnvelope::new(
            ErrorCategory::Overflow,
            "core",
            "insert",
            "document exceeds max_document_bytes",
        );
        assert_eq!(
            OryqStatus::from_error(&doc_large),
            OryqStatus::ErrDocumentTooLarge
        );

        let prec = ErrorEnvelope::new(
            ErrorCategory::Overflow,
            "document",
            "inc",
            "lossless i64-to-f64 conversion failed for 9007199254740993 (exceeds 2^53)",
        );
        assert_eq!(
            OryqStatus::from_error(&prec),
            OryqStatus::ErrNumericPrecision
        );

        let col_named_index = ErrorEnvelope::new(
            ErrorCategory::NotFound,
            "core",
            "collection",
            "collection 'user_index' does not exist",
        );
        assert_eq!(
            OryqStatus::from_error(&col_named_index),
            OryqStatus::ErrNotFound
        );

        let idx_missing = ErrorEnvelope::new(
            ErrorCategory::NotFound,
            "core",
            "search",
            "index 'idx_foo' not found",
        );
        assert_eq!(
            OryqStatus::from_error(&idx_missing),
            OryqStatus::ErrIndexNotFound
        );

        let fts_missing = ErrorEnvelope::new(
            ErrorCategory::NotFound,
            "core",
            "search_fts",
            "FTS index 'idx_fts' not found on collection 'docs'",
        );
        assert_eq!(
            OryqStatus::from_error(&fts_missing),
            OryqStatus::ErrIndexNotFound
        );

        let vec_missing = ErrorEnvelope::new(
            ErrorCategory::NotFound,
            "core",
            "search_vector",
            "vector index 'idx_vec' not found on collection 'docs'",
        );
        assert_eq!(
            OryqStatus::from_error(&vec_missing),
            OryqStatus::ErrIndexNotFound
        );

        // Destructors must preserve prior LAST_ERROR on entry.
        // SAFETY: Passing null pointers is explicitly defined and safe for these C ABI entry points.
        unsafe {
            let st = oryq_db_open(std::ptr::null(), std::ptr::null_mut());
            assert_eq!(st, OryqStatus::ErrInvalidArgument);
            assert!(!oryq_last_error_message().is_null());
            assert_eq!(oryq_cursor_close(std::ptr::null_mut()), OryqStatus::Ok);
            assert_eq!(oryq_read_txn_close(std::ptr::null_mut()), OryqStatus::Ok);
            assert_eq!(
                oryq_write_txn_rollback(std::ptr::null_mut()),
                OryqStatus::Ok
            );
            assert!(!oryq_last_error_message().is_null());
        }
    }
}
