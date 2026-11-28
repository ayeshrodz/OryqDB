//! Spillable aggregation pipeline engine (`$match`, `$project`, `$sort`, `$skip`, `$limit`,
//! `$unwind`, `$lookup`, `$group`) running over a single snapshot via [`QuerySource`].

use std::cmp::Ordering;
use std::collections::BTreeMap;

use oryq_document::{encode_document, DocumentBuf, FieldPath, Value};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};
use serde_json::Value as JsonValue;

use crate::ast::{FilterExpr, FindOptions, ProjectionSpec, SortSpec};
use crate::context::QueryContext;
use crate::cursor::{apply_projection, compare_docs_by_sort, execute_find, sort_documents};
use crate::parse::{parse_filter_value, parse_projection_value, parse_sort_value};
use crate::ports::QuerySource;
use crate::predicates::{compare_owned_values, eval_filter, resolve_path};
use crate::spill::SpillManager;

const MODULE_ID: &str = "query";
const DEFAULT_MAX_SPILL_BYTES: usize = 128 * 1024 * 1024;

/// Supported `$group` accumulator specifications.
#[derive(Debug, Clone, PartialEq)]
pub enum AccumulatorSpec {
    SumConst(f64),
    SumField(FieldPath),
    Avg(FieldPath),
    Min(FieldPath),
    Max(FieldPath),
    Count,
    Push(FieldPath),
    AddToSet(FieldPath),
}

/// One parsed aggregation pipeline stage.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStage {
    Match(FilterExpr),
    Project(ProjectionSpec),
    Sort(SortSpec),
    Skip(u64),
    Limit(u64),
    Unwind {
        path: FieldPath,
        preserve_null_and_empty: bool,
    },
    Lookup {
        from: String,
        local_field: FieldPath,
        foreign_field: FieldPath,
        as_field: String,
    },
    Group {
        id_field: Option<FieldPath>,
        accumulators: Vec<(String, AccumulatorSpec)>,
    },
}

/// Parses a JSON array of aggregation pipeline stages.
pub fn parse_pipeline_json(raw: &str) -> Result<Vec<PipelineStage>> {
    let val: JsonValue = serde_json::from_str(raw).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_pipeline_json",
            format!("invalid JSON pipeline: {err}"),
        )
        .with_source(err)
    })?;
    let arr = val.as_array().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_pipeline_json",
            "aggregation pipeline must be a JSON array",
        )
    })?;

    let mut stages = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item.as_object().ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidQuery,
                MODULE_ID,
                "pipeline::parse_pipeline_json",
                "each pipeline stage must be a JSON object",
            )
        })?;
        if obj.len() != 1 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidQuery,
                MODULE_ID,
                "pipeline::parse_pipeline_json",
                "each pipeline stage object must have exactly one operator key",
            ));
        }
        let (op, body) = obj.iter().next().unwrap();
        match op.as_str() {
            "$match" => stages.push(PipelineStage::Match(parse_filter_value(body)?)),
            "$project" => stages.push(PipelineStage::Project(parse_projection_value(body)?)),
            "$sort" => stages.push(PipelineStage::Sort(parse_sort_value(body)?)),
            "$skip" => {
                let n = body.as_u64().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_pipeline_json",
                        "$skip requires a non-negative integer",
                    )
                })?;
                stages.push(PipelineStage::Skip(n));
            }
            "$limit" => {
                let n = body.as_u64().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_pipeline_json",
                        "$limit requires a non-negative integer",
                    )
                })?;
                stages.push(PipelineStage::Limit(n));
            }
            "$unwind" => stages.push(parse_unwind(body)?),
            "$lookup" => stages.push(parse_lookup(body)?),
            "$group" => stages.push(parse_group(body)?),
            other => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::UnsupportedFeature,
                    MODULE_ID,
                    "pipeline::parse_pipeline_json",
                    format!("unsupported pipeline stage '{other}'"),
                ));
            }
        }
    }
    Ok(stages)
}

