//! Pure stateless document compaction over a base `OryqDoc` and newest-first delta patches.
//!
//! Designed to be invoked directly by `oryq-core`'s `RecordCompactor` adapter for `oryq-engine`.

use std::collections::BTreeMap;

use oryq_types::{BudgetHandle, Reservation, Result};

use crate::codec::encode_document;
use crate::patch::normalize::{decode_physical_patch, PatchOp};
use crate::types::Value;
use crate::view::DocumentView;

/// Materializes a single compacted `OryqDoc` byte vector from `base_bytes` and
/// `patches_newest_first`, charging output bytes against `budget` when supplied.
pub fn compact_document(
    base_bytes: &[u8],
    patches_newest_first: &[&[u8]],
    budget: Option<&BudgetHandle>,
) -> Result<(Vec<u8>, Option<Reservation>)> {
    let base_view = DocumentView::from_bytes(base_bytes)?;
    let mut fields: BTreeMap<String, Value> = BTreeMap::new();
    for (k, v) in base_view.iter() {
        fields.insert(k.to_owned(), v.to_owned_value());
    }

    // Apply patches oldest-to-newest so newer patches shadow older ones.
    for &patch_bytes in patches_newest_first.iter().rev() {
        let patch = decode_physical_patch(patch_bytes)?;
        for entry in patch.entries {
            match entry.op {
                PatchOp::TombstoneUnset => {
                    fields.remove(&entry.top_level_field);
                }
                PatchOp::ReplaceValue => {
                    if let Some(val) = entry.value {
                        fields.insert(entry.top_level_field, val);
                    }
                }
            }
        }
    }

    let ordered: Vec<(String, Value)> = fields.into_iter().collect();
    let (doc_buf, reservation) = encode_document(&ordered, budget)?;
    Ok((doc_buf.as_bytes().to_vec(), reservation))
}
