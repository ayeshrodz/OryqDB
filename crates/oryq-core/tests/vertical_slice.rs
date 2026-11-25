use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, Value};
use oryq_document::FieldPath;

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_core_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn end_to_end_crud_mvcc_checkpoint_seal_and_reopen() {
    let dir = unique_temp_dir("vertical_slice");
    let db_path = dir.join("app.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    let auto_oid: Value;

    // Phase 1: Create collections, insert docs (explicit & auto _id), update, replace, delete
    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        assert!(db.is_sealed().unwrap());

        {
            let mut w = db.begin_write().unwrap();
            w.create_collection("users").unwrap();
            w.create_collection("temp").unwrap();
            assert_eq!(w.list_collections().unwrap(), vec!["temp", "users"]);

            let mut users = w.collection("users").unwrap();
            let (id1, _) = users
                .insert_json(r#"{"_id": 1, "name": "Alice", "score": 10, "tags": ["a"]}"#)
                .unwrap();
            assert_eq!(id1, Value::Int64(1));

            let (id2, _) = users
                .insert_json(r#"{"_id": "bob", "name": "Bob", "score": 20}"#)
                .unwrap();
            assert_eq!(id2, Value::String("bob".into()));

            // Auto-generated ObjectId when `_id` is omitted
            let (gen_id, _) = users
                .insert_json(r#"{"name": "Carol", "role": "admin"}"#)
                .unwrap();
            assert!(matches!(gen_id, Value::ObjectId(_)));
            auto_oid = gen_id;

            w.commit().unwrap();
        }

        // Open MVCC reader snapshot before update/delete
        let r_before = db.begin_read().unwrap();
        assert_eq!(r_before.collection("users").unwrap().count().unwrap(), 3);

        // Phase 2: Update Alice ($set, $inc, $push), replace Bob, delete temp collection
        {
            let mut w = db.begin_write().unwrap();
            {
                let mut users = w.collection("users").unwrap();
                assert!(users
                    .update_by_id(
                        &Value::Int64(1),
                        r#"{"$set": {"profile.country": "NZ"}, "$inc": {"score": 5}, "$push": {"tags": "rust"}}"#
                    )
                    .unwrap());

                // Replace Bob with matching _id succeeds; changing _id fails
                assert_eq!(
                    users
                        .replace_by_id(
                            &Value::String("bob".into()),
                            r#"{"_id": "wrong", "name": "Bob2"}"#
                        )
                        .unwrap_err()
                        .category(),
                    ErrorCategory::InvalidArgument
                );
                assert!(users
                    .replace_by_id(
                        &Value::String("bob".into()),
                        r#"{"_id": "bob", "name": "Robert", "score": 25}"#
                    )
                    .unwrap());

                // Delete Bob
                assert!(users.delete_by_id(&Value::String("bob".into())).unwrap());
                assert!(!users.delete_by_id(&Value::String("bob".into())).unwrap());
            }
            assert!(w.drop_collection("temp").unwrap());
            w.commit().unwrap();
        }

        // Snapshot r_before still sees original pre-update Alice & Bob!
        {
            let users_old = r_before.collection("users").unwrap();
            assert_eq!(users_old.count().unwrap(), 3);
            let alice_old = users_old.find_by_id(&Value::Int64(1)).unwrap().unwrap();
            assert_eq!(
                alice_old
                    .view()
                    .get_i64(&FieldPath::parse("score").unwrap()),
                Some(10)
            );
            assert!(users_old
                .find_by_id(&Value::String("bob".into()))
                .unwrap()
                .is_some());
        }
        drop(r_before);

        // Full checkpoint + WAL reset seals the database
        let outcome = db.checkpoint(true).unwrap();
        assert!(outcome.wal_reset);
        assert!(db.is_sealed().unwrap());
    }

    // Remove .oryq-wal while sealed -> reopen standalone .oryq and verify all persisted data!
    let wal_path = oryq_engine::wal_path_for(&db_path);
    if wal_path.exists() {
        std::fs::remove_file(&wal_path).unwrap();
    }

    {
        let db = Database::open(&db_path, cfg).unwrap();
        assert!(db.is_sealed().unwrap());
        let r = db.begin_read().unwrap();
        assert_eq!(r.list_collections().unwrap(), vec!["users"]);

        let users = r.collection("users").unwrap();
        assert_eq!(users.count().unwrap(), 2);

        let alice = users.find_by_id(&Value::Int64(1)).unwrap().unwrap();
        assert_eq!(
            alice.view().get_i64(&FieldPath::parse("score").unwrap()),
            Some(15)
        );
        assert_eq!(
            alice
                .view()
                .get_str(&FieldPath::parse("profile.country").unwrap()),
            Some("NZ")
        );

        assert!(users
            .find_by_id(&Value::String("bob".into()))
            .unwrap()
            .is_none());

        let carol = users.find_by_id(&auto_oid).unwrap().unwrap();
        assert_eq!(
            carol.view().get_str(&FieldPath::parse("name").unwrap()),
            Some("Carol")
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_count_and_explain_analyze_end_to_end() {
    let dir = unique_temp_dir("query_e2e");
    let db_path = dir.join("query_e2e.oryq");
    let db = Database::open(&db_path, DatabaseConfig::default()).unwrap();

    let mut wtx = db.begin_write().unwrap();
    wtx.create_collection("users").unwrap();
    {
        let mut users = wtx.collection("users").unwrap();
        users
            .create_index(
                "age",
                oryq_core::SortOrder::Asc,
                false,
                false,
                Some("age_1"),
            )
            .unwrap();
        users
            .insert_json(r#"{"_id": 1, "name": "Alice", "age": 30, "city": "Auckland"}"#)
            .unwrap();
        users
            .insert_json(r#"{"_id": 2, "name": "Bob", "age": 22, "city": "Wellington"}"#)
            .unwrap();
        users
            .insert_json(r#"{"_id": 3, "name": "Charlie", "age": 30, "city": "Wellington"}"#)
            .unwrap();
    }
    wtx.commit().unwrap();

    let rtx = db.begin_read().unwrap();
    let users = rtx.collection("users").unwrap();

    // Secondary equality scan + sort + projection
    let docs = users
        .find_json(
            Some(r#"{"age": 30}"#),
            Some(r#"{"name": 1, "_id": 0}"#),
            Some(r#"{"name": -1}"#),
            0,
            None,
        )
        .unwrap()
        .collect_all();
    assert_eq!(docs.len(), 2);
    assert_eq!(
        docs[0].view().get_field("name").unwrap().to_owned_value(),
        Value::String("Charlie".into())
    );
    assert!(docs[0].view().get_field("_id").is_none());

    // Count matching
    let f_range = oryq_core::parse_filter_json(r#"{"age": {"$gte": 25}}"#).unwrap();
    assert_eq!(users.count_matching(Some(&f_range)).unwrap(), 2);

    // Explain analyze chooses secondary range scan
    let exp = users
        .explain_analyze(&oryq_core::FindOptions {
            filter: Some(f_range),
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        exp.plan.access,
        oryq_core::AccessPlan::SecondaryRangeScan { .. }
    ));
    assert_eq!(exp.docs_returned, 2);

    // Primary `_id` equality fast path + ReadTransaction catalog cache + limit == Some(0) parity
    let users_cached = rtx.collection("users").unwrap();
    let cur_hit = users_cached
        .find_json(Some(r#"{"_id": 2}"#), None, None, 0, None)
        .unwrap();
    assert!(matches!(
        cur_hit.plan().access,
        oryq_core::AccessPlan::PrimaryIdLookup { .. }
    ));
    let hit_docs = cur_hit.collect_all();
    assert_eq!(hit_docs.len(), 1);
    assert_eq!(
        hit_docs[0]
            .view()
            .get_field("name")
            .unwrap()
            .to_owned_value(),
        Value::String("Bob".into())
    );
    assert!(users_cached
        .find_json(Some(r#"{"_id": 999}"#), None, None, 0, None)
        .unwrap()
        .collect_all()
        .is_empty());
    assert!(users_cached
        .find_json(Some(r#"{"_id": 2}"#), None, None, 0, Some(0))
        .unwrap()
        .collect_all()
        .is_empty());
    // Invalid `_id` type with `limit == Some(0)` still errors identically to `execute_find`.
    assert!(users_cached
        .find_json(Some(r#"{"_id": null}"#), None, None, 0, Some(0))
        .is_err());
}
