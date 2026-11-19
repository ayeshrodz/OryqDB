//! Bloom-filtered newest-first field resolution across a base `DocumentView` and
//! a chain of state-normalized physical patches (`docs/document-model.md` §4.4).

use oryq_types::Result;

use crate::patch::normalize::{bloom_mask_for_top_level, decode_physical_patch, PatchOp};
use crate::path::FieldPath;
use crate::types::Value;
use crate::view::DocumentView;

/// Resolves `path` against `base` shadowed by `patches_newest_first` (guarded by `bloom_mask`).
///
/// - Checks `bloom_mask` against the top-level path component in `O(1)`.
/// - On bloom miss, falls back directly to `base.get_path(path)`.
/// - On bloom hit, scans `patches_newest_first` from newest to oldest; the first matching
///   top-level replacement or tombstone wins, and child segments traverse that winning value.
pub fn resolve_path_with_patches(
    base: &DocumentView<'_>,
    patches_newest_first: &[&[u8]],
    bloom_mask: u64,
    path: &FieldPath,
) -> Result<Option<Value>> {
    let mut segs = path.segments();
    let Some(top) = segs.next() else {
        return Ok(None);
    };
    let rest: Vec<&str> = segs.collect();

    let needed_bits = bloom_mask_for_top_level(top);
    if !patches_newest_first.is_empty() && (bloom_mask & needed_bits) == needed_bits {
        for &patch_bytes in patches_newest_first {
            let patch = decode_physical_patch(patch_bytes)?;
            if let Some(entry) = patch.entries.iter().find(|e| e.top_level_field == top) {
                return match entry.op {
                    PatchOp::TombstoneUnset => Ok(None),
                    PatchOp::ReplaceValue => {
                        let Some(val) = entry.value.as_ref() else {
                            return Ok(None);
                        };
                        Ok(traverse_owned_value(val, &rest).cloned())
                    }
                };
            }
        }
    }

    Ok(base.get_path(path).map(|v| v.to_owned_value()))
}

fn traverse_owned_value<'a>(mut cur: &'a Value, rest: &[&str]) -> Option<&'a Value> {
    for &seg in rest {
        match cur {
            Value::Document(fields) => {
                cur = fields.iter().find(|(k, _)| k == seg).map(|(_, v)| v)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}
