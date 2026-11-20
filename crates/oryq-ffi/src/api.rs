//! Exported `extern "C"` symbols for OryqDB (`include/oryq.h`).

use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use oryq_core::{from_json_str, to_json_string, Database, DatabaseConfig, Value, ValueView};

use crate::diagnostics::last_error_ptr;
use crate::guard::ffi_guard;
use crate::handles::{OryqCursor, OryqDb, OryqReadTxn, OryqWriteTxn};
use crate::status::OryqStatus;

/// ABI version packed as `(major << 16) | minor` (`1.0` -> `0x0001_0000`).
pub const ORYQ_ABI_VERSION: u32 = 1 << 16;

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
    let wrapped = format!(r#"{{"_id":{id_json}}}"#);
    let (doc, _) =
        from_json_str(&wrapped, None).map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
    doc.view()
        .get_field("_id")
        .map(|v| v.to_owned_value())
        .ok_or_else(|| {
            (
                OryqStatus::ErrInvalidArgument,
                "failed to extract _id value from id_json".to_owned(),
            )
        })
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

/// Opens or creates an OryqDB database file at `path_utf8`.
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
        if db_ref.has_live_children() {
            return Err((
                OryqStatus::ErrBusy,
                "cannot close database while child read/write transactions are still open"
                    .to_owned(),
            ));
        }
        // SAFETY: No child handles remain; reclaim Box allocation and drop `Database`.
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
    ffi_guard(|| {
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
    ffi_guard(|| {
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
        match cur.cursor.next_doc() {
            None => {
                cur.current = None;
                // SAFETY: `out_json` is non-null.
                unsafe {
                    *out_json = std::ptr::null_mut();
                }
                Ok(OryqStatus::CursorDone)
            }
            Some(doc) => {
                let json = to_json_string(&doc.view())
                    .map_err(|e| (OryqStatus::from_error(&e), e.to_string()))?;
                let cstr = CString::new(json).map_err(|_| {
                    (
                        OryqStatus::ErrInternalPanic,
                        "encoded JSON contained interior NUL".to_owned(),
                    )
                })?;
                cur.current = Some(doc);
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
/// `cursor` must be null or a valid pointer from `oryq_find_json` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn oryq_cursor_close(cursor: *mut OryqCursor) -> OryqStatus {
    ffi_guard(|| {
        if !cursor.is_null() {
            // SAFETY: Reclaims owned `OryqCursor`.
            let _ = unsafe { Box::from_raw(cursor) };
        }
        Ok(OryqStatus::Ok)
    })
}
