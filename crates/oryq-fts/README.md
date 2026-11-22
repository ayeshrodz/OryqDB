# `oryq-fts`

Optional full-text search (`index.fts`) index provider crate for OryqDB (`X01a` / `v0.10.0`):

- `#![forbid(unsafe_code)]` with zero dependency on `oryq-core`.
- Unicode-aware lowercase alphanumeric tokenizer (`tokenizer.rs`).
- Inverted-index B+ tree persistence (`codec.rs`) storing corpus stats, per-document token counts, and `(term, RecordId) -> tf` postings via `oryq_index::{IndexReadContext, IndexWriteContext}`.
- Deterministic Okapi BM25 ranking (`bm25.rs`, `k1 = 1.2`, `b = 0.75`) with `RecordId` ascending tie-breaker.
- Allocates persistent `write_bits = [0]` (`FTS_FEATURE_BIT = 0`) in `architecture/capabilities.toml`.
