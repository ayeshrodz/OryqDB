//! `OryqDoc` v1 binary encoder and strict structural validator.
//!
//! Layout follows `docs/file-format.md` Section 4.2:
//! - 8-byte header: `[version: u8 = 0x01][flags: u8][field_count: u16 LE][total_byte_len: u32 LE]`
//! - `12 * field_count` sorted directory: `[name_hash_prefix: u32 BE][name_offset: u32 LE][val_descriptor: u32 LE]`
//! - Key & value heap strictly contained within `[8 + 12 * field_count .. total_byte_len]`

use std::sync::Arc;

use oryq_types::{BudgetHandle, ErrorCategory, ErrorEnvelope, Reservation, Result};
use xxhash_rust::xxh3::xxh3_64;

use crate::array::ArrayView;
use crate::types::{Decimal128, DocumentBuf, ObjectId, OryqTypeTag, Uuid, Value, ValueView};
use crate::view::DocumentView;

const MODULE_ID: &str = "document";

pub const ORYQ_DOC_VERSION: u8 = 0x01;
pub const DOC_HEADER_SIZE: usize = 8;
pub const DIR_ENTRY_SIZE: usize = 12;
pub const MAX_NESTING_DEPTH: usize = 128;
pub const MAX_DESCRIPTOR_OFFSET: usize = (1 << 28) - 1;

const FLAG_HAS_EXTENDED_TYPES: u8 = 0x01;
const FLAG_HAS_OVERFLOW_POINTERS: u8 = 0x02;

/// Computes the 32-bit hash prefix (high 32 bits of `XXH3_64`) for a field name.
#[inline]
pub fn field_name_hash_prefix(name: &[u8]) -> u32 {
    (xxh3_64(name) >> 32) as u32
}

#[inline]
pub(crate) fn pack_descriptor(tag: OryqTypeTag, offset: usize) -> Result<u32> {
    if offset > MAX_DESCRIPTOR_OFFSET {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "codec::pack_descriptor",
            format!("offset {offset} exceeds 28-bit OryqDoc descriptor limit"),
        ));
    }
    Ok(((tag.as_u8() as u32) << 28) | (offset as u32))
}

#[inline]
pub(crate) fn unpack_descriptor(desc: u32) -> (u8, usize) {
    let nibble = (desc >> 28) as u8;
    let offset = (desc & 0x0FFF_FFFF) as usize;
    (nibble, offset)
}

/// Encodes a slice of `(field_name, Value)` pairs into a validated [`DocumentBuf`].
///
/// Charges the final byte length against `budget` when supplied.
pub fn encode_document(
    fields: &[(String, Value)],
    budget: Option<&BudgetHandle>,
) -> Result<(DocumentBuf, Option<Reservation>)> {
    let bytes = encode_doc_bytes(fields, 0)?;
    let reservation = match budget {
        Some(b) => Some(b.reserve(bytes.len())?),
        None => None,
    };
    let arc: Arc<[u8]> = Arc::from(bytes);
    Ok((DocumentBuf::from_validated_arc(arc), reservation))
}

