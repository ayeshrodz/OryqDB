//! Exact semantic filter evaluator backed by `oryq_document::compare_values`
//! (`docs/query-language.md` §3).

use std::cmp::Ordering;

use oryq_document::{compare_values, encode_document, DocumentView, FieldPath, Value, ValueView};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::ast::FilterExpr;

const MODULE_ID: &str = "query";
/// Maximum recursion depth allowed for nested `FilterExpr` evaluation.
pub const MAX_FILTER_DEPTH: usize = 128;

/// Compares two owned [`Value`]s using `oryq_document::compare_values` exact semantics.
pub fn compare_owned_values(a: &Value, b: &Value) -> Ordering {
    // Fast path for scalar variants without encoding.
    match (as_scalar_view(a), as_scalar_view(b)) {
        (Some(va), Some(vb)) => compare_values(&va, &vb),
        _ => {
            let (doc, _) = encode_document(
                &[("a".to_owned(), a.clone()), ("b".to_owned(), b.clone())],
                None,
            )
            .expect("valid values encode cleanly");
            let view = doc.view();
            let va = view.get_field("a").unwrap();
            let vb = view.get_field("b").unwrap();
            compare_values(&va, &vb)
        }
    }
}

fn as_scalar_view(v: &Value) -> Option<ValueView<'_>> {
    match v {
        Value::Null => Some(ValueView::Null),
        Value::Bool(b) => Some(ValueView::Bool(*b)),
        Value::Int64(n) => Some(ValueView::Int64(*n)),
        Value::Float64(f) => Some(ValueView::Float64(*f)),
        Value::DateTimeUtcMs(ms) => Some(ValueView::DateTimeUtcMs(*ms)),
        Value::String(s) => Some(ValueView::String(s.as_str())),
        Value::Binary { subtype, bytes } => Some(ValueView::Binary {
            subtype: *subtype,
            bytes: bytes.as_slice(),
        }),
        Value::ObjectId(id) => Some(ValueView::ObjectId(*id)),
        Value::Uuid(u) => Some(ValueView::Uuid(*u)),
        Value::Decimal128(d) => Some(ValueView::Decimal128(*d)),
        Value::Array(_) | Value::Document(_) => None,
    }
}

/// Evaluates `expr` against `doc` using exact OryqDB semantics:
/// - `Missing` is distinct from `Null` (`$exists: false` matches missing; `$eq: null` matches explicit null).
/// - Scalar predicates on array fields match if ANY element in the array matches.
/// - `$elemMatch` evaluates a nested `FilterExpr` against each object element of an array.
pub fn eval_filter(doc: &DocumentView<'_>, expr: &FilterExpr) -> Result<bool> {
    eval_filter_depth(doc, expr, 0)
}

