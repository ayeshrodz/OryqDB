# OryqDB Pre-`v1.0.0` Production Readiness Checklist (`v0.11.0`)

This checklist tracks every engineering, durability, security, ABI, and cross-platform gate completed across `v0.9.x`–`v0.11.0` prior to evaluating a `v1.0.0` release.

## 1. Core Engine & Durability
- [x] Single-file `.oryq` slotted-page B+ tree storage with dual Page 0 shadow headers (`W03`–`W04`).
- [x] Checksummed `.oryq-wal`, single-process MVCC snapshots, nested statement savepoints, and 7-point restartable crash recovery (`W05`).
- [x] 15-cycle crash/torn-WAL-tail stress suite, page bitflip `Corruption` detection, and MVCC snapshot-pinning WAL reset verification (`#13` / `v0.9.1`).
- [x] Deterministic malformed `OryqDoc` fuzz/property suite (`fuzz_malformed.rs`, zero panics across all truncation prefixes & bitflips, `#13`).
- [x] Sealed online `Database::backup_to`, `Database::integrity_check`, and non-mutating forensic `inspect_{page,wal,locks}` (`W14`).

## 2. Document Model, Indexing & Query Engine
- [x] `OryqDoc` binary format with sorted directory, exact cross-type & numeric ordering, and state-normalized delta-chain compaction (`MAX_DELTA_CHAIN = 4`, `W06`–`W12`).
- [x] Primary, secondary, compound (`R=16` prefix compression), and multi-key array indexes (`W09`, `W13a`).
- [x] Filter AST, rule-based planner with sparse/pluggable-index safety, lending cursors, `explain_analyze`, and memory-bounded spillable aggregation pipelines (`W10`, `W13b`).

## 3. Optional Extensions (`v0.10.x`)
- [x] Optional full-text search crate `oryq-fts` (`--features fts`, Okapi BM25, `write_bits = [0]`, reduced-build fail-closed verified, `#15` / `v0.10.0`).
- [x] Optional vector similarity search crate `oryq-vec` (`--features vec`, Cosine/L2/DotProduct, F32 & Int8 `f64`-scaled quantization, `write_bits = [1]`, hybrid `--features fts,vec` & reduced-build fail-closed verified, `#16` / `v0.10.1`).
- [x] Multi-process `.oryq-shm` coordination (`SharedProcessCoordinator`, reader slot table, horizon tracking, WAL-reset exclusion, OS-kernel-backed crash recovery tested with real child processes on Linux/macOS/Windows, `#17` / `v0.10.2`).
- [x] Queued write batching & single-fsync group commit (`oryq_engine::commit_batch`, GAT `GroupCommitQueue`, `Database::commit_batch`, `Database::submit_batched`, comparative release benchmarks, `#18` / `v0.10.3`).

## 4. C ABI, Language Bindings & CLI
- [x] Panic-safe C ABI v1.1 (`oryq-ffi` + `include/oryq.h`) with full CRUD, zero-copy cursor getters, aggregation, index DDL, online backup, and integrity check (`#14` / `v0.9.2`).
- [x] Official zero-dependency Python `ctypes` package (`bindings/python/oryqdb`) tested in CI across Linux, macOS, and Windows (`#19` / `v0.11.0`).
- [x] Native C99 (`bindings/c/quickstart.c`) and C++17 RAII (`bindings/cpp/quickstart.cpp`) examples compiled with `-Wall -Wextra -Werror` and executed in CI (`#19` / `v0.11.0`).
- [x] Zero-extra-dependency `oryq` CLI covering CRUD, queries, pipelines, checkpoint, backup, integrity-check, and forensic inspection (`W15`).

## 5. Safety, Governance & Multi-OS CI
- [x] `#![forbid(unsafe_code)]` across every crate except `oryq-ffi` (audited under Miri with zero UB/leaks).
- [x] `cargo audit` clean (0 vulnerabilities), MIT license only, MSRV `1.80.0` enforced.
- [x] `xtask architecture` boundary enforcement + `xtask modules` downstream checks + `tools/validate_design_docs.py` link/prototype checks.
- [x] Multi-OS CI (`ubuntu-latest`, `macos-latest`, `windows-latest`) covering default builds, `--no-default-features`, `--features fts`, `--features vec`, `--features fts,vec`, Python bindings, and native C99/C++17 FFI tests.
- [x] End-to-end production developer documentation ([Quickstart](guides/quickstart.md), [Query & Aggregation](guides/query-and-aggregation.md), [C ABI Guide](guides/c-abi-guide.md), [Operations & Recovery Runbook](guides/operations-and-recovery.md)).