fn encode_doc_bytes(fields: &[(String, Value)], depth: usize) -> Result<Vec<u8>> {
    if depth > MAX_NESTING_DEPTH {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "codec::encode_doc_bytes",
            format!("document nesting depth exceeds maximum {MAX_NESTING_DEPTH}"),
        ));
    }
    let field_count = u16::try_from(fields.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "codec::encode_doc_bytes",
            format!("document field count {} exceeds u16::MAX", fields.len()),
        )
    })?;

    // Sort indices by (name_hash_prefix, utf8_name) and reject duplicates or NUL bytes.
    let mut sorted: Vec<(u32, &str, &Value)> = Vec::with_capacity(fields.len());
    for (name, val) in fields {
        if name.as_bytes().contains(&0) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "codec::encode_doc_bytes",
                "document field name must not contain NUL bytes",
            ));
        }
        if name.len() > u16::MAX as usize {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "codec::encode_doc_bytes",
                format!("field name length {} exceeds u16::MAX", name.len()),
            ));
        }
        sorted.push((field_name_hash_prefix(name.as_bytes()), name.as_str(), val));
    }
    sorted.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.as_bytes().cmp(b.1.as_bytes()))
    });
    for pair in sorted.windows(2) {
        if pair[0].1 == pair[1].1 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "codec::encode_doc_bytes",
                format!("duplicate field name `{}` in document", pair[0].1),
            ));
        }
    }

    let heap_start = DOC_HEADER_SIZE + (fields.len() * DIR_ENTRY_SIZE);
    let mut out = vec![0u8; heap_start];
    let mut flags: u8 = 0;

    for (idx, (hash_prefix, name, val)) in sorted.into_iter().enumerate() {
        let name_offset = out.len();
        let name_offset_u32 = u32::try_from(name_offset).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "codec::encode_doc_bytes",
                "document name offset exceeds u32::MAX",
            )
        })?;
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());

        let (tag, val_offset, ext) = encode_value_into(&mut out, val, depth)?;
        if ext {
            flags |= FLAG_HAS_EXTENDED_TYPES;
        }
        let desc = pack_descriptor(tag, val_offset)?;

        let dir_off = DOC_HEADER_SIZE + idx * DIR_ENTRY_SIZE;
        out[dir_off..dir_off + 4].copy_from_slice(&hash_prefix.to_be_bytes());
        out[dir_off + 4..dir_off + 8].copy_from_slice(&name_offset_u32.to_le_bytes());
        out[dir_off + 8..dir_off + 12].copy_from_slice(&desc.to_le_bytes());
    }

    if out.len() > MAX_DESCRIPTOR_OFFSET + 1 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "codec::encode_doc_bytes",
            format!(
                "encoded document length {} exceeds 256 MiB limit",
                out.len()
            ),
        ));
    }

    let total_len_u32 = out.len() as u32;
    out[0] = ORYQ_DOC_VERSION;
    out[1] = flags;
    out[2..4].copy_from_slice(&field_count.to_le_bytes());
    out[4..8].copy_from_slice(&total_len_u32.to_le_bytes());
    Ok(out)
}

fn encode_array_bytes(items: &[Value], depth: usize) -> Result<(Vec<u8>, bool)> {
    if depth > MAX_NESTING_DEPTH {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "codec::encode_array_bytes",
            format!("array nesting depth exceeds maximum {MAX_NESTING_DEPTH}"),
        ));
    }
    let elem_count = u32::try_from(items.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "codec::encode_array_bytes",
            "array length exceeds u32::MAX",
        )
    })?;

    let heap_start = 8usize
        .checked_add(items.len().checked_mul(4).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "codec::encode_array_bytes",
                "array descriptor table size overflow",
            )
        })?)
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "codec::encode_array_bytes",
                "array header size overflow",
            )
        })?;

    let mut out = vec![0u8; heap_start];
    let mut has_ext = false;

    for (idx, val) in items.iter().enumerate() {
        let (tag, offset, ext) = encode_value_into(&mut out, val, depth)?;
        has_ext |= ext;
        let desc = pack_descriptor(tag, offset)?;
        let desc_off = 8 + idx * 4;
        out[desc_off..desc_off + 4].copy_from_slice(&desc.to_le_bytes());
    }

    if out.len() > MAX_DESCRIPTOR_OFFSET + 1 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "codec::encode_array_bytes",
            "encoded array exceeds 256 MiB limit",
        ));
    }

    let total_len = out.len() as u32;
    out[0..4].copy_from_slice(&elem_count.to_le_bytes());
    out[4..8].copy_from_slice(&total_len.to_le_bytes());
    Ok((out, has_ext))
}

