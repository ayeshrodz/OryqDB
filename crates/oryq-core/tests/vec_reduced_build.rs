#![cfg(not(feature = "vec"))]

//! Verifies reduced-build fail-closed behavior: when `oryq-core` is compiled WITHOUT
//! `--features vec`, `CoreRegistries::default_v1()` does NOT register `index.vec`, and
//! opening a database whose catalog contains a persisted `index.vec` index allows reads
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
            "oryqdb_vec_reduced_{label}_{}_{seq}_{nanos}",
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
fn reduced_build_without_vec_rejects_vec_provider_fail_closed() {
    // 1. Registry-level check
    let registries = CoreRegistries::default_v1();
    let vec_id = CapabilityId::new("index.vec").unwrap();
    assert!(!registries.indexes.contains(&vec_id));
    let err = registries.indexes.require(&vec_id).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::UnsupportedFeature);

    // 2. End-to-end Database check with a persisted `index.vec` catalog entry
    let dir = TempDir::new("db");
    let db_path = dir.path().join("with_vec.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        {
            let mut col = wtx.collection("items").unwrap();
            col.insert_json(r#"{"_id": 1, "emb": [1.0, 0.0, 0.0]}"#)
                .unwrap();
        }
        wtx.commit().unwrap();
        db.checkpoint(true).unwrap();
    }

    // Persist an `index.vec` descriptor in the catalog (simulating a DB written by `--features vec`)
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(
            vfs,
            &db_path,
            EngineConfig {
                page_size: PageSize::Size4096,
                ..EngineConfig::default()
            },
            [0x22; 16],
        )
        .unwrap();
        let mut wtx = engine.begin_write().unwrap();
        let cat_root = wtx.catalog_root_page_id();
        let mut entry = load_collection_entry(&wtx, cat_root, "items")
            .unwrap()
            .unwrap();
        let empty_tree = BTree::create_empty(&mut wtx, TreeKind::Index).unwrap();
        entry.indexes.push(IndexDescriptor {
            index_id: IndexId::new(101).unwrap(),
            name: "emb_vec".to_owned(),
            kind: IndexKind::Pluggable("vec".to_owned()),
            fields: vec![IndexFieldSpec {
                path: FieldPath::parse("emb").unwrap(),
                order: SortOrder::Asc,
            }],
            unique: false,
            sparse: true,
            root_page_id: empty_tree.root(),
            required_feature_bit: Some(1),
            provider_id: vec_id,
            data_version: 1,
            options_json: Some(r#"{"dim":3,"metric":"cosine","quantization":"f32"}"#.to_owned()),
        });
        entry.required_write_features = entry
            .required_write_features
            .union(FeatureBitmask::from_raw(1u64 << 1));
        let new_root = save_collection_entry(&mut wtx, cat_root, &entry).unwrap();
        wtx.set_catalog_root_page_id(new_root);
        wtx.commit().unwrap();
        engine.checkpoint(true).unwrap();
    }

    // Open in reduced build (without `vec`): reads succeed, mutations & backup fail closed!
    let db = Database::open(&db_path, cfg).unwrap();
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        assert!(col.find_by_id(&Value::Int64(1)).unwrap().is_some());
    }

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("items").unwrap();
        assert_eq!(
            col.insert_json(r#"{"_id": 2, "emb": [0.0, 1.0, 0.0]}"#)
                .unwrap_err()
                .category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.update_by_id(&Value::Int64(1), r#"{"$set": {"emb": [0.0, 0.0, 1.0]}}"#)
                .unwrap_err()
                .category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.delete_by_id(&Value::Int64(1)).unwrap_err().category(),
            ErrorCategory::UnsupportedFeature
        );
        assert_eq!(
            col.drop_index("emb_vec").unwrap_err().category(),
            ErrorCategory::UnsupportedFeature
        );
    }

    let bk_path = dir.path().join("blocked_backup.oryq");
    assert_eq!(
        db.backup_to(&bk_path).unwrap_err().category(),
        ErrorCategory::UnsupportedFeature
    );

    // Original document remains intact
    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("items").unwrap();
    assert_eq!(col.count().unwrap(), 1);
}