fn parse_field_ref(s: &str, op: &str) -> Result<FieldPath> {
    let trimmed = s.strip_prefix('$').ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_field_ref",
            format!("{op} field reference '{s}' must start with '$'"),
        )
    })?;
    FieldPath::parse(trimmed)
}

fn parse_unwind(body: &JsonValue) -> Result<PipelineStage> {
    if let Some(s) = body.as_str() {
        return Ok(PipelineStage::Unwind {
            path: parse_field_ref(s, "$unwind")?,
            preserve_null_and_empty: false,
        });
    }
    let obj = body.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_unwind",
            "$unwind requires a '$field' string or object",
        )
    })?;
    let path_str = obj.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_unwind",
            "$unwind object requires string 'path'",
        )
    })?;
    let preserve = obj
        .get("preserveNullAndEmptyArrays")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(PipelineStage::Unwind {
        path: parse_field_ref(path_str, "$unwind")?,
        preserve_null_and_empty: preserve,
    })
}

fn parse_lookup(body: &JsonValue) -> Result<PipelineStage> {
    let obj = body.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_lookup",
            "$lookup requires a JSON object",
        )
    })?;
    let get_str = |k: &str| -> Result<String> {
        obj.get(k)
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidQuery,
                    MODULE_ID,
                    "pipeline::parse_lookup",
                    format!("$lookup requires string field '{k}'"),
                )
            })
    };
    let from = get_str("from")?;
    let local_field = FieldPath::parse(&get_str("localField")?)?;
    let foreign_field = FieldPath::parse(&get_str("foreignField")?)?;
    let as_field = get_str("as")?;
    Ok(PipelineStage::Lookup {
        from,
        local_field,
        foreign_field,
        as_field,
    })
}

fn parse_group(body: &JsonValue) -> Result<PipelineStage> {
    let obj = body.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_group",
            "$group requires a JSON object",
        )
    })?;
    let id_val = obj.get("_id").ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "pipeline::parse_group",
            "$group requires an '_id' key",
        )
    })?;
    let id_field = match id_val {
        JsonValue::Null => None,
        JsonValue::String(s) => Some(parse_field_ref(s, "$group._id")?),
        _ => {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidQuery,
                MODULE_ID,
                "pipeline::parse_group",
                "$group '_id' must be null or a '$field' reference",
            ));
        }
    };

    let mut accumulators = Vec::new();
    for (out_field, spec_val) in obj {
        if out_field == "_id" {
            continue;
        }
        let spec_obj = spec_val.as_object().ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidQuery,
                MODULE_ID,
                "pipeline::parse_group",
                format!("$group accumulator for '{out_field}' must be an object"),
            )
        })?;
        if spec_obj.len() != 1 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidQuery,
                MODULE_ID,
                "pipeline::parse_group",
                format!("$group accumulator for '{out_field}' must have 1 operator"),
            ));
        }
        let (acc_op, operand) = spec_obj.iter().next().unwrap();
        let acc = match acc_op.as_str() {
            "$sum" => {
                if let Some(n) = operand.as_f64() {
                    AccumulatorSpec::SumConst(n)
                } else if let Some(s) = operand.as_str() {
                    AccumulatorSpec::SumField(parse_field_ref(s, "$sum")?)
                } else {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$sum requires a number or '$field'",
                    ));
                }
            }
            "$avg" => {
                let s = operand.as_str().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$avg requires '$field'",
                    )
                })?;
                AccumulatorSpec::Avg(parse_field_ref(s, "$avg")?)
            }
            "$min" => {
                let s = operand.as_str().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$min requires '$field'",
                    )
                })?;
                AccumulatorSpec::Min(parse_field_ref(s, "$min")?)
            }
            "$max" => {
                let s = operand.as_str().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$max requires '$field'",
                    )
                })?;
                AccumulatorSpec::Max(parse_field_ref(s, "$max")?)
            }
            "$count" => AccumulatorSpec::Count,
            "$push" => {
                let s = operand.as_str().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$push requires '$field'",
                    )
                })?;
                AccumulatorSpec::Push(parse_field_ref(s, "$push")?)
            }
            "$addToSet" => {
                let s = operand.as_str().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidQuery,
                        MODULE_ID,
                        "pipeline::parse_group",
                        "$addToSet requires '$field'",
                    )
                })?;
                AccumulatorSpec::AddToSet(parse_field_ref(s, "$addToSet")?)
            }
            other => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::UnsupportedFeature,
                    MODULE_ID,
                    "pipeline::parse_group",
                    format!("unsupported $group accumulator '{other}'"),
                ));
            }
        };
        accumulators.push((out_field.clone(), acc));
    }

    Ok(PipelineStage::Group {
        id_field,
        accumulators,
    })
}

