//! `$pull` update operator using canonical [`compare_values`] equality
//! (`docs/document-model.md` §4.3).

use core::cmp::Ordering;

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::codec::encode_document;
use crate::compare::compare_values;
use crate::types::Value;
use crate::updates::registry::UpdateOperator;

const MODULE_ID: &str = "document";

#[derive(Debug, Default)]
pub struct PullOperator;

impl UpdateOperator for PullOperator {
    fn name(&self) -> &'static str {
        "$pull"
    }

    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        operand: &Value,
    ) -> Result<bool> {
        pull_at_path(doc, segments, operand)
    }
}

fn pull_at_path(fields: &mut [(String, Value)], segments: &[&str], target: &Value) -> Result<bool> {
    let (&head, tail) = segments.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "PullOperator::apply_field",
            "empty update path",
        )
    })?;

    if tail.is_empty() {
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
            match existing {
                Value::Array(items) => {
                    let before = items.len();
                    let mut kept = Vec::with_capacity(before);
                    for item in items.drain(..) {
                        if !values_canonically_equal(&item, target)? {
                            kept.push(item);
                        }
                    }
                    let changed = kept.len() != before;
                    *items = kept;
                    return Ok(changed);
                }
                _ => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::TypeMismatch,
                        MODULE_ID,
                        "PullOperator::apply_field",
                        format!("cannot apply $pull to non-array field '{head}'"),
                    ));
                }
            }
        }
        return Ok(false);
    }

    if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
        match existing {
            Value::Document(sub) => pull_at_path(sub, tail, target),
            Value::Array(_) => Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "PullOperator::apply_field",
                format!("array-position $pull through '{head}' is unsupported"),
            )),
            _ => Err(ErrorEnvelope::new(
                ErrorCategory::TypeMismatch,
                MODULE_ID,
                "PullOperator::apply_field",
                format!("cannot traverse non-document field '{head}' during $pull"),
            )),
        }
    } else {
        Ok(false)
    }
}

fn values_canonically_equal(a: &Value, b: &Value) -> Result<bool> {
    let (da, _) = encode_document(&[("v".to_owned(), a.clone())], None)?;
    let (db, _) = encode_document(&[("v".to_owned(), b.clone())], None)?;
    let va = da.view().get_field("v").unwrap();
    let vb = db.view().get_field("v").unwrap();
    Ok(compare_values(&va, &vb) == Ordering::Equal)
}
