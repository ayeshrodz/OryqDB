//! Nearest-neighbor vector similarity search over `index.vec` B+ tree namespaces.

use oryq_engine::{BTree, ReadTxn, TreeKind};
use oryq_index::{IndexDescriptor, IndexProvider, IndexReadContext};
use oryq_types::{NamespaceId, RecordId, Result};

use crate::codec::{decode_vec_key, decode_vector_payload, KEY_PREFIX_VEC};
use crate::distance::{compute_distance, l2_norm, validate_vector};
use crate::provider::{parse_descriptor_options, VectorIndexProvider};

/// A ranked vector search result pairing a [`RecordId`] with its distance (smaller is closer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    pub record_id: RecordId,
    pub distance: f64,
}

/// Executes a top-`limit` nearest-neighbor similarity search against an `index.vec` B+ tree.
///
/// Results are sorted deterministically by `(distance ASC, record_id ASC)`.
pub fn search_vec(
    tx: &ReadTxn<'_>,
    desc: &IndexDescriptor,
    query: &[f32],
    limit: usize,
) -> Result<Vec<VectorHit>> {
    VectorIndexProvider.validate_descriptor(desc)?;
    let options = parse_descriptor_options(desc)?;
    validate_vector(query, options.dimensions, "search_vec")?;

    if limit == 0 {
        return Ok(Vec::new());
    }

    let ns = NamespaceId::new(desc.index_id.raw())?;
    let ctx = IndexReadContext::new(ns, tx);
    let tree = BTree::open(desc.root_page_id, TreeKind::Index);

    let query_norm = l2_norm(query);
    let mut hits: Vec<VectorHit> = Vec::new();

    let mut cur = tree.cursor(&ctx);
    if cur.seek_ge(KEY_PREFIX_VEC)? {
        while let Some((raw_key, raw_val)) = cur.current() {
            if !raw_key.starts_with(KEY_PREFIX_VEC) {
                break;
            }
            let record_id = decode_vec_key(raw_key)?;
            let decoded = decode_vector_payload(raw_val, options.dimensions)?;
            let distance = compute_distance(
                options.metric,
                query,
                query_norm,
                &decoded.values,
                decoded.norm,
            );
            hits.push(VectorHit {
                record_id,
                distance,
            });
            if !cur.step_next()? {
                break;
            }
        }
    }

    hits.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.record_id.raw().cmp(&b.record_id.raw()))
    });
    hits.truncate(limit);
    Ok(hits)
}
