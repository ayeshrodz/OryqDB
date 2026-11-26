use std::sync::Arc;

use oryq_core::{Database, DatabaseConfig, SortOrder, Value};
use oryq_engine::{BTree, EngineConfig, MemoryVfs, PageSize, StorageEngine, StorageIo, TreeKind};
use oryq_types::ErrorCategory;

#[test]
fn large_documents_overflow_up_to_multi_mb_and_delta_near_leaf_limit_succeed() {
    let db = Database::open_in_memory().unwrap();

    // 1. Multi-size documents: 8 KiB, 64 KiB, and 512 KiB (spanning ~65 overflow pages!)
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("blobs").unwrap();
        let mut col = wtx.collection("blobs").unwrap();
        col.create_index("tag", SortOrder::Asc, false, false, Some("tag_idx"))
            .unwrap();

        for (id, kb) in [(1i64, 8usize), (2, 64), (3, 512)] {
            let payload = "X".repeat(kb * 1024);
            let json = format!(r#"{{"_id":{id},"tag":"sz_{kb}","payload":"{payload}"}}"#);
            col.insert_json(&json).unwrap();
        }

        // Exact reproducer from audit: 7,500-byte document updated repeatedly with 250-byte `$set`s
        let near_leaf = "P".repeat(7500);
        col.insert_json(&format!(
            r#"{{"_id":4,"tag":"near","payload":"{near_leaf}","note":"init"}}"#
        ))
        .unwrap();
        for round in 0..6 {
            let note = char::from(b'A' + round).to_string().repeat(250);
            assert!(col
                .update_by_id(
                    &Value::Int64(4),
                    &format!(r#"{{"$set":{{"note":"{note}"}}}}"#)
                )
                .unwrap());
        }
        wtx.commit().unwrap();
    }

    // Verify point reads, secondary index scans, and integrity_check across overflow records
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("blobs").unwrap();
        for (id, kb) in [(1i64, 8usize), (2, 64), (3, 512)] {
            let doc = col.find_by_id(&Value::Int64(id)).unwrap().unwrap();
            let s = match doc.view().get_field("payload").unwrap() {
                oryq_core::ValueView::String(s) => s,
                other => panic!("unexpected field view: {other:?}"),
            };
            assert_eq!(s.len(), kb * 1024);

            let scanned = col
                .find_json(
                    Some(&format!(r#"{{"tag":"sz_{kb}"}}"#)),
                    None,
                    None,
                    0,
                    None,
                )
                .unwrap()
                .collect_all();
            assert_eq!(scanned.len(), 1);
        }
    }
    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);

    // Update 512 KiB doc down to tiny inline doc, replace 64 KiB doc, and delete 8 KiB doc
    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("blobs").unwrap();
        assert!(col
            .update_by_id(&Value::Int64(3), r#"{"$set":{"payload":"shrunk"}}"#)
            .unwrap());
        let rep = "R".repeat(96 * 1024);
        assert!(col
            .replace_by_id(
                &Value::Int64(2),
                &format!(r#"{{"_id":2,"tag":"sz_96","payload":"{rep}"}}"#)
            )
            .unwrap());
        assert!(col.delete_by_id(&Value::Int64(1)).unwrap());
        wtx.commit().unwrap();
    }

    assert!(db.integrity_check().unwrap().ok);

    // Optional 1024-dimension vector index on default 8 KiB pages (previously failed > ~670 dims!)
    #[cfg(feature = "vec")]
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("embeddings").unwrap();
        let mut col = wtx.collection("embeddings").unwrap();
        col.create_vector_index(
            "vec",
            oryq_vec::VectorIndexOptions {
                dimensions: 1024,
                metric: oryq_vec::DistanceMetric::Cosine,
                quantization: oryq_vec::VectorQuantization::F32,
            },
            Some("vec_1024"),
        )
        .unwrap();

        let mut v1 = vec![0.0f32; 1024];
        v1[0] = 1.0;
        let mut v2 = vec![0.0f32; 1024];
        v2[1] = 1.0;
        let v1_json: Vec<String> = v1.iter().map(|x| format!("{x:.1}")).collect();
        let v2_json: Vec<String> = v2.iter().map(|x| format!("{x:.1}")).collect();
        col.insert_json(&format!(r#"{{"_id":1,"vec":[{}]}}"#, v1_json.join(",")))
            .unwrap();
        col.insert_json(&format!(r#"{{"_id":2,"vec":[{}]}}"#, v2_json.join(",")))
            .unwrap();
        wtx.commit().unwrap();

        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("embeddings").unwrap();
        let hits = col.search_vector("vec_1024", &v1, 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].0.view().get_field("_id").unwrap().to_owned_value(),
            Value::Int64(1)
        );
    }
}

#[test]
fn deleted_and_dropped_pages_reclaimed_automatically_without_vacuum() {
    let vfs = Arc::new(MemoryVfs::new());
    let io: Arc<dyn StorageIo> = vfs.clone();
    let path = std::path::Path::new("reclaim_test.oryq");
    let cfg = DatabaseConfig {
        sync_mode: oryq_engine::SyncMode::Off,
        ..DatabaseConfig::default()
    };

    // 1. Insert 1,000 ~250B docs and seal
    {
        let db = Database::open_with_io(io.clone(), path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        let mut col = wtx.collection("items").unwrap();
        let pad = "a".repeat(200);
        for i in 0..1000i64 {
            col.insert_json(&format!(r#"{{"_id":{i},"pad":"{pad}"}}"#))
                .unwrap();
        }
        wtx.commit().unwrap();
    }
    let size_after_first = io
        .open(path, oryq_engine::OpenMode::READ_ONLY)
        .unwrap()
        .len()
        .unwrap();

    // 2. Delete all 1,000 docs (while verifying an older snapshot still sees all 1,000!)
    {
        let db = Database::open_with_io(io.clone(), path, cfg.clone()).unwrap();
        let pinned_reader = db.begin_read().unwrap();
        {
            let mut wtx = db.begin_write().unwrap();
            let mut col = wtx.collection("items").unwrap();
            for i in 0..1000i64 {
                assert!(col.delete_by_id(&Value::Int64(i)).unwrap());
            }
            wtx.commit().unwrap();
        }
        assert_eq!(
            pinned_reader.collection("items").unwrap().count().unwrap(),
            1000
        );
        drop(pinned_reader);
    }

    // 3. Insert 1,000 new docs of the same size WITHOUT calling vacuum() -> file must reuse freed pages!
    {
        let db = Database::open_with_io(io.clone(), path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("items").unwrap();
        let pad = "b".repeat(200);
        for i in 1000..2000i64 {
            col.insert_json(&format!(r#"{{"_id":{i},"pad":"{pad}"}}"#))
                .unwrap();
        }
        wtx.commit().unwrap();
    }
    let size_after_reinsert = io
        .open(path, oryq_engine::OpenMode::READ_ONLY)
        .unwrap()
        .len()
        .unwrap();

    // Allow at most 4 metadata/freelist-trunk pages (32 KiB) overhead vs doubling!
    assert!(
        size_after_reinsert <= size_after_first + 32 * 1024,
        "expected freelist page reuse without VACUUM: initial={size_after_first}B, after_reinsert={size_after_reinsert}B"
    );

    // 4. Verify drop_collection + recreate also reuses pages (including overflow pages)
    {
        let db = Database::open_with_io(io.clone(), path, cfg.clone()).unwrap();
        {
            let mut wtx = db.begin_write().unwrap();
            assert!(wtx.drop_collection("items").unwrap());
            wtx.commit().unwrap();
        }
        {
            let mut wtx = db.begin_write().unwrap();
            wtx.create_collection("items2").unwrap();
            let mut col = wtx.collection("items2").unwrap();
            let big = "Z".repeat(32 * 1024);
            for i in 0..5i64 {
                col.insert_json(&format!(r#"{{"_id":{i},"big":"{big}"}}"#))
                    .unwrap();
            }
            wtx.commit().unwrap();
        }
    }
    let size_after_drop_recreate = io
        .open(path, oryq_engine::OpenMode::READ_ONLY)
        .unwrap()
        .len()
        .unwrap();
    assert!(
        size_after_drop_recreate <= size_after_reinsert + 16 * 1024,
        "expected dropped collection pages to be reused: before={size_after_reinsert}B, after={size_after_drop_recreate}B"
    );
}

#[test]
fn wal_autocheckpoint_and_presync_version_quota_prevent_outage_and_poisoning() {
    // 1. Sustained writes with tight max_wal_hard_bytes = 256 KiB and wal_autocheckpoint_bytes = 64 KiB
    // succeed indefinitely without manual checkpoint() calls.
    let io: Arc<dyn StorageIo> = Arc::new(MemoryVfs::new());
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        sync_mode: oryq_engine::SyncMode::Off,
        max_wal_hard_bytes: 256 * 1024,
        wal_autocheckpoint_bytes: 64 * 1024,
        ..DatabaseConfig::default()
    };
    let db = Database::open_with_io(io, std::path::Path::new("autockpt.oryq"), cfg).unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("logs").unwrap();
        wtx.commit().unwrap();
    }
    for i in 0..150i64 {
        let mut wtx = db.begin_write().unwrap();
        wtx.collection("logs")
            .unwrap()
            .insert_json(&format!(r#"{{"_id":{i},"msg":"entry-{i}"}}"#))
            .unwrap();
        wtx.commit().unwrap();
    }
    let rtx = db.begin_read().unwrap();
    assert_eq!(rtx.collection("logs").unwrap().count().unwrap(), 150);
    drop(rtx);

    // 2. Pre-sync VersionIndex capacity guard returns QuotaExceeded WITHOUT poisoning the engine.
    let eng_io: Arc<dyn StorageIo> = Arc::new(MemoryVfs::new());
    let eng_cfg = EngineConfig {
        page_size: PageSize::Size4096,
        sync_mode: oryq_engine::SyncMode::Off,
        max_version_entries: 2,
        wal_autocheckpoint_bytes: 0,
        ..EngineConfig::default()
    };
    let engine = StorageEngine::open(
        eng_io,
        std::path::Path::new("ver_cap.oryq"),
        eng_cfg,
        [0x55; 16],
    )
    .unwrap();
    let _pin = engine.begin_read().unwrap();
    for step in 0..2u8 {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, &[step], b"ok").unwrap();
        w.set_catalog_root_page_id(tree.root());
        w.commit().unwrap();
    }
    // 3rd commit exceeds max_version_entries=2 -> clean QuotaExceeded, NOT poisoned!
    let mut w3 = engine.begin_write().unwrap();
    let mut tree = BTree::open(w3.catalog_root_page_id(), TreeKind::Collection);
    tree.insert(&mut w3, b"overflow_ver", b"no").unwrap();
    w3.set_catalog_root_page_id(tree.root());
    let err = w3.commit().unwrap_err();
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
    assert!(
        !engine.is_poisoned(),
        "VersionIndex quota exhaustion must never poison StorageEngine"
    );
}
