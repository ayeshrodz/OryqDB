//! Query execution, projection, bounded sorting, skip/limit, and cursor streaming.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::time::Instant;

use oryq_document::{encode_document, DocumentBuf, Value};
use oryq_index::SortOrder;
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::ast::{FindOptions, ProjectionSpec, SortSpec};
use crate::context::QueryContext;
use crate::explain::{AccessPlan, ExplainAnalyze, ExplainPlan};
use crate::planner::plan_find;
use crate::ports::QuerySource;
use crate::predicates::{compare_owned_values, eval_filter, resolve_path};

const MODULE_ID: &str = "query";

/// Materialized cursor yielding matching [`DocumentBuf`] rows one by one.
#[derive(Debug)]
pub struct QueryCursor {
    rows: std::vec::IntoIter<DocumentBuf>,
    plan: ExplainPlan,
}

impl QueryCursor {
    /// Constructs a [`QueryCursor`] directly from pre-fetched rows and an [`ExplainPlan`].
    pub fn from_rows(rows: Vec<DocumentBuf>, plan: ExplainPlan) -> Self {
        Self {
            rows: rows.into_iter(),
            plan,
        }
    }

    pub fn plan(&self) -> &ExplainPlan {
        &self.plan
    }

    pub fn next_doc(&mut self) -> Option<DocumentBuf> {
        self.rows.next()
    }

    pub fn collect_all(self) -> Vec<DocumentBuf> {
        self.rows.collect()
    }
}

/// Executes a `find()` query against `source` within a single snapshot and returns a [`QueryCursor`].
pub fn execute_find(
    source: &dyn QuerySource,
    collection: &str,
    options: &FindOptions,
    ctx: &QueryContext,
) -> Result<QueryCursor> {
    ctx.check_cancelled()?;
    let indexes = source.collection_indexes(collection)?;
    let plan = plan_find(collection, options.filter.as_ref(), &indexes);

    let mut matched = fetch_matching_docs(source, collection, &plan, &indexes, ctx)?;

    // Apply optional sort with memory budget check.
    if let Some(sort) = &options.sort {
        let approx_bytes: usize = matched.iter().map(|d| d.as_bytes().len() + 64).sum();
        let _guard = ctx.budget().reserve(approx_bytes)?;
        sort_documents(&mut matched, sort);
    }

    // Apply skip & limit.
    let skip = usize::try_from(options.skip).unwrap_or(usize::MAX);
    let mut sliced: Vec<DocumentBuf> = matched.into_iter().skip(skip).collect();
    if let Some(limit) = options.limit {
        let lim = usize::try_from(limit).unwrap_or(usize::MAX);
        sliced.truncate(lim);
    }

    // Apply projection.
    let final_rows = if let Some(proj) = &options.projection {
        let mut projected = Vec::with_capacity(sliced.len());
        for doc in &sliced {
            ctx.check_cancelled()?;
            projected.push(apply_projection(doc, proj)?);
        }
        projected
    } else {
        sliced
    };

    ctx.add_docs_returned(final_rows.len() as u64);
    Ok(QueryCursor {
        rows: final_rows.into_iter(),
        plan,
    })
}

/// Executes a `count()` query using the same planner and predicate semantics as `execute_find`.
pub fn execute_count(
    source: &dyn QuerySource,
    collection: &str,
    filter: Option<&crate::ast::FilterExpr>,
    ctx: &QueryContext,
) -> Result<u64> {
    ctx.check_cancelled()?;
    let indexes = source.collection_indexes(collection)?;
    let plan = plan_find(collection, filter, &indexes);
    let matched = fetch_matching_docs(source, collection, &plan, &indexes, ctx)?;
    Ok(matched.len() as u64)
}

