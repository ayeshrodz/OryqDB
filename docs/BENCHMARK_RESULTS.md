# OryqDB Binary Footprint, Baseline & SQLite Comparison Benchmarks (`v0.13.2`)

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

Measured with `cargo build --release -p oryq-ffi && ORYQ_LIB_PATH="$PWD/target/release/liboryq.so" python3 tools/bench_vs_sqlite.py --docs 2000` (Python `3.14.4`, SQLite `3.46.1`, Linux `x86_64`), showing the full **`v0.12.4` → `v0.13.2`** Performance Sprint progression (`#38`–`#40`):

| Metric | OryqDB `v0.12.4` | **OryqDB `v0.13.2` (C ABI via Python)** | Sprint Gain | SQLite3 (CPython C ext) |
| :--- | ---: | ---: | ---: | ---: |
| Batched Insert (`2,000` JSON docs + 1 index) | `220.60 ms` (`9,066/s`) | **`87.75 ms` (`22,791 docs/s`)** | **2.51× faster** | `9.96 ms` (`200,889 docs/s`) |
| Primary `_id` Point Lookups (`2,000` ops) | `55.47 ms` (`36,053/s`) | **`16.87 ms` (`118,545 ops/s`)** | **3.29× faster** | `4.08 ms` (`490,688 ops/s`) |
| Secondary Index Lookups (`200` scans) | `165.00 ms` | **`52.28 ms`** | **3.16× faster** | `7.64 ms` |
| Batched Partial Updates (`500` ops) | `21.57 ms` | **`9.44 ms`** | **2.28× faster** | `1.09 ms` |
| `VACUUM` after 75% delete | `33.10 ms` | **`17.18 ms`** | **1.93× faster** | `0.29 ms` |
| Single-File Size (`100%` docs, sealed, 0 `-wal`) | `1,352.0 KiB` | **`960.0 KiB`** | **-29.0% smaller** | `432.0 KiB` |
| Single-File Size (`25%` docs after `VACUUM`) | `360.0 KiB` | **`240.0 KiB`** (`-75.0%`) | **-33.3% smaller** | `116.0 KiB` (`-73.1%`) |

> **Honest Interpretation:** In Python, `oryqdb` crosses `ctypes` + per-call JSON encoding/decoding for schemaless `OryqDoc` documents, whereas `sqlite3` is a compiled CPython C extension operating on a fixed-schema relational table. Across the `v0.13.x` sprint (`#38`–`#40`), OryqDB achieved **2.3×–3.3×** faster Python C ABI throughput and **29%–33% smaller** single-file `.oryq` databases with zero on-disk format changes. In native Rust (`§3` below), OryqDB delivers **`743,582 ops/sec`** point lookups, **`3,671 scans/sec`** secondary range scans, and **`37,650 ops/sec`** durable single-fsync batched writes.

## 3. Native Rust Storage & Query Benchmarks (`SyncMode::Normal`)

Measured with `cargo run --release -p oryq-core --example bench_suite` on Linux `x86_64` (`8 KiB` pages, `temp_dir` / tmpfs):

| Workload | `v0.12.4` Baseline | **`v0.13.2` Result** | Throughput | Sprint Gain |
| :--- | ---: | ---: | ---: | ---: |
| Batched Insert (`2,000` docs + 1 secondary index) | `159.92 ms` | **`64.59 ms`** | **`30,962 docs/sec`** | **2.48× faster** |
| Primary `_id` Point Lookup (`2,000` lookups) | `13.02 ms` | **`2.69 ms`** | **`743,582 ops/sec`** | **4.84× faster** |
| Partial Delta Updates (`1,200` ops, 6 rounds w/ 4-delta chain + compaction) | `70.08 ms` | **`35.38 ms`** | **`33,915 ops/sec`** | **1.98× faster** |
| Secondary Range Scan (`100` scans × ~`200` rows) | `151.35 ms` | **`27.24 ms`** | **`3,671 scans/sec`** | **5.56× faster** |
| WAL Checkpoint + Reset (`76` pages copied vs `117` in `v0.12.4`) | `0.56 ms` | **`0.45 ms`** | — | **-35% pages** |

## 4. Comparative Group Commit Throughput (`X03`, `SyncMode::Full`, 400 durable writes)

Comparing 8 concurrent writer threads × 50 durable inserts (`400` total inserts, `SyncMode::Full`, `linger = 0`):

| Mode | `v0.12.4` | **`v0.13.2`** | Throughput | Speedup vs Individual |
| :--- | ---: | ---: | ---: | ---: |
| Concurrent Individual Commits (`begin_write` + `commit`, 8 threads) | `28.22 ms` | **`21.38 ms`** | **`18,708 ops/sec`** | `1.00×` |
| Concurrent Group Commit (`Database::submit_batched`, 8 threads, `max_batch_size = 64`) | `25.25 ms` | **`15.25 ms`** | **`26,222 ops/sec`** | `1.40×` |
| Explicit Single-Fsync Batch (`Database::commit_batch`, 400 ops) | `20.03 ms` | **`10.62 ms`** | **`37,650 ops/sec`** | `2.01×` |

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
