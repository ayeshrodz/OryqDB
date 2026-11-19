//! Canonical order-preserving memcomparable key encoding (`docs/indexing.md` §4.1).
//!
//! Lexicographical `memcmp` on encoded byte slices matches `oryq_document::compare_values`
//! exactly across all 12 `OryqDoc` types, including cross-type numeric equality
//! (`Int64`, `Float64`, `Decimal128`).

use oryq_document::{Decimal128, DecodedDecimal128, ValueView};
use oryq_engine::PageSize;
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

const MODULE_ID: &str = "index";

pub const MAX_PRIMARY_STRING_ID_BYTES: usize = 1024;

// Canonical type rank tags (`docs/indexing.md` §4.1).
pub const RANK_MIN_KEY: u8 = 0x00;
pub const RANK_NULL: u8 = 0x05;
pub const RANK_BOOL_FALSE: u8 = 0x10;
pub const RANK_BOOL_TRUE: u8 = 0x11;
pub const RANK_NUMBER_NAN: u8 = 0x1F;
pub const RANK_NUMBER: u8 = 0x20;
pub const RANK_DATETIME: u8 = 0x30;
pub const RANK_STRING: u8 = 0x40;
pub const RANK_BINARY: u8 = 0x50;
pub const RANK_OBJECT_ID: u8 = 0x60;
pub const RANK_UUID: u8 = 0x61;
pub const RANK_ARRAY: u8 = 0x70;
pub const RANK_DOCUMENT: u8 = 0x80;
pub const RANK_MAX_KEY: u8 = 0xFF;

// Number category bytes following `RANK_NUMBER` (`0x20`).
const NUM_NEG_INF: u8 = 0x00;
const NUM_NEG_FINITE: u8 = 0x10;
const NUM_ZERO: u8 = 0x20;
const NUM_POS_FINITE: u8 = 0x30;
const NUM_POS_INF: u8 = 0x40;

/// Component sort direction for index keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

/// Normative maximum complete encoded index key length (`docs/indexing.md` §9):
/// `min(2048, floor((page_size - 64) / 4) - 32)`.
#[inline]
pub const fn max_index_key_bytes(page_size: PageSize) -> usize {
    let per_page = ((page_size.bytes() - 64) / 4) - 32;
    if per_page < 2048 {
        per_page
    } else {
        2048
    }
}

/// Encodes an internal [`RecordId`] as 8 big-endian bytes.
#[inline]
pub fn encode_record_id(id: RecordId) -> [u8; 8] {
    id.raw().to_be_bytes()
}

/// Decodes an 8-byte big-endian [`RecordId`].
pub fn decode_record_id(bytes: &[u8]) -> Result<RecordId> {
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "key_codec::decode_record_id",
            format!("expected 8-byte RecordId, got {} bytes", bytes.len()),
        )
    })?;
    RecordId::new(u64::from_be_bytes(arr))
}

/// Validates and encodes a user `_id` value for the primary index (`docs/document-model.md` §4.2).
///
/// Supported `_id` types: `ObjectId`, `Uuid`, `Int64`, and `String` (<= 1024 UTF-8 bytes).
pub fn encode_primary_id_key(val: &ValueView<'_>, page_size: PageSize) -> Result<Vec<u8>> {
    match val {
        ValueView::ObjectId(_) | ValueView::Uuid(_) | ValueView::Int64(_) => {}
        ValueView::String(s) => {
            if s.len() > MAX_PRIMARY_STRING_ID_BYTES {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "key_codec::encode_primary_id_key",
                    format!(
                        "String _id length {} exceeds max {MAX_PRIMARY_STRING_ID_BYTES} bytes",
                        s.len()
                    ),
                ));
            }
        }
        other => {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "key_codec::encode_primary_id_key",
                format!(
                    "unsupported _id type {:?}; expected ObjectId, Uuid, Int64, or String",
                    other.type_tag()
                ),
            ));
        }
    }
    encode_value_key(val, SortOrder::Asc, max_index_key_bytes(page_size))
}