fn eval_filter_depth(doc: &DocumentView<'_>, expr: &FilterExpr, depth: usize) -> Result<bool> {
    if depth > MAX_FILTER_DEPTH {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "predicates::eval_filter",
            format!("filter nesting depth exceeds maximum allowed ({MAX_FILTER_DEPTH})"),
        ));
    }
    match expr {
        FilterExpr::MatchAll => Ok(true),
        FilterExpr::And(clauses) => {
            for c in clauses {
                if !eval_filter_depth(doc, c, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        FilterExpr::Or(clauses) => {
            for c in clauses {
                if eval_filter_depth(doc, c, depth + 1)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        FilterExpr::Nor(clauses) => {
            for c in clauses {
                if eval_filter_depth(doc, c, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        FilterExpr::Not(inner) => Ok(!eval_filter_depth(doc, inner, depth + 1)?),
        FilterExpr::Exists(path, want_exists) => {
            let exists = resolve_path(doc, path).is_some();
            Ok(exists == *want_exists)
        }
        FilterExpr::Eq(path, target) => match resolve_path(doc, path) {
            None => Ok(false),
            Some(v) => Ok(matches_eq(&v, target)),
        },
        FilterExpr::Ne(path, target) => match resolve_path(doc, path) {
            None => Ok(true),
            Some(v) => Ok(!matches_eq(&v, target)),
        },
        FilterExpr::Lt(path, bound) => Ok(eval_range(doc, path, |v| {
            compare_same_family_or_ordered(v, bound, |o| o == Ordering::Less)
        })),
        FilterExpr::Lte(path, bound) => Ok(eval_range(doc, path, |v| {
            compare_same_family_or_ordered(v, bound, |o| {
                o == Ordering::Less || o == Ordering::Equal
            })
        })),
        FilterExpr::Gt(path, bound) => Ok(eval_range(doc, path, |v| {
            compare_same_family_or_ordered(v, bound, |o| o == Ordering::Greater)
        })),
        FilterExpr::Gte(path, bound) => Ok(eval_range(doc, path, |v| {
            compare_same_family_or_ordered(v, bound, |o| {
                o == Ordering::Greater || o == Ordering::Equal
            })
        })),
        FilterExpr::In(path, list) => match resolve_path(doc, path) {
            None => Ok(false),
            Some(v) => Ok(list.iter().any(|target| matches_eq(&v, target))),
        },
        FilterExpr::Nin(path, list) => match resolve_path(doc, path) {
            None => Ok(true),
            Some(v) => Ok(!list.iter().any(|target| matches_eq(&v, target))),
        },
        FilterExpr::StartsWith(path, prefix) => match resolve_path(doc, path) {
            None => Ok(false),
            Some(Value::String(s)) => Ok(s.starts_with(prefix.as_str())),
            Some(Value::Array(items)) => Ok(items.iter().any(|item| match item {
                Value::String(s) => s.starts_with(prefix.as_str()),
                _ => false,
            })),
            Some(_) => Ok(false),
        },
        FilterExpr::ElemMatch(path, sub) => match resolve_path(doc, path) {
            Some(Value::Array(items)) => {
                for item in items {
                    if let Value::Document(sub_fields) = item {
                        let (sub_doc, _) = encode_document(&sub_fields, None)?;
                        if eval_filter_depth(&sub_doc.view(), sub, depth + 1)? {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        },
    }
}

/// Resolves a dotted [`FieldPath`] against `doc`, traversing recursively through nested
/// documents and intermediate arrays (e.g. `items.meta.tag`).
pub fn resolve_path(doc: &DocumentView<'_>, path: &FieldPath) -> Option<Value> {
    let segs: Vec<&str> = path.segments().collect();
    if segs.is_empty() {
        return None;
    }
    let mut current: ValueView<'_> = doc.get_field(segs[0])?;
    for (idx_pos, &seg) in segs[1..].iter().enumerate() {
        match current {
            ValueView::Document(sub) => {
                current = sub.get_field(seg)?;
            }
            ValueView::Array(arr) => {
                if let Ok(idx) = seg.parse::<usize>() {
                    current = arr.get(idx)?;
                } else {
                    let rest = &segs[1 + idx_pos..];
                    let mut collected = Vec::new();
                    collect_path_from_array(&arr, rest, &mut collected);
                    if collected.is_empty() {
                        return None;
                    }
                    return Some(Value::Array(collected));
                }
            }
            _ => return None,
        }
    }
    Some(current.to_owned_value())
}

fn collect_path_from_array(
    arr: &oryq_document::ArrayView<'_>,
    rest: &[&str],
    out: &mut Vec<Value>,
) {
    if rest.is_empty() {
        for item in arr.iter() {
            out.push(item.to_owned_value());
        }
        return;
    }
    let seg = rest[0];
    let tail = &rest[1..];
    if let Ok(idx) = seg.parse::<usize>() {
        if let Some(el) = arr.get(idx) {
            collect_path_from_value(el, tail, out);
        }
        return;
    }
    for item in arr.iter() {
        if let ValueView::Document(sub) = item {
            if let Some(val) = sub.get_field(seg) {
                collect_path_from_value(val, tail, out);
            }
        }
    }
}

fn collect_path_from_value(val: ValueView<'_>, rest: &[&str], out: &mut Vec<Value>) {
    if rest.is_empty() {
        match val {
            ValueView::Array(arr) => {
                for item in arr.iter() {
                    out.push(item.to_owned_value());
                }
            }
            other => out.push(other.to_owned_value()),
        }
        return;
    }
    match val {
        ValueView::Document(sub) => {
            if let Some(next) = sub.get_field(rest[0]) {
                collect_path_from_value(next, &rest[1..], out);
            }
        }
        ValueView::Array(arr) => collect_path_from_array(&arr, rest, out),
        _ => {}
    }
}

fn matches_eq(actual: &Value, target: &Value) -> bool {
    if compare_owned_values(actual, target) == Ordering::Equal {
        return true;
    }
    if let Value::Array(items) = actual {
        return items
            .iter()
            .any(|el| compare_owned_values(el, target) == Ordering::Equal);
    }
    false
}

fn eval_range<F>(doc: &DocumentView<'_>, path: &FieldPath, pred: F) -> bool
where
    F: Fn(&Value) -> bool,
{
    match resolve_path(doc, path) {
        None => false,
        Some(Value::Array(items)) => items.iter().any(pred),
        Some(v) => pred(&v),
    }
}

fn is_nan_value(v: &Value) -> bool {
    match v {
        Value::Float64(f) => f.is_nan(),
        Value::Decimal128(d) => d.is_nan(),
        _ => false,
    }
}

fn compare_same_family_or_ordered<F>(actual: &Value, bound: &Value, check: F) -> bool
where
    F: FnOnce(Ordering) -> bool,
{
    if !same_order_family(actual, bound) || is_nan_value(actual) || is_nan_value(bound) {
        return false;
    }
    check(compare_owned_values(actual, bound))
}

fn same_order_family(a: &Value, b: &Value) -> bool {
    fn family(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int64(_) | Value::Float64(_) | Value::Decimal128(_) => 2,
            Value::String(_) => 3,
            Value::Binary { .. } => 4,
            Value::ObjectId(_) => 5,
            Value::Uuid(_) => 6,
            Value::DateTimeUtcMs(_) => 7,
            Value::Array(_) => 8,
            Value::Document(_) => 9,
        }
    }
    family(a) == family(b)
}
