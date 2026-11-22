#![cfg(not(feature = "fts"))]

//! Verifies reduced-build fail-closed behavior: when `oryq-core` is compiled WITHOUT
//! `--features fts`, `CoreRegistries::default_v1()` does NOT register `index.fts`, and
//! opening a database whose catalog contains a persisted `index.fts` index allows reads
//! while failing closed (`ErrorCategory::UnsupportedFeature`) on mutations and backup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::catalog::{load_collection_entry, save_collection_entry};
use oryq_core::{
    CoreRegistries, Database, DatabaseConfig, ErrorCategory, IndexDescriptor, IndexKind, PageSize,
    SortOrder, Value,
};
use oryq_document::FieldPath;
use oryq_engine::{BTree, EngineConfig, OsVfs, StorageEngine, TreeKind};
use oryq_index::IndexFieldSpec;
use oryq_types::{CapabilityId, FeatureBitmask, IndexId};

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
            "oryqdb_fts_reduced_{label}_{}_{seq}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reduced_build_without_fts_rejects_fts_provider_fail_closed() {
    // 1. Registry-level check
    let registries = CoreRegistries::default_v1();
    let fts_id = CapabilityId::new("index.fts").unwrap();
    assert!(!registries.indexes.contains(&fts_id));
    let err = registries.indexes.require(&fts_id).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::UnsupportedFeature);

    // 2. End-to-end Database check with a persisted `index.fts` catalog entry
    let dir = TempDir::new("db");
    let db_path = dir.path().join("with_fts.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    // Create collection & insert 1 doc
    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("notes").unwrap();
        {
            let mut col = wtx.collection("notes").unwrap();
            col.insert_json(r#"{"_id": 1, "body": "hello world"}"#)
                .unwrap();
        }
        wtx.commit().unwrap();
        db.checkpoint(true).unwrap();
    }

    // Persist an `index.fts` descriptor in the catalog (simulating a DB written by an `--features fts` build)
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(
            vfs,
            &db_path,
            EngineConfig {
                page_size: PageSize::Size4096,
                ..EngineConfig::default()
            },
            [0x11; 16],
        )
        .unwrap();
        let mut wtx = engine.begin_write().unwrap();
        let cat_root = wtx.catalog_root_page_id();
        let mut entry = load_collection_entry(&wtx, cat_root, "notes")
            .unwrap()
            .unwrap();
        let empty_tree = BTree::create_empty(&mut wtx, TreeKind::Index).unwrap();
        entry.indexes.push(IndexDescriptor {
            index_id: IndexId::new(99).unwrap(),
            name: "body_fts".to_owned(),
            kind: IndexKind::Pluggable("fts".to_owned()),
            fields: vec![IndexFieldSpec {
                path: FieldPath::parse("body").unwrap(),
                order: SortOrder::Asc,
            }],
            unique: false,
            sparse: true,
            root_page_id: empty_tree.root(),
            required_feature_bit: Some(0),
            provider_id: fts_id,
            data_version: 1,
            options_json: None,
        });
        entry.required_write_features = entry
            .required_write_features
            .union(FeatureBitmask::from_raw(1u64 << 0));
        let new_root = save_collection_entry(&mut wtx, cat_root, &entry).unwrap();
        wtx.set_catalog_root_page_id(new_root);
        wtx.commit().unwrap();
        engine.checkpoint(true).unwrap();
    }

    // Open in reduced build (without `fts`): reads succeed, writes & backup fail closed!
    let db = Database::open(&db_path, cfg).unwrap();
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("notes").unwrap();
        let doc = col.find_by_id(&Value::Int64(1)).unwrap().unwrap();
        assert_eq!(
            doc.view().get_field("body").unwrap().to_owned_value(),
            Value::String("hello world".into())
        );
    }

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("notes").unwrap();
        assert_eq!(
            col.insert_json(r#"{"_id": 2, "body": "blocked"}"#)
                .unwrap_err()
                .category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.update_by_id(&Value::Int64(1), r#"{"$set": {"body": "edited"}}"#)
                .unwrap_err()
                .category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.delete_by_id(&Value::Int64(1)).unwrap_err().category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.drop_index("body_fts").unwrap_err().category(),
            ErrorCategory::UnsupportedFeature
        );
    }

    // Online backup also fails closed rather than silently dropping the FTS index
    let bk_path = dir.path().join("blocked_backup.oryq");
    assert_eq!(
        db.backup_to(&bk_path).unwrap_err().category(),
        ErrorCategory::UnsupportedFeature
    );

    // Original document remains intact after failed mutations
    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("notes").unwrap();
    assert_eq!(col.count().unwrap(), 1);
}