/// Executes a parsed aggregation pipeline over `collection` in a single snapshot.
pub fn execute_pipeline(
    source: &dyn QuerySource,
    collection: &str,
    stages: &[PipelineStage],
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    ctx.check_cancelled()?;

    // Push down leading `$match` to `execute_find` so it uses indexes automatically!
    let (mut docs, start_idx) = match stages.first() {
        Some(PipelineStage::Match(filter)) => {
            let cur = execute_find(
                source,
                collection,
                &FindOptions {
                    filter: Some(filter.clone()),
                    ..Default::default()
                },
                ctx,
            )?;
            (cur.collect_all(), 1)
        }
        _ => {
            let cur = execute_find(source, collection, &FindOptions::default(), ctx)?;
            (cur.collect_all(), 0)
        }
    };

    for stage in &stages[start_idx..] {
        ctx.check_cancelled()?;
        docs = match stage {
            PipelineStage::Match(filter) => {
                let mut next = Vec::new();
                for d in docs {
                    ctx.check_cancelled()?;
                    if eval_filter(&d.view(), filter)? {
                        next.push(d);
                    }
                }
                next
            }
            PipelineStage::Project(proj) => {
                let mut next = Vec::with_capacity(docs.len());
                for d in &docs {
                    ctx.check_cancelled()?;
                    next.push(apply_projection(d, proj)?);
                }
                next
            }
            PipelineStage::Sort(sort) => spillable_sort(docs, sort, ctx)?,
            PipelineStage::Skip(n) => {
                let s = usize::try_from(*n).unwrap_or(usize::MAX);
                docs.into_iter().skip(s).collect()
            }
            PipelineStage::Limit(n) => {
                let l = usize::try_from(*n).unwrap_or(usize::MAX);
                docs.into_iter().take(l).collect()
            }
            PipelineStage::Unwind {
                path,
                preserve_null_and_empty,
            } => exec_unwind(docs, path, *preserve_null_and_empty, ctx)?,
            PipelineStage::Lookup {
                from,
                local_field,
                foreign_field,
                as_field,
            } => exec_lookup(
                source,
                docs,
                from,
                local_field,
                foreign_field,
                as_field,
                ctx,
            )?,
            PipelineStage::Group {
                id_field,
                accumulators,
            } => exec_group(docs, id_field.as_ref(), accumulators, ctx)?,
        };
    }

    Ok(docs)
}

/// Convenience JSON wrapper parsing and executing an aggregation pipeline.
pub fn execute_pipeline_json(
    source: &dyn QuerySource,
    collection: &str,
    pipeline_json: &str,
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    let stages = parse_pipeline_json(pipeline_json)?;
    execute_pipeline(source, collection, &stages, ctx)
}

