#![forbid(unsafe_code)]

//! `oryq-fts`: Optional full-text search (Okapi BM25 inverted index) provider crate for OryqDB
//! (`X01a` / `v0.10.0`).

pub mod bm25;
pub mod codec;
pub mod provider;
pub mod tokenizer;

pub use bm25::{search_fts, FtsHit};
pub use codec::CorpusStats;
pub use provider::{fts_index_descriptor, FtsIndexProvider, FTS_FEATURE_BIT, FTS_PROVIDER_ID};
pub use tokenizer::{count_terms, tokenize, MAX_TOKEN_BYTES};
