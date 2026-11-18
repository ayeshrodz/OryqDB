# OryqDB: Embedded Single-File Document Database Engine in Rust

*   **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
*   **Status:** Draft
*   **Date:** 2026-11-18

* **Last Reviewed:** 2026-11-19

--------------------------------------------------------------------------------

## Specification Authority & Readiness

This is a reviewed architecture draft, not a frozen format or an implemented engine. The subsystem specifications in `docs/` own their detailed contracts; [file-format.md](../file-format.md) owns byte layouts, [transactions.md](../transactions.md) owns durability and visibility, and [ROADMAP.md](../ROADMAP.md) owns delivery scope and gates. These reviewed contracts supersede conflicting exploratory details in [concepts_brief.md](../reviews/initial-design/concepts_brief.md) and [draft-spec.md](../draft-spec.md). Changes to persistent encodings require updating golden fixtures before format v1 is frozen. No compatibility promise applies to pre-freeze files.

Source ownership, module dependency rules and the ordered development backlog are specified in [modular-architecture.md](modular-architecture.md) and [MODULE_DEVELOPMENT_PLAN.md](../MODULE_DEVELOPMENT_PLAN.md); those source-placement rules supersede earlier core-relative paths in this document.

The review findings, resolutions, remaining gates, and reviewer completion status are recorded in [goldfish_review_report.md](../reviews/initial-design/goldfish_review_report.md). Performance numbers are hypotheses until measured; no fresh-context review establishes runtime correctness.

## Overview

OryqDB is an open-source (`MIT OR Apache-2.0`), embedded, serverless, single-file document database engine written in Rust. It compiles to a native Rust library (`oryq-core`), a C-compatible shared and static library (`oryq-ffi` exposing `include/oryq.h`), and a standalone command-line inspection utility (`oryq-cli` producing the `oryq` binary).

OryqDB targets applications that require local, low-latency persistent storage for nested, semi-structured documents across Linux, Windows, and macOS (x86-64 and ARM64) without deploying a external database daemon or embedding a managed runtime (.NET, JVM, or V8). It combines single-file deployment portability with native MongoDB-style document operations, zero-copy binary field extraction (`OryqDoc`), slotted-page sub-document delta updates, multi-version concurrency control (MVCC) backed by a write-ahead log (`.oryq-wal`), prefix-compressed compound and multi-key array indexes, and a memory-bounded streaming aggregation pipeline.

## The Problem

Developers building local-first desktop and mobile applications, edge services, CLI tools, and AI agent state stores frequently operate on hierarchical, evolving document structures. Today, they face a structural compromise among three existing embedded storage categories:

1.  **Embedded Relational Engines with JSON Extensions (SQLite + JSONB):**
    SQLite is the industry baseline for single-file reliability, ACID durability, and cross-platform C ABI portability. However, SQLite remains fundamentally tabular:
    *   **Linear Path Scanning & Full-Record Rewrite Overhead:** While SQLite 3.45+ stores binary `JSONB`, locating a field inside a `JSONB` payload still traverses sequential element headers ($O(k)$ in the number of object fields), and modifying a single nested attribute (`json_set`) rewrites the entire row payload and its associated 4 KiB/8 KiB B-tree page.
    *   **Impedance Mismatch on Array & Polymorphic Indexing:** Indexing elements inside nested arrays in SQLite cannot be done transparently by a standard B-tree index; it requires manual trigger-maintained junction tables or query-time `json_each()` table-valued subqueries. Similarly, evolving document schemas require DDL migrations (`ALTER TABLE`) or manual expression indexes (`CREATE INDEX ... ON t(json_extract(doc, '$.path'))`).
2.  **Managed-Runtime Embedded Document Stores (LiteDB):**
    LiteDB demonstrates strong developer ergonomics for single-file MongoDB-style document storage, collections, and BSON querying. However, LiteDB is tied to the .NET CLR runtime and exhibits structural storage bottlenecks:
    *   **BSON Linear Field Seek & Fragmentation:** Standard BSON lacks an offset jump table, forcing linear key scans during filtering and projection.
    *   **Overflow Chains & Heavy Write Locking:** Documents exceeding a single 8192-byte page split across linked overflow page chains, and partial document updates deserialize, mutate, and re-serialize full BSON buffers while contending on coarse collection/engine locks. Reclaiming fragmented free space requires a blocking file-wide rebuild.
3.  **Low-Level Embedded Key-Value Engines (RocksDB, LMDB, `redb`):**
    Key-value storage engines provide raw read/write throughput, but shift the entire burden of document serialization, secondary index maintenance, multi-key array fan-out, query planning, crash-consistent index updates, and aggregation onto the application developer. Furthermore, LSM engines (RocksDB) use multi-file directory layouts that break single-file portability, while physical Copy-on-Write B+ trees (LMDB, `redb`) rewrite ancestor pages up to the root ($O(\text{height})$ page writes) on every small mutation.

**Proposed Solution:**
OryqDB solves these limitations in a single native engine by introducing:
*   **`OryqDoc` & State-Normalized Slotted-Page Deltas:** A little-endian binary document encoding with a sorted field-offset directory header for $O(\log k)$ zero-copy field lookup, paired with bounded page-local normalized value patches (`$set`, `$inc`, `$unset`, `$push`, `$pull`) and a 64-bit slot bloom bitmask so partial updates avoid full-document reconstruction while unmodified fields retain $O(\log k)$ zero-copy read access.
*   **Logical-Page Slotted B+ Tree + `.oryq-wal` + Adaptive Dual-Mode MVCC:** Stable logical page IDs that avoid root-cascading page rewrites on leaf updates, an in-memory Page-Version Indirection Map for concurrent snapshot reads, a serialized durable writer with optional later queued-command group commit, and a unified `SnapshotCoordinator` supporting zero-per-transaction-syscall Single-Process mode alongside a proposed post-baseline Shared Multi-Process mode (`.oryq-shm`).
*   **Native Document Indexing, Streaming Pipelines & Modular Search:** Order-preserving memcomparable B+ tree indexes for primary, nested, prefix-compressed compound (with restart points), and multi-key array fields; a memory-bounded cursor and aggregation pipeline engine; and a WAL-backed `PluggableIndex` substrate supporting compile-time feature-gated Full-Text Search (`oryq-fts`) and Vector Search (`oryq-vec`).

## Goals

