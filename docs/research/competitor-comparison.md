# Embedded Database Competitor Comparison & Differentiation Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](../designs/oryqdb.md)

> **Evidence status:** Comparative implementation/performance claims below are provisional research hypotheses, not verified findings. The pinned source audit and equivalent-durability measurements remain M0/M7 gates; do not cite this matrix as benchmark evidence.

---

## 1. Executive Summary

Per [docs/draft-spec.md Section 3.1](../draft-spec.md), OryqDB is evaluated against established embedded storage engines across four categories:
1. **Relational Embedded Engines with JSON Extensions:** SQLite, SQLite JSON1, and SQLite 3.45+ JSONB.
2. **Embedded Document Databases:** LiteDB (.NET) and UnQLite (C).
3. **Embedded Key-Value B+ Tree & LSM Engines:** LMDB (C), `redb` (Rust), and RocksDB (C++).
4. **Embedded Object Stores:** ObjectBox (C++/Dart/Swift/Java).

Because SQLite 3.45+ already provides ACID single-file transactions and binary `JSONB` payloads, OryqDB cannot compete merely by storing JSON documents inside a basic B-tree. OryqDB differentiates specifically on:
* **$O(\log k)$ Zero-Copy Field Extraction (`OryqDoc`)** vs. SQLite `JSONB` and LiteDB BSON $O(k)$ sequential element headers.
* **State-Normalized Slotted-Page Delta Patches (`$set`, `$inc`, `$unset`, `$push`, `$pull`) + 64-Bit Bloom Mask** vs. full-document and full-page rewrites on partial updates.
* **Native Multi-Key (Array) & Polymorphic Prefix-Compressed Compound Indexes** without SQLite's manual trigger/junction tables or `json_each()` table-valued query workarounds.
* **Zero-Syscall Single-Process Multi-Threaded MVCC** with zero-delay leader-follower group commit and opt-in crash-safe `.oryq-shm` multi-process coordination.

---

## 2. Architectural Comparison Matrix

