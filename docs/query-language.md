# OryqDB Subsystem Specification: Query Language, Planner & Aggregation Pipeline

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document specifies OryqDB's document query syntax, filter evaluation semantics, access-path query planner, step-unpinning streaming cursors (`OryqDocView<'cursor>`), memory-bounded aggregation pipeline stages (`$match`, `$project`, `$sort`, `$limit`, `$skip`, `$group`, `$unwind`, `$lookup`), and execution plan diagnostics (`explain()` and `explain_analyze()`).

## 2. Architecture

Query processing in `crates/oryq-query/src/` executes in four stages:
1. **Parsing & Normalization (`ast.rs`):** Parses a JSON or `OryqDoc` filter/pipeline specification into a typed `FilterExpr` / `PipelineStage` AST, normalizing conjunctive/disjunctive predicates and validating field paths.
2. **Access-Path Planning (`planner.rs`):** Matches `FilterExpr` predicates and leading `$sort` keys against the collection's registered indexes (Primary `_id`, Unique Secondary, Prefix-Compressed Compound, Multi-Key Array, and `PluggableIndex` FTS/Vector plugins) to construct a physical `ExecutionPlan`.
3. **Zero-Copy Streaming Scan (`cursor.rs`):** Iterates the selected access path, pinning at most one leaf page at a time and evaluating residual filter predicates directly against `OryqDocView<'cursor>` using $O(\log k)$ field lookups and $O(1)$ delta bloom bitmask checks without deserializing full documents.
4. **Memory-Bounded Pipeline Execution (`pipeline.rs`):** Streams matching document views through composable pull-based pipeline operators enforced by `Config::max_query_memory_bytes` (default 32 MiB).

## 3. Public Interfaces

```rust
// Illustrative Rust Query & Aggregation Interfaces (Design Only)
impl<'a, 'db> ReadCollection<'a, 'db> {
    pub fn find(&self, filter: FilterExpr) -> Result<Cursor<'a>, OryqError>;
    pub fn find_with_options(&self, filter: FilterExpr, opts: FindOptions) -> Result<Cursor<'a>, OryqError>;
    pub fn find_one(&self, filter: FilterExpr) -> Result<Option<OryqDocBuf>, OryqError>;
    pub fn count(&self, filter: FilterExpr) -> Result<u64, OryqError>;
    pub fn aggregate(&self, pipeline: Vec<PipelineStage>) -> Result<PipelineCursor<'a>, OryqError>;
    pub fn explain(&self, filter: FilterExpr, opts: FindOptions) -> Result<QueryPlanReport, OryqError>;
    pub fn explain_analyze(&self, filter: FilterExpr, opts: FindOptions) -> Result<QueryAnalyzeReport, OryqError>;
}
```

## 4. Internal Data Structures

### 4.1 Filter Operators (`FilterExpr`)

| Operator Category | Operators | Evaluation Semantics on `OryqDocView<'cursor>` |
| :--- | :--- | :--- |
| **Comparison** | `$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte` | Uses exact canonical ordering (`10 == 10.0`). Scalar equality/ranges on arrays evaluate direct elements; array-constant equality compares the complete array. Negative predicates negate the corresponding positive predicate (Section 4.4). |
| **Set Membership** | `$in`, `$nin` | Sorts the target constant list into a binary-searchable vector at AST compile time; evaluates membership in $O(\log M)$ time. |
| **Logical** | `$and`, `$or`, `$not` | Short-circuit boolean evaluation; `$and` reorders cheapest/indexed predicates first. |
| **Existence & Type** | `$exists`, `$type` | Checks whether `doc.get_path(path)` is `Some(_)` (`$exists: true`) or `None` (`$exists: false`), distinguishing missing fields from explicit `Null`. |
| **Array Predicates** | `$elemMatch`, `$size`, `$all` | `$elemMatch` requires at least one single array element to satisfy all sub-predicates simultaneously (enabling multi-condition filtering on arrays of objects). |
| **String Pattern** | `$prefix`, `$contains` | Zero-copy UTF-8 byte prefix and substring matching (`$prefix` uses an exclusive encoded-byte successor bound (Section 4.4)). |
| **Pluggable Search** | `$text`, `$vectorSearch` | Dispatches to `PluggableIndex::search_with_filter()` when `oryq-fts` or `oryq-vec` is enabled, passing a `RoaringTreemap` of candidate `DocId`s pre-filtered by B+ tree predicates. |

