use oryq_document::{
    apply_update, compact_document, decode_physical_patch, from_json_str,
    resolve_path_with_patches, to_json_value, validate_replacement_doc, Decimal128, FieldPath,
    UpdateRegistry, UpdateSpec, UpdateStep, Value,
};
use oryq_types::{BudgetDomain, BudgetHandle, ErrorCategory};

#[test]
fn set_unset_inc_push_pull_and_normalized_patches() {
    let reg = UpdateRegistry::default_v1();
    let (base, _) = from_json_str(
        r#"{
            "_id": 1,
            "count": 10,
            "score": 1.5,
            "temp": "delete-me",
            "tags": ["a", "b"],
            "nums": [10, 20, 10.0, 30]
        }"#,
        None,
    )
    .unwrap();

    let spec = UpdateSpec::from_json_str(
        r#"{
            "$set": { "profile.country": "NZ", "active": true },
            "$unset": { "temp": "" },
            "$inc": { "count": 5, "score": -1.5, "new_counter": 3 },
            "$push": { "tags": "rust", "new_arr": 42 },
            "$pull": { "nums": 10.0 }
        }"#,
        &reg,
    )
    .unwrap();

    let (mutated, _) = apply_update(&base.view(), &spec, &reg, None).unwrap();
    assert!(mutated.modified);
    assert_eq!(
        mutated.touched_top_level_fields,
        vec![
            "active",
            "count",
            "new_arr",
            "new_counter",
            "nums",
            "profile",
            "score",
            "tags",
            "temp",
        ]
    );

    let view = mutated.updated_doc.view();
    assert_eq!(
        view.get_str(&FieldPath::parse("profile.country").unwrap()),
        Some("NZ")
    );
    assert_eq!(
        view.get_bool(&FieldPath::parse("active").unwrap()),
        Some(true)
    );
    assert!(view.get_field("temp").is_none());
    assert_eq!(view.get_i64(&FieldPath::parse("count").unwrap()), Some(15));
    // 1.5 + (-1.5) == +0.0 (not -0.0)
    let score = view.get_f64(&FieldPath::parse("score").unwrap()).unwrap();
    assert_eq!(score, 0.0);
    assert!(!score.is_sign_negative());
    assert_eq!(
        view.get_i64(&FieldPath::parse("new_counter").unwrap()),
        Some(3)
    );

    // $pull: 10.0 removes both Int64(10) and Float64(10.0) via canonical equality
    let nums = view.get_field("nums").unwrap().to_owned_value();
    assert_eq!(nums, Value::Array(vec![Value::Int64(20), Value::Int64(30)]));

    let tags = view.get_field("tags").unwrap().to_owned_value();
    assert_eq!(
        tags,
        Value::Array(vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("rust".into()),
        ])
    );

    // Physical patch decodes cleanly and matches bloom mask
    let decoded = decode_physical_patch(&mutated.patch_bytes).unwrap();
    assert_eq!(decoded.entries.len(), 9);
    assert_eq!(decoded.bloom_mask, mutated.bloom_mask);
}

