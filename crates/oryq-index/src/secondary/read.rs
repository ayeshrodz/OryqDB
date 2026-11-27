//! Ordered equality, range, and full scans over secondary B+ tree indexes.

use oryq_document::ValueView;
use oryq_engine::{BTree, PageStore, TreeKind};
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::descriptor::{IndexDescriptor, IndexKind};
use crate::key_codec::{encode_value_key, max_index_key_bytes, SortOrder};
use crate::secondary::extract::split_record_id_suffix;

const MODULE_ID: &str = "index";
const RECORD_ID_BYTES: usize = 8;

#[inline]
fn is_multi_field_compound(desc: &IndexDescriptor) -> bool {
    desc.kind == IndexKind::Compound && desc.fields.len() > 1
}

/// Compares `entry_prefix` against a single leading-field `bound_prefix`.
///
/// Every encoded value key is prefix-free, so for multi-field compound indexes (`f0 || f1 ...`),
/// `entry_prefix.starts_with(bound_prefix)` holds iff `f0 == bound`.
#[inline]
fn cmp_leading_prefix(
    entry_prefix: &[u8],
    bound_prefix: &[u8],
    is_compound: bool,
) -> core::cmp::Ordering {
    if is_compound && entry_prefix.starts_with(bound_prefix) {
        core::cmp::Ordering::Equal
    } else {
        entry_prefix.cmp(bound_prefix)
    }
}

/// Scans all `RecordId`s in `desc` whose leading indexed field equals `target` (canonical order).
pub fn scan_secondary_eq(
    store: &impl PageStore,
    desc: &IndexDescriptor,
    target: &ValueView<'_>,
) -> Result<Vec<RecordId>> {
    let field = desc.fields.first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "secondary::scan_secondary_eq",
            "secondary index descriptor has no fields",
        )
    })?;
    let is_compound = is_multi_field_compound(desc);
    let max_prefix = max_index_key_bytes(store.page_size()).saturating_sub(RECORD_ID_BYTES);
    let prefix = encode_value_key(target, field.order, max_prefix)?;

    let mut start = Vec::with_capacity(prefix.len() + RECORD_ID_BYTES);
    start.extend_from_slice(&prefix);
    start.extend_from_slice(&[0x00; RECORD_ID_BYTES]);

    let tree = BTree::open(desc.root_page_id, TreeKind::Index);
    let mut cur = tree.cursor(store);
    let mut out = Vec::new();
    if cur.seek_ge(&start)? {
        while let Some((k, _)) = cur.current() {
            let (entry_prefix, rid) = split_record_id_suffix(k)?;
            if cmp_leading_prefix(entry_prefix, prefix.as_slice(), is_compound)
                != core::cmp::Ordering::Equal
            {
                break;
            }
            out.push(rid);
            if !cur.step_next()? {
                break;
            }
        }
    }
    Ok(out)
}

/// Scans `RecordId`s in `desc` whose leading indexed value falls within optional lower/upper bounds
/// (in logical ascending value order; handles `SortOrder::Desc` and multi-field compound prefixes).
pub fn scan_secondary_range(
    store: &impl PageStore,
    desc: &IndexDescriptor,
    lower: Option<(&ValueView<'_>, bool)>, // (value, inclusive)
    upper: Option<(&ValueView<'_>, bool)>, // (value, inclusive)
) -> Result<Vec<RecordId>> {
    let field = desc.fields.first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "secondary::scan_secondary_range",
            "secondary index descriptor has no fields",
        )
    })?;
    let is_compound = is_multi_field_compound(desc);
    let max_prefix = max_index_key_bytes(store.page_size()).saturating_sub(RECORD_ID_BYTES);

    let lower_enc = match lower {
        Some((v, inc)) => Some((encode_value_key(v, field.order, max_prefix)?, inc)),
        None => None,
    };
    let upper_enc = match upper {
        Some((v, inc)) => Some((encode_value_key(v, field.order, max_prefix)?, inc)),
        None => None,
    };

    // For Desc indexes, logical lower bound is byte-order upper bound and vice versa.
    let (byte_lo, byte_hi) = match field.order {
        SortOrder::Asc => (lower_enc, upper_enc),
        SortOrder::Desc => (upper_enc, lower_enc),
    };

    let tree = BTree::open(desc.root_page_id, TreeKind::Index);
    let mut cur = tree.cursor(store);
    let positioned = match &byte_lo {
        Some((prefix, inclusive)) => {
            let mut seek_key = Vec::with_capacity(prefix.len() + RECORD_ID_BYTES);
            seek_key.extend_from_slice(prefix);
            if *inclusive || is_compound {
                // For compound indexes, additional trailing field bytes follow `prefix`, so seeking
                // at `prefix || 0x00*8` avoids skipping any trailing-field bytes and lets the loop
                // filter `!lo_inc` exactly via `cmp_leading_prefix`.
                seek_key.extend_from_slice(&[0x00; RECORD_ID_BYTES]);
            } else {
                seek_key.extend_from_slice(&[0xFF; RECORD_ID_BYTES]);
            }
            cur.seek_ge(&seek_key)?
        }
        None => cur.seek_first()?,
    };

    let mut out = Vec::new();
    if positioned {
        while let Some((k, _)) = cur.current() {
            let (entry_prefix, rid) = split_record_id_suffix(k)?;

            if let Some((ref lo_prefix, lo_inc)) = byte_lo {
                let cmp = cmp_leading_prefix(entry_prefix, lo_prefix.as_slice(), is_compound);
                if cmp == core::cmp::Ordering::Less
                    || (!lo_inc && cmp == core::cmp::Ordering::Equal)
                {
                    if !cur.step_next()? {
                        break;
                    }
                    continue;
                }
            }

            if let Some((ref hi_prefix, hi_inc)) = byte_hi {
                let cmp = cmp_leading_prefix(entry_prefix, hi_prefix.as_slice(), is_compound);
                if cmp == core::cmp::Ordering::Greater
                    || (!hi_inc && cmp == core::cmp::Ordering::Equal)
                {
                    break;
                }
            }

            out.push(rid);
            if !cur.step_next()? {
                break;
            }
        }
    }

    Ok(out)
}

/// Scans all `RecordId`s stored in `desc` in the index's native key order.
pub fn scan_secondary_all(store: &impl PageStore, desc: &IndexDescriptor) -> Result<Vec<RecordId>> {
    let tree = BTree::open(desc.root_page_id, TreeKind::Index);
    let pairs = tree.scan_all(store)?;
    let mut out = Vec::with_capacity(pairs.len());
    for (k, _) in pairs {
        let (_, rid) = split_record_id_suffix(&k)?;
        out.push(rid);
    }
    Ok(out)
}
