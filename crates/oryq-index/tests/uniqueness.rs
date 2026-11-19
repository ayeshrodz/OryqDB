use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{from_json_str, FieldPath, ValueView};
use oryq_engine::{EngineConfig, OsVfs, PageId, PageSize, StorageEngine};
use oryq_index::{
    scan_secondary_all, scan_secondary_eq, scan_secondary_range, IndexDescriptor, IndexId,
    IndexProviderRegistry, RecordId, SortOrder, SECONDARY_PROVIDER_ID,
};
use oryq_types::{CapabilityId, ErrorCategory};

const TEST_UUID: [u8; 16] = [0x88; 16];

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("oryqdb_sec_{label}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn secondary_index_unique_sparse_nested_differential_and_range_scans() {
    let dir = unique_temp_dir("sec_full");
    let db_path = dir.join("sec.oryq");
    let vfs = Arc::new(OsVfs::new());
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let reg = IndexProviderRegistry::default_v1();
    let sec_cap = CapabilityId::new(SECONDARY_PROVIDER_ID).unwrap();
    let provider = reg.require(&sec_cap).unwrap();

    let (email_desc, score_desc, sparse_desc) = {
        let engine = StorageEngine::open(vfs.clone(), &db_path, cfg.clone(), TEST_UUID).unwrap();
        let mut w = engine.begin_write().unwrap();

        // 1. Unique nested index on "profile.email" (Asc, non-sparse)
        let mut email = IndexDescriptor::secondary(
            IndexId::new(2).unwrap(),
            "email_uniq".into(),
            FieldPath::parse("profile.email").unwrap(),
            SortOrder::Asc,
            true,
            false,
            PageId::new(1),
        );
        email.root_page_id = provider.create_empty(&mut w, &email).unwrap();

        // 2. Non-unique Desc index on "score"
        let mut score = IndexDescriptor::secondary(
            IndexId::new(3).unwrap(),
            "score_desc".into(),
            FieldPath::parse("score").unwrap(),
            SortOrder::Desc,
            false,
            false,
            PageId::new(1),
        );
        score.root_page_id = provider.create_empty(&mut w, &score).unwrap();

        // 3. Sparse unique index on "nickname" (missing fields omitted, explicit Null indexed)
        let mut sparse = IndexDescriptor::secondary(
            IndexId::new(4).unwrap(),
            "nick_sparse".into(),
            FieldPath::parse("nickname").unwrap(),
            SortOrder::Asc,
            true,
            true,
            PageId::new(1),
        );
        sparse.root_page_id = provider.create_empty(&mut w, &sparse).unwrap();

        let (d1, _) = from_json_str(
            r#"{"_id": 1, "profile": {"email": "a@example.com"}, "score": 10, "nickname": "ali"}"#,
            None,
        )
        .unwrap();
        let (d2, _) = from_json_str(
            r#"{"_id": 2, "profile": {"email": "b@example.com"}, "score": 30}"#,
            None,
        )
        .unwrap();
        let (d3, _) = from_json_str(
            r#"{"_id": 3, "profile": {"email": "c@example.com"}, "score": 20}"#,
            None,
        )
        .unwrap();

        for (rid_raw, doc) in [(1, &d1), (2, &d2), (3, &d3)] {
            let rid = RecordId::new(rid_raw).unwrap();
            email.root_page_id = provider
                .on_insert(&mut w, &email, rid, &doc.view())
                .unwrap()
                .new_root_page_id;
            score.root_page_id = provider
                .on_insert(&mut w, &score, rid, &doc.view())
                .unwrap()
                .new_root_page_id;
            sparse.root_page_id = provider
                .on_insert(&mut w, &sparse, rid, &doc.view())
                .unwrap()
                .new_root_page_id;
        }

        // Duplicate email rejected on unique index (d2 & d3 both missing nickname are allowed by sparse!)
        let (d_dup, _) = from_json_str(
            r#"{"_id": 4, "profile": {"email": "a@example.com"}, "score": 50}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            provider
                .on_insert(&mut w, &email, RecordId::new(4).unwrap(), &d_dup.view())
                .unwrap_err()
                .category(),
            ErrorCategory::DuplicateKey
        );

        // Differential update bypass: changing unindexed field does 0 index mutations
        let (d1_same_email, _) = from_json_str(
            r#"{"_id": 1, "profile": {"email": "a@example.com"}, "score": 10, "nickname": "ali", "extra": 1}"#,
            None,
        )
        .unwrap();
        let m = provider
            .on_update(
                &mut w,
                &email,
                RecordId::new(1).unwrap(),
                &d1.view(),
                &d1_same_email.view(),
            )
            .unwrap();
        assert_eq!((m.entries_added, m.entries_removed), (0, 0));

        // Differential update changing indexed field removes old & inserts new
        let (d1_new_score, _) = from_json_str(
            r#"{"_id": 1, "profile": {"email": "a@example.com"}, "score": 25, "nickname": "ali"}"#,
            None,
        )
        .unwrap();
        let m2 = provider
            .on_update(
                &mut w,
                &score,
                RecordId::new(1).unwrap(),
                &d1.view(),
                &d1_new_score.view(),
            )
            .unwrap();
        assert_eq!((m2.entries_added, m2.entries_removed), (1, 1));
        score.root_page_id = m2.new_root_page_id;

        w.commit().unwrap();
        (email, score, sparse)
    };

    // Reopen and verify exact equality, Desc ordering, range scans, and sparse omission
    {
        let engine = StorageEngine::open(vfs, &db_path, cfg, TEST_UUID).unwrap();
        let r = engine.begin_read().unwrap();

        assert_eq!(
            scan_secondary_eq(&r, &email_desc, &ValueView::String("b@example.com")).unwrap(),
            vec![RecordId::new(2).unwrap()]
        );

        // Score Desc native order: 30 (rid 2), 25 (rid 1), 20 (rid 3)
        assert_eq!(
            scan_secondary_all(&r, &score_desc).unwrap(),
            vec![
                RecordId::new(2).unwrap(),
                RecordId::new(1).unwrap(),
                RecordId::new(3).unwrap(),
            ]
        );

        // Range [20, 25] inclusive on Desc index -> rid 1 (25), rid 3 (20)
        assert_eq!(
            scan_secondary_range(
                &r,
                &score_desc,
                Some((&ValueView::Int64(20), true)),
                Some((&ValueView::Int64(25), true)),
            )
            .unwrap(),
            vec![RecordId::new(1).unwrap(), RecordId::new(3).unwrap()]
        );

        // Sparse index only contains rid 1 (d2 and d3 omitted because nickname was Missing)
        assert_eq!(
            scan_secondary_all(&r, &sparse_desc).unwrap(),
            vec![RecordId::new(1).unwrap()]
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
