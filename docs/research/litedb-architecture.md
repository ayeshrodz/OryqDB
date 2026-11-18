# LiteDB Reference Architecture Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)

---

## 1. Repository Provenance (Verified)

| Attribute | Verified Value |
| :--- | :--- |
| **Upstream URL** | `https://github.com/litedb-org/LiteDB.git` |
| **Analyzed Branch** | `dev` (v6 prerelease development branch) |
| **Pinned Commit SHA** | `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (committed `2026-10-26 21:40:26 +0100`) |
| **Latest Stable Tag** | `v5.0.22` (`8abcf944ae911e06eba76d7d9d8bec08a2955be1`) |
| **Latest Prerelease Tag** | `v6.0.0-prerelease.77` (`49f74a68fe3ee189d4bba08c413c3069e25d65d4`) |
| **License** | MIT (`LICENSE`, Copyright (c) 2014-2022 Mauricio David) |
| **Target Frameworks** | `netstandard2.0;net8.0;net10.0` (`LiteDB/LiteDB.csproj`) |
| **Runtime Dependencies** | Zero NuGet dependencies on `net8.0`/`net10.0`; 3 BCL polyfills on `netstandard2.0` (`System.Buffers`, `System.Memory`, `System.ComponentModel.TypeConverter`) |

> **Important Branch Note (`dev` vs. `v5.0.22`):** LiteDB's default `dev` branch represents the v6 prerelease engine (file formats v9–v13, gated at runtime by `LITEDB_PREDEV` / `LiteDBPragmas.EnsurePreDevRiskAcknowledged()` in `LiteDB/Client/Database/LiteDatabase.cs:47,66,91`). Compared to v5 stable (format v8), `dev` adds CRC32C page/WAL checksums, `DurableCommits = true` by default, CLOCK `MemoryCache` with `SharedPageReads`, partial MVCC WAL slot reclamation, `BsonType.Vector = 100`, compact document storage mode, `SharedEngine` coordinated reads, and experimental `CoordinatedEngine`.

---

## 2. High-Level Subsystem Architecture (Verified)

LiteDB is organized into three primary layers inside a single C# assembly (`LiteDB/`):

```text
+-------------------------------------------------------------------------+
| Client Layer (LiteDB/Client/)                                           |
| - LiteDatabase, LiteCollection<T>, LiteQueryable<T>, LiteRepository     |
| - BsonMapper, EntityBuilder<T>, LinqExpressionTranslator                |
| - SqlParser (SELECT/INSERT/UPDATE/DELETE/CREATE/DROP/PRAGMA/REBUILD)    |
| - LiteStorage (GridFS-style _files/_chunks), Vector extensions          |
| - Multi-Process Wrappers: SharedEngine (Mutex/MMap), CoordinatedEngine  |
+------------------------------------+------------------------------------+
                                     | ILiteEngine
                                     v
+-------------------------------------------------------------------------+
| Document & Expression Layer (LiteDB/Document/)                          |
| - BsonValue (boxed RawValue), BsonDocument (OrdinalIgnoreCase dict),    |
|   BsonArray, ObjectId, BsonVector (float[])                             |
| - BsonExpression (compiled LINQ Expression delegates + borrowed eval)   |
| - BsonSerializer, JsonSerializer, BsonNumberComparison (exact rational) |
+------------------------------------+------------------------------------+
                                     |
                                     v