/// Encodes any [`ValueView`] into its canonical order-preserving memcomparable byte representation.
pub fn encode_value_key(
    val: &ValueView<'_>,
    order: SortOrder,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    encode_value_into(val, &mut buf)?;
    if order == SortOrder::Desc {
        for b in &mut buf {
            *b ^= 0xFF;
        }
    }
    if buf.len() > max_bytes {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "key_codec::encode_value_key",
            format!(
                "encoded index key ({} bytes) exceeds max_index_key_bytes ({max_bytes})",
                buf.len()
            ),
        ));
    }
    Ok(buf)
}

fn encode_value_into(val: &ValueView<'_>, out: &mut Vec<u8>) -> Result<()> {
    match *val {
        ValueView::Null => out.push(RANK_NULL),
        ValueView::Bool(false) => out.push(RANK_BOOL_FALSE),
        ValueView::Bool(true) => out.push(RANK_BOOL_TRUE),
        ValueView::Int64(n) => encode_i64_number(n, out),
        ValueView::Float64(f) => encode_f64_number(f, out),
        ValueView::Decimal128(d) => encode_decimal128_number(d, out),
        ValueView::DateTimeUtcMs(ms) => {
            out.push(RANK_DATETIME);
            let ordered = (ms as u64) ^ (1u64 << 63);
            out.extend_from_slice(&ordered.to_be_bytes());
        }
        ValueView::String(s) => {
            out.push(RANK_STRING);
            encode_escaped_bytes(s.as_bytes(), out);
        }
        ValueView::Binary { subtype, bytes } => {
            out.push(RANK_BINARY);
            out.push(subtype);
            encode_escaped_bytes(bytes, out);
        }
        ValueView::ObjectId(id) => {
            out.push(RANK_OBJECT_ID);
            out.extend_from_slice(id.as_bytes());
        }
        ValueView::Uuid(id) => {
            out.push(RANK_UUID);
            out.extend_from_slice(id.as_bytes());
        }
        ValueView::Array(arr) => {
            out.push(RANK_ARRAY);
            for item in arr.iter() {
                encode_value_into(&item, out)?;
            }
            out.push(0x00);
        }
        ValueView::Document(doc) => {
            out.push(RANK_DOCUMENT);
            let mut fields: Vec<(&str, ValueView<'_>)> = doc.iter().collect();
            fields.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            for (k, v) in fields {
                out.push(RANK_STRING);
                encode_escaped_bytes(k.as_bytes(), out);
                encode_value_into(&v, out)?;
            }
            out.push(0x00);
        }
    }
    Ok(())
}

fn encode_escaped_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        if b == 0x00 {
            out.push(0x00);
            out.push(0xFF);
        } else {
            out.push(b);
        }
    }
    out.push(0x00);
    out.push(0x00);
}

fn encode_i64_number(n: i64, out: &mut Vec<u8>) {
    if n == 0 {
        out.push(RANK_NUMBER);
        out.push(NUM_ZERO);
        return;
    }
    let negative = n < 0;
    let abs = n.unsigned_abs();
    let digits = abs.to_string();
    encode_decimal_digits(negative, digits.as_bytes(), 0, out);
}

fn encode_decimal128_number(d: Decimal128, out: &mut Vec<u8>) {
    match d.decode() {
        DecodedDecimal128::NaN => out.push(RANK_NUMBER_NAN),
        DecodedDecimal128::Infinity { negative: true } => {
            out.push(RANK_NUMBER);
            out.push(NUM_NEG_INF);
        }
        DecodedDecimal128::Infinity { negative: false } => {
            out.push(RANK_NUMBER);
            out.push(NUM_POS_INF);
        }
        DecodedDecimal128::Finite {
            negative,
            coefficient,
            exponent,
        } => {
            if coefficient == 0 {
                out.push(RANK_NUMBER);
                out.push(NUM_ZERO);
                return;
            }
            let digits = coefficient.to_string();
            encode_decimal_digits(negative, digits.as_bytes(), exponent, out);
        }
    }
}

