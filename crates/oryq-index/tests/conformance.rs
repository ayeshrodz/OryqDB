use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{from_json_str, ValueView};
use oryq_engine::{EngineConfig, OsVfs, PageId, PageSize, StorageEngine};
use oryq_index::{
    lookup_primary_id, scan_primary_record_ids, IndexDescriptor, IndexId, IndexProviderRegistry,
    IndexReadContext, IndexWriteContext, NamespaceId, RecordId, PRIMARY_PROVIDER_ID,
};
use oryq_types::{CapabilityId, ErrorCategory};

const TEST_UUID: [u8; 16] = [0x77; 16];

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("oryqdb_idx_{label}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn primary_index_empty_create_reopen_drop_and_crud_conformance() {
    let dir = unique_temp_dir("primary_conformance");
    let db_path = dir.join("idx.oryq");
    let vfs = Arc::new(OsVfs::new());
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let registry = IndexProviderRegistry::default_v1();
    let primary_cap = CapabilityId::new(PRIMARY_PROVIDER_ID).unwrap();
    let provider = registry.require(&primary_cap).unwrap();
    let idx_id = IndexId::new(1).unwrap();

    // 1. Create empty primary index on an empty collection -> commit -> reopen -> scan empty -> drop
    let empty_root = {
        let engine = StorageEngine::open(vfs.clone(), &db_path, cfg.clone(), TEST_UUID).unwrap();
        let mut w = engine.begin_write().unwrap();
        let draft = IndexDescriptor::primary(idx_id, PageId::new(1));
        let root = provider.create_empty(&mut w, &draft).unwrap();
        w.set_catalog_root_page_id(root);
        w.commit().unwrap();
        root
    };

    {
        let engine = StorageEngine::open(vfs.clone(), &db_path, cfg.clone(), TEST_UUID).unwrap();
        let r = engine.begin_read().unwrap();
        let read_ctx = IndexReadContext::new(NamespaceId::new(1).unwrap(), &r);
        assert!(scan_primary_record_ids(&read_ctx, empty_root)
            .unwrap()
            .is_empty());
        drop(r);

        // Drop empty index cleanly
        let mut w = engine.begin_write().unwrap();
        let desc = IndexDescriptor::primary(idx_id, empty_root);
        provider.drop_index(&mut w, &desc).unwrap();
        w.commit().unwrap();
    }

    // 2. Create fresh primary index, insert diverse _id types, test DuplicateKey savepoint rollback,
    //    immutability on update, delete, and persistence across reopen.
    let active_root = {
        let engine = StorageEngine::open(vfs.clone(), &db_path, cfg.clone(), TEST_UUID).unwrap();
        let mut w = engine.begin_write().unwrap();
        let mut desc = IndexDescriptor::primary(idx_id, PageId::new(1));
        desc.root_page_id = provider.create_empty(&mut w, &desc).unwrap();

        // Exercise restricted IndexWriteContext compilation
        {
            let _write_ctx = IndexWriteContext::new(NamespaceId::new(1).unwrap(), &mut w);
        }

        let (d1, _) = from_json_str(r#"{"_id": 100, "name": "alice"}"#, None).unwrap();
        let (d2, _) = from_json_str(r#"{"_id": "bob-id", "name": "bob"}"#, None).unwrap();
        let (d3, _) = from_json_str(
            r#"{"_id": {"$oid": "660102030405060708090a0b"}, "name": "carol"}"#,
            None,
        )
        .unwrap();

        let r1 = RecordId::new(1).unwrap();
        let r2 = RecordId::new(2).unwrap();
        let r3 = RecordId::new(3).unwrap();

        desc.root_page_id = provider
            .on_insert(&mut w, &desc, r1, &d1.view())
            .unwrap()
            .new_root_page_id;
        desc.root_page_id = provider
            .on_insert(&mut w, &desc, r2, &d2.view())
            .unwrap()
            .new_root_page_id;
        desc.root_page_id = provider
            .on_insert(&mut w, &desc, r3, &d3.view())
            .unwrap()
            .new_root_page_id;

        // Duplicate _id inside statement savepoint fails with DuplicateKey and rolls back cleanly
        let sp = w.savepoint();
        let dup_err = provider
            .on_insert(&mut w, &desc, RecordId::new(99).unwrap(), &d1.view())
            .unwrap_err();
        assert_eq!(dup_err.category(), ErrorCategory::DuplicateKey);
        w.rollback_to(sp).unwrap();

        // Update with same _id succeeds; update with different _id fails
        let (d1_upd, _) = from_json_str(r#"{"_id": 100, "name": "alice-2"}"#, None).unwrap();
        let (d1_bad, _) = from_json_str(r#"{"_id": 101, "name": "alice-bad"}"#, None).unwrap();
        provider
            .on_update(&mut w, &desc, r1, &d1.view(), &d1_upd.view())
            .unwrap();
        assert_eq!(
            provider
                .on_update(&mut w, &desc, r1, &d1.view(), &d1_bad.view())
                .unwrap_err()
                .category(),
            ErrorCategory::InvalidArgument
        );

        // Delete d2
        desc.root_page_id = provider
            .on_delete(&mut w, &desc, r2, &d2.view())
            .unwrap()
            .new_root_page_id;

        w.set_catalog_root_page_id(desc.root_page_id);
        w.commit().unwrap();
        desc.root_page_id
    };

    // Reopen and verify exact lookups & canonical ordering (Int64 < String < ObjectId)
    {
        let engine = StorageEngine::open(vfs, &db_path, cfg, TEST_UUID).unwrap();
        let r = engine.begin_read().unwrap();
        let ctx = IndexReadContext::new(NamespaceId::new(1).unwrap(), &r);

        assert_eq!(
            lookup_primary_id(&ctx, active_root, &ValueView::Int64(100)).unwrap(),
            Some(RecordId::new(1).unwrap())
        );
        assert_eq!(
            lookup_primary_id(&ctx, active_root, &ValueView::String("bob-id")).unwrap(),
            None
        );

        let all = scan_primary_record_ids(&ctx, active_root).unwrap();
        assert_eq!(
            all,
            vec![RecordId::new(1).unwrap(), RecordId::new(3).unwrap()]
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