+-------------------------------------------------------------------------+
| Storage & Query Engine Layer (LiteDB/Engine/)                           |
| - LiteEngine: TransactionMonitor (ThreadLocal), LockService             |
|   (TransactionGate + per-collection CollectionLock), WalIndexService    |
| - Services: Snapshot, DataService (DataBlock chains), IndexService      |
|   (32-level skip lists), VectorIndexService (HNSW-like), RebuildService |
| - Query: QueryExecutor, QueryOptimization (heuristic single-index),     |
|   QueryPipe, GroupByPipe, IndexAggregatePipe, TopNSort, SortService     |
| - Disk: DiskService, StreamPool, MemoryCache (CLOCK + SharedPageReads), |
|   NativeFileSync (F_FULLFSYNC/fsync/FlushFileBuffers), ChecksummedWal   |
+-------------------------------------------------------------------------+
```

### Key Verified Source Files
* **Engine Entry & Lifecycle:** `LiteDB/Engine/Engine/LiteEngine.cs`, `Transaction.cs`, `Insert.cs`, `Update.cs`, `Delete.cs`, `Checkpoint.cs`, `Rebuild.cs`, `Pragma.cs`.
* **Pages & Structures:** `LiteDB/Engine/Pages/{BasePage,HeaderPage,CollectionPage,DataPage,IndexPage}.cs`, `LiteDB/Engine/Structures/{DataBlock,IndexNode,CollectionIndex,VectorIndexNode}.cs`.
* **Buffer Pool & Disk I/O:** `LiteDB/Engine/Disk/{DiskService,DiskReader,MemoryCache,PageBuffer,PageFramePool,CacheReclaimer,SharedPageReads,StreamPool}.cs`, `NativeFileSync.cs`, `ChecksummedWalStream.cs`.
* **Transactions & WAL:** `LiteDB/Engine/Services/{TransactionMonitor,TransactionService,SnapShot,WalIndexService,LockService,TransactionGate,CollectionLock}.cs`.
* **Serialization & Query:** `LiteDB/Engine/Disk/Serializer/{BufferReader,BsonElementReader,BufferWriter,BorrowedDocumentReader}.cs`, `LiteDB/Engine/Query/{QueryExecutor,QueryOptimization,QueryPlan}.cs`, `LiteDB/Engine/Query/Pipeline/{BasePipe,QueryPipe,GroupByPipe,IndexAggregatePipe}.cs`.

---

## 3. Verification & Test Infrastructure (Verified)

LiteDB's `dev` branch invests heavily in adversarial testing:
* **`LiteDB.Tests`:** 724 `.cs` files containing 2,412 `[Fact]`/`[Theory]` tests across `Engine/` (148), `Issues/` (250), `Internals/` (98), `Query/` (82), `Safety/` (9 — `DurableLedger`, `OwnershipMonitor`, `DeadlineWatchdog`), plus `LiteDB.Tests.SharedMutexHarness` for real multi-process tests.
* **`LiteDB.Fuzz`:** 45 registered `IFuzzTarget` suites (`LiteDB.Fuzz/Program.cs:7-20`) covering WAL/page/power-loss crashes, checksums, MVCC retirement, compact codec, shared-process concurrency, rebuild transitions, and structural/semantic/vector integrity verifiers.
* **`LiteDB.Benchmarks`:** BenchmarkDotNet `0.13.12` suite measuring insertion, deletion, borrowed/indexed/vector queries, and memory management.

---

## 4. Architectural Lessons Applied to OryqDB

| Verified LiteDB Architecture Choice | Measured / Verified Limitation in LiteDB | OryqDB Architectural Counter-Measure |
| :--- | :--- | :--- |
| Monolithic single C# assembly (`LiteDB.dll`) | Tight coupling between `LiteEngine`, `BsonDocument`, `BsonExpression`, and `VectorIndexService` inside one compilation unit. | 8 compile-enforced baseline Rust crates (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + optional E1 provider crates (`oryq-fts`, `oryq-vec`) ([modular-architecture.md](../designs/modular-architecture.md)). |
| Managed .NET runtime & boxed `BsonValue.RawValue` | Every scalar `BsonValue` boxes onto the managed heap; requires CLR/.NET runtime. | Native Rust library + panic-safe C ABI (`include/oryq.h`) with zero-copy `DocumentView<'a>` and unaligned-safe scalar reads ([document-model.md](../document-model.md)). |
| 32-level skip-list indexes (`IndexService`) | Per-hop `IndexNode` + two `PageAddress[]` allocations and poor cross-page spatial locality; no native multi-column compound index. | Logical-page B+ trees with prefix-compressed compound keys ($R=16$ restart points) and exact variable-width memcomparable keys ([indexing.md](../indexing.md)). |
| Thread-bound `ThreadLocal<TransactionService>` | Incompatible with cross-thread handle movement or `async` workflows. | Explicit ownership handles (`ReadTransaction`, `WriteTransaction`) with movable `Send + !Sync` FFI handles backed by `Arc` state ([transactions.md](../transactions.md), [ffi.md](../ffi.md)). |
| Exact rational `BsonNumberComparison` (`BigInteger`) | Prevents silent `i64`/`f64`/`Decimal` ordering bugs — a strong correctness pattern worth preserving. | Adopted exact variable-width decimal significand/exponent memcomparable numeric encoding, verified by 26,809-case `Fraction` oracle in [validate_design_docs.py](../../tools/validate_design_docs.py). |
