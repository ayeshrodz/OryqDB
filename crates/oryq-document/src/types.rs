//! Core value, identifier, and owned/borrowed document types for `oryq-document`.

use core::fmt;
use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::array::ArrayView;
use crate::codec::validate_document;
use crate::view::DocumentView;

const MODULE_ID: &str = "document";

/// 4-bit type tags stored in bits `28..31` of `OryqDoc` value and array descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OryqTypeTag {
    Null = 0x0,
    BoolFalse = 0x1,
    BoolTrue = 0x2,
    Int64 = 0x3,
    Float64 = 0x4,
    DateTimeUtcMs = 0x5,
    String = 0x6,
    Binary = 0x7,
    ObjectId = 0x8,
    Uuid = 0x9,
    Decimal128 = 0xA,
    Array = 0xB,
    Document = 0xC,
}

impl OryqTypeTag {
    /// Decodes a 4-bit tag from `0..=15`, rejecting reserved tags and `0xF` (`OverflowRef`)
    /// in exported `OryqDoc` payloads.
    pub fn from_nibble(nibble: u8) -> Result<Self> {
        match nibble {
            0x0 => Ok(Self::Null),
            0x1 => Ok(Self::BoolFalse),
            0x2 => Ok(Self::BoolTrue),
            0x3 => Ok(Self::Int64),
            0x4 => Ok(Self::Float64),
            0x5 => Ok(Self::DateTimeUtcMs),
            0x6 => Ok(Self::String),
            0x7 => Ok(Self::Binary),
            0x8 => Ok(Self::ObjectId),
            0x9 => Ok(Self::Uuid),
            0xA => Ok(Self::Decimal128),
            0xB => Ok(Self::Array),
            0xC => Ok(Self::Document),
            0xF => Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "OryqTypeTag::from_nibble",
                "unresolved OverflowRef tag (0xF) is forbidden in exported OryqDoc bytes",
            )),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "OryqTypeTag::from_nibble",
                format!("reserved OryqTypeTag nibble 0x{other:X}"),
            )),
        }
    }

    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn is_inline(self) -> bool {
        matches!(self, Self::Null | Self::BoolFalse | Self::BoolTrue)
    }
}

/// 12-byte big-endian monotonically sortable object identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub [u8; 12]);

impl ObjectId {
    pub const fn from_bytes(bytes: [u8; 12]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 12] {
        &self.0
    }

    pub fn from_hex(hex: &str) -> Result<Self> {
        let bytes = decode_hex_fixed::<12>(hex, "ObjectId::from_hex")?;
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// 16-byte big-endian RFC 4122 / 9562 UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn parse_str(s: &str) -> Result<Self> {
        let stripped: String = s.chars().filter(|&c| c != '-').collect();
        let bytes = decode_hex_fixed::<16>(&stripped, "Uuid::parse_str")?;
        Ok(Self(bytes))
    }

    pub fn to_hyphenated(&self) -> String {
        let h = encode_hex(&self.0);
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hyphenated())
    }
}

/// Exact 16-byte IEEE 754-2008 Binary Integer Decimal (`decimal128`, little-endian on disk).
///
/// Supports finite decimal numbers with up to 34 decimal digits (`0..=10^34 - 1`),
/// biased exponent `0..=12287` (unbiased `-6176..=6111`), plus signed `Infinity` and `NaN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Decimal128(pub [u8; 16]);

/// Decoded exact representation of a [`Decimal128`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedDecimal128 {
    NaN,
    Infinity {
        negative: bool,
    },
    Finite {
        negative: bool,
        coefficient: u128,
        exponent: i32,
    },
}

const DEC128_BIAS: i32 = 6176;
const DEC128_MAX_COEFF: u128 = 9_999_999_999_999_999_999_999_999_999_999_999; // 10^34 - 1
const DEC128_MAX_EXP: i32 = 6111;
const DEC128_MIN_EXP: i32 = -6176;

