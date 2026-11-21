use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, PageSize, SortOrder, Value};

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_delta_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn delta_chain_up_to_4_and_compaction_on_5th_survives_reopen() {
    let dir = unique_temp_dir("compaction");
    let db_path = dir.join("delta.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("profiles").unwrap();
        {
            let mut col = wtx.collection("profiles").unwrap();
            col.create_index("score", SortOrder::Asc, false, false, Some("score_1"))
                .unwrap();
            col.insert_json(
                r#"{"_id": 1, "name": "Alice", "score": 10, "bio": "embedded database engineer"}"#,
            )
            .unwrap();
        }
        wtx.commit().unwrap();

        // Apply 4 delta patches (stays in delta chain)
        for step in 1..=4 {
            let mut wtx = db.begin_write().unwrap();
            {
                let mut col = wtx.collection("profiles").unwrap();
                assert!(col
                    .update_by_id(&Value::Int64(1), r#"{"$inc": {"score": 5}}"#)
                    .unwrap());
            }
            wtx.commit().unwrap();

            let rtx = db.begin_read().unwrap();
            let col = rtx.collection("profiles").unwrap();
            let doc = col.find_by_id(&Value::Int64(1)).unwrap().unwrap();
            assert_eq!(
                doc.view().get_field("score").unwrap().to_owned_value(),
                Value::Int64(10 + step * 5)
            );
        }

        // 5th patch triggers automatic compaction via DocumentRecordCompactor
        {
            let mut wtx = db.begin_write().unwrap();
            {
                let mut col = wtx.collection("profiles").unwrap();
                assert!(col
                    .update_by_id(
                        &Value::Int64(1),
                        r#"{"$inc": {"score": 5}, "$set": {"tier": "gold"}}"#
                    )
                    .unwrap());
            }
            wtx.commit().unwrap();
        }
        db.checkpoint(true).unwrap();
    }

    // Reopen from disk and verify exact logical state + secondary index consistency
    {
        let db = Database::open(&db_path, cfg).unwrap();
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("profiles").unwrap();
        let doc = col.find_by_id(&Value::Int64(1)).unwrap().unwrap();
        assert_eq!(
            doc.view().get_field("score").unwrap().to_owned_value(),
            Value::Int64(35)
        );
        assert_eq!(
            doc.view().get_field("tier").unwrap().to_owned_value(),
            Value::String("gold".into())
        );

        let via_idx = col
            .find_json(Some(r#"{"score": 35}"#), None, None, 0, None)
            .unwrap()
            .collect_all();
        assert_eq!(via_idx.len(), 1);
    }

    let _ = std::fs::remove_dir_all(&dir);
}
