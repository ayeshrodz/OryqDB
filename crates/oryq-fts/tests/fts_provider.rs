use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{from_json_str, FieldPath};
use oryq_engine::{EngineConfig, OsVfs, PageSize, StorageEngine};
use oryq_fts::{
    fts_index_descriptor, search_fts, tokenize, FtsIndexProvider, FTS_FEATURE_BIT, FTS_PROVIDER_ID,
};
use oryq_index::IndexProvider;
use oryq_types::{IndexId, PageId, RecordId};

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
            "oryqdb_fts_{label}_{}_{seq}_{nanos}",
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
fn tokenizer_handles_unicode_punctuation_and_oversized_tokens() {
    assert_eq!(
        tokenize("Hello, Rust-Lang! OryqDB_v0.10.0 café"),
        vec!["hello", "rust", "lang", "oryqdb", "v0", "10", "0", "café"]
    );
    let long_word = "a".repeat(80);
    assert!(tokenize(&long_word).is_empty());
}

#[test]
fn fts_provider_bm25_ranking_update_delete_and_drop() {
    let dir = TempDir::new("provider");
    let db_path = dir.path().join("fts.oryq");
    let vfs = Arc::new(OsVfs::new());
    let engine = StorageEngine::open(
        vfs,
        &db_path,
        EngineConfig {
            page_size: PageSize::Size4096,
            ..EngineConfig::default()
        },
        [0xF7; 16],
    )
    .unwrap();

    let provider = FtsIndexProvider;
    assert_eq!(provider.provider_id().as_str(), FTS_PROVIDER_ID);

    let mut desc = fts_index_descriptor(
        IndexId::new(10).unwrap(),
        "body_fts".to_owned(),
        vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("body").unwrap(),
        ],
        PageId::new(1),
    );
    assert_eq!(desc.required_feature_bit, Some(FTS_FEATURE_BIT));

    let r1 = RecordId::new(1).unwrap();
    let r2 = RecordId::new(2).unwrap();
    let r3 = RecordId::new(3).unwrap();

    let (d1, _) = from_json_str(
        r#"{"_id": 1, "title": "Rust Database", "body": "Rust embedded NoSQL document database engine"}"#,
        None,
    )
    .unwrap();
    let (d2, _) = from_json_str(
        r#"{"_id": 2, "title": "Cooking Guide", "body": "How to bake sourdough bread in an oven"}"#,
        None,
    )
    .unwrap();
    let (d3, _) = from_json_str(
        r#"{"_id": 3, "title": "Rust CLI", "body": "Command line tools in Rust"}"#,
        None,
    )
    .unwrap();

    {
        let mut tx = engine.begin_write().unwrap();
        desc.root_page_id = provider.create_empty(&mut tx, &desc).unwrap();
        for (rid, doc) in [(r1, &d1), (r2, &d2), (r3, &d3)] {
            let maint = provider
                .on_insert(&mut tx, &desc, rid, &doc.view())
                .unwrap();
            desc.root_page_id = maint.new_root_page_id;
        }
        tx.commit().unwrap();
    }

    // BM25 query "rust database": r1 matches both "rust" and "database", r3 matches only "rust", r2 doesn't match!
    {
        let rtx = engine.begin_read().unwrap();
        let hits = search_fts(&rtx, &desc, "rust database", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].record_id, r1);
        assert_eq!(hits[1].record_id, r3);
        assert!(hits[0].score > hits[1].score);

        // Empty or unmatched query returns empty vec
        assert!(search_fts(&rtx, &desc, "   ", 10).unwrap().is_empty());
        assert!(search_fts(&rtx, &desc, "quantum", 10).unwrap().is_empty());
    }

    // Update r3 to remove "rust" and delete r1 -> search "rust" returns empty!
    let (d3_after, _) = from_json_str(
        r#"{"_id": 3, "title": "Go CLI", "body": "Command line tools in Go"}"#,
        None,
    )
    .unwrap();
    {
        let mut tx = engine.begin_write().unwrap();
        let mu = provider
            .on_update(&mut tx, &desc, r3, &d3.view(), &d3_after.view())
            .unwrap();
        desc.root_page_id = mu.new_root_page_id;
        let md = provider.on_delete(&mut tx, &desc, r1, &d1.view()).unwrap();
        desc.root_page_id = md.new_root_page_id;
        tx.commit().unwrap();
    }

    {
        let rtx = engine.begin_read().unwrap();
        assert!(search_fts(&rtx, &desc, "rust", 10).unwrap().is_empty());
        let go_hits = search_fts(&rtx, &desc, "go", 10).unwrap();
        assert_eq!(go_hits.len(), 1);
        assert_eq!(go_hits[0].record_id, r3);
    }

    // Drop index cleanly frees pages
    {
        let mut tx = engine.begin_write().unwrap();
        provider.drop_index(&mut tx, &desc).unwrap();
        tx.commit().unwrap();
    }
}