### 4.2 Access-Path Selection Hierarchy (`planner.rs`)

The query planner selects the highest-ranked physical access path from the collection catalog:
1. **`PrimaryKeyPointLookup`:** `_id == <const>` ($O(\log_B N)$ single point probe).
2. **`UniqueIndexPointLookup`:** Full equality match on a unique single-field or compound index.
3. **`IndexOnlyCount` / `CoveringIndexScan`:** When the query projects or counts only fields fully reconstructible from a non-multi-key B+ tree index key without reading collection data pages.
4. **`CompoundIndexPrefixRangeScan`:** Matches the index with the longest leading equality prefix `(k_1 = c_1, ..., k_m = c_m)` plus at most one inequality range bound `c_low <= k_{m+1} <= c_high` and optional sort-order satisfaction (eliminating an in-memory `$sort` stage).
5. **`MultiKeyIndexScan`:** Probes a multi-key array index and attaches a `RoaringTreemap` deduplicator so documents matching multiple array elements are emitted at most once.
6. **`IndexUnionScan` (`$or` Optimization):** If every branch of a top-level `$or` has an indexed access path, executes each index scan and unions `DocId`s via a `RoaringTreemap`.
7. **`PluggableSearchScan`:** Executes B+ tree pre-filters into a candidate `RoaringTreemap`, then invokes `PluggableIndex::search_with_filter`.
8. **`FullCollectionScan`:** Sequential leaf-to-leaf scan of the primary collection B+ tree.

### 4.3 Aggregation Pipeline Stages (`pipeline.rs`)

| Stage | Streaming vs. Blocking | Execution Mechanism & Memory Bound |
| :--- | :--- | :--- |
| `$match` | Streaming ($O(1)$) | Pushed down into index planner when at pipeline head; otherwise filters `OryqDocView` in stream. |
| `$project` | Streaming ($O(1)$) | Extracts requested field slices via $O(\log k)$ directory lookups and builds a projected `OryqDocBuf`. |
| `$limit` / `$skip` | Streaming ($O(1)$) | Increments counter and halts/skips upstream cursor advances. |
| `$unwind` | Streaming ($O(1)$) | Holds current document and emits one cloned/modified document per element of the target array path. |
| `$lookup` | Streaming Probe ($O(1)$) or Bounded Hash | Reads the foreign collection under the **exact same `ReadTransaction` `SnapshotTxId`**, probing the foreign collection's index on `foreignField` for each input document. |
| `$sort` | Index-Stream ($O(1)$), Top-$K$ Heap ($O(K)$), or Bounded Spill | Uses index order if satisfied by the scan; coalesces with an immediately following `$limit: K` into a bounded binary heap of size $K$; otherwise buffers up to `max_query_memory_bytes` and spills sorted runs to temporary `StorageIo` scratch pages for external $k$-way merge sort. |
| `$group` | Blocking Accumulator (Bounded Spill) | Maintains hash-partitioned accumulators (`$count`, `$sum`, `$avg`, `$min`, `$max`, `$first`, `$last`, `$addToSet`) bounded by `max_query_memory_bytes`, spilling hash partitions to temporary scratch pages if exceeded. |

### 4.4 Predicate Semantics & Sound Index Bounds

