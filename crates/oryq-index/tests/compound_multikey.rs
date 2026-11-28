use std::sync::Arc;

use oryq_document::{from_json_str, FieldPath, ValueView};
use oryq_engine::{EngineConfig, OsVfs, PageSize, StorageEngine};
use oryq_index::{
    extract_compound_key_prefixes, scan_compound_prefix_eq, CompoundIndexProvider, IndexDescriptor,
    IndexFieldSpec, IndexProvider, SortOrder,
};
use oryq_types::{ErrorCategory, IndexId, PageId, RecordId};

fn open_temp_engine(label: &str) -> StorageEngine {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_idx_compound_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("test.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };
    let vfs = Arc::new(OsVfs::new());
    StorageEngine::open(vfs, &db_path, cfg, [7u8; 16]).unwrap()
}

#[test]
fn compound_and_multikey_index_prefix_scan_dedup_and_parallel_array_rejection() {
    let engine = open_temp_engine("multikey");
    let provider = CompoundIndexProvider;

    let mut desc = IndexDescriptor::compound(
        IndexId::new(10).unwrap(),
        "tags_1_score_-1".to_owned(),
        vec![
            IndexFieldSpec {
                path: FieldPath::parse("tags").unwrap(),
                order: SortOrder::Asc,
            },
            IndexFieldSpec {
                path: FieldPath::parse("score").unwrap(),
                order: SortOrder::Desc,
            },
        ],
        false,
        false,
        PageId::new(1),
    );

    let mut tx = engine.begin_write().unwrap();
    desc.root_page_id = provider.create_empty(&mut tx, &desc).unwrap();

    // Multi-key insert with duplicate array elements ("rust", "rust") -> deduplicated!
    let (d1, _) = from_json_str(
        r#"{"_id": 1, "tags": ["rust", "db", "rust"], "score": 90}"#,
        None,
    )
    .unwrap();
    let (d2, _) =
        from_json_str(r#"{"_id": 2, "tags": ["rust", "cli"], "score": 95}"#, None).unwrap();
    let r1 = RecordId::new(1).unwrap();
    let r2 = RecordId::new(2).unwrap();

    let m1 = provider.on_insert(&mut tx, &desc, r1, &d1.view()).unwrap();
    assert_eq!(m1.entries_added, 2); // "rust" + "db" (deduplicated)
    desc.root_page_id = m1.new_root_page_id;

    let m2 = provider.on_insert(&mut tx, &desc, r2, &d2.view()).unwrap();
    assert_eq!(m2.entries_added, 2);
    desc.root_page_id = m2.new_root_page_id;

    // Leading prefix scan for tags == "rust" returns [r2 (score 95), r1 (score 90)] due to score Desc!
    let hits = scan_compound_prefix_eq(&tx, &desc, &[ValueView::String("rust")]).unwrap();
    assert_eq!(hits, vec![r2, r1]);

    // Compound 2-field exact prefix scan for ("rust", 90) returns [r1]
    let exact = scan_compound_prefix_eq(
        &tx,
        &desc,
        &[ValueView::String("rust"), ValueView::Int64(90)],
    )
    .unwrap();
    assert_eq!(exact, vec![r1]);

    // Differential array update: replace "db" with "embedded"
    let (d1_after, _) = from_json_str(
        r#"{"_id": 1, "tags": ["rust", "embedded"], "score": 90}"#,
        None,
    )
    .unwrap();
    let mu = provider
        .on_update(&mut tx, &desc, r1, &d1.view(), &d1_after.view())
        .unwrap();
    assert_eq!(mu.entries_removed, 1);
    assert_eq!(mu.entries_added, 1);
    desc.root_page_id = mu.new_root_page_id;

    assert!(
        scan_compound_prefix_eq(&tx, &desc, &[ValueView::String("db")])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        scan_compound_prefix_eq(&tx, &desc, &[ValueView::String("embedded")]).unwrap(),
        vec![r1]
    );

    // Parallel-array prohibition: both `tags` and `score` are arrays -> InvalidArgument!
    let (bad, _) =
        from_json_str(r#"{"_id": 3, "tags": ["a", "b"], "score": [1, 2]}"#, None).unwrap();
    let err = extract_compound_key_prefixes(&desc, &bad.view(), PageSize::Size4096).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);

    tx.commit().unwrap();
}