/// Sorts `docs` in memory if within `ctx.budget()`, or spills sorted runs to disk via
/// [`SpillManager`] and performs an external merge when exceeding memory budget.
pub fn spillable_sort(
    mut docs: Vec<DocumentBuf>,
    sort: &SortSpec,
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    if docs.len() <= 1 || sort.keys.is_empty() {
        return Ok(docs);
    }
    let total_bytes: usize = docs.iter().map(|d| d.len() + 64).sum();
    if let Ok(_guard) = ctx.budget().reserve(total_bytes) {
        sort_documents(&mut docs, sort);
        return Ok(docs);
    }

    // External spill-merge fallback: partition into budget-sized runs, sort each, spill to disk.
    let mut spill = SpillManager::new(DEFAULT_MAX_SPILL_BYTES)?;
    let run_cap = ctx.budget().limit_bytes().max(256);
    let mut run_paths = Vec::new();
    let mut chunk: Vec<DocumentBuf> = Vec::new();
    let mut chunk_bytes = 0usize;

    for doc in docs {
        ctx.check_cancelled()?;
        let sz = doc.len() + 64;
        if !chunk.is_empty() && chunk_bytes + sz > run_cap {
            sort_documents(&mut chunk, sort);
            run_paths.push(spill.write_run(&chunk)?);
            chunk.clear();
            chunk_bytes = 0;
        }
        chunk_bytes += sz;
        chunk.push(doc);
    }
    if !chunk.is_empty() {
        sort_documents(&mut chunk, sort);
        run_paths.push(spill.write_run(&chunk)?);
    }

    // Streaming k-way merge over sorted runs using a min-heap (`BinaryHeap`):
    // holds only ONE active head document per run in memory at a time (O(K) merge memory, O(N log K) time).
    let mut readers = Vec::with_capacity(run_paths.len());
    for p in &run_paths {
        readers.push(SpillManager::open_run(p)?);
    }

    struct HeapEntry<'a> {
        doc: DocumentBuf,
        run_idx: usize,
        sort: &'a SortSpec,
    }
    impl PartialEq for HeapEntry<'_> {
        fn eq(&self, other: &Self) -> bool {
            self.run_idx == other.run_idx
                && compare_docs_by_sort(&self.doc, &other.doc, self.sort) == Ordering::Equal
        }
    }
    impl Eq for HeapEntry<'_> {}
    impl PartialOrd for HeapEntry<'_> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for HeapEntry<'_> {
        fn cmp(&self, other: &Self) -> Ordering {
            // Reversed for min-heap behavior in Rust's max-heap BinaryHeap.
            compare_docs_by_sort(&other.doc, &self.doc, self.sort)
                .then_with(|| other.run_idx.cmp(&self.run_idx))
        }
    }

    let mut heap = std::collections::BinaryHeap::with_capacity(readers.len());
    for (run_idx, reader) in readers.iter_mut().enumerate() {
        if let Some(doc) = reader.next_doc()? {
            heap.push(HeapEntry { doc, run_idx, sort });
        }
    }

    let mut merged = Vec::new();
    while let Some(HeapEntry { doc, run_idx, .. }) = heap.pop() {
        ctx.check_cancelled()?;
        merged.push(doc);
        if let Some(next_doc) = readers[run_idx].next_doc()? {
            heap.push(HeapEntry {
                doc: next_doc,
                run_idx,
                sort,
            });
        }
    }

    // `spill` drops here and cleans up all temporary run files automatically!
    Ok(merged)
}

fn exec_unwind(
    docs: Vec<DocumentBuf>,
    path: &FieldPath,
    preserve: bool,
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    let segs: Vec<&str> = path.segments().collect();
    if segs.len() != 1 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "pipeline::exec_unwind",
            "$unwind currently supports top-level array fields",
        ));
    }
    let target_field = segs[0];
    let mut out = Vec::new();
    for doc in &docs {
        ctx.check_cancelled()?;
        let view = doc.view();
        match view.get_field(target_field).map(|v| v.to_owned_value()) {
            Some(Value::Array(items)) if !items.is_empty() => {
                for item in items {
                    let mut fields = Vec::new();
                    for (k, v) in view.iter() {
                        if k == target_field {
                            fields.push((k.to_owned(), item.clone()));
                        } else {
                            fields.push((k.to_owned(), v.to_owned_value()));
                        }
                    }
                    let (encoded, _) = encode_document(&fields, None)?;
                    out.push(encoded);
                }
            }
            _ => {
                if preserve {
                    out.push(doc.clone());
                }
            }
        }
    }
    Ok(out)
}

