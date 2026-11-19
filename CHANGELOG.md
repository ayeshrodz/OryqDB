# Changelog

All notable changes to **OryqDB** will be documented in this file.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (`0.x.y` pre-1.0 phasing until the Milestone 10 baseline release candidate passes all native verification gates).

## [Unreleased]

### Planned
- `0.4.0-alpha.1` (`W09`): `oryq-index` secondary indexes and transactional differential maintenance.
- `0.4.0` (`W10` / M4–M5 Complete): `oryq-query` filter AST, access planner, lending cursors & `explain_analyze()`.

## [0.3.0] - 2026-11-19

### Added (`W08` / Milestone 3 Complete — First End-to-End CRUD Vertical Slice)
- Implemented `oryq-core` (`#![forbid(unsafe_code)]`, public embedded database facade and composition root):
  - **Database & Config (`database.rs`, `config.rs`):** Shareable `Database` (`open`, `open_with_io`, `begin_read`, `begin_write`, `try_begin_write`, `checkpoint`, `is_sealed`) and `DatabaseConfig`.
  - **Public API (`api/`):** `ReadTransaction`, `ReadCollection` (`find_by_id`, `find_all`, `count`), `WriteTransaction` (`create_collection`, `drop_collection`, `list_collections`, `commit`, `rollback`), and `WriteCollection` (`insert`, `insert_json`, `update_by_id`, `replace_by_id`, `delete_by_id`, `find_by_id`, `find_all`).
  - **Frozen v1 Catalog (`catalog/`):** Canonical `OryqDoc` catalog schema (`CATALOG_SCHEMA_VERSION = 1`) persisting collection roots, monotonic `RecordId` counters, required feature bitmasks, and per-index descriptors inside the transactional catalog B+ tree.
  - **Atomic Statement Orchestration (`write/`):** Statement-scoped `SavepointId` rollback across collection B+ tree, all registered indexes, `RecordId` counter, and catalog, plus automatic 12-byte monotonic `ObjectId` generation when `_id` is omitted.
  - **Composition Root & Adapters (`composition/`, `adapters/`):** `CoreRegistries::default_v1()` and `DocumentRecordCompactor` implementing `oryq-engine::RecordCompactor`.
- Added 3 external integration tests in `crates/oryq-core/tests/{vertical_slice,atomic_maintenance}.rs` (**53 total across workspace**), recorded `CC-0008` in `architecture/contract-changes.toml`, and added `xtask modules core` to CI.

## [0.3.0-alpha.2] - 2026-11-19

### Added (`W07` / M3 Primary Index & Key Codec)
- Implemented `oryq-index` (`#![forbid(unsafe_code)]`, depends on `oryq-types`, `oryq-document`, `oryq-engine`):
  - **Canonical Memcomparable Key Codec (`key_codec.rs`):** Order-preserving byte encoding across all 12 `OryqDoc` types matching `oryq_document::compare_values` (including exact variable-width `BigDec` cross-type numeric encoding across `Int64`, `Float64`, and `Decimal128`), `SortOrder::{Asc, Desc}`, primary `_id` type validation (`ObjectId`, `Uuid`, `Int64`, `String <= 1024B`), `max_index_key_bytes`, and 8-byte BE `RecordId` encoding.
  - **Descriptors, Provider & Registry (`descriptor.rs`, `provider.rs`, `registry.rs`):** `IndexDescriptor`, `IndexKind`, `IndexFieldSpec`, `IndexReadCapabilities`, `MaintenanceResult`, object-safe `IndexProvider`, and `IndexProviderRegistry::default_v1()`.
  - **Restricted Plugin Contexts (`plugin/context.rs`):** `IndexReadContext` (`PageStore`) and `IndexWriteContext` (`PageStoreMut`) scoped to a `NamespaceId` without commit/checkpoint/file-handle authority.
  - **Primary Index (`primary/`):** `PrimaryIndexProvider` (`"index.primary"`), `lookup_primary_id`, and `scan_primary_record_ids` enforcing mandatory uniqueness, `_id` immutability, empty-collection create/reopen/drop, and transactional B+ tree maintenance.
- Added 3 external integration tests in `crates/oryq-index/tests/{codec,conformance}.rs` (50 total across workspace), recorded `CC-0007` in `architecture/contract-changes.toml`, and added `xtask modules index` to CI.

## [0.3.0-alpha.1] - 2026-11-19

