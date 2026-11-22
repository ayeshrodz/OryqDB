//! Distance metrics, quantization modes, index options, and vector math helpers for `oryq-vec`.

use oryq_document::{from_json_str, ValueView};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "vec";

/// Maximum supported vector dimensionality per index entry.
///
/// Exact single-entry leaf fit is also validated against the database's configured [`PageSize`]
/// in `VectorIndexProvider::create_empty` (for example, 4 KiB pages fit up to ~1000 `F32`
/// dimensions or the full 1024 `Int8` dimensions; 8 KiB+ pages fit the full 1024 `F32` dimensions).
pub const MAX_VECTOR_DIMENSIONS: usize = 1024;

/// Supported vector similarity / distance metrics.
///
/// All metrics return a distance where **smaller is closer / more similar**:
/// - [`DistanceMetric::Cosine`]: `1.0 - cos_sim(a, b)` in `[0.0, 2.0]` (returns `1.0` if either norm is 0).
/// - [`DistanceMetric::L2`]: Euclidean distance `sqrt(sum((a_i - b_i)^2))` in `[0.0, +inf)`.
/// - [`DistanceMetric::DotProduct`]: negative inner product `-dot(a, b)` so larger dot product ranks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DistanceMetric {
    Cosine = 1,
    L2 = 2,
    DotProduct = 3,
}

impl DistanceMetric {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::L2 => "l2",
            Self::DotProduct => "dot",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "cosine" => Ok(Self::Cosine),
            "l2" => Ok(Self::L2),
            "dot" => Ok(Self::DotProduct),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "DistanceMetric::parse",
                format!(
                    "unsupported vector distance metric '{other}' (expected cosine, l2, or dot)"
                ),
            )),
        }
    }

    pub fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            1 => Ok(Self::Cosine),
            2 => Ok(Self::L2),
            3 => Ok(Self::DotProduct),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "DistanceMetric::from_u8",
                format!("unknown persisted DistanceMetric tag {other}"),
            )),
        }
    }
}

/// On-disk vector storage representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum VectorQuantization {
    /// Exact IEEE-754 32-bit little-endian floats (`4 * dim` payload bytes).
    #[default]
    F32 = 1,
    /// Per-vector min/max 8-bit unsigned scalar quantization (`8 + dim` payload bytes, ~4× smaller).
    Int8 = 2,
}

impl VectorQuantization {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::Int8 => "int8",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "f32" => Ok(Self::F32),
            "int8" => Ok(Self::Int8),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorQuantization::parse",
                format!("unsupported vector quantization '{other}' (expected f32 or int8)"),
            )),
        }
    }

    pub fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            1 => Ok(Self::F32),
            2 => Ok(Self::Int8),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "VectorQuantization::from_u8",
                format!("unknown persisted VectorQuantization tag {other}"),
            )),
        }
    }
}

/// Configuration options persisted in [`oryq_index::IndexDescriptor::options_json`] for `index.vec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorIndexOptions {
    pub dimensions: usize,
    pub metric: DistanceMetric,
    pub quantization: VectorQuantization,
}

impl VectorIndexOptions {
    /// Creates validated options with default exact [`VectorQuantization::F32`] storage.
    pub fn new(dimensions: usize, metric: DistanceMetric) -> Result<Self> {
        let opts = Self {
            dimensions,
            metric,
            quantization: VectorQuantization::F32,
        };
        opts.validate()?;
        Ok(opts)
    }

    /// Creates validated options with an explicit [`VectorQuantization`] mode.
    pub fn with_quantization(
        dimensions: usize,
        metric: DistanceMetric,
        quantization: VectorQuantization,
    ) -> Result<Self> {
        let opts = Self {
            dimensions,
            metric,
            quantization,
        };
        opts.validate()?;
        Ok(opts)
    }

