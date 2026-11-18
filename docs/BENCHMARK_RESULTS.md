# OryqDB Benchmark Methodology & Results Tracker

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Last Updated:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

---

## 1. Current Status

> **Engineering Honesty Notice ([docs/draft-spec.md Section 28](./draft-spec.md)):** No benchmark numbers are reported in this file yet because OryqDB is currently in the Milestone 0 Architecture & Design Phase. All performance figures will be populated in Milestone 7 using the reproducible harness in `benchmarks/`.

---

## 2. Benchmark Competitors & Fairness Rules

When executed in Milestone 7, the `benchmarks/` suite will compare **OryqDB** against four baseline engines on identical hardware and OS settings:

1. **SQLite (C API via `rusqlite`, WAL mode, `PRAGMA synchronous = NORMAL` and `FULL`):**
   * Tested both with normalized relational columns and with SQLite 3.45+ `JSONB` blobs + `json_extract()` expression indexes + `json_each()` array scans.
2. **LiteDB (.NET 8 Native AOT / Release build):**
   * Tested with equivalent BSON collections, primary `_id` lookups, secondary indexes, and partial updates.
3. **`redb` (Pure Rust Copy-on-Write B+ Tree Key-Value Store, + `serde_json` / `bson` serialization):**
   * Quantifies the write-amplification difference between physical shadow-paging CoW and OryqDB's logical-page WAL + state-normalized slotted-page deltas.
4. **RocksDB / LMDB (Optional Reference Baselines):**
   * Provides upper-bound sequential write (LSM) and zero-copy read (`mmap` CoW) reference points.

### Mandatory Fairness Controls ([docs/draft-spec.md Section 17.4](./draft-spec.md))
* **Equivalent Durability Tier:** Durable mode comparisons pair OryqDB (`DurabilityMode::FullSync`) against SQLite (`PRAGMA synchronous = FULL`) and `redb` (default durable commit); group-commit/normal comparisons pair identical `fdatasync` policies.
* **Equivalent Datasets & Indexes:** Every engine indexes the exact same document corpus (1,000; 10,000; 100,000; and 1,000,000 documents) with identical unique and non-unique secondary keys.
* **Warm-Cache vs. Cold-Cache Separation:** Cold-cache runs drop OS page caches or reopen fresh bounded buffer pools (`max_cache_bytes = 64 MiB`); warm-cache runs execute after a standardized pre-warming pass.

---

## 3. Standard Workload Matrix & Metrics to Record

| Workload ID | Workload Description | Primary Architectural Mechanism Tested | Metrics Recorded |
| :--- | :--- | :--- | :--- |
| **W1: Bulk & Single Inserts** | Insert 1K, 10K, 100K, 1M 1 KiB nested documents (1-doc tx vs. 1,000-doc batched tx). | `OryqDoc` directory encoding speed, B+ tree right-leaning `ObjectId` append, leader-follower group commit. | Ops/sec, p50/p95/p99 latency (µs), WAL bytes written, final `.oryq` file size (MiB). |
| **W2: Point & Nested Field Lookups** | Random point lookup by `_id` and by indexed `"email"`, extracting 1 scalar field (`"profile.age"`) out of a 40-field document. | `OryqDocView` $O(\log k)$ zero-copy field extraction vs. SQLite JSONB / LiteDB BSON $O(k)$ linear scan. | Ops/sec, p50/p95/p99 latency (µs), heap allocations per query, CPU cycles/op. |
| **W3: High-Frequency Partial Updates** | Execute 100,000 `$inc: {"login_count": 1}` and `$set: {"active": true}` updates on existing 2 KiB documents. | State-normalized slotted-page delta patches + `SlotDeltaFrame` WAL records + 64-bit bloom bitmask vs. full-document/full-page rewrites. | Ops/sec, p99 write latency (µs), disk write amplification factor ($\text{Bytes Written} / \text{Logical Delta Bytes}$), subsequent read latency on unmodified vs. modified fields. |
| **W4: Compound & Multi-Key Array Queries** | Range scan on `("status", "total")` and array containment query on `"tags"` (`$elemMatch` / equality). | Memcomparable keys, $R=16$ prefix-compression restart points, multi-key `RoaringBitmap` deduplication vs. SQLite `json_each()`. | Queries/sec, p50/p95/p99 latency (ms), index storage overhead (MiB). |
| **W5: Streaming Aggregation Pipeline** | `$match` -> `$group` (by `"customer_id"`, `$sum: "$total"`) -> `$sort` -> `$limit: 10` over 500,000 documents. | Zero-copy streaming `$match` + top-$K$ bounded heap coalescing + `max_query_memory_bytes` cap. | Pipeline execution time (ms), peak RSS / heap allocation (KiB). |
| **W6: Multi-Threaded Read/Write Concurrency** | 8 concurrent snapshot reader threads + 4 concurrent writer threads over 60 seconds. | Wait-free `SingleProcessCoordinator` + `PageVersionIndirectionMap` + leader-follower group commit. | Aggregate read ops/sec, aggregate write commits/sec, reader p99 tail latency during checkpointing. |
| **W7: Crash Recovery Duration** | Reopen database after simulated power loss with 10,000 uncheckpointed WAL frames. | `.oryq-wal` chained XXH3-64 verification and redo replay throughput. | Recovery duration (ms), memory used during recovery (KiB). |