impl Decimal128 {
    pub const fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn to_le_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Encodes a [`DecodedDecimal128`] into IEEE 754-2008 BID little-endian bytes.
    pub fn from_decoded(decoded: DecodedDecimal128) -> Result<Self> {
        let raw: u128 = match decoded {
            DecodedDecimal128::NaN => 0x7C00_0000_0000_0000_0000_0000_0000_0000u128,
            DecodedDecimal128::Infinity { negative } => {
                let sign = if negative { 1u128 << 127 } else { 0 };
                sign | 0x7800_0000_0000_0000_0000_0000_0000_0000u128
            }
            DecodedDecimal128::Finite {
                negative,
                coefficient,
                exponent,
            } => {
                if coefficient > DEC128_MAX_COEFF {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Overflow,
                        MODULE_ID,
                        "Decimal128::from_decoded",
                        "decimal128 coefficient exceeds 34 decimal digits",
                    ));
                }
                if !(DEC128_MIN_EXP..=DEC128_MAX_EXP).contains(&exponent) {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Overflow,
                        MODULE_ID,
                        "Decimal128::from_decoded",
                        format!("decimal128 exponent {exponent} out of range"),
                    ));
                }
                let sign = if negative { 1u128 << 127 } else { 0 };
                let biased_exp = (exponent + DEC128_BIAS) as u128;
                // BID primary encoding: bits 126..125 != 11 => bits 113..126 = biased_exp (14 bits),
                // bits 0..112 = coefficient (113 bits, always holds <= 10^34 - 1 < 2^113).
                sign | (biased_exp << 113) | coefficient
            }
        };
        Ok(Self(raw.to_le_bytes()))
    }

    /// Decodes IEEE 754-2008 BID little-endian bytes into exact sign, coefficient, and exponent.
    pub fn decode(self) -> DecodedDecimal128 {
        let raw = u128::from_le_bytes(self.0);
        let negative = (raw >> 127) != 0;
        // Check bits 126..122:
        // 11111 = NaN (sNaN/qNaN), 11110 = Infinity
        let top5 = ((raw >> 122) & 0x1F) as u8;
        if top5 == 0b11111 {
            return DecodedDecimal128::NaN;
        }
        if top5 == 0b11110 {
            return DecodedDecimal128::Infinity { negative };
        }
        let top2 = ((raw >> 125) & 0x3) as u8;
        let (biased_exp, coeff) = if top2 == 0b11 {
            // Alternate finite encoding (bits 124..111 = exp, implied leading 100 on coeff)
            let exp = ((raw >> 111) & 0x3FFF) as i32;
            let low = raw & ((1u128 << 111) - 1);
            let c = (1u128 << 113) | low;
            // Per IEEE 754-2008 BID, non-canonical coefficients > 10^34 - 1 decode as 0.
            let canonical_c = if c > DEC128_MAX_COEFF { 0 } else { c };
            (exp, canonical_c)
        } else {
            let exp = ((raw >> 113) & 0x3FFF) as i32;
            let c = raw & ((1u128 << 113) - 1);
            let canonical_c = if c > DEC128_MAX_COEFF { 0 } else { c };
            (exp, canonical_c)
        };
        DecodedDecimal128::Finite {
            negative,
            coefficient: coeff,
            exponent: biased_exp - DEC128_BIAS,
        }
    }

    /// Parses an exact decimal string (e.g. `"-123.450"`, `"1e10"`, `"NaN"`, `"-Infinity"`).
    pub fn parse_str(input: &str) -> Result<Self> {
        let s = input.trim();
        if s.eq_ignore_ascii_case("nan") {
            return Self::from_decoded(DecodedDecimal128::NaN);
        }
        let (negative, rest) = if let Some(r) = s.strip_prefix('-') {
            (true, r)
        } else if let Some(r) = s.strip_prefix('+') {
            (false, r)
        } else {
            (false, s)
        };
        if rest.eq_ignore_ascii_case("infinity") || rest.eq_ignore_ascii_case("inf") {
            return Self::from_decoded(DecodedDecimal128::Infinity { negative });
        }

        let (mantissa_str, exp_part) = match rest.find(['e', 'E']) {
            Some(idx) => {
                let exp: i32 = rest[idx + 1..].parse().map_err(|_| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "Decimal128::parse_str",
                        format!("invalid decimal exponent in `{input}`"),
                    )
                })?;
                (&rest[..idx], exp)
            }
            None => (rest, 0),
        };

        let (int_part, frac_part) = match mantissa_str.find('.') {
            Some(dot) => (&mantissa_str[..dot], &mantissa_str[dot + 1..]),
            None => (mantissa_str, ""),
        };

        if int_part.is_empty() && frac_part.is_empty() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Decimal128::parse_str",
                format!("empty decimal mantissa in `{input}`"),
            ));
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Decimal128::parse_str",
                format!("non-digit characters in decimal `{input}`"),
            ));
        }

        let mut coeff: u128 = 0;
        let mut saw_nonzero = false;
        let mut sig_digits = 0usize;
        for b in int_part.bytes().chain(frac_part.bytes()) {
            let d = (b - b'0') as u128;
            if d != 0 || saw_nonzero {
                saw_nonzero = true;
                sig_digits += 1;
                if sig_digits > 34 {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Overflow,
                        MODULE_ID,
                        "Decimal128::parse_str",
                        format!("decimal `{input}` exceeds 34 significant digits"),
                    ));
                }
                coeff = coeff * 10 + d;
            }
        }

        let exponent = exp_part
            .checked_sub(frac_part.len() as i32)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "Decimal128::parse_str",
                    "decimal exponent underflow",
                )
            })?;

        Self::from_decoded(DecodedDecimal128::Finite {
            negative,
            coefficient: coeff,
            exponent,
        })
    }
}

