# OryqDB Architecture Decision Records (ADRs)

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

## ADR-001: Logical-Page Slotted B+ Tree with Write-Ahead Log (`.oryq-wal`) and MVCC Indirection Map

* **Status:** Design decision; implementation evidence pending
* **Context:** We evaluated three storage architectures per [docs/draft-spec.md](./draft-spec.md): (A) Page-oriented B+ tree with Write-Ahead Logging (WAL), (B) Log-Structured Merge (LSM) tree, and (C) Physical Copy-on-Write (CoW) B+ tree. During the 7-lens Goldfish brainstorm, reviewers demonstrated that combining physical shadow-paging CoW with page-local sub-document delta records is self-contradictory because physical CoW allocates a new disk block on every leaf mutation and rewrites every ancestor internal page up to the root ($O(\text{height})$ 8192-byte page writes per transaction). LSM trees were rejected because multi-file SSTable directories break single-file portability and background compaction threads cause latency spikes.
* **Decision:** Use a **Logical-Page Slotted B+ Tree (`page_size = 8192` bytes) backed by an append-only Write-Ahead Log (`.oryq-wal`) and an in-memory `PageVersionIndirectionMap`**. Logical `PageId`s remain stable across leaf mutations so parent B+ tree nodes are never rewritten unless a leaf page splits. The first write to a page after a checkpoint logs a full 8192-byte `FullPageFrame` (preventing torn pages), and subsequent mutations in the same WAL cycle log compact `SlotDeltaFrame` records.
* **Consequences:** Eliminates $O(\text{height})$ CoW write amplification, preserves single-file portability (`.oryq` + transient `.oryq-wal`), and supports concurrent MVCC snapshot reads via `PageVersionIndirectionMap`. Requires startup WAL recovery after unclean shutdown and periodic checkpointing bounded by `max_wal_size_bytes` and `max_snapshot_duration`.

---

## ADR-002: `OryqDoc` Sorted Field-Offset Directory & State-Normalized Slotted-Page Deltas with 64-Bit Bloom Mask

* **Status:** Design decision; implementation evidence pending
* **Context:** Standard BSON encodes fields sequentially, forcing an $O(k)$ linear scan to extract any field. Conversely, chaining raw operation logs (`$inc`, `$push`, `$pull`) as slotted-page deltas forces readers to traverse the delta chain ($O(D + \log k)$), execute operator logic, and heap-allocate computed values on read, while tightly coupling `storage/` page compaction to `document/` update operators.
* **Decision:**
  1. Encode documents in **`OryqDoc` v1**, a little-endian binary format featuring an 8-byte header and a sorted array of 12-byte directory entries (`[name_hash_prefix: u32 BE, name_offset: u32 LE, val_descriptor: u32 LE]`) enabling $O(\log k)$ zero-copy field extraction (`OryqDocView<'a>`) and unaligned-safe scalar reads (`from_le_bytes`).
  2. Evaluate relative update operators (`$inc`, `$push`, `$pull`, `$set`, `$unset`) at **write time** inside `document/patch.rs` to emit **State-Normalized Value Patches** (containing the resulting contiguous field slice or tombstone).
  3. Store a **64-bit 2-hash Field Bloom Bitmask** (`field_bloom_mask: u64`) and `delta_count: u8` (`MAX_DELTA_CHAIN = 4`) in each 16-byte slotted-page directory entry. Readers test the bitmask in $O(1)$ register time; if unset, they bypass deltas and binary-search the base `OryqDoc` header in $O(\log k)$ time.
  4. Decouple `storage/` compaction from `document/` semantics via the pure `RecordCompactor` trait.
* **Consequences:** Preserves $O(\log k)$ zero-copy reads on unmodified fields and $O(1)$ zero-copy slice reads on modified fields without read-time heap allocation. Costs 8 bytes per slotted-page directory entry for the bloom bitmask.

---

## ADR-003: Dual-Mode `SnapshotCoordinator` & Zero-Delay Leader-Follower Group Commit

