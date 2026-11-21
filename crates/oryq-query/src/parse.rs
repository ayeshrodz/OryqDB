//! JSON parser for MongoDB-style filter, projection, and sort specifications.

use oryq_document::{from_json_value, FieldPath, Value};
use oryq_index::SortOrder;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};
use serde_json::Value as JsonValue;

use crate::ast::{FilterExpr, ProjectionSpec, SortSpec};

const MODULE_ID: &str = "query";

/// Parses a MongoDB-style JSON filter string (e.g. `{"age": {"$gte": 18}, "active": true}`).
pub fn parse_filter_json(raw: &str) -> Result<FilterExpr> {
    let val: JsonValue = serde_json::from_str(raw).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_filter_json",
            format!("invalid JSON filter: {err}"),
        )
        .with_source(err)
    })?;
    parse_filter_value(&val)
}

pub fn parse_filter_value(val: &JsonValue) -> Result<FilterExpr> {
    let obj = val.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_filter_value",
            "filter expression must be a JSON object",
        )
    })?;

    if obj.is_empty() {
        return Ok(FilterExpr::MatchAll);
    }

    let mut clauses = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        match k.as_str() {
            "$and" => {
                let arr = expect_array(v, "$and")?;
                let mut sub = Vec::with_capacity(arr.len());
                for item in arr {
                    sub.push(parse_filter_value(item)?);
                }
                clauses.push(FilterExpr::And(sub));
            }
            "$or" => {
                let arr = expect_array(v, "$or")?;
                let mut sub = Vec::with_capacity(arr.len());
                for item in arr {
                    sub.push(parse_filter_value(item)?);
                }
                clauses.push(FilterExpr::Or(sub));
            }
            "$nor" => {
                let arr = expect_array(v, "$nor")?;
                let mut sub = Vec::with_capacity(arr.len());
                for item in arr {
                    sub.push(parse_filter_value(item)?);
                }
                clauses.push(FilterExpr::Nor(sub));
            }
            other if other.starts_with('$') => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::UnsupportedFeature,
                    MODULE_ID,
                    "parse::parse_filter_value",
                    format!("unsupported top-level query operator '{other}'"),
                ));
            }
            raw_path => {
                let path = FieldPath::parse(raw_path)?;
                parse_field_condition(path, v, &mut clauses)?;
            }
        }
    }

    if clauses.len() == 1 {
        Ok(clauses.pop().unwrap())
    } else {
        Ok(FilterExpr::And(clauses))
    }
}

fn parse_field_condition(path: FieldPath, v: &JsonValue, out: &mut Vec<FilterExpr>) -> Result<()> {
    if let Some(obj) = v.as_object() {
        let has_op = obj.keys().any(|k| k.starts_with('$'));
        // Extended JSON literals ($oid, $uuid, $date, $binary, $numberLong/Double/Decimal)
        // are values, not query operators!
        if has_op && !is_extended_json_wrapper(obj) {
            for (op, operand) in obj {
                match op.as_str() {
                    "$eq" => out.push(FilterExpr::Eq(path.clone(), json_to_oryq(operand)?)),
                    "$ne" => out.push(FilterExpr::Ne(path.clone(), json_to_oryq(operand)?)),
                    "$lt" => out.push(FilterExpr::Lt(path.clone(), json_to_oryq(operand)?)),
                    "$lte" => out.push(FilterExpr::Lte(path.clone(), json_to_oryq(operand)?)),
                    "$gt" => out.push(FilterExpr::Gt(path.clone(), json_to_oryq(operand)?)),
                    "$gte" => out.push(FilterExpr::Gte(path.clone(), json_to_oryq(operand)?)),
                    "$in" => {
                        let arr = expect_array(operand, "$in")?;
                        let mut vals = Vec::with_capacity(arr.len());
                        for item in arr {
                            vals.push(json_to_oryq(item)?);
                        }
                        out.push(FilterExpr::In(path.clone(), vals));
                    }
                    "$nin" => {
                        let arr = expect_array(operand, "$nin")?;
                        let mut vals = Vec::with_capacity(arr.len());
                        for item in arr {
                            vals.push(json_to_oryq(item)?);
                        }
                        out.push(FilterExpr::Nin(path.clone(), vals));
                    }
                    "$exists" => {
                        let b = operand.as_bool().ok_or_else(|| {
                            ErrorEnvelope::new(
                                ErrorCategory::InvalidQuery,
                                MODULE_ID,
                                "parse::parse_field_condition",
                                "$exists operand must be a boolean",
                            )
                        })?;
                        out.push(FilterExpr::Exists(path.clone(), b));
                    }
                    "$startsWith" => {
                        let prefix = operand.as_str().ok_or_else(|| {
                            ErrorEnvelope::new(
                                ErrorCategory::InvalidQuery,
                                MODULE_ID,
                                "parse::parse_field_condition",
                                "$startsWith operand must be a string",
                            )
                        })?;
                        out.push(FilterExpr::StartsWith(path.clone(), prefix.to_owned()));
                    }
                    "$not" => {
                        let mut inner = Vec::new();
                        parse_field_condition(path.clone(), operand, &mut inner)?;
                        let expr = if inner.len() == 1 {
                            inner.pop().unwrap()
                        } else {
                            FilterExpr::And(inner)
                        };
                        out.push(FilterExpr::Not(Box::new(expr)));
                    }
                    "$elemMatch" => {
                        let sub = parse_filter_value(operand)?;
                        out.push(FilterExpr::ElemMatch(path.clone(), Box::new(sub)));
                    }
                    other => {
                        return Err(ErrorEnvelope::new(
                            ErrorCategory::UnsupportedFeature,
                            MODULE_ID,
                            "parse::parse_field_condition",
                            format!("unsupported field query operator '{other}'"),
                        ));
                    }
                }
            }
            return Ok(());
        }
    }

    out.push(FilterExpr::Eq(path, json_to_oryq(v)?));
    Ok(())
}

