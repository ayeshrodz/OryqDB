use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::catalog::{decode_catalog_entry, encode_catalog_entry, CatalogEntry};
use oryq_core::{
    BudgetDomain, BudgetHandle, CollectionId, Database, DatabaseConfig, DocumentRecordCompactor,
    ErrorCategory, IndexId, PageId, PageSize, RecordId, Value,
};
use oryq_document::{apply_update, from_json_str, to_json_value, UpdateRegistry, UpdateSpec};
use oryq_engine::RecordCompactor;
use oryq_index::IndexDescriptor;
use oryq_types::FeatureBitmask;

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_atomic_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn statement_savepoint_rollback_on_duplicate_key_and_txn_rollback() {
    let dir = unique_temp_dir("savepoint");
    let db_path = dir.join("atomic.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    let db = Database::open(&db_path, cfg).unwrap();

    // Setup collection and insert 1 doc, attempt duplicate _id (statement rolls back),
    // then insert 2nd doc and commit.
    {
        let mut w = db.begin_write().unwrap();
        w.create_collection("items").unwrap();
        {
            let mut items = w.collection("items").unwrap();
            let (_, r1) = items.insert_json(r#"{"_id": 10, "v": "first"}"#).unwrap();
            assert_eq!(r1, RecordId::new(1).unwrap());

            // Duplicate _id fails with DuplicateKey and rolls back statement savepoint
            let dup = items.insert_json(r#"{"_id": 10, "v": "dup"}"#).unwrap_err();
            assert_eq!(dup.category(), ErrorCategory::DuplicateKey);

            // Next insert succeeds cleanly with next sequential RecordId (2) because
            // savepoint rolled back the failed statement's counter increment!
            let (_, r2) = items.insert_json(r#"{"_id": 20, "v": "second"}"#).unwrap();
            assert_eq!(r2, RecordId::new(2).unwrap());
        }
        w.commit().unwrap();
    }

    // Explicit rollback discards mutations
    {
        let mut w = db.begin_write().unwrap();
        w.collection("items")
            .unwrap()
            .insert_json(r#"{"_id": 30, "v": "rolled-back"}"#)
            .unwrap();
        w.rollback();
    }

    // Drop without commit discards mutations
    {
        let mut w = db.begin_write().unwrap();
        w.create_collection("ghost").unwrap();
    }

    let r = db.begin_read().unwrap();
    assert_eq!(r.list_collections().unwrap(), vec!["items"]);
    let items = r.collection("items").unwrap();
    assert_eq!(items.count().unwrap(), 2);
    assert!(items.find_by_id(&Value::Int64(10)).unwrap().is_some());
    assert!(items.find_by_id(&Value::Int64(20)).unwrap().is_some());
    assert!(items.find_by_id(&Value::Int64(30)).unwrap().is_none());

    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn catalog_v1_codec_roundtrip_and_record_compactor_adapter() {
    let entry = CatalogEntry {
        collection_id: CollectionId::new(42).unwrap(),
        name: "events".into(),
        data_root_page_id: PageId::new(7),
        next_record_id: RecordId::new(100).unwrap(),
        required_write_features: FeatureBitmask::EMPTY,
        indexes: vec![IndexDescriptor::primary(
            IndexId::new(3).unwrap(),
            PageId::new(8),
        )],
    };
    let encoded = encode_catalog_entry(&entry).unwrap();
    let decoded = decode_catalog_entry(&encoded).unwrap();
    assert_eq!(decoded, entry);

    // Exercise DocumentRecordCompactor via engine's RecordCompactor trait
    let compactor: Box<dyn RecordCompactor> = Box::new(DocumentRecordCompactor);
    let reg = UpdateRegistry::default_v1();
    let (base, _) = from_json_str(r#"{"_id": 1, "a": 10, "b": "old"}"#, None).unwrap();
    let spec =
        UpdateSpec::from_json_str(r#"{"$set": {"b": "new"}, "$inc": {"a": 5}}"#, &reg).unwrap();
    let (mutated, _) = apply_update(&base.view(), &spec, &reg, None).unwrap();

    let budget = BudgetHandle::new(BudgetDomain::Query, 8192);
    let compacted = compactor
        .compact(base.as_bytes(), &[&mutated.patch_bytes], &budget)
        .unwrap();
    let view = oryq_document::DocumentView::from_bytes(&compacted).unwrap();
    assert_eq!(
        to_json_value(&view),
        to_json_value(&mutated.updated_doc.view())
    );
}

#[test]
fn multi_index_a_succeeds_b_fails_rollback_then_commit_in_same_txn() {
    let dir = unique_temp_dir("multi_index_rollback");
    let db_path = dir.join("multi.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut w = db.begin_write().unwrap();
        w.create_collection("users").unwrap();
        {
            let mut users = w.collection("users").unwrap();
            // Seed doc before index creation so create_index backfills it
            users
                .insert_json(r#"{"_id": 1, "email": "alice@oryq.dev", "dept": "eng"}"#)
                .unwrap();

            // Index A: non-unique on "dept"; Index B: unique on "email"
            users
                .create_index(
                    "dept",
                    oryq_core::SortOrder::Asc,
                    false,
                    false,
                    Some("idx_dept"),
                )
                .unwrap();
            users
                .create_index(
                    "email",
                    oryq_core::SortOrder::Asc,
                    true,
                    false,
                    Some("idx_email_uniq"),
                )
                .unwrap();
            assert_eq!(users.list_indexes().unwrap().len(), 3);

            // Cannot drop mandatory primary _id_ index
            assert_eq!(
                users.drop_index("_id_").unwrap_err().category(),
                ErrorCategory::InvalidArgument
            );

            // Insert doc 2: primary & idx_dept (A) succeed, but idx_email_uniq (B) FAILS with DuplicateKey!
            // Statement savepoint MUST roll back collection + primary + idx_dept cleanly.
            let err = users
                .insert_json(r#"{"_id": 2, "email": "alice@oryq.dev", "dept": "sales"}"#)
                .unwrap_err();
            assert_eq!(err.category(), ErrorCategory::DuplicateKey);

            // Subsequent valid insert with _id: 2 in the SAME transaction succeeds cleanly!
            users
                .insert_json(r#"{"_id": 2, "email": "bob@oryq.dev", "dept": "sales"}"#)
                .unwrap();
        }
        w.commit().unwrap();
    }

    // Reopen and verify both docs and all 3 indexes survived intact
    {
        let db = Database::open(&db_path, cfg).unwrap();
        let r = db.begin_read().unwrap();
        let users = r.collection("users").unwrap();
        assert_eq!(users.count().unwrap(), 2);
        assert_eq!(users.list_indexes().len(), 3);
        assert!(users.find_by_id(&Value::Int64(1)).unwrap().is_some());
        assert!(users.find_by_id(&Value::Int64(2)).unwrap().is_some());
    }

    let _ = std::fs::remove_dir_all(&dir);
}
