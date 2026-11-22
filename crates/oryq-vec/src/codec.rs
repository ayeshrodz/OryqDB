//! Binary key & value codecs for B+ tree vector index storage (`oryq-vec`).

use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::distance::{l2_norm, DistanceMetric, VectorQuantization};

const MODULE_ID: &str = "vec";

/// Metadata key storing [`VecIndexMeta`] (`vector_count`, `dimensions`, `metric`, `quantization`).
pub const KEY_META: &[u8] = b"\x00M";
/// Prefix for vector entry keys: `0x01 || record_id_be_u64`.
pub const KEY_PREFIX_VEC: &[u8] = b"\x01";

const TAG_F32: u8 = 1;
const TAG_INT8: u8 = 2;

/// Persisted vector index metadata stored under [`KEY_META`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VecIndexMeta {
    pub vector_count: u64,
    pub dimensions: u32,
    pub metric: DistanceMetric,
    pub quantization: VectorQuantization,
}

impl VecIndexMeta {
    pub fn encode(&self) -> [u8; 14] {
        let mut out = [0u8; 14];
        out[0..8].copy_from_slice(&self.vector_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.dimensions.to_le_bytes());
        out[12] = self.metric as u8;
        out[13] = self.quantization as u8;
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 14 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "VecIndexMeta::decode",
                format!("expected 14-byte VecIndexMeta payload, got {}", bytes.len()),
            ));
        }
        let vector_count = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let dimensions = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let metric = DistanceMetric::from_u8(bytes[12])?;
        let quantization = VectorQuantization::from_u8(bytes[13])?;
        Ok(Self {
            vector_count,
            dimensions,
            metric,
            quantization,
        })
    }
}

/// Encodes a vector entry key: `0x01 || record_id (8 bytes BE)`.
pub fn encode_vec_key(record_id: RecordId) -> [u8; 9] {
    let mut key = [0u8; 9];
    key[0] = KEY_PREFIX_VEC[0];
    key[1..9].copy_from_slice(&record_id.raw().to_be_bytes());
    key
}

/// Decodes a [`RecordId`] from a vector entry key produced by [`encode_vec_key`].
pub fn decode_vec_key(key: &[u8]) -> Result<RecordId> {
    if key.len() != 9 || key[0] != KEY_PREFIX_VEC[0] {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "decode_vec_key",
            format!("invalid vector entry key of length {}", key.len()),
        ));
    }
    let raw = u64::from_be_bytes(key[1..9].try_into().unwrap());
    RecordId::new(raw)
}

/// Encodes a validated finite vector into its on-disk B+ tree payload using `quantization`.
pub fn encode_vector_payload(vec: &[f32], quantization: VectorQuantization) -> Vec<u8> {
    let norm = l2_norm(vec);
    match quantization {
        VectorQuantization::F32 => {
            let mut out = Vec::with_capacity(1 + 4 + vec.len() * 4);
            out.push(TAG_F32);
            out.extend_from_slice(&norm.to_le_bytes());
            for &v in vec {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out
        }
        VectorQuantization::Int8 => {
            let mut min_v = f32::INFINITY;
            let mut max_v = f32::NEG_INFINITY;
            for &v in vec {
                if v < min_v {
                    min_v = v;
                }
                if v > max_v {
                    max_v = v;
                }
            }
            let span = max_v - min_v;
            let scale = if span == 0.0 || !span.is_finite() {
                0.0f32
            } else {
                span / 255.0
            };

            let mut out = Vec::with_capacity(1 + 4 + 4 + 4 + vec.len());
            out.push(TAG_INT8);
            out.extend_from_slice(&norm.to_le_bytes());
            out.extend_from_slice(&min_v.to_le_bytes());
            out.extend_from_slice(&scale.to_le_bytes());
            for &v in vec {
                let q = if scale == 0.0 {
                    0u8
                } else {
                    ((v - min_v) / scale).round().clamp(0.0, 255.0) as u8
                };
                out.push(q);
            }
            out
        }
    }
}

/// Decoded vector payload containing its precomputed L2 norm and `f32` values.
#[derive(Debug, Clone)]
pub struct DecodedVector {
    pub norm: f64,
    pub values: Vec<f32>,
}

/// Decodes a vector payload produced by [`encode_vector_payload`], validating length against `expected_dim`.
pub fn decode_vector_payload(bytes: &[u8], expected_dim: usize) -> Result<DecodedVector> {
    if bytes.len() < 5 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "decode_vector_payload",
            "truncated vector payload header",
        ));
    }
    let tag = bytes[0];
    let norm = f64::from(f32::from_le_bytes(bytes[1..5].try_into().unwrap()));
    match tag {
        TAG_F32 => {
            let expected_len = 5 + expected_dim * 4;
            if bytes.len() != expected_len {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "decode_vector_payload",
                    format!(
                        "F32 vector payload length {} != expected {expected_len}",
                        bytes.len()
                    ),
                ));
            }
            let mut values = Vec::with_capacity(expected_dim);
            for chunk in bytes[5..].chunks_exact(4) {
                values.push(f32::from_le_bytes(chunk.try_into().unwrap()));
            }
            Ok(DecodedVector { norm, values })
        }
        TAG_INT8 => {
            let expected_len = 13 + expected_dim;
            if bytes.len() != expected_len {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "decode_vector_payload",
                    format!(
                        "Int8 vector payload length {} != expected {expected_len}",
                        bytes.len()
                    ),
                ));
            }
            let min_v = f32::from_le_bytes(bytes[5..9].try_into().unwrap());
            let scale = f32::from_le_bytes(bytes[9..13].try_into().unwrap());
            let mut values = Vec::with_capacity(expected_dim);
            for &q in &bytes[13..] {
                values.push(min_v + f32::from(q) * scale);
            }
            Ok(DecodedVector { norm, values })
        }
        other => Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "decode_vector_payload",
            format!("unknown vector payload encoding tag {other}"),
        )),
    }
}
