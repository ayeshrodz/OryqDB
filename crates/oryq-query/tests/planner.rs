use std::collections::BTreeMap;

use oryq_document::{from_json_str, DocumentBuf, FieldPath, Value};
use oryq_index::{IndexDescriptor, SortOrder};
use oryq_query::{
    compare_owned_values, execute_find, explain_analyze, parse_filter_json, plan_find,
    resolve_path, AccessPlan, FindOptions, QueryContext, QuerySource,
};
use oryq_types::{IndexId, PageId, RecordId, Result};

#[derive(Debug)]
struct FakeSource {
    docs: BTreeMap<RecordId, DocumentBuf>,
    indexes: Vec<IndexDescriptor>,
}

impl QuerySource for FakeSource {
    fn collection_indexes(&self, _collection: &str) -> Result<Vec<IndexDescriptor>> {
        Ok(self.indexes.clone())
    }

    fn scan_collection(&self, _collection: &str) -> Result<Vec<(RecordId, DocumentBuf)>> {
        Ok(self.docs.iter().map(|(k, v)| (*k, v.clone())).collect())
    }

    fn get_by_record_id(
        &self,
        _collection: &str,
        record_id: RecordId,
    ) -> Result<Option<DocumentBuf>> {
        Ok(self.docs.get(&record_id).cloned())
    }

    fn probe_primary_id(&self, _collection: &str, id_value: &Value) -> Result<Option<RecordId>> {
        for (rid, doc) in &self.docs {
            if let Some(v) = doc.view().get_field("_id") {
                if compare_owned_values(&v.to_owned_value(), id_value) == std::cmp::Ordering::Equal
                {
                    return Ok(Some(*rid));
                }
            }
        }
        Ok(None)
    }

    fn scan_secondary_eq(&self, desc: &IndexDescriptor, target: &Value) -> Result<Vec<RecordId>> {
        let path = &desc.fields[0].path;
        let mut out = Vec::new();
        for (rid, doc) in &self.docs {
            if let Some(val) = resolve_path(&doc.view(), path) {
                if compare_owned_values(&val, target) == std::cmp::Ordering::Equal {
                    out.push(*rid);
                }
            }
        }
        Ok(out)
    }

    fn scan_secondary_range(
        &self,
        desc: &IndexDescriptor,
        lower: Option<(&Value, bool)>,
        upper: Option<(&Value, bool)>,
    ) -> Result<Vec<RecordId>> {
        let path = &desc.fields[0].path;
        let mut out = Vec::new();
        for (rid, doc) in &self.docs {
            if let Some(val) = resolve_path(&doc.view(), path) {
                let ok_lo = match lower {
                    None => true,
                    Some((b, true)) => compare_owned_values(&val, b) != std::cmp::Ordering::Less,
                    Some((b, false)) => {
                        compare_owned_values(&val, b) == std::cmp::Ordering::Greater
                    }
                };
                let ok_hi = match upper {
                    None => true,
                    Some((b, true)) => compare_owned_values(&val, b) != std::cmp::Ordering::Greater,
                    Some((b, false)) => compare_owned_values(&val, b) == std::cmp::Ordering::Less,
                };
                if ok_lo && ok_hi {
                    out.push(*rid);
                }
            }
        }
        Ok(out)
    }
}

#[test]
fn planner_selects_expected_paths_and_matches_full_scan_oracle() {
    let mut docs = BTreeMap::new();
    for (i, j) in [
        r#"{"_id": 1, "age": 20, "city": "Auckland"}"#,
        r#"{"_id": 2, "age": 35, "city": "Wellington"}"#,
        r#"{"_id": 3, "age": 35, "city": "Auckland"}"#,
        r#"{"_id": 4, "city": "Christchurch"}"#,
    ]
    .into_iter()
    .enumerate()
    {
        docs.insert(
            RecordId::new((i + 1) as u64).unwrap(),
            from_json_str(j, None).unwrap().0,
        );
    }

    let indexes = vec![
        IndexDescriptor::primary(IndexId::new(1).unwrap(), PageId::new(10)),
        IndexDescriptor::secondary(
            IndexId::new(2).unwrap(),
            "age_1".to_owned(),
            FieldPath::parse("age").unwrap(),
            SortOrder::Asc,
            false,
            true, // sparse!
            PageId::new(11),
        ),
    ];

    // 1. Primary lookup
    let f_id = parse_filter_json(r#"{"_id": 2}"#).unwrap();
    let p_id = plan_find("users", Some(&f_id), &indexes);
    assert!(matches!(p_id.access, AccessPlan::PrimaryIdLookup { .. }));

    // 2. Secondary equality
    let f_eq = parse_filter_json(r#"{"age": 35, "city": "Auckland"}"#).unwrap();
    let p_eq = plan_find("users", Some(&f_eq), &indexes);
    assert!(matches!(p_eq.access, AccessPlan::SecondaryEqScan { .. }));

    // 3. Secondary range
    let f_rng = parse_filter_json(r#"{"age": {"$gte": 20, "$lt": 35}}"#).unwrap();
    let p_rng = plan_find("users", Some(&f_rng), &indexes);
    assert!(matches!(
        p_rng.access,
        AccessPlan::SecondaryRangeScan { .. }
    ));

    // 4. Sparse index must NOT be chosen for null equality
    let f_null = parse_filter_json(r#"{"age": null}"#).unwrap();
    let p_null = plan_find("users", Some(&f_null), &indexes);
    assert!(matches!(p_null.access, AccessPlan::CollectionScan));

    // Oracle check: indexed execution == scan-only execution
    let indexed_src = FakeSource {
        docs: docs.clone(),
        indexes,
    };
    let scan_src = FakeSource {
        docs,
        indexes: vec![IndexDescriptor::primary(
            IndexId::new(1).unwrap(),
            PageId::new(10),
        )],
    };

    for filter in [f_id, f_eq, f_rng, f_null] {
        let opts = FindOptions {
            filter: Some(filter),
            ..Default::default()
        };
        let r_idx = execute_find(
            &indexed_src,
            "users",
            &opts,
            &QueryContext::default_with_limit(1024 * 1024),
        )
        .unwrap()
        .collect_all();
        let r_scan = execute_find(
            &scan_src,
            "users",
            &opts,
            &QueryContext::default_with_limit(1024 * 1024),
        )
        .unwrap()
        .collect_all();
        assert_eq!(r_idx.len(), r_scan.len());
    }

    // Explain analyze counters
    let exp = explain_analyze(
        &indexed_src,
        "users",
        &FindOptions {
            filter: Some(parse_filter_json(r#"{"age": 35}"#).unwrap()),
            ..Default::default()
        },
        &QueryContext::default_with_limit(1024 * 1024),
    )
    .unwrap();
    assert_eq!(exp.docs_returned, 2);
    assert_eq!(exp.keys_examined, 2);
}
