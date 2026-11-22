use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq::{
    oryq_abi_version, oryq_aggregate_json, oryq_create_collection, oryq_create_index,
    oryq_cursor_close, oryq_cursor_get_bool, oryq_cursor_get_f64, oryq_cursor_get_i64,
    oryq_cursor_get_utf8, oryq_cursor_next, oryq_cursor_next_json, oryq_db_backup_to,
    oryq_db_checkpoint, oryq_db_close, oryq_db_integrity_check, oryq_db_open,
    oryq_delete_by_id_json, oryq_drop_index, oryq_find_json, oryq_free_string, oryq_insert_json,
    oryq_last_error_message, oryq_read_txn_begin, oryq_read_txn_close, oryq_update_by_id_json,
    oryq_version, oryq_write_txn_begin, oryq_write_txn_commit, oryq_write_txn_rollback, OryqCursor,
    OryqDb, OryqReadTxn, OryqStatus, OryqWriteTxn,
};

static DIR_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "oryqdb_ffi_{label}_{}_{seq}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file_cstr(&self, name: &str) -> CString {
        CString::new(self.0.join(name).to_str().unwrap()).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ffi_crud_find_lifecycle_and_child_busy_guard() {
    assert_eq!(oryq_abi_version(), (1 << 16) | 1);
    // SAFETY: `oryq_version` returns a static NUL-terminated ASCII string.
    let ver = unsafe { CStr::from_ptr(oryq_version()) }.to_str().unwrap();
    assert!(!ver.is_empty());

    let dir = TempDir::new("crud");
    let db_path = dir.file_cstr("test.oryq");
    let mut db: *mut OryqDb = ptr::null_mut();

    // SAFETY: All pointers are valid stack locals or CStrings for this test.
    unsafe {
        assert_eq!(oryq_db_open(db_path.as_ptr(), &mut db), OryqStatus::Ok);
        assert!(!db.is_null());

        let mut wtx: *mut OryqWriteTxn = ptr::null_mut();
        assert_eq!(oryq_write_txn_begin(db, &mut wtx), OryqStatus::Ok);

        // Closing DB while write txn is still live must fail with ErrBusy!
        assert_eq!(oryq_db_close(db), OryqStatus::ErrBusy);
        assert!(!oryq_last_error_message().is_null());

        let col = CString::new("users").unwrap();
        let d1 = CString::new(r#"{"_id": 1, "name": "Alice", "age": 30}"#).unwrap();
        let d2 = CString::new(r#"{"_id": 2, "name": "Bob", "age": 25}"#).unwrap();
        assert_eq!(oryq_create_collection(wtx, col.as_ptr()), OryqStatus::Ok);
        assert_eq!(
            oryq_insert_json(wtx, col.as_ptr(), d1.as_ptr()),
            OryqStatus::Ok
        );
        assert_eq!(
            oryq_insert_json(wtx, col.as_ptr(), d2.as_ptr()),
            OryqStatus::Ok
        );

        let id1 = CString::new("1").unwrap();
        let upd = CString::new(r#"{"$inc": {"age": 5}}"#).unwrap();
        assert_eq!(
            oryq_update_by_id_json(wtx, col.as_ptr(), id1.as_ptr(), upd.as_ptr()),
            OryqStatus::Ok
        );

        let id2 = CString::new("2").unwrap();
        assert_eq!(
            oryq_delete_by_id_json(wtx, col.as_ptr(), id2.as_ptr()),
            OryqStatus::Ok
        );

        assert_eq!(oryq_write_txn_commit(wtx), OryqStatus::Ok);
        assert_eq!(oryq_db_checkpoint(db), OryqStatus::Ok);

        let mut rtx: *mut OryqReadTxn = ptr::null_mut();
        assert_eq!(oryq_read_txn_begin(db, &mut rtx), OryqStatus::Ok);
        let filter = CString::new(r#"{"age": {"$gte": 30}}"#).unwrap();
        let mut cur: *mut OryqCursor = ptr::null_mut();
        assert_eq!(
            oryq_find_json(rtx, col.as_ptr(), filter.as_ptr(), &mut cur),
            OryqStatus::Ok
        );

        let mut row_json: *mut std::ffi::c_char = ptr::null_mut();
        assert_eq!(oryq_cursor_next_json(cur, &mut row_json), OryqStatus::Ok);
        assert!(!row_json.is_null());
        let text = CStr::from_ptr(row_json).to_str().unwrap();
        assert!(text.contains("Alice"));
        oryq_free_string(row_json);

        let age_field = CString::new("age").unwrap();
        let mut age_val: i64 = 0;
        assert_eq!(
            oryq_cursor_get_i64(cur, age_field.as_ptr(), &mut age_val),
            OryqStatus::Ok
        );
        assert_eq!(age_val, 35);

        assert_eq!(
            oryq_cursor_next_json(cur, &mut row_json),
            OryqStatus::CursorDone
        );
        assert_eq!(oryq_cursor_close(cur), OryqStatus::Ok);
        assert_eq!(oryq_read_txn_close(rtx), OryqStatus::Ok);

        let mut wtx2: *mut OryqWriteTxn = ptr::null_mut();
        assert_eq!(oryq_write_txn_begin(db, &mut wtx2), OryqStatus::Ok);
        assert_eq!(oryq_write_txn_rollback(wtx2), OryqStatus::Ok);

        assert_eq!(oryq_db_close(db), OryqStatus::Ok);
    }
}

#[test]
fn ffi_zero_copy_getters_indexes_aggregate_backup_and_integrity() {
    let dir = TempDir::new("parity");
    let db_path = dir.file_cstr("parity.oryq");
    let backup_path = dir.file_cstr("parity_backup.oryq");
    let mut db: *mut OryqDb = ptr::null_mut();

    // SAFETY: All pointers are valid stack locals or CStrings for this test.
    unsafe {
        assert_eq!(oryq_db_open(db_path.as_ptr(), &mut db), OryqStatus::Ok);

        let col = CString::new("products").unwrap();
        let f_sku = CString::new("sku").unwrap();
        let idx_name = CString::new("sku_uniq").unwrap();
        let d1 = CString::new(
            r#"{"_id": 1, "sku": "A-100", "note": "", "price": 19.5, "in_stock": true, "qty": 10}"#,
        )
        .unwrap();
        let d2 = CString::new(
            r#"{"_id": 2, "sku": "B-200", "note": "ok", "price": 40.5, "in_stock": false, "qty": 5}"#,
        )
        .unwrap();

        let mut wtx: *mut OryqWriteTxn = ptr::null_mut();
        assert_eq!(oryq_write_txn_begin(db, &mut wtx), OryqStatus::Ok);
        assert_eq!(oryq_create_collection(wtx, col.as_ptr()), OryqStatus::Ok);

        // Invalid sort_order == 0 must return ErrInvalidArgument
        assert_eq!(
            oryq_create_index(
                wtx,
                col.as_ptr(),
                f_sku.as_ptr(),
                0,
                true,
                false,
                ptr::null()
            ),
            OryqStatus::ErrInvalidArgument
        );
        // Valid unique secondary index creation (Asc + explicit name)
        assert_eq!(
            oryq_create_index(
                wtx,
                col.as_ptr(),
                f_sku.as_ptr(),
                1,
                true,
                false,
                idx_name.as_ptr()
            ),
            OryqStatus::Ok
        );
        // Descending index with auto-generated name (`name_or_null = NULL`)
        let f_price_idx = CString::new("price").unwrap();
        assert_eq!(
            oryq_create_index(
                wtx,
                col.as_ptr(),
                f_price_idx.as_ptr(),
                -1,
                false,
                false,
                ptr::null()
            ),
            OryqStatus::Ok
        );
        assert_eq!(
            oryq_insert_json(wtx, col.as_ptr(), d1.as_ptr()),
            OryqStatus::Ok
        );
        assert_eq!(
            oryq_insert_json(wtx, col.as_ptr(), d2.as_ptr()),
            OryqStatus::Ok
        );
        assert_eq!(oryq_write_txn_commit(wtx), OryqStatus::Ok);

        // Zero-copy getters via `oryq_cursor_next` (no JSON serialization!)
        let mut rtx: *mut OryqReadTxn = ptr::null_mut();
        assert_eq!(oryq_read_txn_begin(db, &mut rtx), OryqStatus::Ok);

        let filter = CString::new(r#"{"sku": "A-100"}"#).unwrap();
        let mut cur: *mut OryqCursor = ptr::null_mut();
        assert_eq!(
            oryq_find_json(rtx, col.as_ptr(), filter.as_ptr(), &mut cur),
            OryqStatus::Ok
        );

        // Calling getters before first `oryq_cursor_next` must return ErrNotFound and zero out-param
        let mut pre_val: f64 = 123.4;
        assert_eq!(
            oryq_cursor_get_f64(cur, f_price_idx.as_ptr(), &mut pre_val),
            OryqStatus::ErrNotFound
        );
        assert_eq!(pre_val, 0.0);

        assert_eq!(oryq_cursor_next(cur), OryqStatus::Ok);

        let mut utf8_ptr: *const std::ffi::c_char = ptr::null();
        let mut utf8_len: usize = 0;
        assert_eq!(
            oryq_cursor_get_utf8(cur, f_sku.as_ptr(), &mut utf8_ptr, &mut utf8_len),
            OryqStatus::Ok
        );
        let sku_slice = std::slice::from_raw_parts(utf8_ptr.cast::<u8>(), utf8_len);
        assert_eq!(sku_slice, b"A-100");

        // Empty UTF-8 string check
        let f_note = CString::new("note").unwrap();
        assert_eq!(
            oryq_cursor_get_utf8(cur, f_note.as_ptr(), &mut utf8_ptr, &mut utf8_len),
            OryqStatus::Ok
        );
        assert_eq!(utf8_len, 0);
        assert!(!utf8_ptr.is_null());

        let f_price = CString::new("price").unwrap();
        let mut price: f64 = 0.0;
        assert_eq!(
            oryq_cursor_get_f64(cur, f_price.as_ptr(), &mut price),
            OryqStatus::Ok
        );
        assert!((price - 19.5).abs() < f64::EPSILON);

        let f_stock = CString::new("in_stock").unwrap();
        let mut in_stock = false;
        assert_eq!(
            oryq_cursor_get_bool(cur, f_stock.as_ptr(), &mut in_stock),
            OryqStatus::Ok
        );
        assert!(in_stock);

        // Type mismatch check
        assert_eq!(
            oryq_cursor_get_bool(cur, f_sku.as_ptr(), &mut in_stock),
            OryqStatus::ErrTypeMismatch
        );

        assert_eq!(oryq_cursor_next(cur), OryqStatus::CursorDone);
        // Calling getters after ORYQ_CURSOR_DONE must return ErrNotFound
        assert_eq!(
            oryq_cursor_get_bool(cur, f_stock.as_ptr(), &mut in_stock),
            OryqStatus::ErrNotFound
        );
        assert_eq!(oryq_cursor_close(cur), OryqStatus::Ok);

        // Aggregation pipeline via `oryq_aggregate_json`
        let pipe = CString::new(
            r#"[{"$group": {"_id": null, "total_price": {"$sum": "$price"}, "total_qty": {"$sum": "$qty"}}}]"#,
        )
        .unwrap();
        let mut agg_cur: *mut OryqCursor = ptr::null_mut();
        assert_eq!(
            oryq_aggregate_json(rtx, col.as_ptr(), pipe.as_ptr(), &mut agg_cur),
            OryqStatus::Ok
        );
        assert_eq!(oryq_cursor_next(agg_cur), OryqStatus::Ok);
        let f_total_price = CString::new("total_price").unwrap();
        let f_total_qty = CString::new("total_qty").unwrap();
        let mut total_price: f64 = 0.0;
        let mut total_qty: i64 = 0;
        assert_eq!(
            oryq_cursor_get_f64(agg_cur, f_total_price.as_ptr(), &mut total_price),
            OryqStatus::Ok
        );
        assert!((total_price - 60.0).abs() < f64::EPSILON);
        assert_eq!(
            oryq_cursor_get_i64(agg_cur, f_total_qty.as_ptr(), &mut total_qty),
            OryqStatus::Ok
        );
        assert_eq!(total_qty, 15);
        assert_eq!(oryq_cursor_close(agg_cur), OryqStatus::Ok);
        assert_eq!(oryq_read_txn_close(rtx), OryqStatus::Ok);

        // Drop index + missing index check
        let mut wtx2: *mut OryqWriteTxn = ptr::null_mut();
        assert_eq!(oryq_write_txn_begin(db, &mut wtx2), OryqStatus::Ok);
        assert_eq!(
            oryq_drop_index(wtx2, col.as_ptr(), idx_name.as_ptr()),
            OryqStatus::Ok
        );
        assert_eq!(
            oryq_drop_index(wtx2, col.as_ptr(), idx_name.as_ptr()),
            OryqStatus::ErrIndexNotFound
        );
        assert!(!oryq_last_error_message().is_null());
        assert_eq!(oryq_write_txn_commit(wtx2), OryqStatus::Ok);

        // Integrity check & sealed online backup
        let mut ok = false;
        assert_eq!(oryq_db_integrity_check(db, &mut ok), OryqStatus::Ok);
        assert!(ok);
        assert_eq!(oryq_db_backup_to(db, backup_path.as_ptr()), OryqStatus::Ok);
        assert_eq!(oryq_db_close(db), OryqStatus::Ok);

        // Reopen backup and verify integrity
        let mut bdb: *mut OryqDb = ptr::null_mut();
        assert_eq!(oryq_db_open(backup_path.as_ptr(), &mut bdb), OryqStatus::Ok);
        let mut bok = false;
        assert_eq!(oryq_db_integrity_check(bdb, &mut bok), OryqStatus::Ok);
        assert!(bok);
        assert_eq!(oryq_db_close(bdb), OryqStatus::Ok);
    }
}

#[test]
fn ffi_rejects_null_arguments_and_catches_panics() {
    assert_eq!(oryq_abi_version(), (1 << 16) | 1);
    // SAFETY: `oryq_version` returns a static NUL-terminated ASCII string; `oryq_free_string(null)` is a no-op.
    unsafe {
        assert!(!CStr::from_ptr(oryq_version()).to_str().unwrap().is_empty());
        oryq_free_string(ptr::null_mut());
        assert_eq!(
            oryq_db_open(ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert!(!oryq_last_error_message().is_null());
        assert_eq!(
            oryq_cursor_next(ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_cursor_get_utf8(ptr::null(), ptr::null(), ptr::null_mut(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_cursor_get_f64(ptr::null(), ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_cursor_get_bool(ptr::null(), ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_aggregate_json(ptr::null(), ptr::null(), ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_db_integrity_check(ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_create_index(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                1,
                false,
                false,
                ptr::null()
            ),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_drop_index(ptr::null_mut(), ptr::null(), ptr::null()),
            OryqStatus::ErrInvalidArgument
        );
        assert_eq!(
            oryq_db_backup_to(ptr::null(), ptr::null()),
            OryqStatus::ErrInvalidArgument
        );
    }

    let status = oryq::guard::ffi_guard(|| -> Result<OryqStatus, (OryqStatus, String)> {
        panic!("simulated panic across boundary");
    });
    assert_eq!(status, OryqStatus::ErrInternalPanic);
    // SAFETY: `oryq_last_error_message` returns a valid thread-local C string after error.
    let msg = unsafe { CStr::from_ptr(oryq_last_error_message()) }
        .to_str()
        .unwrap();
    assert!(msg.contains("simulated panic"));
}
