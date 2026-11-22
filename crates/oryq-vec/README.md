# `oryq-vec`

Optional vector similarity search index provider crate (`X01b`) for **OryqDB**.

- Implements `oryq_index::IndexProvider` under capability ID `index.vec` (`oryq.index.vector.v1`, persistent write feature bit `1`).
- Supports **Cosine**, **L2 (Euclidean)**, and **DotProduct** distance metrics with optional **Int8 scalar quantization** (4× smaller on-disk B+ tree footprint) or exact **F32** storage.
- Stores vector metadata and normalized/quantized vector entries inside restricted `IndexWriteContext` / `IndexReadContext` B+ tree namespaces.
- `#![forbid(unsafe_code)]`, zero external dependencies beyond workspace crates.
- Activated optionally in `oryq-core` via `--features vec` (or combined `--features fts,vec`).