fn encode_value_into(
    out: &mut Vec<u8>,
    val: &Value,
    depth: usize,
) -> Result<(OryqTypeTag, usize, bool)> {
    match val {
        Value::Null => Ok((OryqTypeTag::Null, 0, false)),
        Value::Bool(false) => Ok((OryqTypeTag::BoolFalse, 0, false)),
        Value::Bool(true) => Ok((OryqTypeTag::BoolTrue, 0, false)),
        Value::Int64(v) => {
            let off = out.len();
            out.extend_from_slice(&v.to_le_bytes());
            Ok((OryqTypeTag::Int64, off, false))
        }
        Value::Float64(v) => {
            let off = out.len();
            out.extend_from_slice(&v.to_le_bytes());
            Ok((OryqTypeTag::Float64, off, false))
        }
        Value::DateTimeUtcMs(ms) => {
            let off = out.len();
            out.extend_from_slice(&ms.to_le_bytes());
            Ok((OryqTypeTag::DateTimeUtcMs, off, true))
        }
        Value::String(s) => {
            let len = u32::try_from(s.len()).map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "codec::encode_value_into",
                    "string byte length exceeds u32::MAX",
                )
            })?;
            let off = out.len();
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(s.as_bytes());
            Ok((OryqTypeTag::String, off, false))
        }
        Value::Binary { subtype, bytes } => {
            let len = u32::try_from(bytes.len()).map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "codec::encode_value_into",
                    "binary payload length exceeds u32::MAX",
                )
            })?;
            let off = out.len();
            out.extend_from_slice(&len.to_le_bytes());
            out.push(*subtype);
            out.extend_from_slice(bytes);
            Ok((OryqTypeTag::Binary, off, true))
        }
        Value::ObjectId(id) => {
            let off = out.len();
            out.extend_from_slice(id.as_bytes());
            Ok((OryqTypeTag::ObjectId, off, true))
        }
        Value::Uuid(id) => {
            let off = out.len();
            out.extend_from_slice(id.as_bytes());
            Ok((OryqTypeTag::Uuid, off, true))
        }
        Value::Decimal128(dec) => {
            let off = out.len();
            out.extend_from_slice(&dec.to_le_bytes());
            Ok((OryqTypeTag::Decimal128, off, true))
        }
        Value::Array(items) => {
            let (arr_bytes, ext) = encode_array_bytes(items, depth + 1)?;
            let off = out.len();
            out.extend_from_slice(&arr_bytes);
            Ok((OryqTypeTag::Array, off, ext))
        }
        Value::Document(sub_fields) => {
            let sub_bytes = encode_doc_bytes(sub_fields, depth + 1)?;
            let ext = (sub_bytes[1] & FLAG_HAS_EXTENDED_TYPES) != 0;
            let off = out.len();
            out.extend_from_slice(&sub_bytes);
            Ok((OryqTypeTag::Document, off, ext))
        }
    }
}

/// Strictly validates untrusted `OryqDoc` v1 bytes (bounds, UTF-8, directory sort order,
/// non-overlapping heap payloads, tag validity, and recursion depth <= 128).
pub fn validate_document(bytes: &[u8]) -> Result<()> {
    validate_doc_inner(bytes, 0)
}