* **Status:** Design decision; implementation evidence pending
* **Context:** Single-process multithreaded applications should not pay per-transaction OS file-lock syscalls (`fcntl` / `LockFileEx`), yet opening a database without any OS lock risks silent corruption if a second process or the `oryq` CLI opens the file concurrently. Furthermore, in multi-process mode (`.oryq-shm`), creating Rust `&[u8]` references over cross-process mutable `mmap` memory is Undefined Behavior, and user-space spinlocks deadlock permanently if a process is terminated via `SIGKILL`.
* **Decision:**
  1. Abstract concurrency control behind `SnapshotCoordinator` with two modes:
     * **`SingleProcessCoordinator` (Default):** Checks a process-global `(st_dev, st_ino)` file registry (preventing POSIX `close()` from dropping process locks), acquires one exclusive OS advisory lock at `Database::open()`, and uses a short admission mutex and reader tokens for snapshots; serialized writer ownership covers commit sync/publication (**zero per-transaction lock syscalls**).
     * **`SharedShmCoordinator` (Opt-In):** Coordinates across up to 128 OS processes using `.oryq-shm` accessed strictly via raw-pointer atomic intrinsics (`*const AtomicU64`, zero Rust slice references), paired with a 1-byte OS kernel file lock (`16 + slot_idx`) per registered process slot for automatic kernel-assisted `SIGKILL` detection and dead-slot recovery.
  2. Keep explicit write transactions serialized through sync/publication. Later E3 batching executes queued commands serially against a private batch view; never accept independently prepared dirty pages.
* **Consequences:** Default reads avoid OS file-lock calls but are not wait-free. Shared-process mode (E2) and queued-command group commit (E3) require independent protocol tests; see transactions.md Sections 4.1–4.2. No deadlock or throughput claim is yet verified.

---

## ADR-004: Order-Preserving Memcomparable Key Encoding & Prefix-Compressed Compound Indexes ($R = 16$)

* **Status:** Design decision; implementation evidence pending
* **Context:** Supporting polymorphic document types, prefix-compressed compound indexes, and multi-key array indexes requires a total canonical ordering across all `OryqTypeTag` variants that can be compared with raw `memcmp` without full key deserialization. Naive prefix compression across an entire B+ tree leaf page degrades binary search from $O(\log N)$ to $O(N)$ linear scan, and allowing multiple arrays in a compound index causes $O(|A_1| \times |A_2|)$ Cartesian entry explosion.
* **Decision:**
  1. Encode all index keys using a canonical **Order-Preserving Memcomparable Key Encoding** (`MinKey < Missing/Null < Bool < NaN < Number (exact variable-width i64/f64/Decimal) < DateTime < UTF-8 String (\x00\xFF escaped) < Binary < ObjectId/UUID < Array < Document < MaxKey`).
  2. Store an uncompressed **Restart Point** every $R = 16$ slots on prefix-compressed Index Leaf Pages (`0x03`), preserving $O(\log(N/16) + 16)$ binary search speed.
  3. Enforce MongoDB's parallel-array invariant on compound indexes: at most one indexed field in a compound index may evaluate to an array per document (`ORYQ_ERR_PARALLEL_ARRAY_INDEX`).
* **Consequences:** Enables fast `memcmp` B+ tree comparisons, exact cross-numeric equality (`10 == 10.0`), compact compound index pages, and bounded multi-key index fan-out.

---

## ADR-005: WAL-Backed `PluggableIndex` Substrate & Compile-Time Feature Gates (`oryq-fts`, `oryq-vec`)

* **Status:** Design decision; implementation evidence pending
* **Context:** Full-Text Search (BM25) and Vector Search (ANN/kNN) are high-value NoSQL differentiators over SQLite, but baking tokenizers, stemmers, and vector graphs unconditionally into `oryq-core` expands base binary size and violates Section 25 of [docs/draft-spec.md](./draft-spec.md). Furthermore, if pluggable indexes write to external sidecar files or mutate dozens of posting-list/HNSW pages synchronously per document insert, they either break ACID crash recovery or stall transaction commits.
* **Decision:**
  1. Define the index-provider trait and `IndexStorageContext<'tx>` inside `oryq-index`, requiring all plugin pages (`page_type = 0x07`) and pending-delta B+ trees to reside inside the main `.oryq` file and `.oryq-wal`.
  2. Use **Two-Tier Pending-Delta Batching**: `on_doc_write()` appends compact entries to a per-index Pending-Delta B+ Tree, while `on_commit_batch()` folds pending entries into compressed segments.
  3. Register providers exclusively at core composition; providers depend only on types/document/engine/index and never on core. ADR-008 supersedes the former embedding/FFI-host and core-owned provider-contract placement.
  4. Isolate `crates/oryq-fts` and `crates/oryq-vec` as optional post-baseline E1 workspace crates behind Cargo feature flags (`fts`, `vec`), and record active index requirements in Page 0's `required_write_features` bitmask so builds compiled without an active feature fail safely with `ORYQ_ERR_UNSUPPORTED_FEATURE` on conflicting writes.
