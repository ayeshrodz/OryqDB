//! `$push` update operator (`docs/document-model.md` §4.3).

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::types::Value;
use crate::updates::registry::UpdateOperator;

const MODULE_ID: &str = "document";

#[derive(Debug, Default)]
pub struct PushOperator;

impl UpdateOperator for PushOperator {
    fn name(&self) -> &'static str {
        "$push"
    }

    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        operand: &Value,
    ) -> Result<bool> {
        push_at_path(doc, segments, operand.clone())
    }
}

fn push_at_path(fields: &mut Vec<(String, Value)>, segments: &[&str], elem: Value) -> Result<bool> {
    let (&head, tail) = segments.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "PushOperator::apply_field",
            "empty update path",
        )
    })?;

    if tail.is_empty() {
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
            match existing {
                Value::Array(items) => {
                    items.push(elem);
                    return Ok(true);
                }
                _ => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::TypeMismatch,
                        MODULE_ID,
                        "PushOperator::apply_field",
                        format!("cannot apply $push to non-array field '{head}'"),
                    ));
                }
            }
        }
        fields.push((head.to_owned(), Value::Array(vec![elem])));
        return Ok(true);
    }

    if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
        match existing {
            Value::Document(sub) => push_at_path(sub, tail, elem),
            Value::Array(_) => Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "PushOperator::apply_field",
                format!("array-position $push through '{head}' is unsupported"),
            )),
            _ => Err(ErrorEnvelope::new(
                ErrorCategory::TypeMismatch,
                MODULE_ID,
                "PushOperator::apply_field",
                format!("cannot traverse non-document field '{head}' during $push"),
            )),
        }
    } else {
        let mut sub = Vec::new();
        push_at_path(&mut sub, tail, elem)?;
        fields.push((head.to_owned(), Value::Document(sub)));
        Ok(true)
    }
}
