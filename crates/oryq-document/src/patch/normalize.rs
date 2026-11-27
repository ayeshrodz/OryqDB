//! Physical state-normalized delta patch encoder/decoder (`docs/file-format.md` §4.2).
//!
//! Each patch entry targets a **top-level** field name only, sorted by
//! `(path_hash64: u64, utf8_path: &[u8])`. Replacement payloads are standalone
//! encoded `OryqDoc` values (wrapped as a 1-field `"v"` document or inline scalar
//! per type tag) so nested documents/arrays carry their own offset origins.

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};
use xxhash_rust::xxh3::xxh3_64;

use crate::codec::{encode_document, validate_document};
use crate::types::{Decimal128, ObjectId, OryqTypeTag, Uuid, Value};
use crate::view::DocumentView;

const MODULE_ID: &str = "document";

pub const NO_PREV_DELTA_SLOT: u16 = 0xFFFF;
const PATCH_HEADER_SIZE: usize = 4;
const PATCH_ENTRY_FIXED_SIZE: usize = 16; // hash(8) + op(1) + tag(1) + path_len(2) + val_len(4)

/// Patch operation kind (`docs/file-format.md` §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PatchOp {
    ReplaceValue = 0x01,
    TombstoneUnset = 0x02,
}

impl PatchOp {
    fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            0x01 => Ok(Self::ReplaceValue),
            0x02 => Ok(Self::TombstoneUnset),
            other => Err(corrupt(
                "PatchOp::from_u8",
                format!("unknown patch op_kind 0x{other:02x}"),
            )),
        }
    }
}

/// One top-level field replacement or tombstone inside a physical delta patch.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchEntry {
    pub top_level_field: String,
    pub op: PatchOp,
    pub value: Option<Value>,
}

/// Decoded state-normalized physical patch.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalPatch {
    pub prev_delta_slot: u16,
    pub entries: Vec<PatchEntry>,
    pub bloom_mask: u64,
}

/// Computes the 2-hash 64-bit bloom mask for a top-level field name (`docs/file-format.md` §5):
/// bit `hash & 63` OR bit `(hash >> 6) & 63`.
#[inline]
pub fn bloom_mask_for_top_level(name: &str) -> u64 {
    let h = xxh3_64(name.as_bytes());
    (1u64 << (h & 63)) | (1u64 << ((h >> 6) & 63))
}

/// Encodes top-level [`PatchEntry`] items into the normative binary delta patch format,
/// returning `(encoded_bytes, bloom_mask)`.
pub fn encode_physical_patch(
    prev_delta_slot: u16,
    entries: &[PatchEntry],
) -> Result<(Vec<u8>, u64)> {
    let count = u16::try_from(entries.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "patch::encode_physical_patch",
            "patch entry count exceeds u16::MAX",
        )
    })?;

    let mut keyed: Vec<(u64, &PatchEntry)> = Vec::with_capacity(entries.len());
    let mut bloom_mask = 0u64;

    for e in entries {
        if e.top_level_field.is_empty()
            || e.top_level_field.contains('.')
            || e.top_level_field.contains('\0')
        {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "patch::encode_physical_patch",
                format!("invalid top-level patch field name {:?}", e.top_level_field),
            ));
        }
        let h = xxh3_64(e.top_level_field.as_bytes());
        bloom_mask |= bloom_mask_for_top_level(&e.top_level_field);
        keyed.push((h, e));
    }

    keyed.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            a.1.top_level_field
                .as_bytes()
                .cmp(b.1.top_level_field.as_bytes())
        })
    });

    for w in keyed.windows(2) {
        if w[0].1.top_level_field == w[1].1.top_level_field {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "patch::encode_physical_patch",
                format!(
                    "duplicate top-level field {:?} in patch",
                    w[0].1.top_level_field
                ),
            ));
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(&prev_delta_slot.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    for (hash64, entry) in keyed {
        let path_bytes = entry.top_level_field.as_bytes();
        let path_len = u16::try_from(path_bytes.len()).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "patch::encode_physical_patch",
                "patch field name exceeds u16::MAX bytes",
            )
        })?;

        let (tag, payload) = match entry.op {
            PatchOp::TombstoneUnset => (OryqTypeTag::Null, Vec::new()),
            PatchOp::ReplaceValue => {
                let val = entry.value.as_ref().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "patch::encode_physical_patch",
                        "ReplaceValue entry requires a value",
                    )
                })?;
                encode_standalone_value(val)?
            }
        };

        let val_len = u32::try_from(payload.len()).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "patch::encode_physical_patch",
                "patch replacement value exceeds u32::MAX bytes",
            )
        })?;

        out.extend_from_slice(&hash64.to_le_bytes());
        out.push(entry.op as u8);
        out.push(tag.as_u8());
        out.extend_from_slice(&path_len.to_le_bytes());
        out.extend_from_slice(&val_len.to_le_bytes());
        out.extend_from_slice(path_bytes);
        out.extend_from_slice(&payload);
    }

    Ok((out, bloom_mask))
}

