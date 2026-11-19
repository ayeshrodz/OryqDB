# `oryq-core`

Public Rust embedded NoSQL database facade, versioned transactional catalog, atomic statement orchestration, and cross-owner composition root for **OryqDB** (`W08` / Milestone 3 Complete).

## Responsibilities (`W08`)
- **`database` & `config`:** Shareable `Database` handle (`open`, `open_with_io`, `begin_read`, `begin_write`, `try_begin_write`, `checkpoint`, `is_sealed`) and `DatabaseConfig`.
- **`api`:** `ReadTransaction`, `ReadCollection` (`find_by_id`, `find_all`, `count`), `WriteTransaction` (`create_collection`, `drop_collection`, `list_collections`, `commit`, `rollback`), and `WriteCollection` (`insert`, `insert_json`, `update_by_id`, `replace_by_id`, `delete_by_id`, `find_by_id`, `find_all`).
- **`catalog`:** Frozen v1 canonical `OryqDoc` catalog schema (`CATALOG_SCHEMA_VERSION = 1`) storing collection roots, monotonic `RecordId` high watermarks, required feature masks, and per-index descriptors inside the transactional catalog B+ tree.
- **`write`:** Statement-scoped `SavepointId` orchestration ensuring collection data, primary/secondary indexes, `RecordId` counter, and catalog commit or roll back atomically together; automatic 12-byte monotonic `ObjectId` generation when `_id` is omitted.
- **`composition` & `adapters`:** `CoreRegistries::default_v1()` composition root and `DocumentRecordCompactor` (`oryq-engine::RecordCompactor` adapter).

## Dependencies
- Internal: `oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query` (never `ffi`/`cli`).
- Safety: `#![forbid(unsafe_code)]`.

## Verification
```bash
cargo test -p oryq-core
cargo run -p xtask -- modules core
```