fn exec_lookup(
    source: &dyn QuerySource,
    docs: Vec<DocumentBuf>,
    from: &str,
    local_field: &FieldPath,
    foreign_field: &FieldPath,
    as_field: &str,
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    // Snapshot-consistent read of foreign collection through the same `QuerySource`.
    let foreign_docs = source.scan_collection(from)?;
    let mut out = Vec::with_capacity(docs.len());
    for doc in &docs {
        ctx.check_cancelled()?;
        let local_val = resolve_path(&doc.view(), local_field);
        let mut matched = Vec::new();
        if let Some(lv) = &local_val {
            for (_, fdoc) in &foreign_docs {
                if let Some(fv) = resolve_path(&fdoc.view(), foreign_field) {
                    if compare_owned_values(lv, &fv) == Ordering::Equal {
                        let pairs: Vec<(String, Value)> = fdoc
                            .view()
                            .iter()
                            .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
                            .collect();
                        matched.push(Value::Document(pairs));
                    }
                }
            }
        }
        let mut fields: Vec<(String, Value)> = doc
            .view()
            .iter()
            .filter(|(k, _)| *k != as_field)
            .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
            .collect();
        fields.push((as_field.to_owned(), Value::Array(matched)));
        let (encoded, _) = encode_document(&fields, None)?;
        out.push(encoded);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct GroupKey(Value);

impl PartialEq for GroupKey {
    fn eq(&self, other: &Self) -> bool {
        compare_owned_values(&self.0, &other.0) == Ordering::Equal
    }
}
impl Eq for GroupKey {}
impl PartialOrd for GroupKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for GroupKey {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_owned_values(&self.0, &other.0)
    }
}

#[derive(Debug)]
enum AccState {
    Sum {
        total: f64,
        all_int: bool,
        int_total: i64,
    },
    Avg {
        sum: f64,
        count: u64,
    },
    Min(Option<Value>),
    Max(Option<Value>),
    Count(i64),
    Push(Vec<Value>),
    AddToSet(Vec<Value>),
}

impl AccState {
    fn new(spec: &AccumulatorSpec) -> Self {
        match spec {
            AccumulatorSpec::SumConst(_) | AccumulatorSpec::SumField(_) => Self::Sum {
                total: 0.0,
                all_int: true,
                int_total: 0,
            },
            AccumulatorSpec::Avg(_) => Self::Avg { sum: 0.0, count: 0 },
            AccumulatorSpec::Min(_) => Self::Min(None),
            AccumulatorSpec::Max(_) => Self::Max(None),
            AccumulatorSpec::Count => Self::Count(0),
            AccumulatorSpec::Push(_) => Self::Push(Vec::new()),
            AccumulatorSpec::AddToSet(_) => Self::AddToSet(Vec::new()),
        }
    }

    fn update(&mut self, spec: &AccumulatorSpec, doc: &DocumentBuf) {
        match (self, spec) {
            (
                Self::Sum {
                    total,
                    all_int,
                    int_total,
                },
                AccumulatorSpec::SumConst(c),
            ) => {
                *total += *c;
                if *all_int && c.is_finite() && c.fract() == 0.0 && c.abs() < ((1u64 << 53) as f64)
                {
                    if let Some(next) = int_total.checked_add(*c as i64) {
                        *int_total = next;
                    } else {
                        *all_int = false;
                    }
                } else {
                    *all_int = false;
                }
            }
            (
                Self::Sum {
                    total,
                    all_int,
                    int_total,
                },
                AccumulatorSpec::SumField(path),
            ) => match resolve_path(&doc.view(), path) {
                Some(Value::Int64(n)) => {
                    *total += n as f64;
                    if *all_int {
                        if let Some(next) = int_total.checked_add(n) {
                            *int_total = next;
                        } else {
                            *all_int = false;
                        }
                    }
                }
                Some(Value::Float64(f)) => {
                    *total += f;
                    *all_int = false;
                }
                Some(Value::Decimal128(d)) => {
                    if let Ok(f) = d.to_string().parse::<f64>() {
                        *total += f;
                        *all_int = false;
                    }
                }
                _ => {}
            },
            (Self::Avg { sum, count }, AccumulatorSpec::Avg(path)) => {
                match resolve_path(&doc.view(), path) {
                    Some(Value::Int64(n)) => {
                        *sum += n as f64;
                        *count += 1;
                    }
                    Some(Value::Float64(f)) => {
                        *sum += f;
                        *count += 1;
                    }
                    Some(Value::Decimal128(d)) => {
                        if let Ok(f) = d.to_string().parse::<f64>() {
                            *sum += f;
                            *count += 1;
                        }
                    }
                    _ => {}
                }
            }
            (Self::Min(cur), AccumulatorSpec::Min(path)) => {
                if let Some(v) = resolve_path(&doc.view(), path) {
                    let replace = match cur {
                        None => true,
                        Some(existing) => compare_owned_values(&v, existing) == Ordering::Less,
                    };
                    if replace {
                        *cur = Some(v);
                    }
                }
            }
            (Self::Max(cur), AccumulatorSpec::Max(path)) => {
                if let Some(v) = resolve_path(&doc.view(), path) {
                    let replace = match cur {
                        None => true,
                        Some(existing) => compare_owned_values(&v, existing) == Ordering::Greater,
                    };
                    if replace {
                        *cur = Some(v);
                    }
                }
            }
            (Self::Count(c), AccumulatorSpec::Count) => *c += 1,
            (Self::Push(list), AccumulatorSpec::Push(path)) => {
                if let Some(v) = resolve_path(&doc.view(), path) {
                    list.push(v);
                }
            }
            (Self::AddToSet(list), AccumulatorSpec::AddToSet(path)) => {
                if let Some(v) = resolve_path(&doc.view(), path) {
                    if !list
                        .iter()
                        .any(|e| compare_owned_values(e, &v) == Ordering::Equal)
                    {
                        list.push(v);
                    }
                }
            }
            _ => {}
        }
    }

    fn finalize(self) -> Value {
        match self {
            Self::Sum {
                total,
                all_int,
                int_total,
            } => {
                if all_int {
                    Value::Int64(int_total)
                } else {
                    Value::Float64(total)
                }
            }
            Self::Avg { sum, count } => {
                if count == 0 {
                    Value::Null
                } else {
                    Value::Float64(sum / (count as f64))
                }
            }
            Self::Min(v) | Self::Max(v) => v.unwrap_or(Value::Null),
            Self::Count(c) => Value::Int64(c),
            Self::Push(v) | Self::AddToSet(v) => Value::Array(v),
        }
    }
}

fn exec_group(
    docs: Vec<DocumentBuf>,
    id_field: Option<&FieldPath>,
    accumulators: &[(String, AccumulatorSpec)],
    ctx: &QueryContext,
) -> Result<Vec<DocumentBuf>> {
    let mut groups: BTreeMap<GroupKey, Vec<AccState>> = BTreeMap::new();
    let mut _reservations = Vec::new();

    for doc in &docs {
        ctx.check_cancelled()?;
        let key_val = match id_field {
            None => Value::Null,
            Some(p) => resolve_path(&doc.view(), p).unwrap_or(Value::Null),
        };
        let key = GroupKey(key_val);
        if !groups.contains_key(&key) {
            // Charge group state overhead against query memory budget!
            _reservations.push(ctx.budget().reserve(128 + accumulators.len() * 64)?);
            let states: Vec<AccState> = accumulators
                .iter()
                .map(|(_, spec)| AccState::new(spec))
                .collect();
            groups.insert(key.clone(), states);
        }
        let states = groups.get_mut(&key).unwrap();
        for (state, (_, spec)) in states.iter_mut().zip(accumulators.iter()) {
            state.update(spec, doc);
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for (key, states) in groups {
        let mut fields = Vec::with_capacity(1 + accumulators.len());
        fields.push(("_id".to_owned(), key.0));
        for ((name, _), state) in accumulators.iter().zip(states) {
            fields.push((name.clone(), state.finalize()));
        }
        let (encoded, _) = encode_document(&fields, None)?;
        out.push(encoded);
    }
    Ok(out)
}