fn validate_doc_inner(bytes: &[u8], depth: usize) -> Result<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(corrupt(
            "codec::validate_doc_inner",
            format!("document nesting depth exceeds maximum {MAX_NESTING_DEPTH}"),
        ));
    }
    if bytes.len() < DOC_HEADER_SIZE {
        return Err(corrupt(
            "codec::validate_doc_inner",
            format!("document shorter than {DOC_HEADER_SIZE}-byte header"),
        ));
    }
    if bytes[0] != ORYQ_DOC_VERSION {
        return Err(corrupt(
            "codec::validate_doc_inner",
            format!("unsupported OryqDoc version 0x{:02X}", bytes[0]),
        ));
    }
    let flags = bytes[1];
    if (flags & FLAG_HAS_OVERFLOW_POINTERS) != 0 {
        return Err(corrupt(
            "codec::validate_doc_inner",
            "exported OryqDoc must not set HAS_OVERFLOW_POINTERS flag",
        ));
    }
    if (flags & !(FLAG_HAS_EXTENDED_TYPES | FLAG_HAS_OVERFLOW_POINTERS)) != 0 {
        return Err(corrupt(
            "codec::validate_doc_inner",
            format!("unknown OryqDoc header flags 0x{flags:02X}"),
        ));
    }

    let field_count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    let total_byte_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if total_byte_len != bytes.len() {
        return Err(corrupt(
            "codec::validate_doc_inner",
            format!(
                "OryqDoc total_byte_len ({total_byte_len}) does not match slice length ({})",
                bytes.len()
            ),
        ));
    }

    let heap_start = DOC_HEADER_SIZE
        .checked_add(
            field_count
                .checked_mul(DIR_ENTRY_SIZE)
                .ok_or_else(|| corrupt("codec::validate_doc_inner", "directory size overflow"))?,
        )
        .ok_or_else(|| corrupt("codec::validate_doc_inner", "directory end overflow"))?;
    if heap_start > bytes.len() {
        return Err(corrupt(
            "codec::validate_doc_inner",
            "directory extends past end of document",
        ));
    }

    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(field_count * 2);
    let mut prev_key: Option<(u32, &str)> = None;

    for i in 0..field_count {
        let entry_off = DOC_HEADER_SIZE + i * DIR_ENTRY_SIZE;
        let hash_prefix = u32::from_be_bytes([
            bytes[entry_off],
            bytes[entry_off + 1],
            bytes[entry_off + 2],
            bytes[entry_off + 3],
        ]);
        let name_off = u32::from_le_bytes([
            bytes[entry_off + 4],
            bytes[entry_off + 5],
            bytes[entry_off + 6],
            bytes[entry_off + 7],
        ]) as usize;
        let val_desc = u32::from_le_bytes([
            bytes[entry_off + 8],
            bytes[entry_off + 9],
            bytes[entry_off + 10],
            bytes[entry_off + 11],
        ]);

        if name_off < heap_start || name_off.checked_add(2).map_or(true, |e| e > bytes.len()) {
            return Err(corrupt(
                "codec::validate_doc_inner",
                format!("field name offset {name_off} out of heap bounds"),
            ));
        }
        let name_len = u16::from_le_bytes([bytes[name_off], bytes[name_off + 1]]) as usize;
        let name_end = name_off
            .checked_add(2)
            .and_then(|s| s.checked_add(name_len))
            .ok_or_else(|| corrupt("codec::validate_doc_inner", "name slice overflow"))?;
        if name_end > bytes.len() {
            return Err(corrupt(
                "codec::validate_doc_inner",
                "field name extends past end of document",
            ));
        }
        let name_bytes = &bytes[name_off + 2..name_end];
        if name_bytes.contains(&0) {
            return Err(corrupt(
                "codec::validate_doc_inner",
                "field name contains NUL byte",
            ));
        }
        let name_str = core::str::from_utf8(name_bytes)
            .map_err(|_| corrupt("codec::validate_doc_inner", "field name is not valid UTF-8"))?;

        let expected_hash = field_name_hash_prefix(name_bytes);
        if hash_prefix != expected_hash {
            return Err(corrupt(
                "codec::validate_doc_inner",
                format!(
                    "field `{name_str}` hash prefix 0x{hash_prefix:08X} != expected 0x{expected_hash:08X}"
                ),
            ));
        }
        if let Some((prev_h, prev_n)) = prev_key {
            let ord = prev_h
                .cmp(&hash_prefix)
                .then_with(|| prev_n.as_bytes().cmp(name_bytes));
            if ord != core::cmp::Ordering::Less {
                return Err(corrupt(
                    "codec::validate_doc_inner",
                    format!("field directory not strictly sorted at `{name_str}`"),
                ));
            }
        }
        prev_key = Some((hash_prefix, name_str));
        spans.push((name_off, name_end));

        if let Some(val_span) = validate_value_at_descriptor(bytes, heap_start, val_desc, depth)? {
            spans.push(val_span);
        }
    }

    verify_non_overlapping(&mut spans)?;
    Ok(())
}

