//! `$unset` update operator (`docs/document-model.md` §4.3).

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::types::Value;
use crate::updates::registry::UpdateOperator;

const MODULE_ID: &str = "document";

#[derive(Debug, Default)]
pub struct UnsetOperator;

impl UpdateOperator for UnsetOperator {
    fn name(&self) -> &'static str {
        "$unset"
    }

    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        _operand: &Value,
    ) -> Result<bool> {
        unset_at_path(doc, segments)
    }
}

fn unset_at_path(fields: &mut Vec<(String, Value)>, segments: &[&str]) -> Result<bool> {
    let (&head, tail) = segments.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "UnsetOperator::apply_field",
            "empty update path",
        )
    })?;

    if tail.is_empty() {
        if let Some(pos) = fields.iter().position(|(k, _)| k == head) {
            fields.remove(pos);
            return Ok(true);
        }
        return Ok(false);
    }

    if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == head) {
        match existing {
            Value::Document(sub) => unset_at_path(sub, tail),
            Value::Array(_) => Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "UnsetOperator::apply_field",
                format!("array-position $unset through '{head}' is unsupported"),
            )),
            _ => Err(ErrorEnvelope::new(
                ErrorCategory::TypeMismatch,
                MODULE_ID,
                "UnsetOperator::apply_field",
                format!("cannot traverse non-document field '{head}' during $unset"),
            )),
        }
    } else {
        Ok(false)
    }
}
