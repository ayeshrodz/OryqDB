# OryqDB Baseline Benchmark Results (`W12` / `v0.6.0`)

Measured with `cargo run --release -p oryq-core --example bench_suite` on Linux `x86_64` (`rustc 1.99.1`, `8 KiB` pages, `SyncMode::Normal`).

| Workload | Total Time | Throughput |
| :--- | ---: | ---: |
| Batched Insert (`2,000` docs + 1 secondary index) | `151.53 ms` | `13,199 docs/sec` |
| Primary `_id` Point Lookup (`2,000` lookups) | `13.30 ms` | `150,359 ops/sec` |
| Partial Delta Updates (`1,200` ops, 6 rounds w/ 4-delta chain + compaction) | `66.22 ms` | `18,121 ops/sec` |
| Secondary Range Scan (`100` scans × ~`200` rows) | `140.47 ms` | `712 scans/sec` |
| WAL Checkpoint + Reset (`117` pages copied) | `0.60 ms` | — |

## Reproducing

```bash
cargo run --release -p oryq-core --example bench_suite
```