fn validate_array_inner(bytes: &[u8], depth: usize) -> Result<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(corrupt(
            "codec::validate_array_inner",
            format!("array nesting depth exceeds maximum {MAX_NESTING_DEPTH}"),
        ));
    }
    if bytes.len() < 8 {
        return Err(corrupt(
            "codec::validate_array_inner",
            "array shorter than 8-byte header",
        ));
    }
    let elem_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let byte_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if byte_len != bytes.len() {
        return Err(corrupt(
            "codec::validate_array_inner",
            "array byte_len does not match slice length",
        ));
    }
    let heap_start = 8usize
        .checked_add(
            elem_count
                .checked_mul(4)
                .ok_or_else(|| corrupt("codec::validate_array_inner", "array table overflow"))?,
        )
        .ok_or_else(|| corrupt("codec::validate_array_inner", "array heap start overflow"))?;
    if heap_start > bytes.len() {
        return Err(corrupt(
            "codec::validate_array_inner",
            "array descriptor table extends past end of array",
        ));
    }

    let mut spans = Vec::with_capacity(elem_count);
    for i in 0..elem_count {
        let off = 8 + i * 4;
        let desc = u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        if let Some(span) = validate_value_at_descriptor(bytes, heap_start, desc, depth)? {
            spans.push(span);
        }
    }

    verify_non_overlapping(&mut spans)?;
    Ok(())
}

fn validate_value_at_descriptor(
    container: &[u8],
    heap_start: usize,
    desc: u32,
    depth: usize,
) -> Result<Option<(usize, usize)>> {
    let (nibble, off) = unpack_descriptor(desc);
    let tag = OryqTypeTag::from_nibble(nibble)?;
    if tag.is_inline() {
        return Ok(None);
    }
    if off < heap_start || off >= container.len() {
        return Err(corrupt(
            "codec::validate_value",
            format!(
                "value offset {off} outside container heap [{heap_start}..{})",
                container.len()
            ),
        ));
    }
    let end = match tag {
        OryqTypeTag::Null | OryqTypeTag::BoolFalse | OryqTypeTag::BoolTrue => unreachable!(),
        OryqTypeTag::Int64 | OryqTypeTag::Float64 | OryqTypeTag::DateTimeUtcMs => {
            checked_end(off, 8, container.len())?
        }
        OryqTypeTag::ObjectId => checked_end(off, 12, container.len())?,
        OryqTypeTag::Uuid | OryqTypeTag::Decimal128 => checked_end(off, 16, container.len())?,
        OryqTypeTag::String => {
            let hdr_end = checked_end(off, 4, container.len())?;
            let str_len = u32::from_le_bytes([
                container[off],
                container[off + 1],
                container[off + 2],
                container[off + 3],
            ]) as usize;
            let end = checked_end(hdr_end, str_len, container.len())?;
            core::str::from_utf8(&container[hdr_end..end])
                .map_err(|_| corrupt("codec::validate_value", "string value is not valid UTF-8"))?;
            end
        }
        OryqTypeTag::Binary => {
            let hdr_end = checked_end(off, 5, container.len())?;
            let bin_len = u32::from_le_bytes([
                container[off],
                container[off + 1],
                container[off + 2],
                container[off + 3],
            ]) as usize;
            checked_end(hdr_end, bin_len, container.len())?
        }
        OryqTypeTag::Array => {
            let _hdr_end = checked_end(off, 8, container.len())?;
            let arr_len = u32::from_le_bytes([
                container[off + 4],
                container[off + 5],
                container[off + 6],
                container[off + 7],
            ]) as usize;
            let end = checked_end(off, arr_len, container.len())?;
            validate_array_inner(&container[off..end], depth + 1)?;
            end
        }
        OryqTypeTag::Document => {
            let _hdr_end = checked_end(off, DOC_HEADER_SIZE, container.len())?;
            let sub_len = u32::from_le_bytes([
                container[off + 4],
                container[off + 5],
                container[off + 6],
                container[off + 7],
            ]) as usize;
            let end = checked_end(off, sub_len, container.len())?;
            validate_doc_inner(&container[off..end], depth + 1)?;
            end
        }
    };
    Ok(Some((off, end)))
}