/// Strictly decodes and validates a binary state-normalized delta patch.
pub fn decode_physical_patch(bytes: &[u8]) -> Result<PhysicalPatch> {
    if bytes.len() < PATCH_HEADER_SIZE {
        return Err(corrupt(
            "patch::decode_physical_patch",
            "patch buffer shorter than 4-byte header",
        ));
    }
    let prev_delta_slot = u16::from_le_bytes([bytes[0], bytes[1]]);
    let entry_count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    let max_possible_entries = (bytes.len() - PATCH_HEADER_SIZE) / PATCH_ENTRY_FIXED_SIZE;
    if entry_count > max_possible_entries {
        return Err(corrupt(
            "patch::decode_physical_patch",
            format!(
                "patch entry_count ({entry_count}) exceeds buffer capacity ({max_possible_entries})"
            ),
        ));
    }

    let mut offset = PATCH_HEADER_SIZE;
    let mut entries = Vec::with_capacity(entry_count);
    let mut prev_key: Option<(u64, String)> = None;
    let mut bloom_mask = 0u64;

    for _ in 0..entry_count {
        if offset + PATCH_ENTRY_FIXED_SIZE > bytes.len() {
            return Err(corrupt(
                "patch::decode_physical_patch",
                "truncated patch entry header",
            ));
        }
        let stored_hash = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let op = PatchOp::from_u8(bytes[offset + 8])?;
        let tag = OryqTypeTag::from_nibble(bytes[offset + 9])?;
        let path_len = u16::from_le_bytes([bytes[offset + 10], bytes[offset + 11]]) as usize;
        let val_len =
            u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as usize;
        offset += PATCH_ENTRY_FIXED_SIZE;

        let path_end = offset
            .checked_add(path_len)
            .ok_or_else(|| corrupt("patch::decode_physical_patch", "path_len overflow"))?;
        let val_end = path_end
            .checked_add(val_len)
            .ok_or_else(|| corrupt("patch::decode_physical_patch", "val_len overflow"))?;
        if val_end > bytes.len() {
            return Err(corrupt(
                "patch::decode_physical_patch",
                "patch entry extends past buffer end",
            ));
        }

        let path_str = core::str::from_utf8(&bytes[offset..path_end]).map_err(|_| {
            corrupt(
                "patch::decode_physical_patch",
                "invalid UTF-8 in patch field name",
            )
        })?;
        if path_str.is_empty() || path_str.contains('.') || path_str.contains('\0') {
            return Err(corrupt(
                "patch::decode_physical_patch",
                "patch path must be a non-empty top-level field name",
            ));
        }
        let expected_hash = xxh3_64(path_str.as_bytes());
        if stored_hash != expected_hash {
            return Err(corrupt(
                "patch::decode_physical_patch",
                format!("patch path_hash64 mismatch for field {path_str:?}"),
            ));
        }
        if let Some((ph, ref ps)) = prev_key {
            if (stored_hash, path_str) <= (ph, ps.as_str()) {
                return Err(corrupt(
                    "patch::decode_physical_patch",
                    "patch entries are not strictly sorted by (path_hash64, path)",
                ));
            }
        }
        prev_key = Some((stored_hash, path_str.to_owned()));
        bloom_mask |= bloom_mask_for_top_level(path_str);

        let payload = &bytes[path_end..val_end];
        let value = match op {
            PatchOp::TombstoneUnset => {
                if tag != OryqTypeTag::Null || !payload.is_empty() {
                    return Err(corrupt(
                        "patch::decode_physical_patch",
                        "TombstoneUnset must have Null tag and zero payload length",
                    ));
                }
                None
            }
            PatchOp::ReplaceValue => Some(decode_standalone_value(tag, payload)?),
        };

        entries.push(PatchEntry {
            top_level_field: path_str.to_owned(),
            op,
            value,
        });
        offset = val_end;
    }

    if offset != bytes.len() {
        return Err(corrupt(
            "patch::decode_physical_patch",
            "trailing unconsumed bytes after patch entries",
        ));
    }

    Ok(PhysicalPatch {
        prev_delta_slot,
        entries,
        bloom_mask,
    })
}

