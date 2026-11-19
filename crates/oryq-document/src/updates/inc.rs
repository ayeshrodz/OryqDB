//! `$inc` update operator with checked integer overflow and lossless float promotion
//! (`docs/document-model.md` §4.3 & §4.4).

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::types::Value;
use crate::updates::registry::UpdateOperator;

const MODULE_ID: &str = "document";
const MAX_LOSSLESS_F64_INT: i64 = 1i64 << 53;

#[derive(Debug, Default)]
pub struct IncOperator;

impl UpdateOperator for IncOperator {
    fn name(&self) -> &'static str {
        "$inc"
    }

    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        operand: &Value,
    ) -> Result<bool> {
        validate_inc_operand(operand)?;
        inc_at_path(doc, segments, operand)
    }
}

fn validate_inc_operand(operand: &Value) -> Result<()> {
    match operand {
        Value::Int64(_) => Ok(()),
        Value::Float64(f) => {
            if !f.is_finite() {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "IncOperator::validate_inc_operand",
                    "$inc float operand must be finite",
                ));
            }
            Ok(())
        }
        Value::Decimal128(_) => Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "IncOperator::validate_inc_operand",
            "Decimal128 arithmetic in $inc is unsupported in v1 baseline",
        )),
        _ => Err(ErrorEnvelope::new(
            ErrorCategory::TypeMismatch,
            MODULE_ID,
            "IncOperator::validate_inc_operand",
            "$inc operand must be Int64 or Float64",
        )),
    }
}

fn inc_at_path(
    fields: &mut Vec<(String, Value)>,
    segments: &[&str],
    operand: &Value,
) -> Result<bool> {
    let (&head, tail) = segments.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "IncOperator::apply_field",
            "empty update path",
        )
    })?;

    if tail.is_empty() {
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
            let next = add_numeric(existing, operand)?;
            *existing = next;
            return Ok(true);
        }
        // Missing field is treated as 0 (preserving operand type, normalizing -0.0 -> 0.0).
        let initial = match *operand {
            Value::Int64(n) => Value::Int64(n),
            Value::Float64(f) => Value::Float64(normalize_zero(f)),
            _ => unreachable!("validated above"),
        };
        fields.push((head.to_owned(), initial));
        return Ok(true);
    }

    if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
        match existing {
            Value::Document(sub) => inc_at_path(sub, tail, operand),
            Value::Array(_) => Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "IncOperator::apply_field",
                format!("array-position $inc through '{head}' is unsupported"),
            )),
            _ => Err(ErrorEnvelope::new(
                ErrorCategory::TypeMismatch,
                MODULE_ID,
                "IncOperator::apply_field",
                format!("cannot traverse non-document field '{head}' during $inc"),
            )),
        }
    } else {
        let mut sub = Vec::new();
        inc_at_path(&mut sub, tail, operand)?;
        fields.push((head.to_owned(), Value::Document(sub)));
        Ok(true)
    }
}

fn add_numeric(existing: &Value, delta: &Value) -> Result<Value> {
    match (existing, delta) {
        (Value::Int64(a), Value::Int64(b)) => {
            a.checked_add(*b).map(Value::Int64).ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "IncOperator::add_numeric",
                    format!("i64 overflow in $inc ({a} + {b})"),
                )
            })
        }
        (Value::Int64(i), Value::Float64(f)) | (Value::Float64(f), Value::Int64(i)) => {
            if !(-MAX_LOSSLESS_F64_INT..=MAX_LOSSLESS_F64_INT).contains(i) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "IncOperator::add_numeric",
                    format!("lossless i64-to-f64 conversion failed for {i} (exceeds 2^53)"),
                ));
            }
            let sum = (*i as f64) + *f;
            if !sum.is_finite() {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "IncOperator::add_numeric",
                    "f64 overflow in $inc",
                ));
            }
            Ok(Value::Float64(normalize_zero(sum)))
        }
        (Value::Float64(a), Value::Float64(b)) => {
            let sum = *a + *b;
            if !sum.is_finite() {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "IncOperator::add_numeric",
                    "f64 overflow in $inc",
                ));
            }
            Ok(Value::Float64(normalize_zero(sum)))
        }
        (Value::Decimal128(_), _) => Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "IncOperator::add_numeric",
            "Decimal128 arithmetic in $inc is unsupported in v1 baseline",
        )),
        _ => Err(ErrorEnvelope::new(
            ErrorCategory::TypeMismatch,
            MODULE_ID,
            "IncOperator::add_numeric",
            "cannot apply $inc to non-numeric field",
        )),
    }
}

#[inline]
fn normalize_zero(f: f64) -> f64 {
    if f == 0.0 {
        0.0
    } else {
        f
    }
}
