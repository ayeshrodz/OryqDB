# OryqDB Binary Footprint, Baseline & SQLite Comparison Benchmarks (`v0.12.4`)

## 1. Release Binary Footprint Audit (`Linux x86_64`)

Measured on Linux `x86_64` (`strip = "symbols"`, `panic = "unwind"` preserved for FFI `catch_unwind` safety):

| Artifact | Profile | Features | Size on Disk |
| :--- | :--- | :--- | ---: |
| `liboryq.so` (C ABI v1.2) | `--profile release-small` (`opt-level="z"`, fat LTO) | minimal core | **`1.2 MiB`** |
| `liboryq.so` (C ABI v1.2) | `--profile release-small` (`opt-level="z"`, fat LTO) | `--features fts,vec` | **`1.4 MiB`** |
| `liboryq.so` (C ABI v1.2) | `--release` (`opt-level=3`, thin LTO) | minimal core | **`1.4 MiB`** |
| `liboryq.so` (C ABI v1.2) | `--release` (`opt-level=3`, thin LTO) | `--features fts,vec` | **`1.6 MiB`** |
| `oryq` CLI | `--profile release-small` (`opt-level="z"`, fat LTO) | minimal core | **`954 KiB`** |
| `oryq` CLI | `--profile release-small` (`opt-level="z"`, fat LTO) | `--features fts,vec` | **`1.1 MiB`** |
| `oryq` CLI | `--release` (`opt-level=3`, thin LTO) | minimal core | **`1.4 MiB`** |
| `oryq` CLI | `--release` (`opt-level=3`, thin LTO) | `--features fts,vec` | **`1.6 MiB`** |
| *Reference:* system `libsqlite3.so.0.8.6` | distro release | SQLite `3.46.1` | `1.5 MiB` |

> Under `--profile release-small`, both the minimal (`1.2 MiB`) and full hybrid FTS+Vector (`1.4 MiB`) `liboryq.so` shared libraries are **smaller than system `libsqlite3.so` (`1.5 MiB`)**, and the standalone `oryq` CLI fits under **1 MiB (`954 KiB`)**.

## 2. Single-File Comparison: OryqDB vs SQLite3 (`tools/bench_vs_sqlite.py`)

Measured with `cargo build --release -p oryq-ffi && ORYQ_LIB_PATH="$PWD/target/release/liboryq.so" python3 tools/bench_vs_sqlite.py --docs 2000` (Python `3.14.4`, SQLite `3.46.1`, Linux `x86_64`):

| Metric | OryqDB (C ABI via Python `ctypes`) | SQLite3 (CPython C extension) |
| :--- | ---: | ---: |
| Batched Insert (`2,000` JSON docs + 1 index) | `220.60 ms` (`9,066 docs/s`) | `8.03 ms` (`249,218 docs/s`) |
| Primary `_id` Point Lookups (`2,000` ops) | `55.47 ms` (`36,053 ops/s`) | `4.01 ms` (`498,431 ops/s`) |
| Secondary Index Lookups (`200` scans) | `165.00 ms` | `4.41 ms` |
| Batched Partial Updates (`500` ops) | `21.57 ms` | `0.84 ms` |
| `VACUUM` after 75% delete | `33.10 ms` | `0.29 ms` |
| Single-File Size (`100%` docs, sealed, 0 `-wal`) | `1,352.0 KiB` | `436.0 KiB` |
| Single-File Size (`25%` docs after `VACUUM`) | `360.0 KiB` (`-73.4%`) | `116.0 KiB` (`-73.4%`) |

> **Honest Interpretation:** In Python, `oryqdb` crosses `ctypes` + per-call JSON encoding/decoding for schemaless `OryqDoc` documents, whereas `sqlite3` is a compiled CPython C extension operating on a fixed-schema relational table. In native Rust (`§3` below), OryqDB delivers `153,614 ops/sec` point lookups and `19,972 ops/sec` durable single-fsync batched writes, while maintaining a single sealed `.oryq` file with `~73%` space reclamation on `VACUUM`.

## 3. Native Rust Storage & Query Baseline (`W12`, `SyncMode::Normal`)

Measured with `cargo run --release -p oryq-core --example bench_suite` on Linux `x86_64` (`8 KiB` pages, `temp_dir` / tmpfs):

| Workload | Total Time | Throughput |
| :--- | ---: | ---: |
| Batched Insert (`2,000` docs + 1 secondary index) | `159.92 ms` | `12,507 docs/sec` |
| Primary `_id` Point Lookup (`2,000` lookups) | `13.02 ms` | `153,614 ops/sec` |
| Partial Delta Updates (`1,200` ops, 6 rounds w/ 4-delta chain + compaction) | `70.08 ms` | `17,123 ops/sec` |
| Secondary Range Scan (`100` scans × ~`200` rows) | `151.35 ms` | `661 scans/sec` |
| WAL Checkpoint + Reset (`117` pages copied) | `0.56 ms` | — |

## 4. Comparative Group Commit Throughput (`X03` / `v0.10.3`, `SyncMode::Full`, 400 durable writes)

Comparing 8 concurrent writer threads × 50 durable inserts (`400` total inserts, `SyncMode::Full`, `linger = 0`):

| Mode | Total Time | Throughput | Speedup vs Individual |
| :--- | ---: | ---: | ---: |
| Concurrent Individual Commits (`begin_write` + `commit`, 8 threads) | `28.22 ms` | `14,175 ops/sec` | `1.00×` |
| Concurrent Group Commit (`Database::submit_batched`, 8 threads, `max_batch_size = 64`) | `25.25 ms` | `15,840 ops/sec` | `1.12×` |
| Explicit Single-Fsync Batch (`Database::commit_batch`, 400 ops) | `20.03 ms` | `19,972 ops/sec` | `1.41×` |

> **Note:** On `tmpfs`, `fsync` latency is sub-millisecond so CPU/serialization dominates; on physical SSD/NVMe storage where each `fsync` costs `0.5–5 ms`, coalescing `K` concurrent writers into a single WAL `fsync` reduces durable sync overhead proportionally by up to `K×`.

## Reproducing

```bash
# Native Rust storage & group-commit benchmarks
cargo run --release -p oryq-core --example bench_suite

# SQLite3 vs OryqDB single-file benchmark
cargo build --release -p oryq-ffi
ORYQ_LIB_PATH="$PWD/target/release/liboryq.so" python3 tools/bench_vs_sqlite.py --docs 2000

# Release size profiles
cargo build --profile release-small -p oryq-ffi -p oryq-cli
cargo build --profile release-small -p oryq-ffi -p oryq-cli --features fts,vec
```
