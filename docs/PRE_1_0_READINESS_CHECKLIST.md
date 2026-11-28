# OryqDB `v1.0.0` GA Production Readiness Checklist

This checklist tracks every engineering, durability, security, ABI, footprint, and cross-platform gate completed prior to freezing `.oryq` Format v1 & C ABI v1.3 for OryqDB **`v1.0.0` GA**.

## 1. Core Engine, Single-File Ergonomics & Durability
- [x] Single-file `.oryq` slotted-page B+ tree storage with dual Page 0 shadow headers (`W03`–`W04`).
- [x] Checksummed `.oryq-wal`, single-process MVCC snapshots, nested statement savepoints, and 7-point restartable crash recovery (`W05`).
- [x] Multi-page `OverflowStore` (`RECORD_KIND_OVERFLOW = 0x02`) supporting up to `16 MiB` documents (and 1024+ dim vectors) on default 8 KiB pages (`#44` / `v0.14.0`).
- [x] MVCC horizon-gated `FreelistManager` page reclamation & retirement inside `WriteTxn`, preventing deleted/dropped page leaks without requiring offline `VACUUM` (`#44` / `v0.14.0`).
- [x] Configurable automatic WAL checkpointing (`wal_autocheckpoint_bytes = 4 MiB` default), pre-sync `VersionIndex` capacity guards, and configurable `busy_timeout_ms` (`#44`, `#53`).
- [x] Deferred `WriteTxn::commit()` `XXH3_64` page checksums + zero-allocation leaf/internal serialization (`#48` / `v0.15.1`).
- [x] Pure-safe (`#![forbid(unsafe_code)]`), zero-dependency LZ4 raw block document compression (`RECORD_KIND_COMPRESSED = 0x03`) with configurable `compression_threshold_bytes` and OOM pre-allocation bomb guards (`#48` / `v0.15.1`).
- [x] Single-file auto-seal on close (`auto_checkpoint_on_close = true`), chunked bounded-memory `VACUUM` & online `backup_to` (`#28`, `#47`), `:memory:` pure-safe `MemoryVfs` (`#29`), and concurrent shared OS/process-local read-only locks (`#46`).
- [x] 15-cycle crash/torn-WAL-tail stress suite, page bitflip `Corruption` detection, deterministic malformed `OryqDoc` fuzz suite, and full multi-index `integrity_check` (`#13`, `#45`).

## 2. Document Model, Indexing & Query Engine
- [x] `OryqDoc` binary format with sorted directory, exact cross-type & numeric ordering, CSPRNG `ObjectId`/`UUID`, JSON/filter recursion depth limits, and state-normalized delta-chain compaction (`MAX_DELTA_CHAIN = 4`, `#45`).
- [x] Primary, secondary, compound, multi-key array, and sparse indexes with verified multi-hop dotted-path parity (`#45`).
- [x] Lazy O(1)-memory streaming `QueryCursor` & key-only `count()`, streaming k-way `BinaryHeap` external merge in `spillable_sort`, atomic statement-savepoint batch CRUD (`insert_many`, `update_one`, `update_many`, `delete_one`, `delete_many`, `upsert`), and `explain_analyze` (`#47` / `v0.15.0`).
- [x] **300-Query Randomized Differential Planner Oracle (`#49` / `v1.0.0`):** `crates/oryq-core/tests/differential_planner_fuzz.rs` verifies `PrimaryIdLookup`, `SecondaryEqScan`, `SecondaryRangeScan` (across Secondary, Compound, Multikey, Sparse, Asc/Desc, and nested dotted-path indexes), and `CollectionScan` match bit-for-bit across randomized documents, updates, deletes, sorts, pagination, and `count_matching`.

## 3. Optional Extensions
- [x] Optional full-text search crate `oryq-fts` (`--features fts`, Okapi BM25, `write_bits = [0]`, reduced-build fail-closed verified, `#15`).
- [x] Optional vector similarity search crate `oryq-vec` (`--features vec`, Cosine/L2/DotProduct, F32 & Int8 `f64`-scaled quantization, `write_bits = [1]`, hybrid `--features fts,vec` & reduced-build fail-closed verified, `#16`).
- [x] Multi-process `.oryq-shm` coordination (`SharedProcessCoordinator`, `#17`) and single-fsync group commit (`commit_batch`, `submit_batched`, `#18`).

## 4. C ABI v1.3, Official Language Bindings & CLI
- [x] Panic-safe C ABI v1.3 (`oryq-ffi` + `include/oryq.h`) with atomic `CLOSING_BIT` lifecycle CAS gate, `oryq_db_open_with_config`, `oryq_write_txn_try_begin`, `oryq_find_with_options_json`, `oryq_count_json`, batch CRUD, dotted-path zero-copy cursor getters, `VACUUM`, `:memory:`, online backup, integrity check, and optional hybrid FTS+Vector search (`#46`, `#47`, `#48`).
- [x] Official zero-dependency **Python** (`ctypes`), **Go** (`cgo`), **Node.js** (Node-API v8 + TypeScript), and **C# / .NET 8** (P/Invoke + `SafeHandle` + `ReaderWriterLockSlim`) bindings plus C99/C++17 examples compiled with `-Wall -Wextra -Werror` and tested in CI (`#19`, `#31`, `#46`, `#47`, `#53`).
- [x] Zero-extra-dependency `oryq` CLI (`1.1 MiB` under `--profile release-small`) covering CRUD, queries, pipelines, streaming JSONL `export`/`import`, index DDL, `count`, `explain`, `vacuum`, `fts-*`/`vec-*`, `checkpoint`, `backup`, `integrity-check`, and read-only forensic inspection (`#46`, `#47`).

## 5. Footprint, Safety, Governance & Multi-OS Release CI (`v1.0.0` GA Sign-Off)
- [x] **Release Binary Footprint Verified (`#48`):** Under `--profile release-small`, minimal `liboryq.so` is **`1.4 MiB`** (smaller than system `libsqlite3.so` at `1.5 MiB`), and `oryq` CLI is **`1.1 MiB`** (documented in [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md)).
- [x] **Reproducible SQLite & Native Rust Benchmarks (`#48`):** Published at `N = 2,000` and `N = 25,000` in [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md) (`41,286 docs/sec` native Rust batched insert, `855,440 ops/sec` point lookups, `48,828 ops/sec` single-fsync group commit).
- [x] **Nightly Miri Audit Passed (`#49`):** `cargo +nightly miri test` across `oryq-types` (7), `oryq-document` (16, incl. `fuzz_malformed`), and `oryq-ffi` (`ffi_rejects_null_arguments_and_catches_panics` + `:memory:` `ffi_v1_3_config_try_begin_batch_crud_find_options_count_and_nested_cursor` under `-Zmiri-disable-isolation`, 2): **25/25 passed, 0 UB, 0 leaks**.
- [x] **Security Audit Passed (`#49`):** `cargo audit` clean — **0 vulnerabilities** across 51 crate dependencies (1,571 RustSec advisories scanned); `#![forbid(unsafe_code)]` enforced everywhere except `oryq-ffi`; MIT license only; MSRV `1.80.0`.
- [x] **Architecture & Docs Governance:** `xtask architecture`, `xtask modules` (all crates), and `tools/validate_design_docs.py` passing.
- [x] **Multi-OS CI & Cross-Platform Release Packaging (`.github/workflows/ci.yml`, `.github/workflows/release.yml`):** Automated 4-job CI (`ubuntu-latest`, `macos-latest`, `windows-latest`, `Format/Clippy/Architecture`) and automated release packaging (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` + `SHA256SUMS.txt`).
