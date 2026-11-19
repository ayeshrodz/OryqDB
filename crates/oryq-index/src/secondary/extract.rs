//! Single/nested-field key extraction and `RecordId`-suffixed secondary key construction.

use oryq_document::{DocumentView, ValueView};
use oryq_engine::PageSize;
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::descriptor::IndexDescriptor;
use crate::key_codec::{decode_record_id, encode_record_id, encode_value_key, max_index_key_bytes};

const MODULE_ID: &str = "index";
const RECORD_ID_BYTES: usize = 8;

/// Extracts the encoded value-key prefix for a single/nested-path secondary index from `doc`.
///
/// - When `desc.sparse == true` and the field is `Missing`, returns `Ok(None)` (omit entry).
/// - When `desc.sparse == false` and the field is `Missing`, encodes `ValueView::Null` (`0x05`).
/// - Leaves 8 bytes of headroom for the mandatory `RecordId` BE suffix.
pub fn extract_secondary_key_prefix(
    desc: &IndexDescriptor,
    doc: &DocumentView<'_>,
    page_size: PageSize,
) -> Result<Option<Vec<u8>>> {
    let field = desc.fields.first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "secondary::extract_secondary_key_prefix",
            "secondary index descriptor has no fields",
        )
    })?;

    let max_prefix = max_index_key_bytes(page_size).saturating_sub(RECORD_ID_BYTES);
    match doc.get_path(&field.path) {
        Some(val) => Ok(Some(encode_value_key(&val, field.order, max_prefix)?)),
        None => {
            if desc.sparse {
                Ok(None)
            } else {
                Ok(Some(encode_value_key(
                    &ValueView::Null,
                    field.order,
                    max_prefix,
                )?))
            }
        }
    }
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