    pub fn validate(&self) -> Result<()> {
        if self.dimensions == 0 || self.dimensions > MAX_VECTOR_DIMENSIONS {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexOptions::validate",
                format!(
                    "vector dimensions {} out of range 1..={MAX_VECTOR_DIMENSIONS}",
                    self.dimensions
                ),
            ));
        }
        Ok(())
    }

    /// Serializes options into canonical JSON for `IndexDescriptor::options_json`.
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"dim":{},"metric":"{}","quantization":"{}"}}"#,
            self.dimensions,
            self.metric.as_str(),
            self.quantization.as_str()
        )
    }

    /// Parses options from `IndexDescriptor::options_json`.
    pub fn from_json(raw: &str) -> Result<Self> {
        let (doc, _) = from_json_str(raw, None).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexOptions::from_json",
                format!("invalid vector index options JSON: {err}"),
            )
        })?;
        let view = doc.view();
        let dim_i64 = match view.get_field("dim") {
            Some(ValueView::Int64(n)) if n > 0 => n,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "VectorIndexOptions::from_json",
                    "vector index options missing positive integer 'dim'",
                ));
            }
        };
        let dimensions = usize::try_from(dim_i64).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexOptions::from_json",
                "vector index 'dim' exceeds platform usize",
            )
        })?;
        let metric_str = match view.get_field("metric") {
            Some(ValueView::String(s)) => s,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "VectorIndexOptions::from_json",
                    "vector index options missing string 'metric'",
                ));
            }
        };
        let metric = DistanceMetric::parse(metric_str)?;
        let quantization = match view.get_field("quantization") {
            Some(ValueView::String(s)) => VectorQuantization::parse(s)?,
            None => VectorQuantization::F32,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "VectorIndexOptions::from_json",
                    "vector index 'quantization' must be a string",
                ));
            }
        };
        let opts = Self {
            dimensions,
            metric,
            quantization,
        };
        opts.validate()?;
        Ok(opts)
    }
}

/// Validates that `vec` has length `expected_dim` and every element is finite (no `NaN` or `Inf`).
pub fn validate_vector(vec: &[f32], expected_dim: usize, op: &'static str) -> Result<()> {
    if vec.len() != expected_dim {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            op,
            format!(
                "vector length {} does not match index dimensionality {expected_dim}",
                vec.len()
            ),
        ));
    }
    for (idx, &v) in vec.iter().enumerate() {
        if !v.is_finite() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                op,
                format!("vector element at index {idx} is non-finite ({v})"),
            ));
        }
    }
    Ok(())
}

/// Computes the Euclidean L2 norm of `vec` in `f64` precision so large finite `f32` vectors
/// cannot overflow `f32::MAX`.
pub fn l2_norm(vec: &[f32]) -> f64 {
    let sum_sq: f64 = vec.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    sum_sq.sqrt()
}

/// Computes the distance between `query` (with precomputed `query_norm`) and `candidate`
/// (with precomputed `cand_norm`) under `metric`. Smaller values always mean closer matches.
pub fn compute_distance(
    metric: DistanceMetric,
    query: &[f32],
    query_norm: f64,
    candidate: &[f32],
    cand_norm: f64,
) -> f64 {
    debug_assert_eq!(query.len(), candidate.len());
    match metric {
        DistanceMetric::Cosine => {
            let denom = query_norm * cand_norm;
            if denom == 0.0 || !denom.is_finite() {
                return 1.0;
            }
            let dot: f64 = query
                .iter()
                .zip(candidate.iter())
                .map(|(&a, &b)| f64::from(a) * f64::from(b))
                .sum();
            let cos_sim = (dot / denom).clamp(-1.0, 1.0);
            1.0 - cos_sim
        }
        DistanceMetric::L2 => {
            let sum_sq: f64 = query
                .iter()
                .zip(candidate.iter())
                .map(|(&a, &b)| {
                    let d = f64::from(a) - f64::from(b);
                    d * d
                })
                .sum();
            sum_sq.sqrt()
        }
        DistanceMetric::DotProduct => {
            let dot: f64 = query
                .iter()
                .zip(candidate.iter())
                .map(|(&a, &b)| f64::from(a) * f64::from(b))
                .sum();
            -dot
        }
    }
}
