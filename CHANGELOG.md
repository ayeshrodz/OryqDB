# Changelog

All notable changes to **OryqDB** will be documented in this file.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (`0.x.y` pre-1.0 phasing until the Milestone 10 baseline release candidate passes all native verification gates).

## [Unreleased]

### Planned
- `0.1.0` (`W03` / M1 Complete): `oryq-engine` VFS/PAL, Page 0 dual shadow headers, 8192B slotted pager, freelist, and overflow chains.

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