| Dimension | OryqDB (Rust) | SQLite 3.45+ with JSONB (C) | LiteDB v5 (C# / .NET) | `redb` / LMDB (Rust / C) | RocksDB (C++) | ObjectBox |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Primary Data Model** | Native Document (`OryqDoc`) & Collections | Tabular Relational Rows + `JSONB` Column Blobs | BSON Documents & Collections | Raw Byte Key-Value Pairs (`&[u8] -> &[u8]`) | Raw Byte Key-Value Pairs | Schema-Compiled FlatBuffers Objects |
| **Runtime Requirement** | Zero (Native Rust / C ABI `liboryq`) | Zero (Native C Library) | Requires .NET CLR Runtime (or .NET AOT) | Zero (Native Rust / C) | Zero (Native C++, heavy dependency footprint) | Native closed-core C++ binary library |
| **On-Disk File Layout** | Single `.oryq` file + `.oryq-wal` (optional `.oryq-shm`) | Single `.db` file + `-wal` + `-shm` | Single `.db` file + `-log.db` | Single file (Dual-root Copy-on-Write B+ tree) | Multi-file directory (`*.sst`, `MANIFEST`, `*.log`) | Directory (`data.mdb`, `lock.mdb` — LMDB derivative) |
| **Field Lookup Complexity Inside Document** | **$O(\log k)$ binary search** over 12B sorted field directory (`OryqDocView`) | $O(k)$ sequential traversal of `JSONB` element headers | $O(k)$ sequential traversal of BSON `[type][cstring][val]` elements | N/A (Application must deserialize full value) | N/A (Application must deserialize full value) | $O(1)$ fixed FlatBuffers vtable offset (requires pre-compiled static schema) |
| **Partial Document Update Mechanism** | **State-Normalized Slotted-Page Delta Patch** (`MAX_DELTA_CHAIN = 4`) + 64-bit bloom mask + `SlotDeltaFrame` WAL | `jsonb_set()` reconstructs entire `JSONB` blob and rewrites full 4 KiB B-tree page | Deserializes full BSON doc into `BsonDocument`, mutates in RAM, re-serializes full doc | Rewrites entire value + cascades physical CoW B+ tree ancestor pages to root ($O(\text{height})$ pages) | Appends MergeOperator delta to WAL/MemTable; resolved during background SSTable compaction | Rewrites full FlatBuffer object via CoW B+ tree |
| **Array (Multi-Key) Indexing** | **First-Class Multi-Key B+ Tree Index** (automatic fan-out, set-diff update, `RoaringBitmap` dedup) | Manual junction table + triggers, or virtual scan via `json_each()` | Supported via expression index `$.tags[*]` | None (Caller must maintain manual secondary tables) | None | Limited to relation links |
| **Compound Index Prefix Compression** | **Native Memcomparable Keys with $R=16$ Restart Points** | Uncompressed B-tree record keys | Uncompressed BSON index nodes (max key length limits) | Uncompressed raw byte keys | Block prefix compression with restart intervals (in immutable SSTables only) | Uncompressed keys |
| **Concurrency & Isolation Model** | **Adaptive Dual-Mode MVCC:** Zero-syscall in-process atomic snapshots + leader-follower group commit; opt-in `.oryq-shm` | WAL mode MVCC: OS file/shm lock checks per transaction (`-shm` WAL index) | Coarse in-process `ReaderWriterLockSlim` + file stream locks | MVCC via CoW shadow paging (1 writer, lock-free readers; long readers bloat file size) | MVCC via sequence numbers + multi-threaded MemTable/WAL batching | MVCC via LMDB CoW shadow paging |
| **Modular Search (FTS & Vector)** | **WAL-Backed `PluggableIndex` Trait** (`oryq-fts`, `oryq-vec` behind compile-time features) | `FTS5` virtual tables + external `sqlite-vec` extension | Full-text absent / limited BSON expression regex | None | None | Built-in HNSW vector search (proprietary core) |

---

## 3. Deep-Dive Technical Analysis of Competitors

### 3.1 SQLite & SQLite JSONB
* **Architecture hypothesis (source verification pending):** SQLite uses a page-oriented B-tree (`4096` bytes default) with either rollback journaling or a Write-Ahead Log (`-wal`) indexed by a shared-memory hash table (`-shm`). In SQLite 3.45 (2024), `JSONB` was introduced as a binary payload format storing a tree of `[header_size_and_type][payload]` nodes inside standard SQLite column blobs.
* **Strengths:** Extreme maturity, exhaustive branch test coverage, cross-process durability, and a ubiquitous C API.
* **Structural Weaknesses for Document Workloads:**
  1. **No Sorted Field Directory inside `JSONB`:** Because `JSONB` stores object members sequentially as key-value element pairs, `json_extract(doc, '$.field_30')` must scan past the headers of fields `1..29` in $O(k)$ time.
  2. **Full-Blob & Full-Page Rewrite on Partial Update:** Updating a 16-byte field inside a 3 KiB `JSONB` column via `UPDATE t SET doc = jsonb_set(doc, '$.counter', 2)` allocates and rewrites the entire 3 KiB `JSONB` blob and logs a full 4 KiB page frame to the WAL.
  3. **Array Indexing Friction:** Standard SQLite expression indexes (`CREATE INDEX idx ON t(json_extract(doc, '$.tags'))`) index the serialized JSON array string rather than its elements. Indexing individual array elements requires maintaining a normalized child table via `AFTER INSERT / UPDATE / DELETE` triggers.
* **OryqDB Counter-Measure:** `OryqDoc` embeds a 12-byte-per-field sorted directory header for $O(\log k)$ lookup, logs state-normalized delta patches (`SlotDeltaFrame`) guarded by a 64-bit bloom bitmask, and supports declarative multi-key array indexes natively.

### 3.2 LiteDB (.NET)
* **Architecture hypothesis (source verification pending):** Single-file C# engine using 8192-byte pages, BSON document serialization, skip-list/B-tree index nodes, and an appended WAL segment at the tail of the data file (or `-log.db` sidecar).
* **Strengths:** Intuitive collection/document developer experience (`db.GetCollection("users").Insert(...)`, `EnsureIndex`).
* **Structural Weaknesses:**
  1. **.NET CLR Dependency:** Cannot be embedded as a lightweight zero-runtime C library in Rust, C, Go, or Python applications without bundling a .NET runtime or AOT GC layer.
  2. **BSON Linear Scan & GC Pressure:** Queries deserialize or traverse sequential BSON tokens, generating managed heap allocations.
  3. **Overflow Page Fragmentation:** Documents exceeding 8 KiB chain across linked data blocks; frequent updates fragment pages and require offline/blocking file rebuilds.
* **OryqDB Counter-Measure:** Native Rust compilation (`oryq-core`, `liboryq` C ABI), zero-copy `OryqDocView<'cursor>` with step-unpinning page guards, and inline scalar retention when spilling large fields to overflow chains.

### 3.3 LMDB & `redb` (Copy-on-Write B+ Trees)
* **Architecture hypothesis (source verification pending):** Single-file key-value stores using physical shadow-paging Copy-on-Write B+ trees and dual root meta-pages.
* **Strengths:** Zero-copy reads, no WAL replay step on startup, strict ACID semantics.
* **Structural Weaknesses:**
  1. **$O(\text{height})$ Page Write Amplification:** Modifying a 100-byte record on a leaf page allocates a new 4 KiB/8 KiB leaf page and rewrites every ancestor internal page up to the root page (typically 3–5 full pages written per small mutation).
  2. **Reader-Induced File Bloat:** A single stalled read transaction pins historical B+ tree pages and blocks page reclamation across the entire file.
  3. **No Document/Query Layer:** Offers only raw `[u8] -> [u8]` key-value tables.
* **OryqDB Counter-Measure:** Logical-page WAL B+ tree (`PageVersionIndirectionMap`) avoids ancestor page rewrites on leaf updates, evicts old MVCC versions to `.oryq-wal`, and bounds stalled readers with `ORYQ_ERR_SNAPSHOT_TOO_OLD`.

### 3.4 UnQLite & ObjectBox
* **UnQLite:** Uses a virtual machine (Jx9 scripting language) to evaluate document collection filters and updates in C; lacks declarative compound/multi-key B+ tree index planning and modern MVCC concurrency.
* **ObjectBox:** Achieves fast field reads via FlatBuffers vtables, but requires compile-time code generation from static schema files (`.fbs` / annotations) rather than schema-free dynamic JSON/document evolution, and relies on a closed-source C++ core library.
* **OryqDB Counter-Measure:** 100% open-source (`MIT`) dynamic schema-free document model achieving $O(\log k)$ zero-copy field extraction without pre-compiled schema code generation.