impl fmt::Display for Decimal128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.decode() {
            DecodedDecimal128::NaN => f.write_str("NaN"),
            DecodedDecimal128::Infinity { negative: true } => f.write_str("-Infinity"),
            DecodedDecimal128::Infinity { negative: false } => f.write_str("Infinity"),
            DecodedDecimal128::Finite {
                negative,
                coefficient,
                exponent,
            } => {
                if negative {
                    f.write_str("-")?;
                }
                let digits = coefficient.to_string();
                if exponent == 0 {
                    f.write_str(&digits)
                } else if exponent < 0 {
                    let scale = (-exponent) as usize;
                    if scale < digits.len() {
                        let split = digits.len() - scale;
                        write!(f, "{}.{}", &digits[..split], &digits[split..])
                    } else if scale - digits.len() <= 6 {
                        write!(f, "0.{}{digits}", "0".repeat(scale - digits.len()))
                    } else {
                        let adj = exponent + (digits.len() as i32 - 1);
                        if digits.len() == 1 {
                            write!(f, "{digits}E{adj}")
                        } else {
                            write!(f, "{}.{}E{adj}", &digits[..1], &digits[1..])
                        }
                    }
                } else if exponent <= 6 {
                    write!(f, "{digits}{}", "0".repeat(exponent as usize))
                } else {
                    let adj = exponent + (digits.len() as i32 - 1);
                    if digits.len() == 1 {
                        write!(f, "{digits}E+{adj}")
                    } else {
                        write!(f, "{}.{}E+{adj}", &digits[..1], &digits[1..])
                    }
                }
            }
        }
    }
}

/// Owned OryqDB document value used when constructing or materializing documents.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    DateTimeUtcMs(i64),
    String(String),
    Binary { subtype: u8, bytes: Vec<u8> },
    ObjectId(ObjectId),
    Uuid(Uuid),
    Decimal128(Decimal128),
    Array(Vec<Value>),
    Document(Vec<(String, Value)>),
}