/// Parses a JSON projection object (e.g. `{"name": 1, "age": 1, "_id": 0}`).
pub fn parse_projection_json(raw: &str) -> Result<ProjectionSpec> {
    let val: JsonValue = serde_json::from_str(raw).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_projection_json",
            format!("invalid JSON projection: {err}"),
        )
        .with_source(err)
    })?;
    parse_projection_value(&val)
}

pub fn parse_projection_value(val: &JsonValue) -> Result<ProjectionSpec> {
    let obj = val.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_projection_value",
            "projection must be a JSON object",
        )
    })?;

    let mut include_fields = Vec::new();
    let mut exclude_fields = Vec::new();
    let mut id_flag: Option<bool> = None;

    for (raw_path, flag_val) in obj {
        let flag = match flag_val {
            JsonValue::Bool(b) => *b,
            JsonValue::Number(n) if n.as_i64() == Some(1) => true,
            JsonValue::Number(n) if n.as_i64() == Some(0) => false,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidQuery,
                    MODULE_ID,
                    "parse::parse_projection_value",
                    format!("projection flag for '{raw_path}' must be 0/1 or true/false"),
                ));
            }
        };
        if raw_path == "_id" {
            id_flag = Some(flag);
            continue;
        }
        let path = FieldPath::parse(raw_path)?;
        if flag {
            include_fields.push(path);
        } else {
            exclude_fields.push(path);
        }
    }

    if !include_fields.is_empty() && !exclude_fields.is_empty() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_projection_value",
            "cannot mix inclusion and exclusion fields in the same projection",
        ));
    }

    if !include_fields.is_empty() || id_flag == Some(true) {
        Ok(ProjectionSpec::Include {
            fields: include_fields,
            include_id: id_flag.unwrap_or(true),
        })
    } else {
        if id_flag == Some(false) {
            exclude_fields.push(FieldPath::parse("_id")?);
        }
        Ok(ProjectionSpec::Exclude {
            fields: exclude_fields,
        })
    }
}

/// Parses a JSON sort object (e.g. `{"score": -1, "name": 1}`).
pub fn parse_sort_json(raw: &str) -> Result<SortSpec> {
    let val: JsonValue = serde_json::from_str(raw).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_sort_json",
            format!("invalid JSON sort specification: {err}"),
        )
        .with_source(err)
    })?;
    parse_sort_value(&val)
}

pub fn parse_sort_value(val: &JsonValue) -> Result<SortSpec> {
    let obj = val.as_object().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::parse_sort_value",
            "sort specification must be a JSON object",
        )
    })?;
    let mut keys = Vec::with_capacity(obj.len());
    for (raw_path, dir) in obj {
        let path = FieldPath::parse(raw_path)?;
        let order = match dir.as_i64() {
            Some(1) => SortOrder::Asc,
            Some(-1) => SortOrder::Desc,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidQuery,
                    MODULE_ID,
                    "parse::parse_sort_value",
                    format!("sort direction for '{raw_path}' must be 1 or -1"),
                ));
            }
        };
        keys.push((path, order));
    }
    Ok(SortSpec { keys })
}

pub fn json_to_oryq(v: &JsonValue) -> Result<Value> {
    let wrapper = serde_json::json!({ "v": v });
    let (doc, _) = from_json_value(&wrapper, None)?;
    Ok(doc.view().get_field("v").unwrap().to_owned_value())
}

fn expect_array<'a>(v: &'a JsonValue, op: &str) -> Result<&'a Vec<JsonValue>> {
    v.as_array().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidQuery,
            MODULE_ID,
            "parse::expect_array",
            format!("operator '{op}' requires a JSON array operand"),
        )
    })
}

fn is_extended_json_wrapper(obj: &serde_json::Map<String, JsonValue>) -> bool {
    if obj.len() != 1 {
        return false;
    }
    let key = obj.keys().next().unwrap().as_str();
    matches!(
        key,
        "$oid" | "$uuid" | "$date" | "$binary" | "$numberLong" | "$numberDouble" | "$numberDecimal"
    )
}
