//! Single/nested-field key extraction (with multikey array and multi-hop dotted-path expansion)
//! and `RecordId`-suffixed secondary key construction.

use std::collections::BTreeSet;

use oryq_document::{DocumentView, FieldPath, ValueView};
use oryq_engine::PageSize;
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::descriptor::IndexDescriptor;
use crate::key_codec::{decode_record_id, encode_record_id, encode_value_key, max_index_key_bytes};

const MODULE_ID: &str = "index";
const RECORD_ID_BYTES: usize = 8;

/// Outcome of resolving a dotted [`FieldPath`] across documents and intermediate arrays.
pub(crate) struct ExtractedPath<'a> {
    /// All resolved leaf values (with arrays expanded one level at the final segment).
    pub values: Vec<ValueView<'a>>,
    /// True if any segment along the path encountered an array.
    pub encountered_array: bool,
    /// If the path resolved directly to an empty array `[]`, holds that empty array view so
    /// callers can index the empty-array sentinel (`RANK_ARRAY || 0x00`) and enforce uniqueness.
    pub empty_array: Option<ValueView<'a>>,
}

/// Resolves `path` against `doc`, recursively traversing through intermediate arrays
/// (e.g. `items.meta.tag`) and expanding leaf arrays for multikey indexing.
pub(crate) fn extract_path_values<'a>(
    doc: &DocumentView<'a>,
    path: &FieldPath,
) -> ExtractedPath<'a> {
    let segs: Vec<&str> = path.segments().collect();
    let mut out = ExtractedPath {
        values: Vec::new(),
        encountered_array: false,
        empty_array: None,
    };
    if segs.is_empty() {
        return out;
    }
    collect_from_doc(doc, &segs, &mut out);
    out
}

fn collect_from_doc<'a>(doc: &DocumentView<'a>, segs: &[&str], out: &mut ExtractedPath<'a>) {
    let Some(val) = doc.get_field(segs[0]) else {
        return;
    };
    collect_from_value(val, &segs[1..], out);
}

fn collect_from_value<'a>(val: ValueView<'a>, rest: &[&str], out: &mut ExtractedPath<'a>) {
    if rest.is_empty() {
        match val {
            ValueView::Array(arr) => {
                out.encountered_array = true;
                if arr.is_empty() {
                    if out.empty_array.is_none() {
                        out.empty_array = Some(ValueView::Array(arr));
                    }
                } else {
                    for el in arr.iter() {
                        out.values.push(el);
                    }
                }
            }
            scalar => out.values.push(scalar),
        }
        return;
    }

    match val {
        ValueView::Document(sub) => collect_from_doc(&sub, rest, out),
        ValueView::Array(arr) => {
            if let Ok(idx) = rest[0].parse::<usize>() {
                if let Some(el) = arr.get(idx) {
                    collect_from_value(el, &rest[1..], out);
                }
            } else {
                out.encountered_array = true;
                for el in arr.iter() {
                    if let ValueView::Document(sub) = el {
                        collect_from_doc(&sub, rest, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Extracts all unique encoded value-key prefixes for a single/nested-path secondary index from `doc`.
///
/// - Expands array values and multi-hop array dotted paths (`items.meta.tag`) for multikey indexing.
/// - Supports numeric array index segments (`items.0.tag`, `tags.0`) matching `oryq-query::resolve_path`.
/// - Deduplicates repeated element keys within the same document via `BTreeSet`.
/// - Encodes an empty array `[]` using the canonical empty-array key so `unique: true` is enforced.
/// - When `desc.sparse == true` and the field is `Missing`, returns an empty `Vec`.
/// - When `desc.sparse == false` and the field is `Missing`, encodes `ValueView::Null` (`0x05`).
pub fn extract_secondary_key_prefixes(
    desc: &IndexDescriptor,
    doc: &DocumentView<'_>,
    page_size: PageSize,
) -> Result<Vec<Vec<u8>>> {
    let field = desc.fields.first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "secondary::extract_secondary_key_prefixes",
            "secondary index descriptor has no fields",
        )
    })?;

    let max_prefix = max_index_key_bytes(page_size).saturating_sub(RECORD_ID_BYTES);
    let extracted = extract_path_values(doc, &field.path);

    if extracted.values.is_empty() && extracted.empty_array.is_none() {
        if desc.sparse {
            return Ok(Vec::new());
        }
        return Ok(vec![encode_value_key(
            &ValueView::Null,
            field.order,
            max_prefix,
        )?]);
    }

    let mut dedup = BTreeSet::new();
    for val in &extracted.values {
        dedup.insert(encode_value_key(val, field.order, max_prefix)?);
    }
    if let Some(empty_arr) = extracted.empty_array {
        dedup.insert(encode_value_key(&empty_arr, field.order, max_prefix)?);
    }
    Ok(dedup.into_iter().collect())
}

/// Extracts the first encoded value-key prefix for a single/nested-path secondary index from `doc`.
///
/// Retained for backwards compatibility; prefer [`extract_secondary_key_prefixes`] for multikey support.
pub fn extract_secondary_key_prefix(
    desc: &IndexDescriptor,
    doc: &DocumentView<'_>,
    page_size: PageSize,
) -> Result<Option<Vec<u8>>> {
    Ok(extract_secondary_key_prefixes(desc, doc, page_size)?
        .into_iter()
        .next())
}

/// Appends the 8-byte big-endian `RecordId` tie-breaker to a value-key prefix.
pub fn build_secondary_full_key(prefix: &[u8], record_id: RecordId) -> Vec<u8> {
    let mut full = Vec::with_capacity(prefix.len() + RECORD_ID_BYTES);
    full.extend_from_slice(prefix);
    full.extend_from_slice(&encode_record_id(record_id));
    full
}

/// Splits a secondary index B+ tree key into `(value_prefix, RecordId)`.
pub fn split_record_id_suffix(full_key: &[u8]) -> Result<(&[u8], RecordId)> {
    if full_key.len() < RECORD_ID_BYTES {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "secondary::split_record_id_suffix",
            format!(
                "secondary index key ({} bytes) shorter than 8-byte RecordId suffix",
                full_key.len()
            ),
        ));
    }
    let split = full_key.len() - RECORD_ID_BYTES;
    let rid = decode_record_id(&full_key[split..])?;
    Ok((&full_key[..split], rid))
}
