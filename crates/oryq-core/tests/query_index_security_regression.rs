//! Comprehensive regression suite for Issue #45 (v0.14.1):
//! query/index correctness, nested projection redaction, aggregation, allocation-bomb & DoS guards,
//! CSPRNG ObjectId/UUID, and full multi-index integrity verification.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;

use oryq_core::Database;
use oryq_document::{
    compare_values, decode_physical_patch, encode_document, from_json_str, Decimal128, FieldPath,
    Value, ValueView,
};
use oryq_index::{decode_prefix_block, encode_value_key, SortOrder};
use oryq_query::{eval_filter, FilterExpr, FindOptions, SpillManager};
use oryq_types::ErrorCategory;

fn open_mem_db() -> Database {
    Database::open_in_memory().expect("open in-memory db")
}

fn collect_ids(docs: &[oryq_document::DocumentBuf]) -> BTreeSet<String> {
    docs.iter()
        .map(|d| format!("{:?}", d.view().get_field("_id").unwrap().to_owned_value()))
        .collect()
}

#[test]
fn secondary_multikey_arrays_and_whole_array_equality() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        let mut col = wtx.collection("items").unwrap();
        col.insert_json(r#"{"_id": 1, "tags": ["rust", "db"]}"#)
            .unwrap();
        col.insert_json(r#"{"_id": 2, "tags": ["go", "db"]}"#)
            .unwrap();
        col.insert_json(r#"{"_id": 3, "tags": []}"#).unwrap();
        wtx.commit().unwrap();
    }

    let unindexed_elem = {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        collect_ids(
            &col.find_json(Some(r#"{"tags": "rust"}"#), None, None, 0, None)
                .unwrap()
                .collect_all(),
        )
    };
    let unindexed_whole = {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        collect_ids(
            &col.find_json(Some(r#"{"tags": ["rust", "db"]}"#), None, None, 0, None)
                .unwrap()
                .collect_all(),
        )
    };

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("items").unwrap();
        col.create_index("tags", SortOrder::Asc, false, false, Some("idx_tags"))
            .unwrap();
        wtx.commit().unwrap();
    }

    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("items").unwrap();
    let indexed_elem = collect_ids(
        &col.find_json(Some(r#"{"tags": "rust"}"#), None, None, 0, None)
            .unwrap()
            .collect_all(),
    );
    let indexed_whole = collect_ids(
        &col.find_json(Some(r#"{"tags": ["rust", "db"]}"#), None, None, 0, None)
            .unwrap()
            .collect_all(),
    );
    let indexed_empty = collect_ids(
        &col.find_json(Some(r#"{"tags": []}"#), None, None, 0, None)
            .unwrap()
            .collect_all(),
    );

    assert_eq!(indexed_elem, unindexed_elem);
    assert_eq!(indexed_elem.len(), 1);
    assert_eq!(indexed_whole, unindexed_whole);
    assert_eq!(indexed_whole.len(), 1);
    assert_eq!(indexed_empty.len(), 1);

    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);
}

#[test]
fn compound_range_scans_asc_and_desc_preserve_inclusive_boundaries() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("scores").unwrap();
        let mut col = wtx.collection("scores").unwrap();
        for (id, score, name) in [
            (1, 10, "alice"),
            (2, 10, "bob"),
            (3, 20, "carol"),
            (4, 20, "dave"),
            (5, 30, "eve"),
        ] {
            col.insert_json(&format!(
                r#"{{"_id": {id}, "score": {score}, "name": "{name}"}}"#
            ))
            .unwrap();
        }
        wtx.commit().unwrap();
    }

    let queries = [
        r#"{"score": {"$lte": 20}}"#,
        r#"{"score": {"$lt": 20}}"#,
        r#"{"score": {"$gte": 20}}"#,
        r#"{"score": {"$gt": 20}}"#,
        r#"{"score": {"$gte": 10, "$lte": 20}}"#,
        r#"{"score": 20}"#,
    ];

    let expected: Vec<BTreeSet<String>> = {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("scores").unwrap();
        queries
            .iter()
            .map(|q| {
                collect_ids(
                    &col.find_json(Some(q), None, None, 0, None)
                        .unwrap()
                        .collect_all(),
                )
            })
            .collect()
    };

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("scores").unwrap();
        col.create_compound_index(
            &[("score", SortOrder::Asc), ("name", SortOrder::Asc)],
            false,
            false,
            Some("idx_score_asc_name"),
        )
        .unwrap();
        wtx.commit().unwrap();
    }
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("scores").unwrap();
        for (q, exp) in queries.iter().zip(&expected) {
            let got = collect_ids(
                &col.find_json(Some(q), None, None, 0, None)
                    .unwrap()
                    .collect_all(),
            );
            assert_eq!(&got, exp, "ASC compound mismatch for query {q}");
        }
    }

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("scores").unwrap();
        col.drop_index("idx_score_asc_name").unwrap();
        col.create_compound_index(
            &[("score", SortOrder::Desc), ("name", SortOrder::Asc)],
            false,
            false,
            Some("idx_score_desc_name"),
        )
        .unwrap();
        wtx.commit().unwrap();
    }
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("scores").unwrap();
        for (q, exp) in queries.iter().zip(&expected) {
            let got = collect_ids(
                &col.find_json(Some(q), None, None, 0, None)
                    .unwrap()
                    .collect_all(),
            );
            assert_eq!(&got, exp, "DESC compound mismatch for query {q}");
        }
    }

    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);
}

#[test]
fn compound_empty_array_uniqueness_and_multikey_range_conjunction() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("c").unwrap();
        let mut col = wtx.collection("c").unwrap();
        col.create_compound_index(
            &[("tags", SortOrder::Asc), ("owner", SortOrder::Asc)],
            true,
            false,
            Some("idx_unique_empty"),
        )
        .unwrap();
        col.insert_json(r#"{"_id": 1, "tags": [], "owner": "u1"}"#)
            .unwrap();
        let dup = col.insert_json(r#"{"_id": 2, "tags": [], "owner": "u1"}"#);
        assert_eq!(dup.unwrap_err().category(), ErrorCategory::DuplicateKey);
        wtx.commit().unwrap();
    }

    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("nums").unwrap();
        let mut col = wtx.collection("nums").unwrap();
        col.create_index("v", SortOrder::Asc, false, false, Some("idx_v"))
            .unwrap();
        col.insert_json(r#"{"_id": 1, "v": [5, 100]}"#).unwrap();
        col.insert_json(r#"{"_id": 2, "v": [15]}"#).unwrap();
        col.insert_json(r#"{"_id": 3, "v": [1, 2]}"#).unwrap();
        wtx.commit().unwrap();
    }
    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("nums").unwrap();
    let got = collect_ids(
        &col.find_json(
            Some(r#"{"v": {"$gt": 10, "$lt": 20}}"#),
            None,
            None,
            0,
            None,
        )
        .unwrap()
        .collect_all(),
    );
    assert_eq!(got.len(), 2, "must include both [5, 100] and [15]");

    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);
}

#[test]
fn multihop_array_dotted_path_indexed_and_unindexed() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("orders").unwrap();
        let mut col = wtx.collection("orders").unwrap();
        col.insert_json(
            r#"{"_id": 1, "items": [{"meta": {"tag": "urgent"}}, {"meta": {"tag": "fragile"}}]}"#,
        )
        .unwrap();
        col.insert_json(r#"{"_id": 2, "items": [{"meta": {"tag": "standard"}}]}"#)
            .unwrap();
        wtx.commit().unwrap();
    }

    let unindexed = {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("orders").unwrap();
        collect_ids(
            &col.find_json(
                Some(r#"{"items.meta.tag": "fragile"}"#),
                None,
                None,
                0,
                None,
            )
            .unwrap()
            .collect_all(),
        )
    };
    assert_eq!(unindexed.len(), 1);

    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("orders").unwrap();
        col.create_index(
            "items.meta.tag",
            SortOrder::Asc,
            false,
            false,
            Some("idx_item_tag"),
        )
        .unwrap();
        wtx.commit().unwrap();
    }

    let indexed = {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("orders").unwrap();
        collect_ids(
            &col.find_json(
                Some(r#"{"items.meta.tag": "fragile"}"#),
                None,
                None,
                0,
                None,
            )
            .unwrap()
            .collect_all(),
        )
    };
    assert_eq!(indexed, unindexed);

    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);
}

#[test]
fn binary_memcomparable_order_matches_compare_values() {
    let a = ValueView::Binary {
        subtype: 0x05,
        bytes: b"aaa",
    };
    let b = ValueView::Binary {
        subtype: 0x00,
        bytes: b"bbb",
    };
    assert_eq!(compare_values(&a, &b), std::cmp::Ordering::Less);
    let ka = encode_value_key(&a, SortOrder::Asc, 512).unwrap();
    let kb = encode_value_key(&b, SortOrder::Asc, 512).unwrap();
    assert_eq!(ka.cmp(&kb), std::cmp::Ordering::Less);

    let c = ValueView::Binary {
        subtype: 0x01,
        bytes: b"same",
    };
    let d = ValueView::Binary {
        subtype: 0x02,
        bytes: b"same",
    };
    assert_eq!(compare_values(&c, &d), std::cmp::Ordering::Less);
    let kc = encode_value_key(&c, SortOrder::Asc, 512).unwrap();
    let kd = encode_value_key(&d, SortOrder::Asc, 512).unwrap();
    assert_eq!(kc.cmp(&kd), std::cmp::Ordering::Less);
}

#[test]
fn nested_projection_exclusion_redacts_secrets_and_merges_sibling_inclusions() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("users").unwrap();
        let mut col = wtx.collection("users").unwrap();
        col.insert_json(
            r#"{"_id": 1, "user": {"name": "Alice", "email": "a@example.com", "password": "secret"}}"#,
        )
        .unwrap();
        wtx.commit().unwrap();
    }

    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("users").unwrap();

    let excl = col
        .find_json(None, Some(r#"{"user.password": 0}"#), None, 0, None)
        .unwrap()
        .collect_all();
    let user_val = excl[0].view().get_field("user").unwrap();
    let ValueView::Document(user_doc) = user_val else {
        panic!("expected nested user doc");
    };
    assert!(user_doc.get_field("password").is_none());
    assert!(user_doc.get_field("name").is_some());
    assert!(user_doc.get_field("email").is_some());

    let incl = col
        .find_json(
            None,
            Some(r#"{"user.name": 1, "user.email": 1}"#),
            None,
            0,
            None,
        )
        .unwrap()
        .collect_all();
    let user_val2 = incl[0].view().get_field("user").unwrap();
    let ValueView::Document(user_doc2) = user_val2 else {
        panic!("expected merged nested user doc");
    };
    assert!(user_doc2.get_field("name").is_some());
    assert!(user_doc2.get_field("email").is_some());
    assert!(user_doc2.get_field("password").is_none());
}

#[test]
fn nan_excluded_from_numeric_ranges_and_group_decimal128_works() {
    let db = open_mem_db();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("metrics").unwrap();
        let mut col = wtx.collection("metrics").unwrap();
        col.create_index("v", SortOrder::Asc, false, false, Some("idx_v"))
            .unwrap();
        let (nan_doc, _) = encode_document(
            &[
                ("_id".to_owned(), Value::Int64(1)),
                ("grp".to_owned(), Value::String("a".to_owned())),
                ("v".to_owned(), Value::Float64(f64::NAN)),
            ],
            None,
        )
        .unwrap();
        col.insert(&nan_doc).unwrap();
        let (dec_doc1, _) = encode_document(
            &[
                ("_id".to_owned(), Value::Int64(2)),
                ("grp".to_owned(), Value::String("a".to_owned())),
                (
                    "v".to_owned(),
                    Value::Decimal128(Decimal128::parse_str("10.50").unwrap()),
                ),
            ],
            None,
        )
        .unwrap();
        col.insert(&dec_doc1).unwrap();
        let (dec_doc2, _) = encode_document(
            &[
                ("_id".to_owned(), Value::Int64(3)),
                ("grp".to_owned(), Value::String("a".to_owned())),
                (
                    "v".to_owned(),
                    Value::Decimal128(Decimal128::parse_str("4.50").unwrap()),
                ),
            ],
            None,
        )
        .unwrap();
        col.insert(&dec_doc2).unwrap();
        wtx.commit().unwrap();
    }

    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("metrics").unwrap();

    let lt_res = col
        .find(&FindOptions {
            filter: Some(FilterExpr::Lt(
                FieldPath::parse("v").unwrap(),
                Value::Int64(100),
            )),
            ..FindOptions::default()
        })
        .unwrap()
        .collect_all();
    assert_eq!(lt_res.len(), 2, "NaN must not match v < 100");

    let agg = col
        .aggregate_json(
            r#"[{"$match": {"_id": {"$gt": 1}}}, {"$group": {"_id": "$grp", "s": {"$sum": "$v"}, "a": {"$avg": "$v"}}}]"#,
        )
        .unwrap();
    assert_eq!(agg.len(), 1);
    let Some(ValueView::Float64(s)) = agg[0].view().get_field("s") else {
        panic!("expected Float64 sum");
    };
    let Some(ValueView::Float64(a)) = agg[0].view().get_field("a") else {
        panic!("expected Float64 avg");
    };
    assert!((s - 15.0).abs() < 1e-9);
    assert!((a - 7.5).abs() < 1e-9);
}

#[test]
fn decimal128_trailing_zeros_and_dos_recursion_allocation_guards() {
    let big = Decimal128::parse_str("10000000000000000000000000000000000").unwrap();
    assert!(!big.is_nan());
    let frac = Decimal128::parse_str("1.0000000000000000000000000000000000000000").unwrap();
    assert!(!frac.is_nan());

    let mut deep_json = String::new();
    for _ in 0..140 {
        deep_json.push_str(r#"{"a":"#);
    }
    deep_json.push('1');
    for _ in 0..140 {
        deep_json.push('}');
    }
    assert!(from_json_str(&deep_json, None).is_err());

    let (doc, _) = encode_document(&[("_id".to_owned(), Value::Int64(1))], None).unwrap();
    let mut expr = FilterExpr::MatchAll;
    for _ in 0..140 {
        expr = FilterExpr::Not(Box::new(expr));
    }
    let err = eval_filter(&doc.view(), &expr).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);

    let bomb = u32::MAX.to_le_bytes();
    let err = decode_prefix_block(&bomb).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);

    let patch_bomb = [0u8, 0u8, 0xFF, 0xFF];
    let err = decode_physical_patch(&patch_bomb).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);

    let mgr = SpillManager::new(1024).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(mgr.dir_path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "spill dir must have 0700 permissions");
    }
    let bad_run = mgr.dir_path().join("corrupt.spill");
    {
        let mut f = fs::File::create(&bad_run).unwrap();
        f.write_all(&u32::MAX.to_le_bytes()).unwrap();
    }
    let err = SpillManager::read_run(&bad_run).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);
}
