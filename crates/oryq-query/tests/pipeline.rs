use std::collections::BTreeMap;

use oryq_document::{from_json_str, DocumentBuf, Value};
use oryq_index::IndexDescriptor;
use oryq_query::{
    compare_owned_values, execute_pipeline_json, parse_sort_json, resolve_path, spillable_sort,
    QueryContext, QuerySource, SpillManager,
};
use oryq_types::{BudgetDomain, BudgetHandle, CancellationSource, ErrorCategory, RecordId, Result};

#[derive(Debug, Default)]
struct MultiColSource {
    collections: BTreeMap<String, Vec<(RecordId, DocumentBuf)>>,
}

impl MultiColSource {
    fn add(&mut self, col: &str, json: &str) {
        let entry = self.collections.entry(col.to_owned()).or_default();
        let rid = RecordId::new((entry.len() + 1) as u64).unwrap();
        let (doc, _) = from_json_str(json, None).unwrap();
        entry.push((rid, doc));
    }
}

impl QuerySource for MultiColSource {
    fn collection_indexes(&self, _collection: &str) -> Result<Vec<IndexDescriptor>> {
        Ok(Vec::new())
    }

    fn scan_collection(&self, collection: &str) -> Result<Vec<(RecordId, DocumentBuf)>> {
        Ok(self
            .collections
            .get(collection)
            .cloned()
            .unwrap_or_default())
    }

    fn get_by_record_id(
        &self,
        collection: &str,
        record_id: RecordId,
    ) -> Result<Option<DocumentBuf>> {
        Ok(self.collections.get(collection).and_then(|v| {
            v.iter()
                .find(|(r, _)| *r == record_id)
                .map(|(_, d)| d.clone())
        }))
    }

    fn probe_primary_id(&self, collection: &str, id_value: &Value) -> Result<Option<RecordId>> {
        if let Some(list) = self.collections.get(collection) {
            for (rid, doc) in list {
                if let Some(v) = doc.view().get_field("_id") {
                    if compare_owned_values(&v.to_owned_value(), id_value)
                        == std::cmp::Ordering::Equal
                    {
                        return Ok(Some(*rid));
                    }
                }
            }
        }
        Ok(None)
    }

    fn scan_secondary_eq(&self, _desc: &IndexDescriptor, _target: &Value) -> Result<Vec<RecordId>> {
        Ok(Vec::new())
    }

    fn scan_secondary_range(
        &self,
        _desc: &IndexDescriptor,
        _lower: Option<(&Value, bool)>,
        _upper: Option<(&Value, bool)>,
    ) -> Result<Vec<RecordId>> {
        Ok(Vec::new())
    }
}

#[test]
fn pipeline_unwind_group_sort_lookup_and_project() {
    let mut src = MultiColSource::default();
    src.add(
        "orders",
        r#"{"_id": 1, "cust_id": 10, "amount": 100, "tags": ["a", "b"]}"#,
    );
    src.add(
        "orders",
        r#"{"_id": 2, "cust_id": 10, "amount": 50, "tags": ["a"]}"#,
    );
    src.add(
        "orders",
        r#"{"_id": 3, "cust_id": 20, "amount": 200, "tags": ["c"]}"#,
    );
    src.add("customers", r#"{"_id": 10, "name": "Alice"}"#);
    src.add("customers", r#"{"_id": 20, "name": "Bob"}"#);

    let ctx = QueryContext::default_with_limit(4 * 1024 * 1024);

    // $match -> $group -> $sort -> $limit
    let pipeline = r#"[
        {"$match": {"amount": {"$gte": 50}}},
        {"$group": {
            "_id": "$cust_id",
            "total": {"$sum": "$amount"},
            "avg_amt": {"$avg": "$amount"},
            "cnt": {"$count": {}}
        }},
        {"$sort": {"total": -1}}
    ]"#;
    let grouped = execute_pipeline_json(&src, "orders", pipeline, &ctx).unwrap();
    assert_eq!(grouped.len(), 2);
    assert_eq!(
        grouped[0].view().get_field("_id").unwrap().to_owned_value(),
        Value::Int64(20)
    );
    assert_eq!(
        grouped[0]
            .view()
            .get_field("total")
            .unwrap()
            .to_owned_value(),
        Value::Int64(200)
    );
    assert_eq!(
        grouped[1]
            .view()
            .get_field("total")
            .unwrap()
            .to_owned_value(),
        Value::Int64(150)
    );
    assert_eq!(
        grouped[1].view().get_field("cnt").unwrap().to_owned_value(),
        Value::Int64(2)
    );

    // $unwind + $group with $addToSet
    let unwind_pipe = r#"[
        {"$unwind": "$tags"},
        {"$group": {"_id": "$tags", "order_ids": {"$addToSet": "$_id"}}},
        {"$sort": {"_id": 1}}
    ]"#;
    let unwound = execute_pipeline_json(&src, "orders", unwind_pipe, &ctx).unwrap();
    assert_eq!(unwound.len(), 3); // "a", "b", "c"

    // $lookup snapshot-consistent join
    let lookup_pipe = r#"[
        {"$lookup": {
            "from": "customers",
            "localField": "cust_id",
            "foreignField": "_id",
            "as": "customer"
        }},
        {"$sort": {"_id": 1}},
        {"$limit": 1}
    ]"#;
    let joined = execute_pipeline_json(&src, "orders", lookup_pipe, &ctx).unwrap();
    assert_eq!(joined.len(), 1);
    let cust = resolve_path(
        &joined[0].view(),
        &oryq_document::FieldPath::parse("customer.name").unwrap(),
    );
    assert_eq!(
        cust,
        Some(Value::Array(vec![Value::String("Alice".into())]))
    );
}

#[test]
fn spillable_sort_spills_under_tight_budget_and_cleans_up_files() {
    let mut docs = Vec::new();
    for i in (1..=40i64).rev() {
        let j = format!(
            r#"{{"_id": {i}, "val": {i}, "pad": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}}"#
        );
        docs.push(from_json_str(&j, None).unwrap().0);
    }

    // Tight 512-byte memory budget forces external spill runs!
    let ctx = QueryContext::default_with_limit(512);
    let sort = parse_sort_json(r#"{"val": 1}"#).unwrap();
    let sorted = spillable_sort(docs, &sort, &ctx).unwrap();
    assert_eq!(sorted.len(), 40);
    for (idx, doc) in sorted.iter().enumerate() {
        assert_eq!(
            doc.view().get_field("val").unwrap().to_owned_value(),
            Value::Int64((idx + 1) as i64)
        );
    }

    // Verify SpillManager RAII cleanup directly
    let spill_dir;
    {
        let mut sm = SpillManager::new(1024 * 1024).unwrap();
        spill_dir = sm.dir_path().clone();
        sm.write_run(&sorted[..2]).unwrap();
        assert!(spill_dir.exists());
    }
    assert!(
        !spill_dir.exists(),
        "SpillManager must remove temp dir on drop"
    );
}

#[test]
fn pipeline_respects_cancellation_token() {
    let mut src = MultiColSource::default();
    src.add("items", r#"{"_id": 1, "x": 10}"#);

    let cancel_src = CancellationSource::new();
    cancel_src.cancel();
    let ctx = QueryContext::new(
        BudgetHandle::new(BudgetDomain::Query, 1024 * 1024),
        cancel_src.token(),
    );
    let err = execute_pipeline_json(&src, "items", r#"[{"$match": {}}]"#, &ctx).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Cancelled);
}
