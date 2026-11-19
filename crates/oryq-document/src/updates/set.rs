//! `$set` update operator (`docs/document-model.md` §4.3).

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::types::Value;
use crate::updates::registry::UpdateOperator;

const MODULE_ID: &str = "document";

#[derive(Debug, Default)]
pub struct SetOperator;

impl UpdateOperator for SetOperator {
    fn name(&self) -> &'static str {
        "$set"
    }

    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        operand: &Value,
    ) -> Result<bool> {
        set_at_path(doc, segments, operand.clone())
    }
}

pub(crate) fn set_at_path(
    fields: &mut Vec<(String, Value)>,
    segments: &[&str],
    new_val: Value,
) -> Result<bool> {
    let (&head, tail) = segments.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "SetOperator::apply_field",
            "empty update path",
        )
    })?;

    if tail.is_empty() {
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
            if *existing == new_val {
                return Ok(false);
            }
            *existing = new_val;
            return Ok(true);
        }
        fields.push((head.to_owned(), new_val));
        return Ok(true);
    }

    // Intermediate segment: create sub-document if missing; fail if non-document.
    if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
        match existing {
            Value::Document(sub) => set_at_path(sub, tail, new_val),
            Value::Array(_) => Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "SetOperator::apply_field",
                format!("numeric/nested array-position update through '{head}' is unsupported"),
            )),
            _ => Err(ErrorEnvelope::new(
                ErrorCategory::TypeMismatch,
                MODULE_ID,
                "SetOperator::apply_field",
                format!("cannot traverse non-document field '{head}' during $set"),
            )),
        }
    } else {
        let mut sub = Vec::new();
        set_at_path(&mut sub, tail, new_val)?;
        fields.push((head.to_owned(), Value::Document(sub)));
        Ok(true)
    }
}
