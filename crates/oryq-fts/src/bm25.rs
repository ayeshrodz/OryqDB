//! Deterministic Okapi BM25 full-text search execution over an FTS B+ tree index.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use oryq_engine::{BTree, ReadTxn, TreeKind};
use oryq_index::{IndexDescriptor, IndexReadContext};
use oryq_types::{NamespaceId, RecordId, Result};

use crate::codec::{
    decode_posting_record_id, decode_u32_le, encode_doc_len_key, encode_posting_prefix,
    CorpusStats, KEY_STATS,
};
use crate::tokenizer::tokenize;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// Ranked full-text search hit returned by [`search_fts`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FtsHit {
    pub record_id: RecordId,
    pub score: f64,
}

/// Executes an Okapi BM25 full-text search for `query` against `desc` within a read snapshot.
///
/// Results are sorted deterministically by `score` descending, breaking ties by `RecordId` ascending.
pub fn search_fts(
    tx: &ReadTxn<'_>,
    desc: &IndexDescriptor,
    query: &str,
    limit: usize,
) -> Result<Vec<FtsHit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let unique_terms: BTreeSet<String> = tokenize(query).into_iter().collect();
    if unique_terms.is_empty() {
        return Ok(Vec::new());
    }

    let ns = NamespaceId::new(desc.index_id.raw())?;
    let ctx = IndexReadContext::new(ns, tx);
    let tree = BTree::open(desc.root_page_id, TreeKind::Index);

    let stats = match tree.get(&ctx, KEY_STATS)? {
        Some(raw) => CorpusStats::decode(&raw)?,
        None => return Ok(Vec::new()),
    };
    if stats.doc_count == 0 || stats.total_tokens == 0 {
        return Ok(Vec::new());
    }

    let n = stats.doc_count as f64;
    let avgdl = (stats.total_tokens as f64) / n;

    let mut scores: BTreeMap<RecordId, f64> = BTreeMap::new();
    let mut doc_len_cache: BTreeMap<RecordId, f64> = BTreeMap::new();

    for term in &unique_terms {
        let prefix = encode_posting_prefix(term);
        let mut postings: Vec<(RecordId, u32)> = Vec::new();
        let mut cur = tree.cursor(&ctx);
        if cur.seek_ge(&prefix)? {
            while let Some((k, v)) = cur.current() {
                if !k.starts_with(&prefix) {
                    break;
                }
                let rid = decode_posting_record_id(k, &prefix)?;
                let tf = decode_u32_le(v, "bm25::search_fts")?;
                postings.push((rid, tf));
                if !cur.step_next()? {
                    break;
                }
            }
        }

        if postings.is_empty() {
            continue;
        }
        let df = postings.len() as f64;
        let idf = (((n - df + 0.5) / (df + 0.5)) + 1.0).ln();

        for (rid, tf_u32) in postings {
            let dl = match doc_len_cache.get(&rid) {
                Some(&cached) => cached,
                None => {
                    let dl_key = encode_doc_len_key(rid);
                    let dl_val = match tree.get(&ctx, &dl_key)? {
                        Some(raw) => f64::from(decode_u32_le(&raw, "bm25::doc_len")?),
                        None => avgdl,
                    };
                    doc_len_cache.insert(rid, dl_val);
                    dl_val
                }
            };
            let tf = f64::from(tf_u32);
            let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avgdl));
            let term_score = idf * ((tf * (BM25_K1 + 1.0)) / denom);
            *scores.entry(rid).or_insert(0.0) += term_score;
        }
    }

    let mut hits: Vec<FtsHit> = scores
        .into_iter()
        .map(|(record_id, score)| FtsHit { record_id, score })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.record_id.cmp(&b.record_id))
            .then(Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}