#[test]
fn fts_provider_handles_empty_transitions_limits_duplicate_terms_and_string_arrays() {
    let dir = TempDir::new("edge_cases");
    let db_path = dir.path().join("fts_edge.oryq");
    let vfs = Arc::new(OsVfs::new());
    let engine = StorageEngine::open(
        vfs,
        &db_path,
        EngineConfig {
            page_size: PageSize::Size4096,
            ..EngineConfig::default()
        },
        [0xE1; 16],
    )
    .unwrap();

    let provider = FtsIndexProvider;
    let mut desc = fts_index_descriptor(
        IndexId::new(11).unwrap(),
        "tags_fts".to_owned(),
        vec![FieldPath::parse("tags").unwrap()],
        PageId::new(1),
    );

    let r1 = RecordId::new(1).unwrap();
    let r2 = RecordId::new(2).unwrap();
    let (d1, _) = from_json_str(
        r#"{"_id": 1, "tags": ["rust", "embedded", "storage"]}"#,
        None,
    )
    .unwrap();
    let (d2, _) = from_json_str(r#"{"_id": 2, "tags": ["rust", "cli"]}"#, None).unwrap();
    let (d1_empty, _) = from_json_str(r#"{"_id": 1, "tags": []}"#, None).unwrap();
    let (d1_restored, _) = from_json_str(r#"{"_id": 1, "tags": ["rust", "bm25"]}"#, None).unwrap();

    {
        let mut tx = engine.begin_write().unwrap();
        desc.root_page_id = provider.create_empty(&mut tx, &desc).unwrap();
        desc.root_page_id = provider
            .on_insert(&mut tx, &desc, r1, &d1.view())
            .unwrap()
            .new_root_page_id;
        desc.root_page_id = provider
            .on_insert(&mut tx, &desc, r2, &d2.view())
            .unwrap()
            .new_root_page_id;
        tx.commit().unwrap();
    }

    // Duplicate query terms & limit truncation (limit = 0, limit = 1)
    {
        let rtx = engine.begin_read().unwrap();
        let single = search_fts(&rtx, &desc, "rust", 10).unwrap();
        let dup = search_fts(&rtx, &desc, "rust rust RUST", 10).unwrap();
        assert_eq!(single, dup);
        assert_eq!(search_fts(&rtx, &desc, "rust", 1).unwrap().len(), 1);
        assert!(search_fts(&rtx, &desc, "rust", 0).unwrap().is_empty());
    }

    // Non-empty -> empty transition removes doc from index & decrements stats
    {
        let mut tx = engine.begin_write().unwrap();
        desc.root_page_id = provider
            .on_update(&mut tx, &desc, r1, &d1.view(), &d1_empty.view())
            .unwrap()
            .new_root_page_id;
        tx.commit().unwrap();
    }
    {
        let rtx = engine.begin_read().unwrap();
        let hits = search_fts(&rtx, &desc, "rust", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_id, r2);
    }

    // Empty -> non-empty transition re-indexes doc cleanly
    {
        let mut tx = engine.begin_write().unwrap();
        desc.root_page_id = provider
            .on_update(&mut tx, &desc, r1, &d1_empty.view(), &d1_restored.view())
            .unwrap()
            .new_root_page_id;
        tx.commit().unwrap();
    }
    {
        let rtx = engine.begin_read().unwrap();
        let hits = search_fts(&rtx, &desc, "bm25", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_id, r1);
    }
}