#[test]
fn id_immutability_and_conflicting_paths_rejected() {
    let reg = UpdateRegistry::default_v1();

    // Direct or nested _id mutation rejected
    assert_eq!(
        UpdateSpec::from_json_str(r#"{"$set": {"_id": 2}}"#, &reg)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        UpdateSpec::from_json_str(r#"{"$unset": {"_id.sub": ""}}"#, &reg)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );

    // Replacement document _id immutability
    let (doc1, _) = from_json_str(r#"{"_id": 100, "x": 1}"#, None).unwrap();
    let (doc_same, _) = from_json_str(r#"{"_id": 100, "x": 2}"#, None).unwrap();
    let (doc_diff, _) = from_json_str(r#"{"_id": 101, "x": 2}"#, None).unwrap();
    let (doc_diff_type, _) = from_json_str(r#"{"_id": "100", "x": 2}"#, None).unwrap();
    let (doc_missing, _) = from_json_str(r#"{"x": 2}"#, None).unwrap();

    validate_replacement_doc(&doc1.view(), &doc_same.view()).unwrap();
    assert_eq!(
        validate_replacement_doc(&doc1.view(), &doc_diff.view())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        validate_replacement_doc(&doc1.view(), &doc_diff_type.view())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        validate_replacement_doc(&doc1.view(), &doc_missing.view())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );

    // Ancestor/descendant and cross-operator duplicate path conflicts
    assert_eq!(
        UpdateSpec::from_json_str(r#"{"$set": {"a": 1, "a.b": 2}}"#, &reg)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        UpdateSpec::from_json_str(r#"{"$set": {"x": 1}, "$inc": {"x": 2}}"#, &reg)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );

    // Unknown operator
    assert_eq!(
        UpdateSpec::from_json_str(r#"{"$mul": {"x": 2}}"#, &reg)
            .unwrap_err()
            .category(),
        ErrorCategory::UnsupportedFeature
    );
}

#[test]
fn inc_overflow_precision_loss_decimal_and_type_errors() {
    let reg = UpdateRegistry::default_v1();
    let big_int = (1i64 << 53) + 1;
    let (base, _) = from_json_str(
        &format!(
            r#"{{
                "_id": 1,
                "max_i": {},
                "big_i": {},
                "dec": {{"$numberDecimal": "1.25"}},
                "str": "hello",
                "arr": [1, 2]
            }}"#,
            i64::MAX,
            big_int
        ),
        None,
    )
    .unwrap();

    // i64 overflow
    let spec_ovf = UpdateSpec::from_json_str(r#"{"$inc": {"max_i": 1}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_ovf, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::Overflow
    );

    // Lossy i64 -> f64 promotion rejected
    let spec_prec = UpdateSpec::from_json_str(r#"{"$inc": {"big_i": 0.5}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_prec, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::Overflow
    );

    // Decimal128 target or operand rejected with UnsupportedFeature
    let spec_dec_target = UpdateSpec::from_json_str(r#"{"$inc": {"dec": 1}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_dec_target, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::UnsupportedFeature
    );
    let spec_dec_op = UpdateSpec::from_steps(
        vec![UpdateStep {
            operator: "$inc".into(),
            path: FieldPath::parse("big_i").unwrap(),
            operand: Value::Decimal128(Decimal128::parse_str("1.0").unwrap()),
        }],
        &reg,
    )
    .unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_dec_op, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::UnsupportedFeature
    );

    // Type mismatch: $inc on string, $push on string, array-position $set
    let spec_tm1 = UpdateSpec::from_json_str(r#"{"$inc": {"str": 1}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_tm1, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::TypeMismatch
    );
    let spec_tm2 = UpdateSpec::from_json_str(r#"{"$push": {"str": "x"}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_tm2, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::TypeMismatch
    );
    let spec_arr_pos = UpdateSpec::from_json_str(r#"{"$set": {"arr.0": 99}}"#, &reg).unwrap();
    assert_eq!(
        apply_update(&base.view(), &spec_arr_pos, &reg, None)
            .unwrap_err()
            .category(),
        ErrorCategory::UnsupportedFeature
    );
}

#[test]
fn multi_patch_ancestor_descendant_resolution_and_pure_compaction() {
    let reg = UpdateRegistry::default_v1();
    let (base, _) = from_json_str(
        r#"{"_id": 7, "a": {"b": 0, "keep": "orig"}, "unchanged": 123}"#,
        None,
    )
    .unwrap();

    // Apply 4 sequential updates, accumulating top-level physical patches
    let updates = [
        r#"{"$set": {"a.b": 1}}"#,
        r#"{"$set": {"a.c": 2, "x": "v1"}}"#,
        r#"{"$unset": {"a": ""}}"#,
        r#"{"$set": {"a": {"b": 99}, "tags": ["rust"]}}"#,
    ];

    let mut current_doc = base.clone();
    let mut patches_oldest_first: Vec<Vec<u8>> = Vec::new();
    let mut combined_bloom = 0u64;

    for raw in updates {
        let spec = UpdateSpec::from_json_str(raw, &reg).unwrap();
        let (res, _) = apply_update(&current_doc.view(), &spec, &reg, None).unwrap();
        combined_bloom |= res.bloom_mask;
        patches_oldest_first.push(res.patch_bytes);
        current_doc = res.updated_doc;
    }

    // Build newest-first slice array for patch resolution & compaction
    let patches_newest_first: Vec<&[u8]> = patches_oldest_first
        .iter()
        .rev()
        .map(Vec::as_slice)
        .collect();

    // After patch 3 unset "a" and patch 4 re-created "a": {"b": 99}:
    // - "a.b" resolves to 99
    // - "a.keep" and "a.c" resolve to None (shadowed by patch 4's top-level replacement!)
    // - "unchanged" misses bloom and resolves from base to 123
    assert_eq!(
        resolve_path_with_patches(
            &base.view(),
            &patches_newest_first,
            combined_bloom,
            &FieldPath::parse("a.b").unwrap()
        )
        .unwrap(),
        Some(Value::Int64(99))
    );
    assert_eq!(
        resolve_path_with_patches(
            &base.view(),
            &patches_newest_first,
            combined_bloom,
            &FieldPath::parse("a.keep").unwrap()
        )
        .unwrap(),
        None
    );
    assert_eq!(
        resolve_path_with_patches(
            &base.view(),
            &patches_newest_first,
            combined_bloom,
            &FieldPath::parse("a.c").unwrap()
        )
        .unwrap(),
        None
    );
    assert_eq!(
        resolve_path_with_patches(
            &base.view(),
            &patches_newest_first,
            combined_bloom,
            &FieldPath::parse("unchanged").unwrap()
        )
        .unwrap(),
        Some(Value::Int64(123))
    );

    // Pure compaction with BudgetHandle produces an identical document to `current_doc`
    let budget = BudgetHandle::new(BudgetDomain::Query, 8192);
    let (compacted_bytes, res) =
        compact_document(base.as_bytes(), &patches_newest_first, Some(&budget)).unwrap();
    assert_eq!(budget.used_bytes(), compacted_bytes.len());
    drop(res);
    assert_eq!(budget.used_bytes(), 0);

    let compacted_view = oryq_document::DocumentView::from_bytes(&compacted_bytes).unwrap();
    assert_eq!(
        to_json_value(&compacted_view),
        to_json_value(&current_doc.view())
    );
}