* **Consequences:** Keeps the default `oryq-core` binary minimal and dependency-light while providing single-file ACID durability and hybrid B+ tree bitmap pre-filtering (`search_with_filter`) when FTS or Vector Search is enabled.

---

## ADR-006: Step-Unpinning Lending Cursors & Dual-Path Panic-Safe C ABI (`include/oryq.h`)

* **Status:** Design decision; implementation evidence pending
* **Context:** Returning zero-copy slices directly from a bounded buffer pool requires pinning cache frames; if callers accumulate zero-copy references across a large scan, they pin all cache frames and deadlock the buffer pool. Across the `oryq_*` C ABI, forcing JSON serialization on every read erases `OryqDoc`'s speed advantage, whereas returning untracked raw pointers risks Use-After-Free (UAF) crashes.
* **Decision:**
  1. In Rust, `Cursor::next_view()` returns an ephemeral `OryqDocView<'cursor>` that automatically unpins the previous leaf `PageGuard` before pinning the next leaf page (pinning at most 1 leaf frame per cursor), paired with `.to_owned_doc() -> OryqDocBuf` (`Arc<[u8]>`) when callers need references that outlive the cursor step.
  2. In `include/oryq.h`, provide both JSON convenience APIs and cursor-scoped zero-copy getters (`oryq_cursor_get_utf8`, `oryq_cursor_get_i64`, etc.) whose returned `(ptr, len)` slices are valid until the next `oryq_cursor_next()` or `oryq_cursor_close()` call (matching `sqlite3_column_blob` semantics), plus `oryq_cursor_clone_doc()` for standalone owned `oryq_doc_t *` handles.
  3. Wrap every `extern "C"` entrypoint in `std::panic::catch_unwind`.
* **Consequences:** Inline scalar reads can avoid allocations; raw/overflow export uses bounded owned scratch. Cursors retain transaction state; completion with children returns BUSY, and `oryq_txn_close` frees completed handles. Panic fallback depends on return type; thread-local diagnostics support failed open. See ffi.md.


## ADR-007: Reviewed Correctness Contracts & Baseline Release Scope

* **Status:** Design decision; implementation evidence pending.
* **Decision:** Subsystem documents own detailed contracts; the roadmap owns sequencing. M10 releases the baseline, while search, shared processes and group-commit optimization have separate extension gates. Reader cancellation cannot revoke live pins. Sync failure has an unknown outcome and faults the engine. Checkpoint syncs data before headers and retires WAL only after redundant reset intent. Exact variable-width numeric keys replace the rejected nine-byte codec; internal non-reused RecordIds support collision-free bitmaps. FFI child ownership and shared query budgets are mandatory.
* **Alternatives:** Retaining the float-plus-byte codec loses exact integers; forcibly advancing reader floors can recycle live storage; treating failed sync as rollback can misreport durable transactions. Their correctness cost outweighs smaller keys, immediate reclamation or simpler error handling.
* **Validation:** [Review acceptance matrix](ROADMAP.md#review-driven-acceptance-matrix). No format freeze or runtime proof is asserted.


## ADR-008: Domain Crates, Owner-Local Features and One Composition Root

* **Status:** Proposed source architecture; implementation evidence pending.
* **Decision:** Keep `oryq-core` as the public facade and move shared contracts, documents, the physical engine, indexes and queries into domain crates. FFI/CLI depend only on core. Optional providers depend on lower-layer ports and register at core composition. Each domain owns local feature folders, registries, tests and specifications; source placement follows [modular-architecture.md](designs/modular-architecture.md).
* **Reason:** Cargo dependencies and narrow exports enforce major boundaries; owner-local feature registries avoid adding scattered consumers for each operator/provider. A pure compactor adapter and QuerySource inversion close the storage/document and query/core cycles.
* **Alternatives:** One crate with folders relies mostly on review to police coupling; one crate per operator adds excessive package/API overhead; dynamic native plugins add ABI/unload guarantees outside the product scope.
* **Compatibility:** Source organization changes; reviewed storage bytes, transaction ordering and stable C symbols do not. Shared contract/format changes still involve their direct consumers. Removing a persisted provider requires capable-build drop/reduced-build tests, not just deleting files.
* **Delivery:** [W00–W16 and X01–X03](MODULE_DEVELOPMENT_PLAN.md); product M0–M10/E1–E3 acceptance remains intact.