### Added (`W06` / M3 Document Updates)
- Implemented `oryq-document` update operators (`updates/`):
  - Owner-local `UpdateOperator` trait and `UpdateRegistry::default_v1()` registering `$set`, `$unset`, `$inc`, `$push`, and `$pull`.
  - Common pre-mutation validation in `UpdateSpec`: `_id` immutability (including `validate_replacement_doc`), ancestor/descendant and cross-operator path-conflict rejection, unknown operator rejection, checked `i64` overflow, lossless `i64 -> f64` (`|i| <= 2^53`) promotion, `Decimal128` `$inc` rejection (`UnsupportedFeature`), and canonical equality in `$pull`.
  - `apply_update` producing `NormalizedMutation` (`updated_doc`, `touched_top_level_fields`, binary `patch_bytes`, `bloom_mask`, `modified`).
- Implemented `oryq-document` state-normalized physical delta patches (`patch/`):
  - Normative binary encoder/decoder (`encode_physical_patch` / `decode_physical_patch`), 2-hash 64-bit `bloom_mask_for_top_level`, newest-first `resolve_path_with_patches`, and pure stateless `compact_document` with `BudgetHandle` accounting.
- Added 4 external integration tests in `crates/oryq-document/tests/updates.rs` (15 total in `oryq-document`) and recorded `CC-0006` in `architecture/contract-changes.toml`.

## [0.2.0] - 2026-11-19

### Added (`W05` / Milestone 2 Complete)
- Implemented `oryq-engine` WAL, single-process MVCC transactions, nested savepoints, checkpoint/reset, and restartable crash recovery:
  - **Engine Ports (`ports.rs`):** `RecordCompactor` pure compaction trait, `CommitReceipt`, and `CheckpointOutcome`.
  - **Snapshot Coordinator (`coordinator/`):** `SingleProcessCoordinator`, `SnapshotGuard`, and `WriterPermit` with atomic admission, nested same-thread readers, `max_readers` enforcement, single-writer exclusion, and uncertain-commit poisoning.
  - **WAL Recovery (`recovery/`):** `scan_wal` (chained `XXH3_64_with_seed` verification, incomplete-tail truncation, fail-closed corruption on bad complete frames) and restartable `replay_into_version_index` without domain callbacks.
  - **Checkpointing & Sealed Files (`checkpoint/`):** `copy_committed_pages_to_main` (horizon-bounded passive checkpoint), `unseal_for_first_write` (durable dual-slot `wal_required=1`), `reset_wal_after_full_checkpoint` (redundant dual-slot `reset_intent_seq` + seal), and `complete_interrupted_reset`.
  - **Transactions (`transaction/`):** `StorageEngine`, `ReadTxn` (`PageStore`), `WriteTxn` (`PageStoreMut`, nested `SavepointId` rollback/release with allocation rollback, auto-rollback on `Drop`, hard `max_wal_hard_bytes` cap, and `CommitOutcomeUnknown` poisoning on sync failure).
- Added 8 new external integration tests across `tests/admission.rs`, `tests/checkpoint.rs`, and `tests/recovery.rs` (19 total in `oryq-engine`) and recorded `CC-0005` in `architecture/contract-changes.toml`.

### Fixed
- Upgraded `.github/workflows/ci.yml` from `actions/checkout@v4` to `actions/checkout@v6` to eliminate GitHub Actions Node.js 20 deprecation warnings.
- Removed unused `homepage` field and unused `oryq-ffi` entry from root `Cargo.toml` to resolve Cargo manifest warnings on Windows CI.

## [0.2.0-alpha.1] - 2026-11-19

### Added (`W04` / M2 Foundation)
- Implemented `oryq-engine` bounded pin-aware page cache and MVCC version metadata (`cache/`):
  - `PageCache` & `PinnedPage`: LRU eviction bounded by `BudgetHandle` (`BudgetDomain::Cache`), skipping actively pinned `Arc<[u8]>` handles and returning `QuotaExceeded` rather than evicting pinned pages.
  - `VersionIndex` & `PageVersionEntry`: bounded per-page committed version chains with `resolve_for_snapshot` (`tx_id <= snapshot_id`) and horizon-safe `prune_below`.
