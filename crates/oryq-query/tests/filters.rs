use oryq_document::{from_json_str, DocumentBuf};
use oryq_query::{
    apply_projection, eval_filter, parse_filter_json, parse_projection_json, parse_sort_json,
    sort_documents,
};

fn doc(json: &str) -> DocumentBuf {
    from_json_str(json, None).expect("valid json").0
}

#[test]
fn filter_semantics_numeric_missing_null_and_arrays() {
    let d1 = doc(
        r#"{"_id": 1, "name": "Alice", "age": 30, "score": 9.5, "tags": ["rust", "db"], "items": [{"sku": "A", "qty": 2}, {"sku": "B", "qty": 5}], "opt": null}"#,
    );
    let d2 = doc(r#"{"_id": 2, "name": "Bob", "age": 17, "tags": ["go"]}"#);

    // Cross-type numeric equality & range
    let f = parse_filter_json(r#"{"age": {"$eq": 30.0}}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f).unwrap());
    assert!(!eval_filter(&d2.view(), &f).unwrap());

    let f_range = parse_filter_json(r#"{"age": {"$gte": 18, "$lt": 40}}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_range).unwrap());
    assert!(!eval_filter(&d2.view(), &f_range).unwrap());

    // Missing vs Null
    let f_exists = parse_filter_json(r#"{"opt": {"$exists": true}}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_exists).unwrap());
    assert!(!eval_filter(&d2.view(), &f_exists).unwrap());

    let f_missing = parse_filter_json(r#"{"opt": {"$exists": false}}"#).unwrap();
    assert!(!eval_filter(&d1.view(), &f_missing).unwrap());
    assert!(eval_filter(&d2.view(), &f_missing).unwrap());

    let f_null = parse_filter_json(r#"{"opt": null}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_null).unwrap());
    assert!(!eval_filter(&d2.view(), &f_null).unwrap());

    // Array scalar match & $elemMatch
    let f_arr = parse_filter_json(r#"{"tags": "rust"}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_arr).unwrap());
    assert!(!eval_filter(&d2.view(), &f_arr).unwrap());

    let f_elem =
        parse_filter_json(r#"{"items": {"$elemMatch": {"sku": "B", "qty": {"$gte": 5}}}}"#)
            .unwrap();
    assert!(eval_filter(&d1.view(), &f_elem).unwrap());
    assert!(!eval_filter(&d2.view(), &f_elem).unwrap());

    // $startsWith, $in, $nin, $or, $nor, $not
    let f_prefix = parse_filter_json(r#"{"name": {"$startsWith": "Ali"}}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_prefix).unwrap());
    assert!(!eval_filter(&d2.view(), &f_prefix).unwrap());

    let f_in = parse_filter_json(r#"{"name": {"$in": ["Bob", "Charlie"]}}"#).unwrap();
    assert!(!eval_filter(&d1.view(), &f_in).unwrap());
    assert!(eval_filter(&d2.view(), &f_in).unwrap());

    let f_nor =
        parse_filter_json(r#"{"$nor": [{"age": {"$lt": 20}}, {"name": "Charlie"}]}"#).unwrap();
    assert!(eval_filter(&d1.view(), &f_nor).unwrap());
    assert!(!eval_filter(&d2.view(), &f_nor).unwrap());
}

#[test]
fn projection_and_sort_ordering() {
    let mut docs = vec![
        doc(r#"{"_id": 1, "name": "Charlie", "age": 25}"#),
        doc(r#"{"_id": 2, "name": "Alice", "age": 30}"#),
        doc(r#"{"_id": 3, "name": "Bob", "age": 30}"#),
    ];

    let sort = parse_sort_json(r#"{"age": -1, "name": 1}"#).unwrap();
    sort_documents(&mut docs, &sort);
    let names: Vec<String> = docs
        .iter()
        .map(
            |d| match d.view().get_field("name").unwrap().to_owned_value() {
                oryq_document::Value::String(s) => s,
                _ => unreachable!(),
            },
        )
        .collect();
    assert_eq!(names, vec!["Alice", "Bob", "Charlie"]);

    let inc = parse_projection_json(r#"{"name": 1, "_id": 0}"#).unwrap();
    let p = apply_projection(&docs[0], &inc).unwrap();
    assert!(p.view().get_field("_id").is_none());
    assert!(p.view().get_field("name").is_some());
    assert!(p.view().get_field("age").is_none());

    let exc = parse_projection_json(r#"{"age": 0}"#).unwrap();
    let p2 = apply_projection(&docs[0], &exc).unwrap();
    assert!(p2.view().get_field("_id").is_some());
    assert!(p2.view().get_field("name").is_some());
    assert!(p2.view().get_field("age").is_none());
}
