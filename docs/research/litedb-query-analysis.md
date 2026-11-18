# LiteDB Document Serialization, Indexing & Query Engine Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)
* **Pinned LiteDB Reference:** `https://github.com/litedb-org/LiteDB.git` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (`dev`, MIT)

---

## 1. Document Model & Serialization (Verified)

* **Boxed `BsonValue` & Case-Insensitive `BsonDocument`:**
  * `BsonType` (`LiteDB/Document/BsonType.cs:6-31`) defines 15 standard types (`MinValue=0`..`MaxValue=14`) plus `Vector=100` (`BsonVector` wrapping `float[]`, `BsonVector.cs:5-18`).
  * `BsonValue.RawValue` (`BsonValue.cs:46`) boxes every primitive scalar onto the managed heap.
  * `BsonDocument` (`BsonDocument.cs:14-17`) uses `Dictionary<string, BsonValue>(StringComparer.OrdinalIgnoreCase)`, making field lookups and document comparisons case-insensitive despite an outdated XML doc comment claiming case sensitivity.
* **Exact Numeric Comparison (`BsonNumberComparison.cs:11-90`):**
  * Comparing mixed numeric types (`Int32`, `Int64`, `Double`, `Decimal`) uses exact value comparison without lossy rounding: integral values within `[-2^53, 2^53]` compare as `double`, while values outside that range or fractional decimals convert to exact `BigInteger` rational fractions and cross-multiply. Equal numeric values across types hash identically via GCD reduction.
* **Linear BSON Scanning & Partial Read Behavior (`BufferReader.cs:393-489`, `BorrowedDocumentReader.cs:10-271`):**
  * BSON has no field offset directory. Even when `BufferReader.ReadDocument(HashSet<string> fields, ...)` locates every requested field (`remaining.Count == 0`), it **does not early-exit** — it continues scanning through the end of the document to validate container boundaries (`BufferReader.cs:408-423`).
  * `BufferReader.SkipContainer` (`BufferReader.cs:470-489`) walks every nested element recursively rather than jumping `length - 5` bytes.
  * `BorrowedDocumentReader` (added on `dev`) reduces allocations for up to 64 ASCII scalar paths on raw BSON using a `ulong` bitmask, but remains linear forward-only and falls back to full deserialization for non-ASCII keys (`> 0x7F`), compact-encoded documents, arrays, or `INCLUDE`s.

---

## 2. Indexing Architecture (Verified)

* **32-Level Doubly-Linked Skip Lists (`IndexService.cs`, `IndexNode.cs`, `IndexPage.cs`):**
  * `Constants.MAX_LEVEL_LENGTH = 32` (`Constants.cs:52`); level height is chosen by a `p = 0.5` coin-flip loop (`IndexService.Flip`, `IndexService.cs:180-191`).
  * Each `IndexNode` (`IndexNode.cs:16-26, 81-101`) has a 12-byte fixed header (`Slot`, `Levels`, 5B `DataBlock`, 5B `NextNode`), `Levels * 10` bytes of interleaved `Prev`/`Next` `PageAddress` pointers (up to 320 bytes), and a key payload capped at `MAX_INDEX_KEY_LENGTH = 1023` bytes (`Constants.cs:57`).
  * `IndexNode.NextNode` chains all index nodes belonging to one document starting at the PK (`_id`) node (`Insert.cs:75-92`), allowing `Update`/`Delete`/`DropIndex` to find all index entries without re-evaluating expressions.
* **Multikey & Compound Index Limitations:**
  * Multikey array expressions (`$.Tags[*]`) are supported for non-unique indexes (`Engine/Index.cs:56`).
  * **No native multi-column compound index structure exists.** Users approximate compound indexes with composite expressions (`{a: $.a, b: $.b}`), which cannot satisfy single-column prefix seeks or mixed ASC/DESC range bounds.
