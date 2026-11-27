//! Rule-based query planner selecting primary, secondary equality, secondary range, or
//! collection scan with strict sparse-index completeness safety (`docs/modules/query.md` §4).

use std::cmp::Ordering;

use oryq_document::{FieldPath, Value};
use oryq_index::{IndexDescriptor, IndexKind};

use crate::ast::FilterExpr;
use crate::explain::{AccessPlan, ExplainPlan};
use crate::predicates::compare_owned_values;

/// Plans physical access for `filter` over `collection` given its active `indexes`.
///
/// Completeness invariant:
/// - A `sparse` index is ONLY selected if the filter guarantees the indexed field is present
///   (e.g. an equality, range, or `$startsWith` predicate on that field in a top-level conjunction).
/// - Whenever a secondary index is used, `residual_filter` is retained if any other predicate
///   remains (or if a full recheck is needed) so index plans always match `CollectionScan` exactly.
pub fn plan_find(
    collection: &str,
    filter: Option<&FilterExpr>,
    indexes: &[IndexDescriptor],
) -> ExplainPlan {
    let Some(filter) = filter else {
        return ExplainPlan {
            collection: collection.to_owned(),
            access: AccessPlan::CollectionScan,
            residual_filter: None,
        };
    };

    if matches!(filter, FilterExpr::MatchAll) {
        return ExplainPlan {
            collection: collection.to_owned(),
            access: AccessPlan::CollectionScan,
            residual_filter: None,
        };
    }

    // Flatten top-level conjunctions into atomic clauses.
    let mut clauses = Vec::new();
    flatten_and(filter, &mut clauses);

    // 1. Primary `_id` equality probe has highest priority.
    let id_path = FieldPath::parse("_id").expect("_id is valid");
    for (idx, clause) in clauses.iter().enumerate() {
        if let FilterExpr::Eq(path, val) = clause {
            if *path == id_path {
                let residual = build_residual(&clauses, idx);
                return ExplainPlan {
                    collection: collection.to_owned(),
                    access: AccessPlan::PrimaryIdLookup {
                        target: val.clone(),
                    },
                    residual_filter: residual,
                };
            }
        }
    }

    // 2. Secondary equality scan on leading indexed field (Secondary & Compound only; never Pluggable).
    // Whole-array equality (`Value::Array`) falls back to `CollectionScan` because multikey indexes
    // expand arrays into element keys (except empty `[]`), so an element-key probe cannot match a
    // whole non-empty array target.
    for desc in indexes {
        if !matches!(desc.kind, IndexKind::Secondary | IndexKind::Compound)
            || desc.fields.is_empty()
        {
            continue;
        }
        let lead = &desc.fields[0].path;
        for clause in &clauses {
            if let FilterExpr::Eq(path, val) = clause {
                if matches!(val, Value::Array(items) if !items.is_empty()) {
                    continue;
                }
                if path == lead && (!desc.sparse || !matches!(val, Value::Null)) {
                    // Always retain full filter as residual to preserve exact array/multi-clause semantics.
                    return ExplainPlan {
                        collection: collection.to_owned(),
                        access: AccessPlan::SecondaryEqScan {
                            index_id: desc.index_id,
                            index_name: desc.name.clone(),
                            target: val.clone(),
                        },
                        residual_filter: Some(filter.clone()),
                    };
                }
            }
        }
    }

    // 3. Secondary range scan on leading indexed field ($gt/$gte/$lt/$lte; Secondary & Compound only).
    for desc in indexes {
        if !matches!(desc.kind, IndexKind::Secondary | IndexKind::Compound)
            || desc.fields.is_empty()
        {
            continue;
        }
        let lead = &desc.fields[0].path;
        let mut lower: Option<(Value, bool)> = None;
        let mut upper: Option<(Value, bool)> = None;
        for clause in &clauses {
            match clause {
                FilterExpr::Gt(path, v) if path == lead && !is_nan_bound(v) => {
                    tighten_lower(&mut lower, v.clone(), false);
                }
                FilterExpr::Gte(path, v) if path == lead && !is_nan_bound(v) => {
                    tighten_lower(&mut lower, v.clone(), true);
                }
                FilterExpr::Lt(path, v) if path == lead && !is_nan_bound(v) => {
                    tighten_upper(&mut upper, v.clone(), false);
                }
                FilterExpr::Lte(path, v) if path == lead && !is_nan_bound(v) => {
                    tighten_upper(&mut upper, v.clone(), true);
                }
                _ => {}
            }
        }
        if lower.is_some() || upper.is_some() {
            // Multikey soundness: for an array field like `[5, 100]`, `{x: {$gt: 10, $lt: 20}}`
            // matches because `100 > 10` and `5 < 20` even though no single element lies in `(10, 20)`.
            // Restricting the index scan to one bound (with full `residual_filter` recheck) guarantees
            // zero false negatives while still pruning keys on one side.
            if lower.is_some() && upper.is_some() {
                upper = None;
            }
            return ExplainPlan {
                collection: collection.to_owned(),
                access: AccessPlan::SecondaryRangeScan {
                    index_id: desc.index_id,
                    index_name: desc.name.clone(),
                    lower,
                    upper,
                },
                residual_filter: Some(filter.clone()),
            };
        }
    }

    // 4. Fallback: full collection scan.
    ExplainPlan {
        collection: collection.to_owned(),
        access: AccessPlan::CollectionScan,
        residual_filter: Some(filter.clone()),
    }
}

fn is_nan_bound(v: &Value) -> bool {
    match v {
        Value::Float64(f) => f.is_nan(),
        Value::Decimal128(d) => d.is_nan(),
        _ => false,
    }
}

fn tighten_lower(slot: &mut Option<(Value, bool)>, cand: Value, cand_inc: bool) {
    match slot {
        None => *slot = Some((cand, cand_inc)),
        Some((cur, cur_inc)) => match compare_owned_values(&cand, cur) {
            Ordering::Greater => *slot = Some((cand, cand_inc)),
            Ordering::Equal if !cand_inc => *cur_inc = false,
            _ => {}
        },
    }
}

fn tighten_upper(slot: &mut Option<(Value, bool)>, cand: Value, cand_inc: bool) {
    match slot {
        None => *slot = Some((cand, cand_inc)),
        Some((cur, cur_inc)) => match compare_owned_values(&cand, cur) {
            Ordering::Less => *slot = Some((cand, cand_inc)),
            Ordering::Equal if !cand_inc => *cur_inc = false,
            _ => {}
        },
    }
}

fn flatten_and<'a>(expr: &'a FilterExpr, out: &mut Vec<&'a FilterExpr>) {
    match expr {
        FilterExpr::And(list) => {
            for item in list {
                flatten_and(item, out);
            }
        }
        other => out.push(other),
    }
}

fn build_residual(clauses: &[&FilterExpr], consumed_idx: usize) -> Option<FilterExpr> {
    let rem: Vec<FilterExpr> = clauses
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != consumed_idx)
        .map(|(_, c)| (*c).clone())
        .collect();
    match rem.len() {
        0 => None,
        1 => Some(rem.into_iter().next().unwrap()),
        _ => Some(FilterExpr::And(rem)),
    }
}
