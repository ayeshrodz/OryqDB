# OryqDB Baseline & Group-Commit Benchmark Results (`v0.10.3`)

Measured with `cargo run --release -p oryq-core --example bench_suite` on Linux `x86_64` (`rustc 1.99.1`, `8 KiB` pages, `temp_dir` / tmpfs).

## Core Storage & Query Baseline (`W12`, `SyncMode::Normal`)

| Workload | Total Time | Throughput |
| :--- | ---: | ---: |
| Batched Insert (`2,000` docs + 1 secondary index) | `159.92 ms` | `12,507 docs/sec` |
| Primary `_id` Point Lookup (`2,000` lookups) | `13.02 ms` | `153,614 ops/sec` |
| Partial Delta Updates (`1,200` ops, 6 rounds w/ 4-delta chain + compaction) | `70.08 ms` | `17,123 ops/sec` |
| Secondary Range Scan (`100` scans × ~`200` rows) | `151.35 ms` | `661 scans/sec` |
| WAL Checkpoint + Reset (`117` pages copied) | `0.56 ms` | — |

## Comparative Group Commit Throughput (`X03` / `v0.10.3`, `SyncMode::Full`, 400 durable writes)

Comparing 8 concurrent writer threads × 50 durable inserts (`400` total inserts, `SyncMode::Full`, `linger = 0`):

| Mode | Total Time | Throughput | Speedup vs Individual |
| :--- | ---: | ---: | ---: |
| Concurrent Individual Commits (`begin_write` + `commit`, 8 threads) | `28.22 ms` | `14,175 ops/sec` | `1.00×` |
| Concurrent Group Commit (`Database::submit_batched`, 8 threads, `max_batch_size = 64`) | `25.25 ms` | `15,840 ops/sec` | `1.12×` |
| Explicit Single-Fsync Batch (`Database::commit_batch`, 400 ops) | `20.03 ms` | `19,972 ops/sec` | `1.41×` |

> **Note:** On `tmpfs`, `fsync` latency is sub-millisecond so CPU/serialization dominates; on physical SSD/NVMe storage where each `fsync` costs `0.5–5 ms`, coalescing `K` concurrent writers into a single WAL `fsync` reduces durable sync overhead proportionally by up to `K×`.

## Reproducing

```bash
cargo run --release -p oryq-core --example bench_suite
```
