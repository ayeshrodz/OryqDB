# LiteDB Feature Inventory & OryqDB Parity Matrix

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)
* **Pinned LiteDB Reference:** `https://github.com/litedb-org/LiteDB.git` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (`dev`, MIT)

---

## 1. Classification Tiers

Per [docs/draft-spec.md](../draft-spec.md) (Section 32 Step 4), every verified LiteDB feature is classified into one of five tiers:
* **`Required for MVP`** — Delivered in Phase 1 (`W01–W11` / M1–M6).
* **`Required for v1`** — Delivered in Phase 2–3 baseline (`W12–W16` / M7–M10).
* **`Optional`** — Post-baseline modular extension (`X01–X03` / E1–E3).
* **`Deferred`** — Post-v1 future opportunity (documented in `ROADMAP.md` / `oryqdb.md`).
* **`Not planned`** — Explicitly out of scope for OryqDB.

---

## 2. Verified LiteDB Feature Parity Matrix

| Subsystem | LiteDB Feature (Verified Source Path) | Classification | OryqDB Target Package & Notes |
| :--- | :--- | :--- | :--- |
| **Storage** | Single-file persistent database + WAL sidecar (`DiskService.cs`, `FileStreamFactory.cs`) | `Required for MVP` | `W03`, `W05` — `.oryq` + `.oryq-wal` (8192B slotted pages). |
| **Storage** | Page & WAL checksums (`PageChecksum.cs`, `WalChecksum.cs`, format v10+) | `Required for MVP` | `W03`, `W05` — XXH3-64 page & chained WAL frame checksums from day one. |
| **Storage** | Durable commits via OS sync (`NativeFileSync.cs`: `F_FULLFSYNC`/`fsync`/`FlushFileBuffers`) | `Required for MVP` | `W03`, `W05` — `oryq-engine/src/io/platform/`. |
| **Storage** | Overflow large-document storage (`DataBlock.cs`, `DataService.cs`, max ~15.9 MiB) | `Required for MVP` | `W03` — Whole-record overflow chains with bounded materialization. |
| **Storage** | Bounded CLOCK page cache (`MemoryCache.cs`, `CacheReclaimer.cs`) | `Required for MVP` | `W04` — Bounded Clock-Pro buffer pool (`max_cache_bytes`). |
| **Storage** | In-memory database mode (`MemoryStream` constructor) | `Required for MVP` | `W03` — Supported via `StorageIo` VFS abstraction. |
| **Storage** | Compact document storage mode (`DocumentStorageCodec.cs`, format v12) | `Required for v1` | `W12` — Superseded by `OryqDoc` + state-normalized slotted-page deltas. |
| **Storage** | Online / offline `Rebuild()` compaction (`RebuildService.cs`) | `Required for v1` | `W14` — Snapshot-consistent sealed `Database::backup_to()` + compaction. |
| **Storage** | AES-256-ECB encryption (`AesStream.cs`, marked insecure in LiteDB `README.md`) | `Deferred` | Post-v1 AEAD page encryption (AES-256-GCM / ChaCha20-Poly1305); ECB will never be implemented. |
| **Transactions** | Explicit ACID transactions (`BeginTrans`, `Commit`, `Rollback`) | `Required for MVP` | `W05`, `W08` — Explicit `ReadTransaction` / `WriteTransaction` handles with statement savepoints. |
| **Transactions** | MVCC snapshot isolation (`SnapShot.cs`, `WalIndexService.cs`) | `Required for MVP` | `W05` — Logical-page `PageVersionIndirectionMap` + admission mutex + reader tokens. |
| **Transactions** | Partial WAL checkpointing & slot reclamation (`WalIndexService.Checkpoint.cs`) | `Required for MVP` | `W05` — Checkpointing bounded by `min_active_reader_tx_id` with redundant reset intent. |
| **Concurrency** | Multi-threaded in-process reads + serialized writes (`LockService.cs`, `TransactionGate.cs`) | `Required for MVP` | `W05` — `SingleProcessCoordinator` with connection-lifetime OS lock + in-process file registry. |
| **Concurrency** | Multi-process `SharedEngine` (`Client/Shared/`) & experimental `CoordinatedEngine` (`Client/Coordinated/`) | `Optional` | `X02` (E2) — `SharedShmCoordinator` (`.oryq-shm` raw-pointer atomics + kernel byte-range heartbeat locks). |
| **Documents** | Typed binary document model (`BsonType.cs`: Null, Bool, Int64, Double, Decimal, String, Binary, DateTime, ObjectId, Guid, Array, Document) | `Required for MVP` | `W02` — `OryqDoc` v1 with 12-byte sorted field-offset directory ($O(\log k)$ zero-copy). |
| **Documents** | Exact cross-type numeric ordering (`BsonNumberComparison.cs`) | `Required for MVP` | `W02`, `W07` — Exact rational comparator + exact variable-width memcomparable numeric keys. |
| **Documents** | Auto-generated `_id` (`BsonAutoId`: Int32/Int64/ObjectId/Guid) | `Required for MVP` | `W02`, `W08` — `ObjectId` default + `Uuid`, `Int64`, `String` support. |
| **Documents** | JSON & Extended-JSON serialization (`JsonSerializer.cs`) | `Required for MVP` | `W02` — Deterministic JSON / Extended-JSON conversion. |
| **Documents** | Partial document updates (`UpdateMany` / `EXTEND`) | `Required for MVP` | `W06` (logical `$set`/`$unset`/`$inc`) and `W12` (slotted-page delta patches + `$push`/`$pull`). |
| **Documents** | Case-insensitive `BsonDocument` keys (`OrdinalIgnoreCase`) | `Not planned` | OryqDB uses standard case-sensitive UTF-8 JSON/MongoDB field names. |
| **Documents** | .NET POCO reflection `BsonMapper` & `DbRef` auto-include (`Client/Mapper/`) | `Not planned` | Replaced by Rust `serde` (`Serialize`/`Deserialize`) and `$lookup` pipeline joins. |
| **Indexing** | Primary `_id` unique index (`IndexService.cs`) | `Required for MVP` | `W07`, `W08` — Primary `_id` -> monotonic `u64` `RecordId` B+ tree. |
| **Indexing** | Single-field, nested-field & unique secondary indexes (`EnsureIndex`) | `Required for MVP` | `W09` — B+ tree secondary indexes with savepoint rollback. |
| **Indexing** | Multikey array indexes (`$.Tags[*]`) | `Required for v1` | `W13a` — Native multi-key B+ tree with `RoaringTreemap` `RecordId` dedup and parallel-array guard. |
| **Indexing** | Native prefix-compressed compound indexes (absent in LiteDB) | `Required for v1` | `W13a` — OryqDB differentiator ($R=16$ restart points). |
| **Indexing** | Vector search (`BsonType.Vector = 100`, `VectorIndexService.cs`, `Client/Vector/`) | `Optional` | `X01` (E1) — `oryq-vec` crate behind `vec` Cargo feature via WAL-backed `IndexProvider` (fixing LiteDB's $O(N^2)$ BFS upsert and filtered-ANN fallback). |
| **Indexing** | Full-text search (absent in LiteDB) | `Optional` | `X01` (E1) — `oryq-fts` crate behind `fts` Cargo feature via WAL-backed `IndexProvider`. |
| **Indexing** | Arbitrary ICU `Collation` (`Collation.cs`, `CollationFingerprint.cs`) | `Deferred` | Baseline uses canonical UTF-8 binary/ordinal collation; locale collations deferred post-v1. |
| **Query** | Filter predicates, projection, sorting, `Skip`/`Limit`, `Count`/`Exists` (`QueryPipe.cs`) | `Required for MVP` | `W10` — Streaming `Cursor<'tx>` with $O(\log k)$ `DocumentView` predicates. |
| **Query** | Explain plan diagnostics (`QueryPlan.GetExecutionPlan`) | `Required for MVP` | `W10` — `explain()` and `explain_analyze()` execution metrics. |
| **Query** | External merge sort (`SortService.cs`) & Top-N bounded heap (`TopNSort.cs`) | `Required for MVP` | `W10` (Top-$K$ heap) and `W13b` (external spill sort under `max_query_memory_bytes`). |
| **Query** | `GROUP BY` / `HAVING` / multi-stage aggregation (`GroupByPipe.cs`) | `Required for v1` | `W13b` — Full streaming/spillable pipeline (`$match`, `$project`, `$group`, `$sort`, `$limit`, `$skip`, `$unwind`, `$lookup`). |
| **Query** | SQL-like text parser (`Client/SqlParser/`) & .NET LINQ translator | `Not planned` | OryqDB uses structured MongoDB-style JSON/document filter and pipeline APIs (`docs/draft-spec.md` Section 25). |
| **Tooling** | Virtual system collections (`$database`, `$cols`, `$indexes`, `$transactions`, `$dump`, `$page_list`) | `Required for v1` | `W08`, `W14`, `W15` — Exposed via `Database::stats()`, `Database::integrity_check()`, `inspect()`, and `oryq` CLI forensic commands. |
| **Tooling** | `LiteStorage` chunked FileStorage (`_files`/`_chunks`) | `Deferred` | Large binary values fit up to `max_document_bytes` via overflow pages; dedicated chunked blob API deferred post-v1. |
| **Tooling** | Native C ABI (`include/oryq.h`) (absent in LiteDB) | `Required for MVP` | `W11` — Core OryqDB differentiator enabling C/Python/Go/Node/.NET bindings without a CLR. |
| **Tooling** | Standalone CLI (`LiteDB.Shell`) | `Required for v1` | `W15` — `oryq-cli` (`oryq` binary). |
| **APIs** | Async APIs, Change Streams, Triggers (absent in LiteDB) | `Deferred` | Change streams deferred post-v1 (`docs/designs/oryqdb.md`); core remains synchronous zero-runtime. |
