# OryqDB Pre-`v1.0.0` Production Readiness Checklist (`v0.12.4`)

This checklist tracks every engineering, durability, security, ABI, footprint, and cross-platform gate completed across `v0.9.x`–`v0.12.4` prior to evaluating a `v1.0.0` release.

## 1. Core Engine, Single-File Ergonomics & Durability
- [x] Single-file `.oryq` slotted-page B+ tree storage with dual Page 0 shadow headers (`W03`–`W04`).
- [x] Checksummed `.oryq-wal`, single-process MVCC snapshots, nested statement savepoints, and 7-point restartable crash recovery (`W05`).
- [x] Single-file auto-seal on close (`auto_checkpoint_on_close = true` by default on `DatabaseConfig`) removing empty `.oryq-wal` sidecars across Linux, macOS, and Windows (`#28` / `v0.12.0`).
- [x] Crash-safe, exclusive in-place `VACUUM` file compaction (`Database::vacuum`, `<db>-vacuum` durable recovery sidecar + deterministic open-time replay, `oryq_db_vacuum`, `oryq vacuum`, `#28` / `v0.12.0`).
- [x] Zero-disk in-memory ephemeral databases (`:memory:` / pure-safe `oryq_engine::MemoryVfs`) with zero-disk in-memory `VACUUM` and online `backup_to(disk_path)` (`#29` / `v0.12.1`).
- [x] 15-cycle crash/torn-WAL-tail stress suite, page bitflip `Corruption` detection, and MVCC snapshot-pinning WAL reset verification (`#13` / `v0.9.1`).
- [x] Deterministic malformed `OryqDoc` fuzz/property suite (`fuzz_malformed.rs`, zero panics across all truncation prefixes & bitflips, `#13`).
- [x] Sealed online `Database::backup_to`, `Database::integrity_check`, and non-mutating forensic `inspect_{page,wal,locks}` (`W14`).

## 2. Document Model, Indexing & Query Engine
- [x] `OryqDoc` binary format with sorted directory, exact cross-type & numeric ordering, and state-normalized delta-chain compaction (`MAX_DELTA_CHAIN = 4`, `W06`–`W12`).
- [x] Primary, secondary, compound (`R=16` prefix compression), and multi-key array indexes (`W09`, `W13a`).
- [x] Filter AST, rule-based planner with sparse/pluggable-index safety, lending cursors, `explain_analyze`, and memory-bounded spillable aggregation pipelines (`W10`, `W13b`).

## 3. Optional Extensions (`v0.10.x` & `v0.12.2`)
- [x] Optional full-text search crate `oryq-fts` (`--features fts`, Okapi BM25, `write_bits = [0]`, reduced-build fail-closed verified, `#15` / `v0.10.0`).
- [x] Optional vector similarity search crate `oryq-vec` (`--features vec`, Cosine/L2/DotProduct, F32 & Int8 `f64`-scaled quantization, `write_bits = [1]`, hybrid `--features fts,vec` & reduced-build fail-closed verified, `#16` / `v0.10.1`).
- [x] Hybrid FTS & Vector search exposed across feature-gated C ABI v1.2 (`oryq_create_fts_index`, `oryq_search_fts`, `oryq_create_vector_index`, `oryq_search_vector`, `oryq_cursor_get_score`), `oryq` CLI, and all language bindings (`#30` / `v0.12.2`).
- [x] Multi-process `.oryq-shm` coordination (`SharedProcessCoordinator`, reader slot table, horizon tracking, WAL-reset exclusion, OS-kernel-backed crash recovery tested with real child processes on Linux/macOS/Windows, `#17` / `v0.10.2`).
- [x] Queued write batching & single-fsync group commit (`oryq_engine::commit_batch`, GAT `GroupCommitQueue`, `Database::commit_batch`, `Database::submit_batched`, comparative release benchmarks, `#18` / `v0.10.3`).

## 4. C ABI v1.2, Official Language Bindings & CLI
- [x] Panic-safe C ABI v1.2 (`oryq-ffi` + `include/oryq.h`) with full CRUD, zero-copy cursor getters, aggregation, index DDL, `VACUUM`, `:memory:`, online backup, integrity check, and optional hybrid FTS+Vector search.
- [x] Official zero-dependency **Python** (`ctypes`, `bindings/python/oryqdb`), **Go** (`cgo`, `bindings/go`), **Node.js** (Node-API v8 + TypeScript, `bindings/node`), and **C# / .NET 8** (P/Invoke + `SafeHandle`, `bindings/dotnet`) bindings tested in CI (`#19`, `#31`).
- [x] Native C99 (`bindings/c/quickstart.c`) and C++17 RAII (`bindings/cpp/quickstart.cpp`) examples compiled with `-Wall -Wextra -Werror` and executed in CI.
- [x] Zero-extra-dependency `oryq` CLI (`954 KiB` under `--profile release-small`) covering CRUD, queries, pipelines, `vacuum`, `fts-*`/`vec-*`, `checkpoint`, `backup`, `integrity-check`, and forensic inspection.

## 5. Footprint, Safety, Governance & Multi-OS CI (`v0.12.4` Final Sign-Off)
- [x] **Release Binary Footprint Verified (`#32`):** Under `--profile release-small`, `liboryq.so` is `1.2 MiB` minimal / `1.4 MiB` hybrid (smaller than system `libsqlite3.so` at `1.5 MiB`), and `oryq` CLI is `954 KiB` minimal / `1.1 MiB` hybrid (documented in [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md)).
- [x] **Reproducible SQLite Comparison Benchmark (`#32`):** `tools/bench_vs_sqlite.py` verified and published in [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md).
- [x] **Miri Audit Passed (`#32`):** `cargo +nightly miri test` across `oryq-types` (7), `oryq-document` (16, incl. `fuzz_malformed`), and `oryq-ffi` (`ffi_rejects_null_arguments_and_catches_panics`, 1): **24/24 passed, 0 UB, 0 leaks**.
- [x] **Security Audit Passed (`#32`):** `cargo audit` clean — **0 vulnerabilities** across 51 crate dependencies (1,563 RustSec advisories scanned); `#![forbid(unsafe_code)]` enforced everywhere except `oryq-ffi`; MIT license only; MSRV `1.80.0`.
- [x] **Architecture & Docs Governance:** `xtask architecture`, `xtask modules` (all crates), and `tools/validate_design_docs.py` passing.
- [x] **Multi-OS CI (`ubuntu-latest`, `macos-latest`, `windows-latest`):** Covering default builds, `--no-default-features`, `--features fts`, `--features vec`, `--features fts,vec`, Python, Go, Node.js, C#/.NET, and C99/C++17 FFI tests.
- [x] End-to-end production developer documentation ([Quickstart](guides/quickstart.md), [Query & Aggregation](guides/query-and-aggregation.md), [C ABI Guide](guides/c-abi-guide.md), [Operations & Recovery Runbook](guides/operations-and-recovery.md)).