/// Zero-copy borrowed value view referencing validated `OryqDoc` bytes.
#[derive(Debug, Clone, Copy)]
pub enum ValueView<'a> {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    DateTimeUtcMs(i64),
    String(&'a str),
    Binary { subtype: u8, bytes: &'a [u8] },
    ObjectId(ObjectId),
    Uuid(Uuid),
    Decimal128(Decimal128),
    Array(ArrayView<'a>),
    Document(DocumentView<'a>),
}

impl<'a> ValueView<'a> {
    /// Returns the wire-format [`OryqTypeTag`] corresponding to this value view.
    #[inline]
    pub fn type_tag(&self) -> OryqTypeTag {
        match self {
            Self::Null => OryqTypeTag::Null,
            Self::Bool(false) => OryqTypeTag::BoolFalse,
            Self::Bool(true) => OryqTypeTag::BoolTrue,
            Self::Int64(_) => OryqTypeTag::Int64,
            Self::Float64(_) => OryqTypeTag::Float64,
            Self::DateTimeUtcMs(_) => OryqTypeTag::DateTimeUtcMs,
            Self::String(_) => OryqTypeTag::String,
            Self::Binary { .. } => OryqTypeTag::Binary,
            Self::ObjectId(_) => OryqTypeTag::ObjectId,
            Self::Uuid(_) => OryqTypeTag::Uuid,
            Self::Decimal128(_) => OryqTypeTag::Decimal128,
            Self::Array(_) => OryqTypeTag::Array,
            Self::Document(_) => OryqTypeTag::Document,
        }
    }

    pub fn to_owned_value(&self) -> Value {
        match *self {
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(b),
            Self::Int64(v) => Value::Int64(v),
            Self::Float64(v) => Value::Float64(v),
            Self::DateTimeUtcMs(v) => Value::DateTimeUtcMs(v),
            Self::String(s) => Value::String(s.to_owned()),
            Self::Binary { subtype, bytes } => Value::Binary {
                subtype,
                bytes: bytes.to_vec(),
            },
            Self::ObjectId(id) => Value::ObjectId(id),
            Self::Uuid(id) => Value::Uuid(id),
            Self::Decimal128(d) => Value::Decimal128(d),
            Self::Array(arr) => Value::Array(arr.iter().map(|v| v.to_owned_value()).collect()),
            Self::Document(doc) => Value::Document(
                doc.iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned_value()))
                    .collect(),
            ),
        }
    }
}

/// Immutable, reference-counted owned `OryqDoc` byte buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentBuf {
    bytes: Arc<[u8]>,
}

impl DocumentBuf {
    /// Validates raw `OryqDoc` bytes and wraps them in an immutable `DocumentBuf`.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self> {
        let bytes = bytes.into();
        validate_document(&bytes)?;
        Ok(Self { bytes })
    }

    /// Internal constructor for bytes already validated/emitted by `codec`.
    pub(crate) fn from_validated_arc(bytes: Arc<[u8]>) -> Self {
        Self { bytes }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns a zero-copy [`DocumentView`] borrowing from this buffer.
    #[inline]
    pub fn view(&self) -> DocumentView<'_> {
        DocumentView::from_validated_bytes(&self.bytes)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn decode_hex_fixed<const N: usize>(hex: &str, op: &'static str) -> Result<[u8; N]> {
    if hex.len() != N * 2 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            op,
            format!("expected {} hex characters, got {}", N * 2, hex.len()),
        ));
    }
    let mut out = [0u8; N];
    let bytes = hex.as_bytes();
    for i in 0..N {
        let hi = hex_nibble(bytes[i * 2], op)?;
        let lo = hex_nibble(bytes[i * 2 + 1], op)?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8, op: &'static str) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            op,
            format!("invalid hex byte 0x{b:02X}"),
        )),
    }
}