- Implemented `oryq-engine` logical-page byte B+ tree (`btree/`):
  - `TreeKind`, `PageStore` / `PageStoreMut` / `PagerPageStore`, and normative leaf (`[key_len:u16][key][val_len:u32][val]`) & internal (`[sep_len:u16][sep][right_child:u64]` with sentinel slot 0) codecs.
  - `BTree` & `BTreeCursor`: point lookup, cycle/height-checked descent, upsert, multi-level leaf/internal/root splits, sibling borrow/merge deletion, root collapse, and stack-based forward/reverse range scans without on-disk sibling links.
- Added 4 external integration tests in `crates/oryq-engine/tests/btree_cache.rs` (including a 400-key randomized oracle fuzz/reopen/collapse test) and recorded `CC-0004` in `architecture/contract-changes.toml`.

## [0.1.0] - 2026-11-19

### Added (`W03` / Milestone 1 Complete)
- Implemented `oryq-engine` physical persistence foundation (`#![forbid(unsafe_code)]`, depends only on `oryq-types`, `xxhash-rust`, and `fs4`):
  - **VFS & PAL (`io/`):** `StorageIo` and `FileHandle` traits, `OsVfs` with isolated Unix (`pread`/`pwrite`, `(st_dev, st_ino)` identity, parent dir `fsync`) and Windows (`seek_read`/`seek_write`, canonical path identity) adapters, cross-platform exclusive OS locking (`fs4`), and process-local `FileRegistry` / `FileLease` alias protection (`LockedByProcess`).
  - **Persistent Binary Format Codecs (`format/`):** Authoritative little-endian encoders/decoders and `XXH3_64` checksum validators for Page 0 dual shadow `StorageHeader` (Slot A/B selection and failover), 32B `PageHeader`, 16B `SlotEntry` (`SlottedPageView` / `SlottedPageMut` with compaction and non-overlapping span checks), `OverflowHeader` / `OverflowRef`, `FreelistTrunkHeader` / `FreelistEntry`, and `.oryq-wal` (`WalHeader`, chained `WalFrameHeader`, `CommitPayload`).
  - **Pager, Allocator, Freelist & Overflow (`pager/`):** `Pager` (create/reopen across 4096/8192/16384B page sizes, alternating shadow-slot commits), `PageBuf` / `PageGuard` (RAII pins + `BudgetHandle`), `FreelistManager` (MVCC snapshot-horizon reuse gating `retired_tx_id < min_active_snapshot_tx_id` + chain validation), `PageAllocator`, and `OverflowStore` (bounded chain read/write/free with cycle detection).
- Added 7 external integration tests (`persistence.rs`, `locking.rs`), `FaultVfs` fault-injection harness, `tests/fixtures/README.md`, `CC-0003` in `architecture/contract-changes.toml`, and `xtask modules engine` in CI.

## [0.1.0-alpha.2] - 2026-11-19

### Added (`W02` / `oryq-document`)
- Implemented `oryq-document` (`#![forbid(unsafe_code)]`, depends only on `oryq-types`, `serde_json`, and `xxhash-rust` — independent of `oryq-engine`):
  - `OryqDoc` v1 binary encoder (`encode_document` with optional `BudgetHandle` accounting) and strict structural validator (`validate_document` checking header version/flags, `xxh3_64` hash-prefix directory sort order, non-overlapping spans, valid UTF-8, tag validity, and nesting depth `<= 128`).
  - Zero-copy `DocumentView<'a>` (`O(log k)` binary search over sorted 12B directory + dotted `FieldPath` resolution) and `ArrayView<'a>` (`O(1)` random access + `ExactSizeIterator`).
  - Core document types (`OryqTypeTag`, `ObjectId`, `Uuid`, IEEE 754-2008 BID `Decimal128`, `Value`, `ValueView<'a>`, `DocumentBuf`) and `FieldPath` validator (`MAX_PATH_DEPTH = 128`).
  - Canonical cross-type comparator (`compare_values`) with exact rational `BigUint` arithmetic across `Int64`, `Float64`, and `Decimal128` (`-0.0 == +0.0`, `NaN < -Inf`, `10 == 10.0 == Decimal("10.0")`, `Decimal("0.1") != 0.1_f64`).
  - Extended-JSON codec (`from_json_str`, `from_json_value`, `to_json_value`, `to_json_string`) supporting `$oid`, `$uuid`, `$date`, `$binary`, `$numberLong`, `$numberDouble`, and `$numberDecimal`.