*   **G1 — Single-File Transactional Durability:** Persist all collections, documents, primary/secondary B+ tree indexes, and pluggable index pages inside one portable `.oryq` file (with a transient `.oryq-wal` log during active transactions) that passes deterministic fault-injection crash recovery across all 7 critical failure points defined in [docs/draft-spec.md](../draft-spec.md).
*   **G2 — $O(\log k)$ Zero-Copy Field Extraction (`OryqDoc`):** Extract top-level fields in $O(\log k)$ comparisons (and depth-$d$ nested paths in $O(d \log k)$) directly from pinned buffer-pool pages via `OryqDocView<'cursor>` without heap allocation or full-document deserialization.
*   **G3 — Low-Amplification Partial Updates:** Execute `$set`, `$inc`, `$unset`, `$push`, and `$pull` mutations via state-normalized page-local delta patches (capped at `MAX_DELTA_CHAIN = 4` and guarded by a 64-bit field-hash bloom bitmask) so unmodified fields bypass delta chains in $O(1)$ register time.
*   **G4 — Concurrent Snapshot Reads & Safe Commit Publication:** Support concurrent multi-threaded readers and a single active writer pipeline under snapshot isolation (queued-command group commit follows its extension gate) with zero per-transaction OS lock syscalls in default Single-Process mode, and bounded WAL/memory footprints enforced via `max_cache_bytes`, `max_wal_size_bytes`, and snapshot leases (`ORYQ_ERR_SNAPSHOT_TOO_OLD`).
*   **G5 — Expressive MongoDB-Style Indexing & Query Pipelines:** Provide transactional primary, single-field, nested-path, prefix-compressed compound ($R=16$ restart interval), and multi-key array indexes, a rule-based query planner with `explain()` / `explain_analyze()`, and a memory-bounded (`max_query_memory_bytes`) streaming aggregation pipeline (`$match`, `$project`, `$sort`, `$limit`, `$skip`, `$group`, `$unwind`, `$lookup`).
*   **G6 — Idiomatic Rust API, Panic-Safe C ABI (`oryq_*`), and Forensic CLI (`oryq`):** Deliver a safe Rust crate (`oryq-core`), a C header and FFI library (`include/oryq.h`, `oryq-ffi`) exposing both JSON and binary `OryqDoc` accessors with `catch_unwind` panic containment, and a diagnostic CLI (`oryq-cli`) supporting CRUD, online backups, page/WAL inspection, and integrity checks.
*   **G7 — Cross-Platform Binary Portability & Minimal Base Footprint:** Guarantee identical on-disk binary layout across Linux, Windows, and macOS on x86-64 and ARM64 while isolating optional Full-Text Search (`oryq-fts`) and Vector Search (`oryq-vec`) behind compile-time Cargo features over the WAL-backed `PluggableIndex` trait.

## Non-Goals

*   **NG1 — Standalone Network Server or Distributed Cluster:** OryqDB will not include a TCP daemon, MongoDB wire-protocol listener, distributed consensus (Raft/Paxos), multi-node replication, or horizontal sharding.
*   **NG2 — SQL Parser or Relational Schema Enforcement:** OryqDB will not parse SQL statements or enforce rigid relational table schemas; all queries use structured document filters and pipeline stages.
*   **NG3 — Binary File Compatibility with LiteDB or SQLite:** OryqDB defines its own versioned `.oryq` binary format (`docs/file-format.md`) and will not read or write `.db` files created by LiteDB or SQLite.
*   **NG4 — Concurrent Multi-Writer Page-Latch Crabbing:** Write transactions serialize through a single logical writer lock and leader-follower group-commit queue rather than concurrent multi-writer B+ tree latch crabbing.
*   **NG5 — External Non-Transactional Index Files:** Feature-gated index extensions (`oryq-fts`, `oryq-vec`) are prohibited from creating external sidecar files outside the core page allocator and `.oryq-wal`.

## Open Questions

The storage and API decisions below have been revised by the EGM review. Implementation must still establish the following evidence before release:

* M1–M2: byte fixtures, checkpoint crash ordering, reader registration/reclamation schedules, and failed-sync recovery tests.
* M3–M6: exact numeric ordering oracle, nested delta equivalence, bounded query execution, and C parent/child lifecycle tests.
* M10: native portability and release checks. Optional search and shared-process support have separate extension gates and do not block the baseline release.

## Critical User Journeys (CUJs)

### CUJ 1: Embedded Rust Zero-Copy Document CRUD & Partial Delta Updates

