//! Deterministic JSON and Extended-JSON conversion for `OryqDoc`.

use oryq_types::{BudgetHandle, ErrorCategory, ErrorEnvelope, Reservation, Result};
use serde_json::{Map, Number, Value as JsonValue};

use crate::codec::encode_document;
use crate::types::{Decimal128, DocumentBuf, ObjectId, Uuid, Value, ValueView};
use crate::view::DocumentView;

const MODULE_ID: &str = "document";

/// Parses a JSON or Extended-JSON object string into a validated [`DocumentBuf`].
pub fn from_json_str(
    json: &str,
    budget: Option<&BudgetHandle>,
) -> Result<(DocumentBuf, Option<Reservation>)> {
    let parsed: JsonValue = serde_json::from_str(json).map_err(|e| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "json::from_json_str",
            format!("invalid JSON input: {e}"),
        )
    })?;
    from_json_value(&parsed, budget)
}

/// Converts a `serde_json::Value` object into a validated [`DocumentBuf`].
pub fn from_json_value(
    json: &JsonValue,
    budget: Option<&BudgetHandle>,
) -> Result<(DocumentBuf, Option<Reservation>)> {
    let JsonValue::Object(map) = json else {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "json::from_json_value",
            "top-level OryqDoc JSON must be an object",
        ));
    };
    let mut fields = Vec::with_capacity(map.len());
    for (k, v) in map {
        fields.push((k.clone(), json_to_value(v)?));
    }
    encode_document(&fields, budget)
}

/// Converts a [`DocumentView`] into a lossless Extended-JSON `serde_json::Value`.
pub fn to_json_value(view: &DocumentView<'_>) -> JsonValue {
    let mut map = Map::with_capacity(view.field_count());
    for (k, v) in view.iter() {
        map.insert(k.to_owned(), value_view_to_json(&v));
    }
    JsonValue::Object(map)
}

/// Serializes a [`DocumentView`] into a compact Extended-JSON string.
pub fn to_json_string(view: &DocumentView<'_>) -> Result<String> {
    serde_json::to_string(&to_json_value(view)).map_err(|e| {
        ErrorEnvelope::new(
            ErrorCategory::Internal,
            MODULE_ID,
            "json::to_json_string",
            format!("failed to serialize JSON: {e}"),
        )
    })
}

pub(crate) fn json_to_value(v: &JsonValue) -> Result<Value> {
    match v {
        JsonValue::Null => Ok(Value::Null),
        JsonValue::Bool(b) => Ok(Value::Bool(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int64(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float64(f))
            } else {
                Err(ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "json::json_to_value",
                    format!("JSON number `{n}` out of Int64/Float64 range"),
                ))
            }
        }
        JsonValue::String(s) => Ok(Value::String(s.clone())),
        JsonValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_value(item)?);
            }
            Ok(Value::Array(out))
        }
        JsonValue::Object(map) => {
            if let Some(ext) = try_parse_extended_json(map)? {
                return Ok(ext);
            }
            let mut fields = Vec::with_capacity(map.len());
            for (k, val) in map {
                fields.push((k.clone(), json_to_value(val)?));
            }
            Ok(Value::Document(fields))
        }
    }
}

fn try_parse_extended_json(map: &Map<String, JsonValue>) -> Result<Option<Value>> {
    if map.len() != 1 {
        return Ok(None);
    }
    let (key, val) = map.iter().next().unwrap();
    match key.as_str() {
        "$oid" => {
            let hex = val.as_str().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$oid value must be a hex string",
                )
            })?;
            Ok(Some(Value::ObjectId(ObjectId::from_hex(hex)?)))
        }
        "$uuid" => {
            let s = val.as_str().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$uuid value must be a string",
                )
            })?;
            Ok(Some(Value::Uuid(Uuid::parse_str(s)?)))
        }
        "$numberLong" => {
            let s = val.as_str().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$numberLong value must be a string",
                )
            })?;
            let n: i64 = s.parse().map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    format!("invalid $numberLong integer `{s}`"),
                )
            })?;
            Ok(Some(Value::Int64(n)))
        }
        "$numberDouble" => {
            let s = val.as_str().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$numberDouble value must be a string",
                )
            })?;
            let f = match s {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                other => other.parse::<f64>().map_err(|_| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "json::extended",
                        format!("invalid $numberDouble value `{other}`"),
                    )
                })?,
            };
            Ok(Some(Value::Float64(f)))
        }
        "$numberDecimal" => {
            let s = val.as_str().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$numberDecimal value must be a string",
                )
            })?;
            Ok(Some(Value::Decimal128(Decimal128::parse_str(s)?)))
        }
        "$date" => {
            let ms = match val {
                JsonValue::Object(inner) if inner.len() == 1 => {
                    let s = inner
                        .get("$numberLong")
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| {
                            ErrorEnvelope::new(
                                ErrorCategory::InvalidArgument,
                                MODULE_ID,
                                "json::extended",
                                "$date object must contain $numberLong string",
                            )
                        })?;
                    s.parse::<i64>().map_err(|_| {
                        ErrorEnvelope::new(
                            ErrorCategory::InvalidArgument,
                            MODULE_ID,
                            "json::extended",
                            format!("invalid $date epoch millis `{s}`"),
                        )
                    })?
                }
                JsonValue::Number(n) => n.as_i64().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "json::extended",
                        "$date numeric value must fit in i64",
                    )
                })?,
                _ => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "json::extended",
                        "$date must be an i64 or {$numberLong: \"...\"}",
                    ));
                }
            };
            Ok(Some(Value::DateTimeUtcMs(ms)))
        }
        "$binary" => {
            let obj = val.as_object().ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    "$binary must be an object with base64 and subType",
                )
            })?;
            let b64 = obj
                .get("base64")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "json::extended",
                        "$binary.base64 must be a string",
                    )
                })?;
            let sub_str = obj
                .get("subType")
                .and_then(JsonValue::as_str)
                .unwrap_or("00");
            let subtype = u8::from_str_radix(sub_str, 16).map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "json::extended",
                    format!("invalid $binary.subType `{sub_str}`"),
                )
            })?;
            let bytes = decode_base64(b64)?;
            Ok(Some(Value::Binary { subtype, bytes }))
        }
        _ => Ok(None),
    }
}