Names/paths follow [document-model.md](document-model.md#44-paths-patch-overlap--materialization). Traversing arrays for filters fans out direct elements; `get_path` itself does not implicitly fan out. Positive predicates match if any resolved candidate matches, but independent predicates can match different elements; only `$elemMatch` requires the same element. No implicit recursive flattening.

| Predicate | Missing path | Explicit Null | Scalar | Array |
| :--- | :--- | :--- | :--- | :--- |
| `$eq: v` | false | true iff `v` is Null | exact canonical equality | scalar `v`: any direct element equals `v`; array `v`: complete ordered array equality |
| `$in: [v...]` | false | equality to any constant | equality to any constant | disjunction of `$eq` rules |
| `$gt/$gte/$lt/$lte: v` | false | compare canonical ranks | compare canonical values | compare each direct element to scalar `v`; array operands for ranges are rejected |
| `$exists: true` | false | true | true | true, including empty array |
| `$type: t` | false | true iff `t` is Null | stored type | stored Array type, not element types |

`$ne`, `$nin` and field `$not` are boolean complements of `$eq`, `$in`, and the enclosed predicate respectively, so missing fields match negative predicates. `$exists:false` negates existence. `$all` requires an array containing every requested constant under direct-element equality (an empty constant list matches any array); `$size` requires an array and exact nonnegative integer length. `$elemMatch` is false for missing/scalar/Null. `$prefix`/`$contains` require a String or a direct string array element and otherwise return false. Numeric equality, NaNs and ordering use the index codec's exact comparator. Compound documents compare sorted UTF-8 field names and canonical values. `$and:[]` is true and `$or:[]` false; malformed operand types fail during AST validation.

For ascending string prefix scans, encode rank plus escaped prefix bytes **without** the end terminator as `P`. Lower bound is `P` inclusive; upper bound is `successor(P)` exclusive, found by incrementing the last non-`0xFF` byte and truncating later bytes. If no successor exists the upper bound is unbounded. This is a byte bound, not a fabricated Unicode string. Empty prefix covers all strings. Translate bounds to reversed order for descending components and recheck predicates. Include embedded NUL and `"prefix\u{10FFFF}suffix"` oracle tests.

### 4.5 Query Budget & Spill Contract

All stages share a single reservation tracker for `max_query_memory_bytes`: AST constants, retained input/output documents, page-copy/overflow scratch, top-K entries, hash tables, merge buffers, candidate sets and dedup state reserve bytes before allocation. Buffer-pool pages belong to the separate cache budget. Limits are per active query, not a database-wide RSS promise. Writer dirty buffers have a separate `max_transaction_bytes` budget (default 64 MiB); reject oversized work with `TransactionMemoryLimitExceeded` before mutation.

`allow_disk_spill` defaults false. With spill disabled or an unspillable allocation, return `QueryMemoryLimitExceeded` deterministically. With spill enabled, only implemented spillable operators may spill; multikey/OR/plugin candidate sets initially return a quota error rather than allocate unboundedly. Top-K is a **max heap for ascending sorts**, holding the worst retained item at its root; reverse for descending order. Its memory is the total retained keys/documents, not merely K pointers. `$lookup` uses index probes or a bounded nested scan and returns a quota error if a single joined output is too large. `$addToSet` and one oversized group must spill recursively or return the same quota error; partitioning alone is not a bound.

Scratch files use a configured temp directory independently of read-only database access, unpredictable create-new names and restrictive permissions. Enforce `max_spill_bytes` (default 1 GiB per query), propagate I/O failures, close/delete scratch on success, cancellation and error. Crash orphans are disposable and never replayed; clean only verified OryqDB-owned scratch files. Tie-breaking for sort is RecordId for find and stable input sequence for pipelines; `$first`/`$last` use that stream order. `$project` supports inclusion or exclusion (not mixed, except `_id`), preserves source value types and errors if output exceeds the document limit. `$sum`/`$avg` accept numeric inputs only, ignore missing/Null, and follow checked arithmetic from the document model; empty sum is integer 0 and empty average Null. No implicit lossy conversion except the explicitly rounded `$avg` result below.

### 4.6 Accumulator Arithmetic

`$sum` uses checked i64 addition while all numeric inputs are integers. Encountering a floating input switches to f64 only when the accumulated integer and subsequent integer operands are exactly representable; otherwise return `NumericPrecisionLoss`. Floating additions use IEEE round-to-nearest, ties-to-even and return `NumericOverflow` on nonfinite results. Decimal128 arithmetic, including `$sum`/`$avg`, returns `UnsupportedFeature` until a decimal rounding/context contract is implemented. Booleans and nonnumeric non-Null inputs are type errors; NaN/infinite numeric inputs are rejected by arithmetic stages.

`$avg` accumulates an **exact rational sum and u64 count**, with allocation charged to the shared query budget, then divides exactly by count and rounds once to Float64 (nearest, ties-to-even). Thus `[1,2]` produces Float64 `1.5`; large i64 inputs do not first round through f64. An unrepresentable finite result returns `NumericOverflow`; empty/all-missing/Null input returns Null. Spill serializes the exact accumulator and count, preserving results across partition/merge order. Count overflow returns `NumericOverflow`. Tests cover fractional integer averages, mixed numeric inputs, i64 limits, empty inputs and quota/spill failures.

## 5. Invariants

1. **Single-Page Cursor Pinning:** At any point during `Cursor::next_view()` iteration, the cursor holds at most one collection leaf `PageGuard` and one index leaf `PageGuard`. Stepping to the next page drops the prior `PageGuard` before acquiring the next.
2. **Multi-Key Result Uniqueness:** A `find()` query over a multi-key array index must never return the same document twice, even if multiple elements of the document's array satisfy an `$in` or range predicate.
3. **Strict Per-Query Memory Quota:** No query or aggregation pipeline may allocate more than `Config::max_query_memory_bytes` (default 32 MiB) of intermediate sort, group, or bitmap state in RAM.

## 6. Error Handling

* Unknown filter or aggregation operators return `OryqError::InvalidQuery { operator, reason }`.
* Exceeding `max_query_memory_bytes` when external disk spill is disabled returns `OryqError::QueryMemoryLimitExceeded { limit_bytes, stage }`.
* Querying `$text` or `$vectorSearch` when the corresponding feature or index is absent returns `OryqError::UnsupportedFeature` or `OryqError::IndexNotFound`.

## 7. Concurrency Behavior

Every `Cursor<'tx>` and `PipelineCursor<'tx>` borrows an immutable `ReadTransaction<'db>` (or active `WriteTransaction<'db>`), guaranteeing that all index probes, collection page lookups, and `$lookup` cross-collection joins observe a single point-in-time MVCC snapshot `TxId`.

## 8. Performance Characteristics

* **Filter Evaluation Overhead:** Zero full-document deserialization; predicates inspect only referenced fields via $O(\log k)$ `OryqDocView` binary search.
* **Top-$K$ Sort Coalescing:** Sorting 1,000,000 matching records with `$sort + $limit: 20` uses a 20-entry binary heap ($< 16\text{ KiB}$ RAM) rather than materializing 1,000,000 documents.

## 9. Known Limitations

* Cost-based histogram statistics are not maintained in Phase 1; the Phase 1–2 planner uses deterministic rule ranking and index B+ tree page counts, with execution feedback verifiable via `explain_analyze()`.
* Recursive graph traversal (`$graphLookup`) and JavaScript user-defined functions (`$where`) are out of scope.

## 10. Testing Methodology

* **Differential Query Oracle Tests:** Execute randomly generated filter ASTs and multi-stage pipelines against both the indexed query planner and an unindexed in-memory reference evaluator, asserting identical result sets and ordering.
* **`explain_analyze()` Assertion Tests:** Verify that indexed queries select the expected access path (`PrimaryKeyPointLookup`, `CompoundIndexPrefixRangeScan`, `MultiKeyIndexScan`) and record accurate `index_keys_scanned` and `delta_chains_resolved` counters.

* **Completeness/budget gates:** Compare indexed and unindexed results for Missing/Null/negation, empty/nested arrays, scalar versus whole-array equality, maximal Unicode/NUL prefixes and typed projections. Force every shared reservation/spill failure and verify no leaked scratch or snapshot pins.
