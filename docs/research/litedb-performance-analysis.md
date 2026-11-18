# LiteDB Performance Bottlenecks & Benchmark Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)
* **Pinned LiteDB Reference:** `https://github.com/litedb-org/LiteDB.git` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (`dev`, MIT)

---

## 1. Existing LiteDB Benchmark Suite (Verified)

LiteDB ships a BenchmarkDotNet `0.13.12` project in `LiteDB.Benchmarks/` (`net10.0`, `MemoryDiagnoser.Default`, `GcForce(true)` in `LiteDB.Benchmarks/Program.cs`):
* **Insert / Delete:** `InsertionBasicBenchmark`, `InsertionIgnoreExpressionPropertyBenchmark`, `InsertionInMemoryBenchmark`, `DeletionBenchmark`.
* **Query & Vector:** `BorrowedBsonQueryBenchmark`, `QueryAllBenchmark`, `QueryCompoundIndexBenchmark`, `QueryCountBenchmark`, `QueryIgnoreExpressionPropertiesBenchmark`, `QueryMultipleParametersBenchmark`, `QuerySimpleIndexBenchmarks`, `QueryWithDateTimeOffsetBenchmark`, `QueryWithVectorSimilarity`.
* **Memory:** `MemoryManagementBenchmarks`.

> **Engineering Honesty Note:** No LiteDB or OryqDB runtime benchmarks have been executed in Milestone 0 (`W00`). Section 2 documents **verified structural bottlenecks** directly present in LiteDB's C# source code; Section 3 lists **testable hypotheses** to be measured empirically in Milestone 7 (`W12`) using `benchmarks/` and recorded in [docs/BENCHMARK_RESULTS.md](../BENCHMARK_RESULTS.md).

---

## 2. Verified Structural Performance Bottlenecks in LiteDB Source

| Bottleneck Area | Verified Mechanism & Source Citation | Cost Driver |
| :--- | :--- | :--- |
| **1. Skip-List Per-Hop Heap Allocations** | `IndexService.GetNode()` -> `IndexPage.GetIndexNode()` -> `new IndexNode(...)` (`IndexNode.cs:106-127`) allocates a new `IndexNode`, two `PageAddress[]` arrays (`Prev`/`Next`), and a boxed `BsonValue` key on every hop during traversal. | Managed heap allocation & GC pressure on every indexed search; poor spatial locality across random 8 KiB `IndexPage`s. |
| **2. Linear BSON Scanning & Non-Skipping `SkipContainer`** | `BufferReader.ReadDocument()` (`BufferReader.cs:393-431`) does not early-exit after finding all requested fields, and `BufferReader.SkipContainer()` (`BufferReader.cs:470-489`) walks nested elements recursively instead of jumping container length. | $O(\text{document bytes})$ CPU work even when projecting a single scalar field; `BorrowedDocumentReader` mitigates only ASCII scalar paths on raw BSON. |
| **3. Full-Document Rewrite on Partial Update** | `DataService.Update()` (`DataService.cs:87-162`) and `UpdateMany()` (`Update.cs:50-93`) deserialize, mutate, re-serialize, and overwrite the full `DataBlock` chain for any field change. | High write amplification and WAL volume for `$set`/`$inc` workloads on medium/large documents. |
| **4. Double Document Loads in Sorting** | Non-borrowed `BasePipe.OrderBy()` / `OrderTopN()` (`BasePipe.cs:245-260`, `BasePipe.TopN.cs:13-29`) loads documents once to extract the sort key and reloads surviving documents by `PageAddress`. | $2\times$ document reads and BSON deserializations on unindexed sorted queries. |
| **5. Single-Monitor Cache & Full Checkpoint Cache Wipe** | `MemoryCache._sync` (`MemoryCache.cs:18,71,221`) serializes misses, CLOCK sweeps (up to `2 * TotalPages` scans), and writable frame acquisition; full checkpoint calls `_disk.Cache.Clear()` (`WalIndexService.Checkpoint.cs:201`). | Lock convoying under high miss rates and post-checkpoint cold-cache latency spikes. |
| **6. Synchronous Per-Commit `fsync` Without Group Commit** | `TransactionService.Commit()` (`TransactionService.cs:314`, `DiskService.WalWrite.cs:165`) serializes commits on `lock (_header)` + `WalWriterLock` and runs one `FlushToDisk()` per commit. | Throughput ceiling under concurrent durable writers (`DurableCommits = true`). |
| **7. $O(N^2)$ Vector Upsert & Unfiltered-Only ANN** | `VectorIndexService.Upsert()` (`VectorIndexService.cs:42-55`) calls `Delete()` -> `TryFindNode()` (`VectorIndexService.cs:488-503`), running a full graph BFS on every insert; any `WHERE` filter forces full PK collection scan (`QueryOptimization.Vector.cs:53-58`). | Quadratic insert scaling and linear full-scan fallback on hybrid metadata + vector queries. |

---

## 3. Hypotheses & Target Benchmark Workloads for Milestone 7 (`W12`)

The following hypotheses will be tested and verified (or refuted) with reproducible measurements in `W12` ([docs/BENCHMARK_RESULTS.md](../BENCHMARK_RESULTS.md)):

1. **Hypothesis H1 (Point Lookup & Single-Field Projection — Workload W2):** `OryqDoc`'s 12-byte sorted field-offset directory ($O(\log k)$ binary search with zero heap allocation on `DocumentView`) and B+ tree index pages will yield lower p50/p99 latency and zero per-hop allocations compared to LiteDB's skip-list `IndexNode` allocations and linear BSON scan.
2. **Hypothesis H2 (High-Frequency Partial Updates — Workload W3):** OryqDB's write-time state-normalized slotted-page delta patches (`MAX_DELTA_CHAIN = 4`, 64-bit bloom bitmask, `SlotDeltaFrame` WAL frames) will reduce WAL bytes written and p99 update latency on 2 KiB documents compared to LiteDB's full-document `DataService.Update()` rewrite and SQLite's `jsonb_set()` full-row rewrite.
3. **Hypothesis H3 (Compound & Multi-Key Queries — Workload W4):** OryqDB's native prefix-compressed compound B+ trees ($R=16$) and `RoaringTreemap` multi-key deduplication will reduce index footprint and eliminate full scans on multi-column prefix + range queries that LiteDB cannot satisfy with single-expression skip lists.
4. **Hypothesis H4 (Post-Checkpoint Tail Latency — Workload W6):** Retaining clean pages in `oryq-engine`'s sharded Clock-Pro buffer pool across checkpoints will avoid the post-checkpoint p99 latency spike caused by LiteDB's `_disk.Cache.Clear()`.