fn encode_f64_number(f: f64, out: &mut Vec<u8>) {
    if f.is_nan() {
        out.push(RANK_NUMBER_NAN);
        return;
    }
    if f == 0.0 {
        out.push(RANK_NUMBER);
        out.push(NUM_ZERO);
        return;
    }
    if f.is_infinite() {
        out.push(RANK_NUMBER);
        out.push(if f.is_sign_negative() {
            NUM_NEG_INF
        } else {
            NUM_POS_INF
        });
        return;
    }

    // Exact IEEE-754 decomposition: value = (-1)^sign * mantissa * 2^exp2.
    let bits = f.to_bits();
    let negative = (bits >> 63) != 0;
    let raw_exp = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (mantissa, exp2) = if raw_exp == 0 {
        (frac, 1 - 1023 - 52)
    } else {
        ((1u64 << 52) | frac, raw_exp - 1023 - 52)
    };

    // Convert exact `mantissa * 2^exp2` into exact decimal integer * 10^scale10:
    // - If exp2 >= 0: value = mantissa * 2^exp2, scale10 = 0
    // - If exp2 < 0: value = (mantissa * 5^(-exp2)) * 10^exp2, scale10 = exp2
    let mut dec_digits = BigDec::from_u64(mantissa);
    let scale10 = if exp2 >= 0 {
        dec_digits.mul_pow2(exp2 as u32);
        0
    } else {
        dec_digits.mul_pow5((-exp2) as u32);
        exp2
    };
    let ascii = dec_digits.to_ascii_digits();
    encode_decimal_digits(negative, &ascii, scale10, out);
}

/// Encodes a nonzero finite decimal `(-1)^negative * int(ascii_digits) * 10^scale10`.
///
/// Strips trailing zeroes, computes `E = scale10 + trimmed.len() - 1`, and writes:
/// `[RANK_NUMBER][category][biased_E: 4B BE][digits: 0x10 + d][0x00]`,
/// inverting every magnitude byte for negative finite numbers.
fn encode_decimal_digits(negative: bool, ascii_digits: &[u8], scale10: i32, out: &mut Vec<u8>) {
    let mut end = ascii_digits.len();
    while end > 1 && ascii_digits[end - 1] == b'0' {
        end -= 1;
    }
    let trailing_zeros = (ascii_digits.len() - end) as i32;
    let trimmed = &ascii_digits[..end];
    let adj_exp = scale10 + trailing_zeros + (trimmed.len() as i32 - 1);
    let biased_exp = (adj_exp as u32) ^ 0x8000_0000;

    out.push(RANK_NUMBER);
    out.push(if negative {
        NUM_NEG_FINITE
    } else {
        NUM_POS_FINITE
    });

    let mag_start = out.len();
    out.extend_from_slice(&biased_exp.to_be_bytes());
    for &ch in trimmed {
        out.push(0x10 + (ch - b'0'));
    }
    out.push(0x00);

    if negative {
        for b in &mut out[mag_start..] {
            *b ^= 0xFF;
        }
    }
}

/// Minimal exact base-10^9 big unsigned integer for exact `f64` decimal expansion.
struct BigDec {
    limbs: Vec<u32>, // base 1_000_000_000, least-significant limb first
}

impl BigDec {
    const BASE: u64 = 1_000_000_000;

    fn from_u64(mut v: u64) -> Self {
        let mut limbs = Vec::new();
        while v > 0 {
            limbs.push((v % Self::BASE) as u32);
            v /= Self::BASE;
        }
        if limbs.is_empty() {
            limbs.push(0);
        }
        Self { limbs }
    }

    fn mul_small(&mut self, factor: u64) {
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let prod = (*limb as u64) * factor + carry;
            *limb = (prod % Self::BASE) as u32;
            carry = prod / Self::BASE;
        }
        while carry > 0 {
            self.limbs.push((carry % Self::BASE) as u32);
            carry /= Self::BASE;
        }
    }

    fn mul_pow2(&mut self, mut exp: u32) {
        while exp >= 30 {
            self.mul_small(1u64 << 30);
            exp -= 30;
        }
        if exp > 0 {
            self.mul_small(1u64 << exp);
        }
    }

    fn mul_pow5(&mut self, mut exp: u32) {
        const POW5_13: u64 = 1_220_703_125; // 5^13 < 2^31
        while exp >= 13 {
            self.mul_small(POW5_13);
            exp -= 13;
        }
        if exp > 0 {
            self.mul_small(5u64.pow(exp));
        }
    }

    fn to_ascii_digits(&self) -> Vec<u8> {
        let Some(&top) = self.limbs.last() else {
            return vec![b'0'];
        };
        let mut s = top.to_string();
        for &limb in self.limbs.iter().rev().skip(1) {
            s.push_str(&format!("{limb:09}"));
        }
        s.into_bytes()
    }
}