- Added 11 external integration tests across `roundtrip.rs`, `ordering.rs`, and `malformed.rs`, plus `tests/fixtures/README.md`, `CC-0002` in `architecture/contract-changes.toml`, and `xtask modules document` in CI.

## [0.1.0-alpha.1] - 2026-11-19

### Added (`W01` / M1 Setup)
- Eight-crate baseline Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, with `rustfmt.toml`, `.clippy.toml`, and GitHub Actions CI workflow (`.github/workflows/ci.yml`) triggered on PRs and merges to `main`.
- Implemented `oryq-types` (`#![forbid(unsafe_code)]`, zero runtime dependencies): checked identity newtypes (`PageId`, `TxId`/`SnapshotId`, `CollectionId`, `IndexId`, `NamespaceId`, `RecordId`), boxed pointer-sized `ErrorEnvelope` and `ErrorCategory`, concurrent RAII `BudgetHandle` / `Reservation` tokens, `CancellationSource` / `CancellationToken`, and `CapabilityId` / `FeatureBitmask` / `VersionRange`.
- Authored `architecture/{modules,capabilities,contract-changes}.toml` (schema v1) and implemented `xtask architecture` (static dependency allowlist, cycle detection, prefix overlap, bit allocation, and `--base` / `--changed-paths` contract-change enforcement with 6 self-tests) and `xtask modules <id>` (owner tests + topological downstream `cargo check`).

## [0.0.2] - 2026-11-19

### Added (`W00` / M0 Complete)
- Pinned source-code audit of LiteDB (`https://github.com/litedb-org/LiteDB.git`, branch `dev`, commit `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`, latest stable tag `v5.0.22`, MIT) across six research reports:
  - [docs/research/litedb-architecture.md](docs/research/litedb-architecture.md)
  - [docs/research/litedb-feature-inventory.md](docs/research/litedb-feature-inventory.md)
  - [docs/research/litedb-storage-analysis.md](docs/research/litedb-storage-analysis.md)
  - [docs/research/litedb-query-analysis.md](docs/research/litedb-query-analysis.md)
  - [docs/research/litedb-concurrency-analysis.md](docs/research/litedb-concurrency-analysis.md)
  - [docs/research/litedb-performance-analysis.md](docs/research/litedb-performance-analysis.md)

### Changed
- Updated `VERSION` to `0.0.2`, marked `W00` (M0) as `Verified` in `docs/MODULE_DEVELOPMENT_PLAN.md`, and updated `docs/PROJECT_STATUS.md` and `docs/KNOWN_ISSUES.md` (`R11`).

## [0.0.1] - 2026-11-19

### Added
- Converged EGM master engine design specification ([docs/designs/oryqdb.md](docs/designs/oryqdb.md)) and modular technical specification ([docs/designs/modular-architecture.md](docs/designs/modular-architecture.md)).
- Eight per-module ownership and contract guides ([docs/modules/README.md](docs/modules/README.md)) and ordered module work-package plan ([docs/MODULE_DEVELOPMENT_PLAN.md](docs/MODULE_DEVELOPMENT_PLAN.md)).
- Eight normative subsystem specifications ([docs/architecture.md](docs/architecture.md), [docs/file-format.md](docs/file-format.md), [docs/document-model.md](docs/document-model.md), [docs/transactions.md](docs/transactions.md), [docs/indexing.md](docs/indexing.md), [docs/query-language.md](docs/query-language.md), [docs/ffi.md](docs/ffi.md), [docs/compatibility.md](docs/compatibility.md)).
- Architecture Decision Records ADR-001 through ADR-008 ([docs/ARCHITECTURE_DECISIONS.md](docs/ARCHITECTURE_DECISIONS.md)), product roadmap ([docs/ROADMAP.md](docs/ROADMAP.md)), risk register ([docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md)), benchmark specification ([docs/BENCHMARK_RESULTS.md](docs/BENCHMARK_RESULTS.md)), and competitor comparison ([docs/research/competitor-comparison.md](docs/research/competitor-comparison.md)).
- EGM Goldfish review reports under `docs/reviews/initial-design/` and `docs/reviews/modular-architecture/`.
- Automated design-doc validator ([tools/validate_design_docs.py](tools/validate_design_docs.py)) covering local link/fragment verification, 26,809-case exact numeric key ordering, 100-case prefix bounds, and C/C++ syntax checks of the proposed ABI header.
