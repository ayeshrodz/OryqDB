//! Common update validation (`_id` immutability, ancestor/descendant conflict checks)
//! and before/after document normalization (`docs/document-model.md` §4.3 & §4.4).

pub mod inc;
pub mod pull;
pub mod push;
pub mod registry;
pub mod set;
pub mod unset;

use std::collections::{BTreeMap, BTreeSet};

use oryq_types::{BudgetHandle, ErrorCategory, ErrorEnvelope, Reservation, Result};

use crate::codec::encode_document;
use crate::compare::compare_values;
use crate::json::json_to_value;
use crate::patch::{encode_physical_patch, PatchEntry, PatchOp, NO_PREV_DELTA_SLOT};
use crate::path::FieldPath;
use crate::types::{DocumentBuf, Value};
use crate::view::DocumentView;

pub use registry::{UpdateOperator, UpdateRegistry};

const MODULE_ID: &str = "document";

/// One validated `(operator_name, field_path, operand)` step in an update specification.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStep {
    pub operator: String,
    pub path: FieldPath,
    pub operand: Value,
}

/// Validated update expression ready for pure evaluation against any [`DocumentView`].
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateSpec {
    steps: Vec<UpdateStep>,
}

impl UpdateSpec {
    /// Validates and constructs an [`UpdateSpec`] from explicit steps.
    ///
    /// Rejects `_id` mutations, empty updates, unknown operators, and any duplicate or
    /// ancestor/descendant path overlaps before any document is touched.
    pub fn from_steps(steps: Vec<UpdateStep>, registry: &UpdateRegistry) -> Result<Self> {
        if steps.is_empty() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "UpdateSpec::from_steps",
                "update expression must contain at least one operator field",
            ));
        }

        let mut seg_lists: Vec<Vec<&str>> = Vec::with_capacity(steps.len());
        for step in &steps {
            if registry.get(&step.operator).is_none() {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::UnsupportedFeature,
                    MODULE_ID,
                    "UpdateSpec::from_steps",
                    format!("unsupported update operator {:?}", step.operator),
                ));
            }
            let segs: Vec<&str> = step.path.segments().collect();
            if segs.first() == Some(&"_id") {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "UpdateSpec::from_steps",
                    "primary identifier field '_id' is immutable and cannot be modified",
                ));
            }
            seg_lists.push(segs);
        }

        // Check all pairs for duplicate or ancestor/descendant path conflicts.
        for i in 0..seg_lists.len() {
            for j in (i + 1)..seg_lists.len() {
                if paths_conflict(&seg_lists[i], &seg_lists[j]) {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "UpdateSpec::from_steps",
                        format!(
                            "conflicting update paths '{}' and '{}'",
                            steps[i].path.as_str(),
                            steps[j].path.as_str()
                        ),
                    ));
                }
            }
        }

        Ok(Self { steps })
    }

    /// Parses and validates a MongoDB-style JSON update document
    /// (e.g. `{"$set": {"a.b": 1}, "$inc": {"count": 2}}`).
    pub fn from_json_str(json: &str, registry: &UpdateRegistry) -> Result<Self> {
        let root: serde_json::Value = serde_json::from_str(json).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "UpdateSpec::from_json_str",
                format!("invalid JSON update document: {err}"),
            )
            .with_source(err)
        })?;
        let obj = root.as_object().ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "UpdateSpec::from_json_str",
                "update specification must be a JSON object",
            )
        })?;

        let mut steps = Vec::new();
        for (op_name, spec_val) in obj {
            if !op_name.starts_with('$') {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "UpdateSpec::from_json_str",
                    format!("update operator {op_name:?} must start with '$'"),
                ));
            }
            let fields_obj = spec_val.as_object().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "UpdateSpec::from_json_str",
                    format!("update operator {op_name:?} payload must be a JSON object"),
                )
            })?;
            for (raw_path, val_json) in fields_obj {
                let path = FieldPath::parse(raw_path)?;
                let operand = json_to_value(val_json)?;
                steps.push(UpdateStep {
                    operator: op_name.clone(),
                    path,
                    operand,
                });
            }
        }

        Self::from_steps(steps, registry)
    }

    pub fn steps(&self) -> &[UpdateStep] {
        &self.steps
    }
}