fn checked_end(start: usize, len: usize, limit: usize) -> Result<usize> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| corrupt("codec::checked_end", "payload slice arithmetic overflow"))?;
    if end > limit {
        return Err(corrupt(
            "codec::checked_end",
            format!("payload slice [{start}..{end}) exceeds container length {limit}"),
        ));
    }
    Ok(end)
}

fn verify_non_overlapping(spans: &mut [(usize, usize)]) -> Result<()> {
    spans.sort_unstable_by_key(|&(start, _)| start);
    for pair in spans.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(corrupt(
                "codec::verify_non_overlapping",
                format!(
                    "overlapping payload slices [{:?} and {:?}]",
                    pair[0], pair[1]
                ),
            ));
        }
    }
    Ok(())
}

/// Reads a validated field name slice from a directory entry at `entry_off`.
pub(crate) fn read_entry_name(bytes: &[u8], entry_off: usize) -> &str {
    let name_off = u32::from_le_bytes([
        bytes[entry_off + 4],
        bytes[entry_off + 5],
        bytes[entry_off + 6],
        bytes[entry_off + 7],
    ]) as usize;
    let name_len = u16::from_le_bytes([bytes[name_off], bytes[name_off + 1]]) as usize;
    core::str::from_utf8(&bytes[name_off + 2..name_off + 2 + name_len])
        .expect("validated OryqDoc UTF-8 field name")
}

/// Reads a validated value view at `desc` from `container` using unaligned-safe LE decoding.
pub(crate) fn read_value_at_descriptor(container: &[u8], desc: u32) -> ValueView<'_> {
    let (nibble, off) = unpack_descriptor(desc);
    match nibble {
        0x0 => ValueView::Null,
        0x1 => ValueView::Bool(false),
        0x2 => ValueView::Bool(true),
        0x3 => {
            let arr: [u8; 8] = container[off..off + 8].try_into().unwrap();
            ValueView::Int64(i64::from_le_bytes(arr))
        }
        0x4 => {
            let arr: [u8; 8] = container[off..off + 8].try_into().unwrap();
            ValueView::Float64(f64::from_le_bytes(arr))
        }
        0x5 => {
            let arr: [u8; 8] = container[off..off + 8].try_into().unwrap();
            ValueView::DateTimeUtcMs(i64::from_le_bytes(arr))
        }
        0x6 => {
            let len = u32::from_le_bytes(container[off..off + 4].try_into().unwrap()) as usize;
            let s = core::str::from_utf8(&container[off + 4..off + 4 + len])
                .expect("validated OryqDoc UTF-8 string");
            ValueView::String(s)
        }
        0x7 => {
            let len = u32::from_le_bytes(container[off..off + 4].try_into().unwrap()) as usize;
            let subtype = container[off + 4];
            ValueView::Binary {
                subtype,
                bytes: &container[off + 5..off + 5 + len],
            }
        }
        0x8 => {
            let arr: [u8; 12] = container[off..off + 12].try_into().unwrap();
            ValueView::ObjectId(ObjectId::from_bytes(arr))
        }
        0x9 => {
            let arr: [u8; 16] = container[off..off + 16].try_into().unwrap();
            ValueView::Uuid(Uuid::from_bytes(arr))
        }
        0xA => {
            let arr: [u8; 16] = container[off..off + 16].try_into().unwrap();
            ValueView::Decimal128(Decimal128::from_le_bytes(arr))
        }
        0xB => {
            let len = u32::from_le_bytes(container[off + 4..off + 8].try_into().unwrap()) as usize;
            ValueView::Array(ArrayView::from_validated_bytes(&container[off..off + len]))
        }
        0xC => {
            let len = u32::from_le_bytes(container[off + 4..off + 8].try_into().unwrap()) as usize;
            ValueView::Document(DocumentView::from_validated_bytes(
                &container[off..off + len],
            ))
        }
        _ => unreachable!("validated OryqTypeTag nibble"),
    }
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
