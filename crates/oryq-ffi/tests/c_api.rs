use std::ffi::{CStr, CString};
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq::{
    oryq_abi_version, oryq_create_collection, oryq_cursor_close, oryq_cursor_get_i64,
    oryq_cursor_next_json, oryq_db_checkpoint, oryq_db_close, oryq_db_open, oryq_delete_by_id_json,
    oryq_find_json, oryq_free_string, oryq_insert_json, oryq_last_error_message,
    oryq_read_txn_begin, oryq_read_txn_close, oryq_update_by_id_json, oryq_version,
    oryq_write_txn_begin, oryq_write_txn_commit, oryq_write_txn_rollback, OryqCursor, OryqDb,
    OryqReadTxn, OryqStatus, OryqWriteTxn,
};

fn unique_db_path(label: &str) -> CString {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("oryqdb_ffi_{label}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.oryq");
    CString::new(path.to_str().unwrap()).unwrap()
}

#[test]
fn ffi_crud_find_lifecycle_and_child_busy_guard() {
    assert_eq!(oryq_abi_version(), 1 << 16);
    // SAFETY: `oryq_version` returns a static NUL-terminated ASCII string.
    let ver = unsafe { CStr::from_ptr(oryq_version()) }.to_str().unwrap();
    assert!(!ver.is_empty());

    let db_path = unique_db_path("crud");
    let mut db: *mut OryqDb = ptr::null_mut();

    // SAFETY: All pointers are valid stack locals or CStrings for this test.
    unsafe {
        assert_eq!(oryq_db_open(db_path.as_ptr(), &mut db), OryqStatus::Ok);
        assert!(!db.is_null());

        // Write transaction: create collection, insert, update, delete
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

        // Read transaction + cursor + zero-copy i64 accessor
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

        // Rollback test
        let mut wtx2: *mut OryqWriteTxn = ptr::null_mut();
        assert_eq!(oryq_write_txn_begin(db, &mut wtx2), OryqStatus::Ok);
        assert_eq!(oryq_write_txn_rollback(wtx2), OryqStatus::Ok);

        // Now closing DB succeeds cleanly!
        assert_eq!(oryq_db_close(db), OryqStatus::Ok);
    }
}

#[test]
fn ffi_rejects_null_arguments_and_catches_panics() {
    // SAFETY: Testing explicit null argument rejection.
    unsafe {
        assert_eq!(
            oryq_db_open(ptr::null(), ptr::null_mut()),
            OryqStatus::ErrInvalidArgument
        );
        assert!(!oryq_last_error_message().is_null());
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