/// Pure normalized output of applying an [`UpdateSpec`] to a [`DocumentView`].
#[derive(Debug, Clone)]
pub struct NormalizedMutation {
    /// Complete post-update encoded `OryqDoc` buffer.
    pub updated_doc: DocumentBuf,
    /// Sorted unique top-level field names touched by this mutation.
    pub touched_top_level_fields: Vec<String>,
    /// State-normalized top-level physical delta patch bytes (`docs/file-format.md` §4.2).
    pub patch_bytes: Vec<u8>,
    /// 64-bit 2-hash bloom bitmask covering `touched_top_level_fields`.
    pub bloom_mask: u64,
    /// Whether any logical field value changed.
    pub modified: bool,
}

/// Evaluates `update` against `base` using `registry`, producing a [`NormalizedMutation`]
/// without touching any storage/page layer.
pub fn apply_update(
    base: &DocumentView<'_>,
    update: &UpdateSpec,
    registry: &UpdateRegistry,
    budget: Option<&BudgetHandle>,
) -> Result<(NormalizedMutation, Option<Reservation>)> {
    let mut fields: Vec<(String, Value)> = base
        .iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
        .collect();

    let mut touched_top = BTreeSet::new();
    let mut any_modified = false;

    for step in update.steps() {
        let op = registry.get(&step.operator).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "updates::apply_update",
                format!("unsupported update operator {:?}", step.operator),
            )
        })?;
        let segs: Vec<&str> = step.path.segments().collect();
        if op.apply_field(&mut fields, &segs, &step.operand)? {
            any_modified = true;
            touched_top.insert(segs[0].to_owned());
        }
    }

    // Build top-level physical patch entries for every touched top-level field.
    let field_map: BTreeMap<&str, &Value> = fields.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let mut patch_entries = Vec::with_capacity(touched_top.len());
    for top in &touched_top {
        if let Some(&val) = field_map.get(top.as_str()) {
            patch_entries.push(PatchEntry {
                top_level_field: top.clone(),
                op: PatchOp::ReplaceValue,
                value: Some(val.clone()),
            });
        } else {
            patch_entries.push(PatchEntry {
                top_level_field: top.clone(),
                op: PatchOp::TombstoneUnset,
                value: None,
            });
        }
    }

    let (patch_bytes, bloom_mask) = encode_physical_patch(NO_PREV_DELTA_SLOT, &patch_entries)?;
    let (updated_doc, reservation) = encode_document(&fields, budget)?;

    Ok((
        NormalizedMutation {
            updated_doc,
            touched_top_level_fields: touched_top.into_iter().collect(),
            patch_bytes,
            bloom_mask,
            modified: any_modified,
        },
        reservation,
    ))
}

/// Validates that a full replacement document preserves the existing document's `_id`
/// (`docs/document-model.md` §4.4).
pub fn validate_replacement_doc(
    existing: &DocumentView<'_>,
    replacement: &DocumentView<'_>,
) -> Result<()> {
    let old_id = existing.get_field("_id").ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "updates::validate_replacement_doc",
            "existing document is missing '_id'",
        )
    })?;
    let new_id = replacement.get_field("_id").ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "updates::validate_replacement_doc",
            "replacement document must include matching '_id'",
        )
    })?;

    if old_id.type_tag() != new_id.type_tag()
        || compare_values(&old_id, &new_id) != core::cmp::Ordering::Equal
    {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "updates::validate_replacement_doc",
            "primary identifier '_id' is immutable across document replacement",
        ));
    }
    Ok(())
}

fn paths_conflict(a: &[&str], b: &[&str]) -> bool {
    let min_len = a.len().min(b.len());
    a[..min_len] == b[..min_len]
}
