#![forbid(unsafe_code)]

//! `oryq-vec`: Optional vector similarity search index provider (`index.vec`) for OryqDB.
//!
//! Provides exact F32 and Int8 scalar-quantized vector persistence in B+ tree pages via
//! restricted [`oryq_index::IndexWriteContext`] / [`oryq_index::IndexReadContext`] namespaces,
//! supporting Cosine, L2 (Euclidean), and DotProduct distance metrics.

pub mod codec;
pub mod distance;
pub mod provider;
pub mod search;

pub use distance::{
    compute_distance, l2_norm, DistanceMetric, VectorIndexOptions, VectorQuantization,
    MAX_VECTOR_DIMENSIONS,
};
pub use provider::{vec_index_descriptor, VectorIndexProvider, VEC_FEATURE_BIT, VEC_PROVIDER_ID};
pub use search::{search_vec, VectorHit};
