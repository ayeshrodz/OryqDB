use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{from_json_str, FieldPath};
use oryq_engine::{BTree, EngineConfig, OsVfs, PageSize, StorageEngine, TreeKind};
use oryq_index::{IndexProvider, IndexReadContext};
use oryq_types::{ErrorCategory, IndexId, NamespaceId, PageId, RecordId};
use oryq_vec::codec::{decode_vector_payload, encode_vector_payload, VecIndexMeta, KEY_META};
use oryq_vec::{
    search_vec, vec_index_descriptor, DistanceMetric, VectorIndexOptions, VectorIndexProvider,
    VectorQuantization, MAX_VECTOR_DIMENSIONS, VEC_FEATURE_BIT, VEC_PROVIDER_ID,
};

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
            "oryqdb_vec_provider_{label}_{}_{seq}_{nanos}",
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

fn open_test_engine(dir: &TempDir, file: &str) -> StorageEngine {
    let vfs = Arc::new(OsVfs::new());
    StorageEngine::open(
        vfs,
        &dir.path().join(file),
        EngineConfig {
            page_size: PageSize::Size4096,
            ..EngineConfig::default()
        },
        [0x5A; 16],
    )
    .unwrap()
}

#[test]
fn options_json_roundtrip_and_validation() {
    let opts = VectorIndexOptions::with_quantization(
        128,
        DistanceMetric::Cosine,
        VectorQuantization::Int8,
    )
    .unwrap();
    let json = opts.to_json();
    let parsed = VectorIndexOptions::from_json(&json).unwrap();
    assert_eq!(parsed, opts);

    // Zero dimensions rejected
    assert_eq!(
        VectorIndexOptions::new(0, DistanceMetric::L2)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    // Unknown metric rejected
    assert_eq!(
        VectorIndexOptions::from_json(r#"{"dim":4,"metric":"manhattan"}"#)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
}

#[test]
fn cosine_l2_dot_ranking_and_tie_breaking() {
    let dir = TempDir::new("metrics");
    let engine = open_test_engine(&dir, "metrics.oryq");
    let provider = VectorIndexProvider;
    assert_eq!(provider.provider_id().as_str(), VEC_PROVIDER_ID);

    // 1. Cosine metric with tie-breaking and zero-vector safety
    let opts_cos = VectorIndexOptions::new(3, DistanceMetric::Cosine).unwrap();
    let mut desc_cos = vec_index_descriptor(
        IndexId::new(10).unwrap(),
        "emb_cos".into(),
        FieldPath::parse("emb").unwrap(),
        opts_cos,
        PageId::new(1),
    )
    .unwrap();
    assert_eq!(desc_cos.required_feature_bit, Some(VEC_FEATURE_BIT));

    let mut wtx = engine.begin_write().unwrap();
    desc_cos.root_page_id = provider.create_empty(&mut wtx, &desc_cos).unwrap();

    let (d1, _) = from_json_str(r#"{"_id": 1, "emb": [1.0, 0.0, 0.0]}"#, None).unwrap();
    let (d2, _) = from_json_str(r#"{"_id": 2, "emb": [0.0, 1.0, 0.0]}"#, None).unwrap();
    let (d3, _) = from_json_str(r#"{"_id": 3, "emb": [1.0, 0.0, 0.0]}"#, None).unwrap();
    let (d4, _) = from_json_str(r#"{"_id": 4, "emb": [0.0, 0.0, 0.0]}"#, None).unwrap();
    let (d5, _) = from_json_str(r#"{"_id": 5, "note": "no vector field"}"#, None).unwrap();

    for (rid, doc) in [(1, &d1), (2, &d2), (3, &d3), (4, &d4), (5, &d5)] {
        let m = provider
            .on_insert(
                &mut wtx,
                &desc_cos,
                RecordId::new(rid).unwrap(),
                &doc.view(),
            )
            .unwrap();
        desc_cos.root_page_id = m.new_root_page_id;
    }
    wtx.commit().unwrap();

    let rtx = engine.begin_read().unwrap();
    let hits = search_vec(&rtx, &desc_cos, &[1.0, 0.0, 0.0], 10).unwrap();
    assert_eq!(hits.len(), 4);
    // Record 1 and 3 tie at distance 0.0 -> RecordId ASC tie-break
    assert_eq!(hits[0].record_id.raw(), 1);
    assert!(hits[0].distance.abs() < 1e-6);
    assert_eq!(hits[1].record_id.raw(), 3);
    assert!(hits[1].distance.abs() < 1e-6);
    // Orthogonal (2) and zero-vector (4) both have distance 1.0 -> RecordId ASC
    assert_eq!(hits[2].record_id.raw(), 2);
    assert!((hits[2].distance - 1.0).abs() < 1e-6);
    assert_eq!(hits[3].record_id.raw(), 4);
    assert!((hits[3].distance - 1.0).abs() < 1e-6);

    // limit = 1 and limit = 0
    assert_eq!(
        search_vec(&rtx, &desc_cos, &[1.0, 0.0, 0.0], 1)
            .unwrap()
            .len(),
        1
    );
    assert!(search_vec(&rtx, &desc_cos, &[1.0, 0.0, 0.0], 0)
        .unwrap()
        .is_empty());

    // Wrong query dimension or non-finite query rejected
    assert_eq!(
        search_vec(&rtx, &desc_cos, &[1.0, 0.0], 5)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        search_vec(&rtx, &desc_cos, &[f32::NAN, 0.0, 0.0], 5)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    drop(rtx);

    // 2. L2 and DotProduct checks
    let opts_l2 = VectorIndexOptions::new(2, DistanceMetric::L2).unwrap();
    let mut desc_l2 = vec_index_descriptor(
        IndexId::new(11).unwrap(),
        "emb_l2".into(),
        FieldPath::parse("v").unwrap(),
        opts_l2,
        PageId::new(1),
    )
    .unwrap();
    let opts_dot = VectorIndexOptions::new(2, DistanceMetric::DotProduct).unwrap();
    let mut desc_dot = vec_index_descriptor(
        IndexId::new(12).unwrap(),
        "emb_dot".into(),
        FieldPath::parse("v").unwrap(),
        opts_dot,
        PageId::new(1),
    )
    .unwrap();

    let mut wtx = engine.begin_write().unwrap();
    desc_l2.root_page_id = provider.create_empty(&mut wtx, &desc_l2).unwrap();
    desc_dot.root_page_id = provider.create_empty(&mut wtx, &desc_dot).unwrap();
    let (a, _) = from_json_str(r#"{"_id": 1, "v": [1, 0]}"#, None).unwrap();
    let (b, _) = from_json_str(r#"{"_id": 2, "v": [10, 0]}"#, None).unwrap();
    for (rid, doc) in [(1, &a), (2, &b)] {
        desc_l2.root_page_id = provider
            .on_insert(&mut wtx, &desc_l2, RecordId::new(rid).unwrap(), &doc.view())
            .unwrap()
            .new_root_page_id;
        desc_dot.root_page_id = provider
            .on_insert(
                &mut wtx,
                &desc_dot,
                RecordId::new(rid).unwrap(),
                &doc.view(),
            )
            .unwrap()
            .new_root_page_id;
    }
    wtx.commit().unwrap();

    let rtx = engine.begin_read().unwrap();
    // Query [1, 0]: L2 prefers [1, 0] (dist=0), DotProduct prefers [10, 0] (dist=-10)
    let l2_hits = search_vec(&rtx, &desc_l2, &[1.0, 0.0], 2).unwrap();
    assert_eq!(l2_hits[0].record_id.raw(), 1);
    assert!(l2_hits[0].distance.abs() < 1e-6);
    assert_eq!(l2_hits[1].record_id.raw(), 2);
    assert!((l2_hits[1].distance - 9.0).abs() < 1e-6);

    let dot_hits = search_vec(&rtx, &desc_dot, &[1.0, 0.0], 2).unwrap();
    assert_eq!(dot_hits[0].record_id.raw(), 2);
    assert!((dot_hits[0].distance - (-10.0)).abs() < 1e-6);
    assert_eq!(dot_hits[1].record_id.raw(), 1);
    assert!((dot_hits[1].distance - (-1.0)).abs() < 1e-6);
}

#[test]
fn int8_quantization_update_transitions_and_drop() {
    let dir = TempDir::new("int8_lifecycle");
    let engine = open_test_engine(&dir, "int8.oryq");
    let provider = VectorIndexProvider;

    let opts =
        VectorIndexOptions::with_quantization(4, DistanceMetric::Cosine, VectorQuantization::Int8)
            .unwrap();
    let mut desc = vec_index_descriptor(
        IndexId::new(20).unwrap(),
        "emb_int8".into(),
        FieldPath::parse("vec").unwrap(),
        opts,
        PageId::new(1),
    )
    .unwrap();

    let (d1, _) = from_json_str(r#"{"_id": 1, "vec": [0.9, 0.1, 0.0, 0.0]}"#, None).unwrap();
    let (d2, _) = from_json_str(r#"{"_id": 2, "vec": [0.0, 0.0, 0.9, 0.1]}"#, None).unwrap();
    let (d1_null, _) = from_json_str(r#"{"_id": 1, "vec": null}"#, None).unwrap();
    let (d1_restored, _) =
        from_json_str(r#"{"_id": 1, "vec": [0.0, 0.0, 0.1, 0.9]}"#, None).unwrap();
    let (bad_dim, _) = from_json_str(r#"{"_id": 3, "vec": [1.0, 2.0]}"#, None).unwrap();
    let (bad_elem, _) =
        from_json_str(r#"{"_id": 4, "vec": [1.0, "oops", 0.0, 0.0]}"#, None).unwrap();

    let mut wtx = engine.begin_write().unwrap();
    desc.root_page_id = provider.create_empty(&mut wtx, &desc).unwrap();
    desc.root_page_id = provider
        .on_insert(&mut wtx, &desc, RecordId::new(1).unwrap(), &d1.view())
        .unwrap()
        .new_root_page_id;
    desc.root_page_id = provider
        .on_insert(&mut wtx, &desc, RecordId::new(2).unwrap(), &d2.view())
        .unwrap()
        .new_root_page_id;

    // Invalid dimension & non-numeric element fail cleanly
    assert_eq!(
        provider
            .on_insert(&mut wtx, &desc, RecordId::new(3).unwrap(), &bad_dim.view())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        provider
            .on_insert(&mut wtx, &desc, RecordId::new(4).unwrap(), &bad_elem.view())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );

    // Transition: non-empty -> null removes entry
    desc.root_page_id = provider
        .on_update(
            &mut wtx,
            &desc,
            RecordId::new(1).unwrap(),
            &d1.view(),
            &d1_null.view(),
        )
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();

    {
        let rtx = engine.begin_read().unwrap();
        let hits = search_vec(&rtx, &desc, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_id.raw(), 2);
    }

    // Transition: null -> non-empty restores entry
    let mut wtx = engine.begin_write().unwrap();
    desc.root_page_id = provider
        .on_update(
            &mut wtx,
            &desc,
            RecordId::new(1).unwrap(),
            &d1_null.view(),
            &d1_restored.view(),
        )
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();

    {
        let rtx = engine.begin_read().unwrap();
        let hits = search_vec(&rtx, &desc, &[0.0, 0.0, 0.0, 1.0], 5).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].record_id.raw(), 1);
    }

    // Delete & drop index cleanly
    let mut wtx = engine.begin_write().unwrap();
    desc.root_page_id = provider
        .on_delete(&mut wtx, &desc, RecordId::new(2).unwrap(), &d2.view())
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();

    {
        let rtx = engine.begin_read().unwrap();
        assert_eq!(
            search_vec(&rtx, &desc, &[0.0, 0.0, 1.0, 0.0], 5)
                .unwrap()
                .len(),
            1
        );
    }

    let mut wtx = engine.begin_write().unwrap();
    provider.drop_index(&mut wtx, &desc).unwrap();
    wtx.commit().unwrap();
}

#[test]
fn page_size_fit_validation_extreme_int8_and_meta_vector_count() {
    let dir = TempDir::new("page_fit_and_meta");
    let provider = VectorIndexProvider;

    // 1. 1024-dim F32 on 4 KiB page rejected at create_empty; 1024-dim Int8 on 4 KiB succeeds
    let engine_4k = open_test_engine(&dir, "p4k.oryq");
    let opts_f32_1024 = VectorIndexOptions::new(MAX_VECTOR_DIMENSIONS, DistanceMetric::L2).unwrap();
    let desc_f32_1024 = vec_index_descriptor(
        IndexId::new(30).unwrap(),
        "f32_1024".into(),
        FieldPath::parse("v").unwrap(),
        opts_f32_1024,
        PageId::new(1),
    )
    .unwrap();
    let opts_i8_1024 = VectorIndexOptions::with_quantization(
        MAX_VECTOR_DIMENSIONS,
        DistanceMetric::Cosine,
        VectorQuantization::Int8,
    )
    .unwrap();
    let mut desc_i8_1024 = vec_index_descriptor(
        IndexId::new(31).unwrap(),
        "i8_1024".into(),
        FieldPath::parse("v").unwrap(),
        opts_i8_1024,
        PageId::new(1),
    )
    .unwrap();

    let mut wtx = engine_4k.begin_write().unwrap();
    assert_eq!(
        provider
            .create_empty(&mut wtx, &desc_f32_1024)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    desc_i8_1024.root_page_id = provider.create_empty(&mut wtx, &desc_i8_1024).unwrap();
    wtx.commit().unwrap();

    // 2. 1024-dim F32 on 8 KiB page succeeds
    let vfs = Arc::new(OsVfs::new());
    let engine_8k = StorageEngine::open(
        vfs,
        &dir.path().join("p8k.oryq"),
        EngineConfig {
            page_size: PageSize::Size8192,
            ..EngineConfig::default()
        },
        [0x5B; 16],
    )
    .unwrap();
    let mut wtx8 = engine_8k.begin_write().unwrap();
    let root8 = provider.create_empty(&mut wtx8, &desc_f32_1024).unwrap();
    assert!(root8.raw() > 0);
    wtx8.commit().unwrap();

    // 3. Int8 constant vector, negative values, and extreme finite range [-3e38, 3e38]
    for sample in [
        vec![5.0f32, 5.0, 5.0, 5.0],
        vec![-1.0f32, -0.5, 0.25, 0.75],
        vec![-3.0e38f32, 0.0, 1.5e38, 3.0e38],
    ] {
        let min_v = sample
            .iter()
            .copied()
            .map(f64::from)
            .fold(f64::INFINITY, f64::min);
        let max_v = sample
            .iter()
            .copied()
            .map(f64::from)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_err = ((max_v - min_v) / 510.0).max(1e-5);
        let encoded = encode_vector_payload(&sample, VectorQuantization::Int8);
        let decoded = decode_vector_payload(&encoded, sample.len()).unwrap();
        assert!(decoded.norm.is_finite());
        assert_eq!(decoded.values.len(), sample.len());
        for (&orig, &deq) in sample.iter().zip(decoded.values.iter()) {
            assert!(deq.is_finite());
            let err = (f64::from(orig) - f64::from(deq)).abs();
            assert!(
                err <= max_err * 1.01,
                "orig={orig}, deq={deq}, err={err}, max_err={max_err}"
            );
        }
    }

    // 4. VecIndexMeta.vector_count accuracy + f32::INFINITY / f64->f32 overflow (1e300) rejection
    let opts = VectorIndexOptions::new(2, DistanceMetric::L2).unwrap();
    let mut desc = vec_index_descriptor(
        IndexId::new(32).unwrap(),
        "meta_check".into(),
        FieldPath::parse("v").unwrap(),
        opts,
        PageId::new(1),
    )
    .unwrap();
    let (d1, _) = from_json_str(r#"{"_id": 1, "v": [1.0, 2.0]}"#, None).unwrap();
    let (d1_updated, _) = from_json_str(r#"{"_id": 1, "v": [3.0, 4.0]}"#, None).unwrap();
    let (d1_null, _) = from_json_str(r#"{"_id": 1, "v": null}"#, None).unwrap();
    let (d_overflow, _) = from_json_str(r#"{"_id": 2, "v": [1e300, 0.0]}"#, None).unwrap();

    let read_count = |engine: &StorageEngine, d: &oryq_index::IndexDescriptor| -> u64 {
        let rtx = engine.begin_read().unwrap();
        let ns = NamespaceId::new(d.index_id.raw()).unwrap();
        let ctx = IndexReadContext::new(ns, &rtx);
        let tree = BTree::open(d.root_page_id, TreeKind::Index);
        let raw = tree.get(&ctx, KEY_META).unwrap().unwrap();
        VecIndexMeta::decode(&raw).unwrap().vector_count
    };

    let mut wtx = engine_4k.begin_write().unwrap();
    desc.root_page_id = provider.create_empty(&mut wtx, &desc).unwrap();
    // f64 -> f32 overflow (1e300 -> f32::INFINITY) rejected cleanly
    assert_eq!(
        provider
            .on_insert(
                &mut wtx,
                &desc,
                RecordId::new(2).unwrap(),
                &d_overflow.view()
            )
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    desc.root_page_id = provider
        .on_insert(&mut wtx, &desc, RecordId::new(1).unwrap(), &d1.view())
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();
    assert_eq!(read_count(&engine_4k, &desc), 1);

    // f32::INFINITY in query rejected cleanly
    {
        let rtx = engine_4k.begin_read().unwrap();
        assert_eq!(
            search_vec(&rtx, &desc, &[f32::INFINITY, 0.0], 5)
                .unwrap_err()
                .category(),
            ErrorCategory::InvalidArgument
        );
    }

    // Update non-empty -> non-empty keeps vector_count == 1
    let mut wtx = engine_4k.begin_write().unwrap();
    desc.root_page_id = provider
        .on_update(
            &mut wtx,
            &desc,
            RecordId::new(1).unwrap(),
            &d1.view(),
            &d1_updated.view(),
        )
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();
    assert_eq!(read_count(&engine_4k, &desc), 1);

    // Update non-empty -> null decrements vector_count to 0
    let mut wtx = engine_4k.begin_write().unwrap();
    desc.root_page_id = provider
        .on_update(
            &mut wtx,
            &desc,
            RecordId::new(1).unwrap(),
            &d1_updated.view(),
            &d1_null.view(),
        )
        .unwrap()
        .new_root_page_id;
    wtx.commit().unwrap();
    assert_eq!(read_count(&engine_4k, &desc), 0);
}