fn encode_standalone_value(val: &Value) -> Result<(OryqTypeTag, Vec<u8>)> {
    match val {
        Value::Null => Ok((OryqTypeTag::Null, Vec::new())),
        Value::Bool(false) => Ok((OryqTypeTag::BoolFalse, Vec::new())),
        Value::Bool(true) => Ok((OryqTypeTag::BoolTrue, Vec::new())),
        Value::Int64(v) => Ok((OryqTypeTag::Int64, v.to_le_bytes().to_vec())),
        Value::Float64(v) => Ok((OryqTypeTag::Float64, v.to_le_bytes().to_vec())),
        Value::DateTimeUtcMs(v) => Ok((OryqTypeTag::DateTimeUtcMs, v.to_le_bytes().to_vec())),
        Value::String(s) => Ok((OryqTypeTag::String, s.as_bytes().to_vec())),
        Value::Binary { subtype, bytes } => {
            let mut out = Vec::with_capacity(1 + bytes.len());
            out.push(*subtype);
            out.extend_from_slice(bytes);
            Ok((OryqTypeTag::Binary, out))
        }
        Value::ObjectId(id) => Ok((OryqTypeTag::ObjectId, id.as_bytes().to_vec())),
        Value::Uuid(id) => Ok((OryqTypeTag::Uuid, id.as_bytes().to_vec())),
        Value::Decimal128(d) => Ok((OryqTypeTag::Decimal128, d.to_le_bytes().to_vec())),
        Value::Document(fields) => {
            let (doc, _) = encode_document(fields, None)?;
            Ok((OryqTypeTag::Document, doc.as_bytes().to_vec()))
        }
        Value::Array(_) => {
            // Wrap array in a single-field `"a"` OryqDoc so its offset origin is self-contained.
            let (doc, _) = encode_document(&[("a".to_owned(), val.clone())], None)?;
            Ok((OryqTypeTag::Array, doc.as_bytes().to_vec()))
        }
    }
}

fn decode_standalone_value(tag: OryqTypeTag, payload: &[u8]) -> Result<Value> {
    match tag {
        OryqTypeTag::Null => {
            if !payload.is_empty() {
                return Err(corrupt("patch::decode_standalone_value", "Null must be 0B"));
            }
            Ok(Value::Null)
        }
        OryqTypeTag::BoolFalse => {
            if !payload.is_empty() {
                return Err(corrupt("patch::decode_standalone_value", "Bool must be 0B"));
            }
            Ok(Value::Bool(false))
        }
        OryqTypeTag::BoolTrue => {
            if !payload.is_empty() {
                return Err(corrupt("patch::decode_standalone_value", "Bool must be 0B"));
            }
            Ok(Value::Bool(true))
        }
        OryqTypeTag::Int64 => {
            let arr: [u8; 8] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "Int64 must be 8B"))?;
            Ok(Value::Int64(i64::from_le_bytes(arr)))
        }
        OryqTypeTag::Float64 => {
            let arr: [u8; 8] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "Float64 must be 8B"))?;
            Ok(Value::Float64(f64::from_le_bytes(arr)))
        }
        OryqTypeTag::DateTimeUtcMs => {
            let arr: [u8; 8] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "DateTime must be 8B"))?;
            Ok(Value::DateTimeUtcMs(i64::from_le_bytes(arr)))
        }
        OryqTypeTag::String => {
            let s = core::str::from_utf8(payload)
                .map_err(|_| corrupt("patch::decode_standalone_value", "invalid UTF-8 string"))?;
            Ok(Value::String(s.to_owned()))
        }
        OryqTypeTag::Binary => {
            if payload.is_empty() {
                return Err(corrupt(
                    "patch::decode_standalone_value",
                    "Binary missing subtype byte",
                ));
            }
            Ok(Value::Binary {
                subtype: payload[0],
                bytes: payload[1..].to_vec(),
            })
        }
        OryqTypeTag::ObjectId => {
            let arr: [u8; 12] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "ObjectId must be 12B"))?;
            Ok(Value::ObjectId(ObjectId::from_bytes(arr)))
        }
        OryqTypeTag::Uuid => {
            let arr: [u8; 16] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "Uuid must be 16B"))?;
            Ok(Value::Uuid(Uuid::from_bytes(arr)))
        }
        OryqTypeTag::Decimal128 => {
            let arr: [u8; 16] = payload
                .try_into()
                .map_err(|_| corrupt("patch::decode_standalone_value", "Decimal128 must be 16B"))?;
            Ok(Value::Decimal128(Decimal128::from_le_bytes(arr)))
        }
        OryqTypeTag::Document => {
            validate_document(payload)?;
            let view = DocumentView::from_bytes(payload)?;
            Ok(Value::Document(
                view.iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
                    .collect(),
            ))
        }
        OryqTypeTag::Array => {
            validate_document(payload)?;
            let view = DocumentView::from_bytes(payload)?;
            let val = view.get_field("a").ok_or_else(|| {
                corrupt(
                    "patch::decode_standalone_value",
                    "array patch wrapper missing field 'a'",
                )
            })?;
            Ok(val.to_owned_value())
        }
    }
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