* **Vector Indexes on `dev` (`VectorIndexService.cs`, `VectorIndexNode.cs`, `QueryOptimization.Vector.cs`):**
  * Persisted HNSW-like graph with hardcoded `MaxLevels = 4`, `MaxNeighborsPerLevel = 8`, `EfConstruction = 24`, `DefaultEfSearch = 32`.
  * **Verified $O(N^2)$ insert/upsert scaling:** `VectorIndexService.Upsert` (`VectorIndexService.cs:42-55`) unconditionally calls `Delete(metadata, dataBlock)` before `Insert` (even on fresh inserts at `Insert.cs:96`), and `Delete` calls `TryFindNode` (`VectorIndexService.cs:488-503`), which executes a **full BFS over the entire HNSW graph** because no `DataBlock -> VectorIndexNode` reverse lookup exists.
  * **Verified filtered-ANN fallback:** `QueryOptimization.Vector.cs:13-14, 53-58` disables HNSW ANN search for SQL queries, `OFFSET > 0`, `GROUP BY`, `INCLUDE`, or any query with additional `WHERE` predicates, falling back to a full PK collection scan (`VectorIndexQuery.Scan`).

---

## 3. Query Planning, Pipelines & Sorting (Verified)

* **Heuristic Single-Index Planner (`QueryOptimization.cs:193-253`, `QueryOptimization.Index.cs`):**
  * Selects at most **one** index per query using static hardcoded costs (`IndexEmpty=0`, `Vector=1`, `Equals=1/10`, `Like=10/100`, `Range=20`, `NotEquals/Scan=80`, `All=100`) with zero cardinality statistics or multi-index intersection.
  * Supports single-field covering index lookup (`IsIndexKeyOnly`, `QueryOptimization.cs:244-248`) and `IndexAggregatePipe` for `COUNT(*)`/`EXISTS`.
* **Execution Pipelines (`QueryPipe.cs`, `GroupByPipe.cs`, `BasePipe.cs`):**
  * Pushes `Offset`/`Limit` directly onto `IndexNode`s when no residual filters, `IncludeBefore`, or `OrderBy` exist (`QueryPipe.cs:38-43`).
  * `TopNSort` (`TopNSort.cs:9-77`) uses a bounded heap only when `offset + limit <= 1024`. Larger sorts use `SortService` (`SortService.cs:22-200`) with `800 KiB` containers (`CONTAINER_SORT_SIZE = 100 * PAGE_SIZE`) spilled to `SortDisk`, still capped at `MAX_INDEX_KEY_LENGTH = 1023` bytes per sort key.
  * `GroupByPipe` (`GroupByPipe.cs:11-292`) has no hash aggregation — unindexed `GROUP BY` always pre-sorts, and post-group `ORDER BY` (`OrderGroupedResult`, lines 241-290) buffers all grouped results in memory without spilling.
  * Non-borrowed `OrderBy`/`TopNSort` loads documents twice: once to extract the sort key, and a second time by `PageAddress` for surviving rows (`BasePipe.cs:245-260`).
  * Multikey/non-unique deduplication (`Index.cs:64-65`) uses an unbounded in-memory `HashSet<PageAddress>` (`DistinctBy`).

---

## 4. Proposed OryqDB Improvements Over LiteDB's Query & Index Engine

1. **$O(\log k)$ Zero-Copy Field Extraction:** `OryqDoc`'s 12-byte sorted field-offset directory eliminates BSON linear scanning and `SkipContainer` recursion ([document-model.md](../document-model.md)).
2. **B+ Trees with Native Prefix-Compressed Compound Indexes ($R=16$):** Replaces 32-level skip lists (which allocate `IndexNode` + two `PageAddress[]` per hop and lack compound prefix seeks) with cache-local B+ tree pages and exact variable-width memcomparable keys ([indexing.md](../indexing.md)).
3. **Bounded Memory & Spillable Pipelines:** Replaces LiteDB's unbounded `DistinctBy` `HashSet` and in-memory post-group sort with `RoaringTreemap` `RecordId` deduplication and a single shared `BudgetHandle` (`max_query_memory_bytes`) governing `$sort`, `$group`, and `$lookup` ([query-language.md](../query-language.md)).
4. **Generic Filtered Search Substrate (`E1`):** `IndexProvider::search_with_filter` with candidate `RoaringTreemap` pushdown and reverse `RecordId` maintenance avoids LiteDB's $O(N^2)$ vector BFS upsert and full-scan fallback on filtered vector queries ([modular-architecture.md](../designs/modular-architecture.md#91-generic-provider-search-wiring)).
