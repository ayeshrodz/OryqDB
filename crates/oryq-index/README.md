# `oryq-index`

Canonical memcomparable key encoding, index descriptors, owner-local provider registry, restricted plugin storage contexts, and primary `_id -> RecordId` index maintenance for **OryqDB** (`W07`, `W09`, `W13a`).

## Responsibilities (`W07`)
- **`key_codec`:** Order-preserving memcomparable byte encoding across all 12 `OryqDoc` types matching `oryq_document::compare_values`, exact variable-width cross-type numeric encoding (`Int64`, `Float64`, `Decimal128`), `SortOrder::{Asc, Desc}`, primary `_id` type validation (`ObjectId`, `Uuid`, `Int64`, `String <= 1024B`), and `RecordId` 8B BE codec.
- **`descriptor`:** `IndexDescriptor`, `IndexKind`, `IndexFieldSpec`, `IndexReadCapabilities`, and `MaintenanceResult`.
- **`provider` & `registry`:** Object-safe `IndexProvider` trait and owner-local `IndexProviderRegistry::default_v1()`.
- **`plugin`:** Restricted `IndexReadContext` (`PageStore`) and `IndexWriteContext` (`PageStoreMut`) without commit/checkpoint/file-handle authority.
- **`primary`:** `PrimaryIndexProvider` (`"index.primary"`), `lookup_primary_id`, and `scan_primary_record_ids` enforcing mandatory uniqueness and transactional B+ tree maintenance.
- **`secondary`:** `SecondaryIndexProvider` (`"index.secondary"`), single/nested-path extraction, `unique` collision checks, `sparse` missing-field omission, differential `on_update` bypass, and ordered equality/range scans (`scan_secondary_eq`, `scan_secondary_range`, `scan_secondary_all`).

## Dependencies
- Internal: `oryq-types`, `oryq-document`, `oryq-engine` (never `query`/`core`/`ffi`/`cli`).
- Safety: `#![forbid(unsafe_code)]`.

## Verification
```bash
cargo test -p oryq-index
cargo run -p xtask -- modules index
```