/// Executes a `find()` query and returns the [`ExplainAnalyze`] plan + execution metrics.
pub fn explain_analyze(
    source: &dyn QuerySource,
    collection: &str,
    options: &FindOptions,
    ctx: &QueryContext,
) -> Result<ExplainAnalyze> {
    let start = Instant::now();
    let cursor = execute_find(source, collection, options, ctx)?;
    let elapsed_us = start.elapsed().as_micros() as u64;
    Ok(ExplainAnalyze {
        plan: cursor.plan().clone(),
        keys_examined: ctx.keys_examined(),
        docs_examined: ctx.docs_examined(),
        docs_returned: ctx.docs_returned(),
        execution_time_us: elapsed_us,
    })
}

fn fetch_matching_docs(
    source: &dyn QuerySource,
    collection: &str,
    plan: &ExplainPlan,
    indexes: &[oryq_index::IndexDescriptor],
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    let mut results = Vec::new();
    match &plan.access {
        AccessPlan::PrimaryIdLookup { target } => {
            ctx.add_keys_examined(1);
            if let Some(rid) = source.probe_primary_id(collection, target)? {
                if let Some(doc) = source.get_by_record_id(collection, rid)? {
                    ctx.add_docs_examined(1);
                    if passes_residual(&doc, plan.residual_filter.as_ref())? {
                        results.push(doc);
                    }
                }
            }
        }
        AccessPlan::SecondaryEqScan {
            index_id, target, ..
        } => {
            let desc = indexes
                .iter()
                .find(|d| d.index_id == *index_id)
                .ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Internal,
                        MODULE_ID,
                        "cursor::fetch_matching_docs",
                        format!("planned index {index_id:?} not found in catalog"),
                    )
                })?;
            let rids = source.scan_secondary_eq(desc, target)?;
            collect_by_rids(source, collection, rids, plan, ctx, &mut results)?;
        }
        AccessPlan::SecondaryRangeScan {
            index_id,
            lower,
            upper,
            ..
        } => {
            let desc = indexes
                .iter()
                .find(|d| d.index_id == *index_id)
                .ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Internal,
                        MODULE_ID,
                        "cursor::fetch_matching_docs",
                        format!("planned index {index_id:?} not found in catalog"),
                    )
                })?;
            let lower_ref = lower.as_ref().map(|(v, inc)| (v, *inc));
            let upper_ref = upper.as_ref().map(|(v, inc)| (v, *inc));
            let rids = source.scan_secondary_range(desc, lower_ref, upper_ref)?;
            collect_by_rids(source, collection, rids, plan, ctx, &mut results)?;
        }
        AccessPlan::CollectionScan => {
            let all = source.scan_collection(collection)?;
            for (_rid, doc) in all {
                ctx.check_cancelled()?;
                ctx.add_docs_examined(1);
                if passes_residual(&doc, plan.residual_filter.as_ref())? {
                    results.push(doc);
                }
            }
        }
    }
    Ok(results)
}

fn collect_by_rids(
    source: &dyn QuerySource,
    collection: &str,
    rids: Vec<RecordId>,
    plan: &ExplainPlan,
    ctx: &QueryContext,
    out: &mut Vec<DocumentBuf>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for rid in rids {
        ctx.check_cancelled()?;
        ctx.add_keys_examined(1);
        if !seen.insert(rid) {
            continue;
        }
        if let Some(doc) = source.get_by_record_id(collection, rid)? {
            ctx.add_docs_examined(1);
            if passes_residual(&doc, plan.residual_filter.as_ref())? {
                out.push(doc);
            }
        }
    }
    Ok(())
}

fn passes_residual(doc: &DocumentBuf, residual: Option<&crate::ast::FilterExpr>) -> Result<bool> {
    match residual {
        None => Ok(true),
        Some(expr) => eval_filter(&doc.view(), expr),
    }
}

/// Sorts documents in place according to `sort` using exact `compare_values` ordering.
pub fn sort_documents(docs: &mut [DocumentBuf], sort: &SortSpec) {
    if sort.keys.is_empty() {
        return;
    }
    docs.sort_by(|a, b| compare_docs_by_sort(a, b, sort));
}