*   **Given**: A Rust application opens or creates `app.oryq` via `Database::open("app.oryq")` using default Single-Process Multi-Threaded configuration (8192-byte pages, 64 MiB page cache) and accesses collection `"users"` with a unique secondary index on `"email"`.
*   **When**: The application inserts a JSON/struct document `{"_id": "u-101", "name": "Alice", "email": "alice@example.com", "login_count": 1, "profile": {"country": "NZ"}}`, commits, then executes a partial update `update!({"_id": "u-101"}, {"$inc": {"login_count": 1}, "$set": {"active": true}})`, and queries the document back via `users.find_one(query!({"email": "alice@example.com"}))`.
*   **Then**:
    1.  The initial insert encodes the document into `OryqDoc` format with a sorted field-offset directory and writes entries to the primary `_id` B+ tree and unique `"email"` secondary B+ tree inside a single `.oryq-wal` transaction.
    2.  The partial update appends a state-normalized delta patch (`login_count = 2`, `active = true`) to the leaf slotted page and updates the slot's 64-bit field-hash bloom bitmask without rewriting unmodified fields or updating the `"email"` secondary index.
    3.  The lookup resolves `"email"` via the secondary B+ tree, pins the leaf page in an `OryqDocView<'cursor>`, reads `"profile.country"` in $O(\log k)$ time directly from the base `OryqDoc` directory (bypassing the delta chain because `"profile.country"`'s hash bits are unset in the slot bitmask), and reads `"login_count"` (`2`) in $O(1)$ time from the normalized delta patch.

### CUJ 2: Compound, Multi-Key Array Indexing & Memory-Bounded Aggregation Pipeline

*   **Given**: A collection `"orders"` contains 100,000 documents with nested fields and tags arrays (e.g., `{"_id": 500, "customer_id": "c-9", "status": "paid", "total": 149.50, "tags": ["priority", "export"]}`), indexed by a prefix-compressed compound index on `("status", "total")` and a multi-key index on `"tags"`.
*   **When**: A caller executes an aggregation pipeline `[ {"$match": {"status": "paid", "tags": "priority"}}, {"$group": {"_id": "$customer_id", "spend": {"$sum": "$total"}}}, {"$sort": {"spend": -1}}, {"$limit": 10} ]` and inspects the execution plan via `orders.explain_analyze(...)`.
*   **Then**:
    1.  The query planner selects the highest-selectivity index scan, deduplicates `RecordId`s from any multi-key range scan using a bounded bitmap, and streams matching `OryqDocView` records into the `$group` accumulator.
    2.  The `$sort` + `$limit: 10` stage coalesces into a bounded top-10 heap (worst retained item at root) ($O(K)$ memory), keeping total query memory strictly below `max_query_memory_bytes` (32 MiB).
    3.  `explain_analyze()` returns execution metrics (counts are deterministic for a fixed plan; timing is measured) (`index_selected`, `index_keys_scanned`, `pages_hit_cache`, `pages_read_disk`, `delta_chains_resolved`, `rows_emitted`, `elapsed_us`).

### CUJ 3: Concurrent Snapshot Reads, Group Commit & Crash Recovery

*   **Given**: Eight reader threads continuously iterate cursors over collection `"accounts"` while four writer threads submit concurrent balance-transfer transactions (`$inc` on two account documents per transaction). Mid-checkpoint, the process terminates unexpectedly (`SIGKILL` or simulated `StorageIo` failpoint).
*   **When**: The application restarts and calls `Database::open("app.oryq")`.
*   **Then**:
    1.  Before the crash, every reader thread observes a transactionally consistent MVCC snapshot (`SnapshotTxId`) with zero torn transfers and zero per-transaction OS lock syscalls, while explicit writer transactions serialize through durable publication; optional queued-command batching is tested separately.
    2.  Upon reopen after the crash, recovery scans `.oryq-wal`, verifies 64-bit frame checksums and commit markers, replays committed transactions, discards any trailing uncommitted frames after the last valid commit marker, verifies the dual shadow file headers on Page 0, and restores full B+ tree and index invariants.

### CUJ 4: Cross-Language C ABI (`oryq_*`) Consumer & CLI (`oryq`) Inspection

*   **Given**: A C (or Python/Go FFI) application links against `liboryq` (`include/oryq.h`), opens `service.oryq`, inserts documents via `oryq_insert_json()`, and queries them via `oryq_find_json()` and zero-copy cursor getters (`oryq_cursor_get_utf8()`). While the application is running, an operator runs `oryq info service.oryq` from a terminal.
*   **When**: The C application iterates the cursor, triggers an invalid query argument, and the operator executes `oryq info` and `oryq integrity-check`.
*   **Then**:
    1.  The C application reads UTF-8 string slices directly from the cursor's pinned buffer-pool page without intermediate JSON serialization; invalid arguments return `ORYQ_ERR_INVALID_ARGUMENT` and caught Rust panics return `ORYQ_ERR_INTERNAL_PANIC` with a human-readable message from `oryq_last_error_message()` and never unwinds across the FFI boundary.
    2.  If `service.oryq` was opened in default Single-Process mode, `oryq info` detects the active connection-lifetime OS lock and either reports `ORYQ_ERR_LOCKED_BY_PROCESS` with the locking PID and guidance, or attaches safely if Shared Multi-Process mode (`.oryq-shm`) is active, preventing silent cross-process corruption.

## Technical Plan (Architecture)

### 1. System Architecture & Module Boundaries

The source architecture is defined by the [modular technical specification](modular-architecture.md#1-dependency-graph). `oryq-types` contains shared IDs/error/budget contracts; `oryq-document` owns values and pure updates; `oryq-engine` owns byte storage, cache, physical transactions and recovery; `oryq-index` owns providers/key encoding; `oryq-query` owns bounded planning/execution. `oryq-core` composes them into the application API, catalog and atomic collection writes. FFI and CLI call that facade only. Optional providers depend on domain ports and are registered by core, preventing cycles.

A pure engine compactor port and core adapter allow byte pages to compact document patches without engine importing document code. QuerySource is defined by query and implemented by core, so query can resolve snapshot-visible collections without importing the facade. Every active index/provider mutation shares the collection statement savepoint and engine transaction.

The canonical [file inventory](modular-architecture.md#2-proposed-file-structure) and [module work packages](../MODULE_DEVELOPMENT_PLAN.md#ordered-work-packages) replace the previous five-crate monolithic source sketch. Behavioral algorithms below remain governed by their detailed subsystem specifications.

### 2. On-Disk Binary Formats (`.oryq`, `.oryq-wal`, `.oryq-shm`)

All on-disk integers use explicit little-endian byte encoding (`to_le_bytes` / `from_le_bytes`) except for index key payloads inside B+ tree index pages, which use big-endian memcomparable byte encoding so lexicographical `memcmp` preserves value ordering. No Rust struct is ever serialized via raw memory representation.

#### 2.1 Main Database File (`.oryq`) & Dual Shadow Headers (Page 0)

The `.oryq` file is divided into fixed-size logical pages (`page_size = 8192` bytes by default; configurable at database creation to `4096`, `8192`, or `16384`). Page 0 stores two 256-byte shadow file headers at byte offsets `0..256` (Header Slot A) and `256..512` (Header Slot B), followed by reserved zero bytes through `page_size`. Catalog roots name ordinary pages; root transitions commit through WAL metadata. Schema roots commit in WAL metadata. Checkpoint writes data and syncs before alternating the header; WAL reset uses the redundant intent protocol defined in transactions.md. Header checksums alone do not make unsynced data safe.

| Byte Offset (Relative to Slot) | Width (Bytes) | Field Name | Description |
| :--- | :--- | :--- | :--- |
| `0..8` | 8 | `magic` | Exact ASCII bytes `"ORYQDB\x00\x01"` |
| `8..10` | 2 | `format_version` | On-disk format version (`u16`, initial value `1`) |
| `10..12` | 2 | `page_size_log2` | Base-2 logarithm of page size (`u16`, `13` for 8192 bytes) |
| `12..20` | 8 | `header_generation` | Monotonic commit generation counter (`u64`) |
| `20..28` | 8 | `required_read_features` | Bitmask of features required to open file for reading (`u64`) |
| `28..36` | 8 | `required_write_features` | Bitmask of features required to modify collections (`u64`, e.g., bit 0 = `FTS`, bit 1 = `VEC`) |
| `36..52` | 16 | `database_uuid` | Random 128-bit database identifier (`[u8; 16]`) |
| `52..60` | 8 | `total_page_count` | Total allocated logical pages in `.oryq` (`u64`) |
| `60..68` | 8 | `catalog_root_page_id` | Logical Page ID of the system catalog B+ tree root (`u64`) |
| `68..76` | 8 | `freelist_trunk_page_id` | Logical Page ID of the free-page trunk list head (`u64`, `0` if empty) |
| `76..84` | 8 | `freelist_page_count` | Total free pages tracked in the freelist (`u64`) |
| `84..92` | 8 | `checkpointed_tx_id` | Highest `TxId` fully checkpointed into `.oryq` (`u64`) |
| `92..100` | 8 | `checkpointed_wal_seq` | Last checkpointed WAL sequence number (`u64`) |
| `100..108` | 8 | `reset_intent_seq` | Durable WAL reset intent; 0 when inactive |
| `108..109` | 1 | `wal_required` | 0 sealed/standalone checkpoint; 1 requires WAL |
| `109..248` | 139 | `reserved` | Zero-filled reserved bytes (must be `0` in v1) |
| `248..256` | 8 | `header_checksum` | XXH3-64 checksum over bytes `0..248` (`u64`) |

#### 2.2 Slotted-Page Envelope & Slot Directory Entry Layout

Every logical page (`PageId >= 1`) starts with a 32-byte page header followed by a forward-growing array of 16-byte Slot Directory Entries and a backward-growing payload heap:

```text
+-----------------------------------------------------------------------+ 0
| 32-Byte Page Header                                                   |
| [page_id: u64][page_type: u8][flags: u8][slot_count: u16]             |
| [free_space_end: u16][fragmented_bytes: u16][lsn_tx_id: u64]          |
| [page_checksum: u64 (XXH3-64)]                                        |
+-----------------------------------------------------------------------+ 32
| Slot Directory (16 bytes * slot_count) -> Grows Downward              |
| Slot 0: [offset: u16][len: u16][flags: u8][delta_count: u8]           |
|         [head_delta_slot: u16][field_bloom_mask: u64]                 |
| Slot 1: ...                                                           |
+-----------------------------------------------------------------------+ 32 + 16*N
|                        Unallocated Free Space                         |
+-----------------------------------------------------------------------+ free_space_end
| <- Grows Upward: Document Base Records, Normalized Delta Patches,     |
|    or B+ Tree Index Key/Child Entries                                 |
+-----------------------------------------------------------------------+ 8192
```

*   **Page Types (`page_type: u8`):** `0x01` = Collection Leaf Page, `0x02` = Collection Internal B+ Tree Page, `0x03` = Index Leaf Page, `0x04` = Index Internal Page, `0x05` = Overflow Payload Page, `0x06` = Freelist Trunk/Leaf Page, `0x07` = Pluggable Index Custom Page.
*   **Slot Flags (`flags: u8`):** Bit 0 = `IS_BASE_DOC`, Bit 1 = `IS_DELTA_PATCH`, Bit 2 = `HAS_OVERFLOW_CHAIN`, Bit 3 = `IS_TOMBSTONE`.
*   **Overflow Handling:** Baseline v1 spills the entire encoded document once it exceeds `PAGE_INLINE_THRESHOLD = page_size / 4`, using a storage-envelope OverflowRef. [file-format.md](../file-format.md#b-tree-overflow--freelist-payloads) owns the standard page header, `next_page:u64`, `chunk_len:u32` and bounded chain validation. Reads materialize the document in budgeted cursor-owned storage; selective field overflow is deferred. Exported OryqDoc never contains unresolved storage pointers.

#### 2.3 Write-Ahead Log Sidecar (`.oryq-wal`)

The canonical 64-byte WAL header, 48-byte frame header, FullPageFrame, byte-range SlotDeltaFrame and 48-byte CommitFrame payload are defined in [file-format.md](../file-format.md#43-section-c-write-ahead-log-sidecar-oryq-wal). Frame headers carry `salt1`; `salt2` participates through the checksummed header seed. The frame chain and commit metadata cover page allocation and catalog-root changes. Replay uses committed full-page baselines and bounded delta chains. Checkpoint/reset obeys the ordered data-sync/header-sync/redundant-reset-intent protocol in [transactions.md](../transactions.md#41-snapshot-admission-checkpoint--recovery-protocol). Failed sync has an indeterminate outcome and faults the database.

#### 2.4 Shared-Memory Coordination Sidecar (`.oryq-shm`)

Shared-process support is a post-baseline extension with an unfrozen synchronization ABI. Until its cross-process protocol and death/reuse tests pass, requesting it returns `UnsupportedFeature`. [file-format.md](../file-format.md#44-section-d-shared-memory-coordination-sidecar-oryq-shm) records the proposed layout and required gates. Each process publishes the minimum of its local snapshots; a fixed latest-only hash table never substitutes for historical WAL resolution.

### 3. `OryqDoc` Binary Encoding & State-Normalized Delta Patches

#### 3.1 `OryqDoc` Binary Layout

An `OryqDoc` buffer is a self-contained, little-endian byte sequence that enables $O(\log k)$ binary search across fields at any nesting level without parsing preceding fields:

```text
+-----------------------------------------------------------------------+ 0
| 8-Byte Document Header                                                |
| [version: u8 = 0x01][flags: u8][field_count: u16][total_byte_len: u32]|
+-----------------------------------------------------------------------+ 8
| Sorted Field-Offset Directory (12 bytes * field_count)                |
| Sorted ascending by `name_hash_prefix: u32`, tie-broken by UTF-8 name |
| Entry i:                                                              |
|   [name_hash_prefix: u32] (First 4 bytes of XXH3(field_name) big-end) |
|   [name_offset: u32]      (Offset to [name_len: u16, utf8_bytes...])  |
|   [val_descriptor: u32]   (Bits 0..27: value_offset or inline scalar; |
|                            Bits 28..31: 4-bit OryqTypeTag)            |
+-----------------------------------------------------------------------+ 8 + 12*k
| Key & Value Heap                                                      |
| - Deduplicated UTF-8 Field Names: [len: u16][utf8_bytes]              |
| - Scalars: i64 (8B LE), f64 (8B LE), DateTimeUtcMs (i64 LE),          |
|            ObjectId (12B), UUID (16B), Decimal128 (16B)               |
| - Booleans & Null: Encoded directly inside `val_descriptor` (0 heap B)|
| - Strings & Binary: [byte_len: u32][raw_bytes...]                     |
| - Arrays: [elem_count: u32][total_bytes: u32][u32 offset table][elems]|
| - Nested Documents: Recursive embedded `OryqDoc` byte slice           |
+-----------------------------------------------------------------------+ total_byte_len
```

*   **Lookup Complexity ($O(\log k)$):** To look up field `"email"`, the reader computes the 32-bit `name_hash_prefix` of `"email"` in registers and binary-searches the 12-byte directory entries by `name_hash_prefix`. Only on a 32-bit prefix match does the reader compare the UTF-8 slice at `name_offset`. For a nested path `"profile.country"`, the reader resolves `"profile"` in $O(\log k_{\text{root}})$ to obtain a sub-slice `OryqDocView` and resolves `"country"` in $O(\log k_{\text{sub}})$.
*   **Unaligned-Safe Zero-Copy Access:** Because variable-length strings can place scalar offsets at arbitrary byte alignments, `OryqDocView` never casts raw byte pointers to `&i64` or `&f64`. Instead, strings, binaries, and sub-documents return zero-copy borrowed slices (`&'cursor str`, `&'cursor [u8]`, `OryqDocView<'cursor>`), while 8-byte scalars copy 8 bytes into a CPU register via `i64::from_le_bytes(slice.try_into().unwrap())`.

#### 3.2 State-Normalized Slotted-Page Deltas & 64-Bit Bloom Bitmask

The writer evaluates relative update operators once, then stores replacement/tombstone patches. [document-model.md](../document-model.md#44-paths-patch-overlap--materialization) defines top-level physical normalization: updating `a.b` stores the resulting whole value of `a`, and both readers and writers hash `a` for bloom checks. Search newest-to-oldest; top-level replacement shadows all older descendants. This closes ancestor/descendant false negatives and keeps borrowed subdocuments backed by stable bytes. The fifth patch, overflow or insufficient page space materializes a full logical document. Full-document export uses owned bounded scratch; scalar reads of inline values can remain zero-copy.

### 4. Logical-Page WAL B+ Tree & Adaptive Dual-Mode MVCC

#### 4.1 Why Logical Page IDs Avoid Physical CoW Write Amplification

In a physical Copy-on-Write B+ tree, changing a single leaf page assigns it a new disk block address, forcing every ancestor internal node up to the root to be rewritten. OryqDB avoids this by using **stable logical page IDs** (`PageId: u64`):
*   Internal B+ tree branch pages point to child `LogicalPageId`s.
*   When a leaf page is updated without splitting, its `LogicalPageId` does not change, so parent branch pages are **not** dirtied or rewritten.
*   Instead, the modified leaf frame is written to `.oryq-wal` under `(LogicalPageId, CommitTxId)` and indexed in the in-memory **Page-Version Indirection Map** (`DashMap` / sharded lock-free hash table mapping `LogicalPageId -> SmallVec<[(TxId, FrameLocation); 2]>`).
*   A reader holding `ReadTransaction { snapshot_tx_id: S }` looks up `LogicalPageId` in the Page-Version Indirection Map:
    *   If a version with `tx_id <= S` exists in the map, the reader reads that buffer-pool frame or `.oryq-wal` offset.
    *   Otherwise, the reader loads the baseline page at offset `LogicalPageId * page_size` from the main `.oryq` file.

#### 4.2 Unified `SnapshotCoordinator` & Writer Ownership

Default `SingleProcess` keeps a process-lifetime exclusive OS file lock. A short in-process admission mutex atomically registers each snapshot with reclamation; nested reads receive distinct tokens. This is zero per-read OS file-lock calls, not a wait-free algorithm. [transactions.md](../transactions.md) defines publication, token saturation and all checkpoint interleavings.

An explicit write transaction holds the logical writer lock through sync and publication. Optional group commit accepts queued commands executed serially against one batch view; independently prepared dirty pages cannot be queued safely. The baseline works with one sync per explicit commit. Shared-process mode and group-commit optimization have separate evidence gates.

#### 4.3 Bounded Memory & WAL Retention Quotas

Cache pages and version metadata share `max_cache_bytes` (64 MiB). Intermediate queries share `max_query_memory_bytes` (32 MiB/query), and dirty transaction buffers use `max_transaction_bytes` (64 MiB). WAL pressure above the 128 MiB target can request cancellation of snapshots older than 300s, but retention continues through guard release. At the 256 MiB hard cap reject writes rather than recycle live history. These are component budgets, not a fixed process RSS guarantee.

### 5. Indexing Engine & WAL-Backed `PluggableIndex` Substrate

#### 5.1 Canonical Type Ordering & Order-Preserving Memcomparable Keys

[indexing.md](../indexing.md#41-canonical-order-preserving-memcomparable-key-encoding-key_codecrs) owns the exact type ranks and encoding. Exact variable-width decimal significands/exponents unify `i64`, `f64` and Decimal128 without rounding; the previous nine-byte float-plus-tail design is withdrawn. Signed zero and NaNs are canonicalized. Strings escape NUL as `00 FF` and terminate with `00 00`. Descending components invert all bytes. Key-only projections cannot reconstruct numeric storage type or distinguish Missing from Null.

User `_id` and internal `RecordId:u64` are distinct. Primary indexes map `_id` to RecordId; collection trees map RecordId to document bytes. Secondary keys append RecordId. `RoaringTreemap` uses exact RecordIds, never hashed user keys.

#### 5.2 Prefix-Compressed Compound Indexes ($R = 16$ Restart Points) & Multi-Key Array Rules

*   **Prefix Compression with Restart Intervals:** Compound index keys `(field_1, field_2, ..., RecordId)` share common leading byte prefixes on B+ tree leaf pages. Every $R = 16$ slots on an Index Leaf Page (`0x03`), the page stores a **Restart Point** containing the full uncompressed key. Slots between restart points encode `[shared_prefix_len: u16, unshared_suffix_len: u16, unshared_bytes...]`. Lookups binary-search the Restart Point table in $O(\log(N/16))$ comparisons and scan at most 16 delta-encoded entries.
*   **Multi-Key (Array) Index Fan-Out & Parallel Array Guard:**
    *   When an indexed field evaluates to an array `["rust", "database"]`, the index engine inserts one B+ tree entry per distinct array element pointing to the same `RecordId`.
    *   On document update, the writer computes the set difference `(old_keys \ new_keys)` to delete and `(new_keys \ old_keys)` to insert.
    *   In a compound index `(a, b, c)`, **at most one field** may be an array in any inserted or updated document. Attempting to insert a document where two fields in the same compound index are arrays returns `ORYQ_ERR_PARALLEL_ARRAY_INDEX`, preventing $O(|A_1| \times |A_2|)$ Cartesian index explosion.

#### 5.3 WAL-Backed `PluggableIndex` Trait (`oryq-fts` and `oryq-vec`)

```rust
pub trait PluggableIndex: Send + Sync {
    fn index_type_name(&self) -> &'static str;
    fn required_feature_bit(&self) -> u64;
    fn on_doc_write(
        &self,
        ctx: &mut IndexStorageContext<'_>,
        doc_id: RecordId,
        old_doc: Option<OryqDocView<'_>>,
        new_doc: Option<OryqDocView<'_>>,
    ) -> Result<(), OryqError>;
    fn on_commit_batch(&self, ctx: &mut IndexStorageContext<'_>) -> Result<(), OryqError>;
    fn search_with_filter(
        &self,
        ctx: &IndexReadContext<'_>,
        predicate: &PluginQueryPredicate,
        candidate_docs: Option<&RoaringTreemap>,
        limit: usize,
    ) -> Result<Vec<ScoredRecordId>, OryqError>;
}
```

*   **Transactional Storage Guarantee:** `IndexStorageContext<'tx>` wraps the core transactional page allocator and `.oryq-wal` writer. Neither `oryq-fts` nor `oryq-vec` performs direct filesystem I/O; their inverted posting lists and vector graph/quantized pages live inside the single `.oryq` file and recover atomically via `.oryq-wal`.
*   **Two-Tier Pending-Delta Batching:** To prevent per-document FTS posting-list expansion or HNSW graph linking from stalling transactions, `on_doc_write()` appends a compact `(doc_id, extracted_terms_or_vector)` entry into a per-index **Pending-Delta B+ Tree** inside `.oryq`. During a normal WAL-backed merge write transaction, the plugin merges pending entries into compressed inverted segments (`oryq-fts`) or vector graph pages (`oryq-vec`). Queries union results from the merged pages and the bounded pending-delta B+ tree, intersecting with `candidate_docs` bitmaps pushed down from standard B+ tree predicates.

---

### 6. Streaming Query Planner & Memory-Bounded Aggregation Pipeline

#### 6.1 Access-Path Selection & Cursor-Scoped Lending Lifetimes

Given a filter AST (`$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte`, `$and`, `$or`, `$in`, `$nin`, `$exists`, `$not`, `$elemMatch`), the query planner ranks available indexes using a deterministic rule + cardinality-estimate hierarchy:
1.  **Primary Key Point / Range Lookup** (`_id` equality or range).
2.  **Unique Secondary / Compound Prefix Equality Lookup**.
3.  **Compound / Single-Field / Multi-Key Secondary Range Scan** (selecting the index matching the longest leading equality prefix + range predicate + sort order).
4.  **Pluggable Index (`$text` / `$vectorSearch`) with B+ Tree Bitmap Pre-Filter**.
5.  **Full Collection B+ Tree Leaf Scan**.

Cursors use a **Step-Unpinning Lending Model**:
*   `Cursor::next_view(&mut self) -> Result<Option<OryqDocView<'_>>, OryqError>` holds at most **one** pinned leaf `PageGuard` at a time. Calling `next_view()` drops the previous `PageGuard` before pinning the next leaf page, ensuring a scan over 10,000,000 documents pins only 1 page frame in the bounded buffer pool.
*   For multi-key index scans where one document can appear under multiple index keys, the cursor tracks emitted `RecordId`s in a roaring bitmap bounded by `max_query_memory_bytes`.

#### 6.2 Memory-Bounded Aggregation Pipeline

Pipeline stages execute as composable pull-based physical operators:
*   **Streaming Stages ($O(1)$ Memory):** `$match` (evaluates predicates directly against `OryqDocView` via $O(\log k)$ field extraction), `$project` (builds projected `OryqDocBuf`), `$limit`, `$skip`, and `$unwind` (yields one view per array element).
*   **Snapshot-Consistent `$lookup`:** Executes a correlated index probe (or hash-build if unindexed and within memory budget) against the foreign collection using the **exact same `ReadTransaction` snapshot `TxId`**.
*   **Blocking Stages with Memory Quotas (`$group`, `$sort`):**
    *   If `$sort` matches the chosen B+ tree index order, it executes as a zero-memory streaming pass.
    *   If `$sort` is immediately followed by `$limit: K`, it uses a bounded top-$K$ binary heap consuming $O(K)$ memory.
    *   Otherwise, `$sort` and `$group` track accumulated bytes against `QueryConfig::max_query_memory_bytes` (default 32 MiB). If exceeded, `$sort` spills sorted runs to temporary scratch pages managed by `StorageIo` for an external $k$-way merge sort, and `$group` spills hash partitions (or returns `ORYQ_ERR_QUERY_MEMORY_LIMIT` if temp-spill is disabled in config).

---

### 7. Public APIs: Rust, C ABI (`include/oryq.h`), and Diagnostic CLI (`oryq`)

#### 7.1 Rust Public API (`crates/oryq-core`)

```rust
// Illustrative Rust Public API (Design Only)
pub struct Database { /* Arc<DatabaseInner> (Send + Sync) */ }

impl Database {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, OryqError>;
    pub fn open_with_config<P: AsRef<Path>>(path: P, cfg: Config) -> Result<Self, OryqError>;
    pub fn begin_read(&self) -> Result<ReadTransaction<'_>, OryqError>;
    pub fn begin_write(&self) -> Result<WriteTransaction<'_>, OryqError>;
    pub fn collection(&self, name: &str) -> Result<CollectionHandle<'_>, OryqError>;
    pub fn stats(&self) -> OryqStats;
    pub fn checkpoint(&self) -> Result<CheckpointReport, OryqError>;
    pub fn backup_to<P: AsRef<Path>>(&self, dest: P) -> Result<(), OryqError>;
}
```

#### 7.2 Panic-Safe C ABI (`crates/oryq-ffi`, `include/oryq.h`)

[ffi.md](../ffi.md) owns signatures, status values, diagnostics and the handle state machine. Database/owned-document handles support shared reads; transactions and their child cursors require external serialization. Cursors retain the transaction state; commit/rollback/close return BUSY with live children. Completion changes transaction state without freeing the handle; `oryq_txn_close` releases it. `oryq_close(db)` releases only that handle and cannot invalidate descendants. No Rust borrowed lifetime is widened to `'static`.

Status functions map unwinding panics to `ORYQ_ERR_INTERNAL_PANIC`; other return types have explicit sentinel fallbacks. Thread-local error text supports failed-open diagnosis via NULL and remains valid until the next non-diagnostic call on the same thread. Optional symbols stay exported and return `UnsupportedFeature` until implemented.

#### 7.3 Observability, Forensic Errors & `oryq` CLI (`crates/oryq-cli`)

*   **Structured Forensic Corruption Errors:** Every corruption error carries `{ source: CorruptionSource (MainDb | Wal | Shm), page_or_frame_id: u64, file_byte_offset: u64, expected_checksum: u64, actual_checksum: u64, reason: CorruptionReason }`.
*   **Runtime Telemetry (`OryqStats`):** Atomic counters for `active_readers`, `oldest_active_snapshot_tx_id`, `latest_committed_tx_id`, `wal_frame_count`, `wal_bytes`, `cache_hits`, `cache_misses`, `evictions`, `delta_patches_appended`, `delta_bloom_bypasses`, `delta_chain_traversals`, `in_page_compactions`, and `page_splits`.
*   **CLI Commands (`oryq`):**
    *   Standard operations: `oryq info <db> [--stats]`, `oryq collections <db>`, `oryq collection create|drop <db> <col>`, `oryq insert <db> <col> <file_or_json>`, `oryq find <db> <col> <query_json>`, `oryq aggregate <db> <col> <pipeline_json>`, `oryq indexes <db> <col>`, `oryq backup <db> <dest>`, `oryq version`.
    *   Forensic triage operations: `oryq integrity-check <db> [--verbose]`, `oryq page-inspect <db> <page_id>`, `oryq wal-inspect <db>`, and `oryq inspect-locks <db>`.

## Alternatives Considered

### Alternative A: Physical Copy-on-Write (CoW) B+ Tree (LMDB / `redb` Shadow Paging)

*   **Pros**: Eliminates a separate `.oryq-wal` recovery replay pass by writing all modified pages to unallocated disk blocks and flipping a root pointer in the dual shadow header on Page 0; naturally supports snapshot reads without a WAL frame indirection table.
*   **Cons**: Mutating even a few bytes on a leaf page allocates a new physical page and cascades parent pointer updates up to the B+ tree root ($O(\text{height})$ full 8192-byte page writes per transaction). This cascading write amplification erases the write-volume reduction of sub-document delta patches. Additionally, a single long-lived reader pins old tree roots and prevents physical free-page reclamation, causing rapid database file growth under write load.
*   **Reason for Rejection**: High write amplification on partial document mutations ($O(\text{height})$ page writes per commit) conflicts with high-frequency `$set`/`$inc`/`$push` workloads and negates page-local delta storage.

### Alternative B: Log-Structured Merge (LSM) Tree Storage Engine (RocksDB / LevelDB Style)

*   **Pros**: Converts random document inserts and updates into sequential append-only MemTable and SSTable writes, yielding high raw write throughput on write-heavy workloads.
*   **Cons**: Stores data across a multi-file directory of immutable SSTables, MANIFEST files, and logs, violating the single-file deployment and backup requirement ([docs/draft-spec.md](../draft-spec.md)). Point lookups and secondary index range scans suffer read amplification across multiple SSTable levels, and background compaction threads cause unpredictable CPU, memory, and tail-latency spikes in embedded applications.
*   **Reason for Rejection**: Multi-file SSTable directory topology violates single-file portability, and background compaction causes tail-latency spikes and read/space amplification unsuitable for a lightweight embedded library.

### Alternative C: Standard BSON On-Disk Document Representation

*   **Pros**: Matches existing BSON library specifications used by MongoDB wire protocols and LiteDB.
*   **Cons**: Standard BSON encodes fields as sequential `[type_byte][cstring_key][value]` elements without a sorted header directory or offset jump table. Extracting a single projected or filtered field requires an $O(k)$ linear scan over all preceding fields in the document, and array element access requires linear traversal of preceding elements.
*   **Reason for Rejection**: Linear $O(k)$ field scanning bottlenecks query filtering, secondary index key extraction, and partial projections compared to `OryqDoc`'s $O(\log k)$ sorted field-offset directory.

### Alternative D: Unbounded Operation-Log Delta Chains in Slotted Pages

*   **Pros**: Appending raw operation logs (`$inc: 1`, `$push: "tag"`) to slotted pages avoids reading the prior field value during unindexed updates.
*   **Cons**: Forces every reader to walk an unbounded delta chain ($O(D + \log k)$), execute update operator logic on read, and allocate heap memory to materialize computed numbers or mutated arrays—breaking zero-copy field extraction (`OryqDocView<'cursor>`). It also tightly couples low-level `storage/` page-split compaction to high-level MongoDB update operator semantics.
*   **Reason for Rejection**: Degrades read latency, breaks zero-copy slice access on mutated fields, and violates module separation between `storage/` and `document/`. Replaced with write-time state-normalized value patches, a 64-bit slot bloom bitmask, and a hard cap of `MAX_DELTA_CHAIN = 4`.

### Alternative E: Mandatory Per-Transaction OS File Locks (`fcntl` / `LockFileEx`) in All Modes

*   **Pros**: Uses a single cross-process file-locking path on every transaction start and commit regardless of whether one or multiple OS processes access the database.
*   **Cons**: Invokes multiple kernel syscalls (`fcntl` or `LockFileEx`) on every read and write transaction, penalizing the common embedded case where a single multithreaded application process owns the database file.
*   **Reason for Rejection**: Unnecessary per-transaction kernel syscall overhead for single-process multithreaded applications. Replaced with the `SnapshotCoordinator` trait defaulting to a connection-lifetime exclusive OS lock at `Database::open()` with zero per-transaction syscalls, plus an opt-in `.oryq-shm` multi-process mode.

### Alternative F: User-Space Spinlocks or Rust Slice References (`&[u8]`) Over `.oryq-shm` Shared Memory

*   **Pros**: Avoids OS kernel lock interaction when coordinating across multiple processes.
*   **Cons**: Constructing Rust `&[u8]` or `&Header` references over memory-mapped regions concurrently mutated by another OS process is immediate Undefined Behavior under the Rust/LLVM aliasing model (`noalias`). Furthermore, user-space spinlocks or futexes in shared memory deadlock permanently if a holding process is terminated by `SIGKILL`.
*   **Reason for Rejection**: Causes Rust Undefined Behavior on cross-process concurrent mutations and unrecoverable deadlocks on `SIGKILL`. Replaced with raw-pointer atomic operations (`*const AtomicU64`) paired with 1-byte OS kernel file range locks per process slot for automatic kernel-assisted crash detection.

### Alternative G: Monolithic Inclusion of Full-Text Search and Vector Search inside `oryq-core`

*   **Pros**: Ships BM25 full-text search and vector similarity search in a single crate without feature flags.
*   **Cons**: Increases compiled binary size and compilation times for all embedded consumers, complicates unsafe-code auditing, and violates Section 25 of [docs/draft-spec.md](../draft-spec.md).
*   **Reason for Rejection**: Expands base binary footprint and violates the incremental milestone rules of `docs/draft-spec.md`. Replaced with a WAL-backed `PluggableIndex` substrate in `oryq-core` and compile-time feature-gated `crates/oryq-fts` and `crates/oryq-vec` crates in post-baseline E1.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Memory Safety & Security:**
    *   `oryq-core` defaults to safe Rust (`#![forbid(unsafe_code)]` across `document/`, `query/`, `index/`, and `transaction/` in Single-Process mode). The permitted `unsafe` blocks are isolated inside `crates/oryq-ffi` (C ABI pointer access) and the storage platform/SHM adapters (`pal.rs`, `shm.rs`, for OS calls and proposed cross-process atomics), each documented with explicit `// SAFETY:` invariants and audited via Miri and AddressSanitizer/ThreadSanitizer.
    *   Every byte read from `.oryq`, `.oryq-wal`, `.oryq-shm`, or FFI inputs is treated as untrusted: page headers, slot offsets, record boundaries, UTF-8 validity, and XXH3-64 checksums are verified before slice construction, and malformed inputs are continuously tested via `cargo-fuzz`.
*   **Performance, Latency & Resource Quotas:**
    *   **Field Extraction Latency:** $O(\log k)$ binary search over `OryqDoc` directory headers for unmodified fields (with $O(1)$ 64-bit bloom bitmask bypass of delta slots) and $O(1)$ lookup across at most `MAX_DELTA_CHAIN = 4` normalized value patches.
    *   **Bounded Memory Quotas:** Configurable buffer pool (`max_cache_bytes`, default 64 MiB), per-query memory ceiling for blocking pipeline stages (`max_query_memory_bytes`, default 32 MiB, with top-$K$ bounded heap coalescing and external page spill), and WAL horizon cap (`max_wal_size_bytes`, default 128 MiB; `max_snapshot_duration`, default 300s).
*   **Rollout, Backward Compatibility & Feature-Gate Safety:**
    *   Page 0 dual shadow headers carry explicit `format_version: u16`, `required_read_features: u64`, and `required_write_features: u64` bitmasks.
    *   If a database containing an active feature-gated index (`oryq-fts` or `oryq-vec`) is opened by a binary compiled without that feature flag, read queries on standard B+ tree indexes continue to work while write transactions touching the affected collection fail deterministically with `ORYQ_ERR_UNSUPPORTED_FEATURE`, preventing silent index corruption.

--------------------------------------------------------------------------------

### Additional Resources

*   Primary Project Specification: [docs/draft-spec.md](../draft-spec.md)
*   EGM Brainstorm Synthesis Brief: [concepts_brief.md](../reviews/initial-design/concepts_brief.md)
*   Reference Implementation for Milestone 0 Analysis: LiteDB (`https://github.com/litedb-org/LiteDB`, MIT License)

### Future Improvements

*   **At-Rest Page Encryption (AEAD):** Page-level authenticated encryption (AES-256-GCM or ChaCha20-Poly1305) keyed via Argon2id key derivation, storing per-page nonces/tags in reserved page header/trailer bytes after core storage verification ([docs/draft-spec.md](../draft-spec.md)).
*   **Change Streams & Reactive Watch Cursors:** In-process event subscriptions notifying local listeners of committed collection mutations filtered by `$match` predicates ([docs/draft-spec.md](../draft-spec.md)).
*   **Dictionary-Trained Page & Document Compression:** Optional Zstd dictionary compression for cold overflow payload pages and historical archive collections ([docs/draft-spec.md](../draft-spec.md)).

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Canonical Module Ownership and Delivery

[modular-architecture.md](modular-architecture.md) supersedes the original monolithic core file manifest. The [module catalog](../modules/README.md) specifies owner files, public ports and local conformance tests; the [module development plan](../MODULE_DEVELOPMENT_PLAN.md) sequences W00–W16 and X01–X03 under the existing product milestones.

The public Rust crate remains `oryq-core`. Its document, physical engine, index and query implementations live behind separate crate boundaries; types owns the small shared contracts. FFI/CLI depend only on core, while optional providers depend on lower-layer ports and register at core composition. This source split does not alter the persistent-byte, MVCC, query or C ABI contracts.

### Verification Plan

Use the owner selections in [MODULE_DEVELOPMENT_PLAN.md](../MODULE_DEVELOPMENT_PLAN.md#verification-commands-to-implement). These commands become runnable after the corresponding packages create the workspace; they are not current implementation evidence.

* Build/format/lint the active baseline workspace and run `cargo run -p xtask -- architecture`; optional provider packages enter the feature matrix only after E1 exists.
* Run document roundtrip/parser/comparator/updates tests in `oryq-document`, byte-page/WAL/snapshot/checkpoint tests in `oryq-engine`, provider conformance in `oryq-index`, query semantics/budget/spill in `oryq-query`, and cross-module atomicity/backup/removal in `oryq-core`.
* Run `cargo +nightly miri test -p oryq-document` for pure value/slice logic. Use an explicit Miri-compatible test selection in engine for pure page/transaction helpers; native PAL and cross-process mmap behavior require separate subprocess/platform evidence. Miri is not claimed to validate OS lock or cross-process memory semantics.
* Native ABI checks compile/link `crates/oryq-ffi/tests/c/api.c` and `crates/oryq-ffi/tests/c/lifecycle.c` against the actual release library. FFI owns platform linker configuration; `cargo test -p oryq-ffi` invokes these as native integration checks after W11. The generated header remains `include/oryq.h`.
* Coverage is measured across types/document/engine/index/query/core/FFI owners, with 90%+ line/branch targets for critical domain code and all seven crash stages. Report actual tool/platform support and uncovered behavior; moved source paths are not evidence.
* Core integration inventory is `vertical_slice`, `atomic_maintenance`, `feature_lifecycle`, and `backup`; engine owns crash/admission/checkpoint/locking suites. Test commands must select these owner files rather than the superseded global tests/c_abi or core/src/storage paths.

*Drafted with EGM*