fn value_view_to_json(v: &ValueView<'_>) -> JsonValue {
    match *v {
        ValueView::Null => JsonValue::Null,
        ValueView::Bool(b) => JsonValue::Bool(b),
        ValueView::Int64(n) => JsonValue::Number(Number::from(n)),
        ValueView::Float64(f) => {
            if let Some(num) = Number::from_f64(f) {
                JsonValue::Number(num)
            } else {
                let s = if f.is_nan() {
                    "NaN"
                } else if f.is_sign_negative() {
                    "-Infinity"
                } else {
                    "Infinity"
                };
                let mut m = Map::new();
                m.insert("$numberDouble".to_owned(), JsonValue::String(s.to_owned()));
                JsonValue::Object(m)
            }
        }
        ValueView::DateTimeUtcMs(ms) => {
            let mut inner = Map::new();
            inner.insert("$numberLong".to_owned(), JsonValue::String(ms.to_string()));
            let mut outer = Map::new();
            outer.insert("$date".to_owned(), JsonValue::Object(inner));
            JsonValue::Object(outer)
        }
        ValueView::String(s) => JsonValue::String(s.to_owned()),
        ValueView::Binary { subtype, bytes } => {
            let mut inner = Map::new();
            inner.insert("base64".to_owned(), JsonValue::String(encode_base64(bytes)));
            inner.insert(
                "subType".to_owned(),
                JsonValue::String(format!("{subtype:02x}")),
            );
            let mut outer = Map::new();
            outer.insert("$binary".to_owned(), JsonValue::Object(inner));
            JsonValue::Object(outer)
        }
        ValueView::ObjectId(id) => {
            let mut m = Map::new();
            m.insert("$oid".to_owned(), JsonValue::String(id.to_hex()));
            JsonValue::Object(m)
        }
        ValueView::Uuid(id) => {
            let mut m = Map::new();
            m.insert("$uuid".to_owned(), JsonValue::String(id.to_hyphenated()));
            JsonValue::Object(m)
        }
        ValueView::Decimal128(dec) => {
            let mut m = Map::new();
            m.insert(
                "$numberDecimal".to_owned(),
                JsonValue::String(dec.to_string()),
            );
            JsonValue::Object(m)
        }
        ValueView::Array(arr) => {
            JsonValue::Array(arr.iter().map(|e| value_view_to_json(&e)).collect())
        }
        ValueView::Document(doc) => to_json_value(&doc),
    }
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "json::decode_base64",
            "base64 length must be a multiple of 4",
        ));
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let v0 = b64_val(chunk[0])?;
        let v1 = b64_val(chunk[1])?;
        out.push((v0 << 2) | (v1 >> 4));
        if chunk[2] != b'=' {
            let v2 = b64_val(chunk[2])?;
            out.push((v1 << 4) | (v2 >> 2));
            if chunk[3] != b'=' {
                let v3 = b64_val(chunk[3])?;
                out.push((v2 << 6) | v3);
            }
        } else if chunk[3] != b'=' {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "json::decode_base64",
                "invalid base64 padding",
            ));
        }
    }
    Ok(out)
}

fn b64_val(b: u8) -> Result<u8> {
    match b {
        b'A'..=b'Z' => Ok(b - b'A'),
        b'a'..=b'z' => Ok(b - b'a' + 26),
        b'0'..=b'9' => Ok(b - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "json::decode_base64",
            format!("invalid base64 byte 0x{b:02X}"),
        )),
    }
}