pub fn compare_docs_by_sort(a: &DocumentBuf, b: &DocumentBuf, sort: &SortSpec) -> Ordering {
    let va = a.view();
    let vb = b.view();
    for (path, order) in &sort.keys {
        let fa = resolve_path(&va, path);
        let fb = resolve_path(&vb, path);
        let cmp = match (fa, fb) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(xa), Some(xb)) => compare_owned_values(&xa, &xb),
        };
        let ordered = match order {
            SortOrder::Asc => cmp,
            SortOrder::Desc => cmp.reverse(),
        };
        if ordered != Ordering::Equal {
            return ordered;
        }
    }
    Ordering::Equal
}

/// Applies an inclusion or exclusion [`ProjectionSpec`] to a document.
pub fn apply_projection(doc: &DocumentBuf, proj: &ProjectionSpec) -> Result<DocumentBuf> {
    let view = doc.view();
    let fields: Vec<(String, Value)> = match proj {
        ProjectionSpec::Include {
            fields: inc_paths,
            include_id,
        } => {
            let mut out = Vec::new();
            if *include_id {
                if let Some(id_val) = view.get_field("_id") {
                    out.push(("_id".to_owned(), id_val.to_owned_value()));
                }
            }
            for path in inc_paths {
                let segs: Vec<&str> = path.segments().collect();
                if segs.is_empty() || segs[0] == "_id" {
                    continue;
                }
                if segs.len() == 1 {
                    if let Some(val) = view.get_field(segs[0]) {
                        merge_nested_field(&mut out, &segs, val.to_owned_value());
                    }
                } else if let Some(val) = resolve_path(&view, path) {
                    merge_nested_field(&mut out, &segs, val);
                }
            }
            out
        }
        ProjectionSpec::Exclude { fields: exc_paths } => {
            let owned_fields: Vec<(String, Value)> = view
                .iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
                .collect();
            let path_vecs: Vec<Vec<&str>> =
                exc_paths.iter().map(|p| p.segments().collect()).collect();
            let path_slices: Vec<&[&str]> = path_vecs.iter().map(Vec::as_slice).collect();
            exclude_nested_fields(owned_fields, &path_slices)
        }
    };
    let (encoded, _) = encode_document(&fields, None)?;
    Ok(encoded)
}

fn merge_nested_field(target: &mut Vec<(String, Value)>, segs: &[&str], val: Value) {
    let Some((&head, tail)) = segs.split_first() else {
        return;
    };
    if tail.is_empty() {
        if let Some((_, existing)) = target.iter_mut().find(|(k, _)| k == head) {
            *existing = val;
        } else {
            target.push((head.to_owned(), val));
        }
        return;
    }
    if let Some((_, Value::Document(sub_fields))) = target.iter_mut().find(|(k, _)| k == head) {
        merge_nested_field(sub_fields, tail, val);
        return;
    }
    let mut sub_fields = Vec::new();
    merge_nested_field(&mut sub_fields, tail, val);
    if let Some((_, existing)) = target.iter_mut().find(|(k, _)| k == head) {
        *existing = Value::Document(sub_fields);
    } else {
        target.push((head.to_owned(), Value::Document(sub_fields)));
    }
}

fn exclude_nested_fields(
    fields: Vec<(String, Value)>,
    exc_paths: &[&[&str]],
) -> Vec<(String, Value)> {
    let mut out = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        let drop_whole = exc_paths
            .iter()
            .any(|segs| segs.len() == 1 && segs[0] == k.as_str());
        if drop_whole {
            continue;
        }
        let sub_paths: Vec<&[&str]> = exc_paths
            .iter()
            .filter(|segs| segs.len() > 1 && segs[0] == k.as_str())
            .map(|segs| &segs[1..])
            .collect();
        if sub_paths.is_empty() {
            out.push((k, v));
            continue;
        }
        let filtered_val = match v {
            Value::Document(sub_fields) => {
                Value::Document(exclude_nested_fields(sub_fields, &sub_paths))
            }
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(|item| match item {
                        Value::Document(sub_fields) => {
                            Value::Document(exclude_nested_fields(sub_fields, &sub_paths))
                        }
                        other => other,
                    })
                    .collect(),
            ),
            other => other,
        };
        out.push((k, filtered_val));
    }
    out
}
