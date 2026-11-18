# LiteDB Transactions, WAL Recovery & Concurrency Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)
* **Pinned LiteDB Reference:** `https://github.com/litedb-org/LiteDB.git` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (`dev`, MIT)

---

## 1. Transaction Lifecycle & Thread Affinity (Verified)

* **Thread-Bound Transactions (`TransactionMonitor.cs:17,51-100`, `TransactionService.cs:29`):**
  * `TransactionMonitor` tracks active transactions in `ThreadLocal<TransactionService> _slot`, and `TransactionService` stores `_ownerThread = Thread.CurrentThread`.
  * `ILiteDatabase.BeginTrans()` (`ILiteDatabase.cs:46`) explicitly prohibits crossing thread boundaries (`"Keep the block synchronous, with no await"`).
  * Transactions automatically flush unconfirmed dirty pages to `-log.db` via `Safepoint()` whenever dirty page count reaches `Constants.MAX_TRANSACTION_SIZE = 1000` (`TransactionService.cs:129-162`).
* **Commit Protocol (`TransactionService.cs:306-353`, `DiskService.WalWrite.cs:50,160-178`):**
  * Acquires `lock (_header)` (plus `lock (_header.PublicationLock)` when collection mappings changed).
  * Writes dirty pages sequentially to `-log.db` under `lock (_logPool.Writer.Value)`, marking the final page `IsConfirmed = true`.
  * Calls `FlushConfirmedLog()` (`NativeFileSync.FlushToDisk()` when `DurableCommits = true`) — strictly one synchronous disk flush per commit, with **no group commit**.
  * Publishes frame offsets in `_walIndex.ConfirmTransaction()` (`WalIndexService.cs:180-217`) under `_indexLock.EnterWriteLock()`, advancing `_currentReadVersion`.
* **Rollback Protocol (`TransactionService.Rollback.cs:14-122`):**
  * Discards dirty/clean buffers in RAM. If the transaction allocated new pages (`NewPages.Count > 0`), it executes a synthetic compensating WAL commit under `lock (_header)` writing those pages as `PageType.Empty` chained onto `_header.FreeEmptyPageList`.

---

## 2. Isolation Model & WAL Recovery (Verified)

* **MVCC Snapshot Isolation (`SnapShot.cs:51-107, 189-234`):**
  * Snapshot creation acquires `lock (_header.PublicationLock)` to atomically pair `_readVersion = _walIndex.PinSnapshot()` with the collection's `CollectionPageID`.
  * Readers resolve pages from `_localPages` -> `_walIndex.GetPageIndex(pageID, _readVersion)` (highest version `<= _readVersion`) -> `FileOrigin.Data`.
  * Readers do not block writers; writers to the *same* collection serialize via per-collection `CollectionLock` (`SnapShot.cs:74-77`), while writers to *different* collections run concurrently until commit serialization on `lock (_header)` and `WalWriterLock`.
* **Checksummed WAL Frames & Startup Recovery (`WalChecksum.cs:8-128`, `WalIndexService.cs:223-316`):**
  * Format v10+ writes `8192 + 64 = 8256`-byte frames (`ChecksummedWalStream`) with magic `WAL1`, 16-byte WAL salt, per-transaction frame count, rolling nonlinear XOR digest, monotonic `Sequence`, and CRC32C checksum.
  * Startup `RestoreIndex()` scans `-log.db` sequentially, buffering `PagePosition`s per `transactionID` and applying them only when a valid `IsConfirmed == true` frame is verified. Unconfirmed frames are ignored; highest observed `transactionID` is preserved to prevent ID reuse.
* **Full vs. Partial MVCC Checkpointing (`WalIndexService.Checkpoint.cs:56-235`, `WalIndexService.Reclamation.cs:14-59`):**
  * Triggered automatically after commit when `-log.db` reaches `Pragmas.Checkpoint * 8192` (default 1000 pages = 8 MiB).
  * If no readers are pinned (`live.Length == 0`) and exclusive access is obtained: backfills committed frames to `.db`, syncs, clears `_disk.Cache.Clear()`, rotates WAL salt, and truncates `-log.db` to `0`.
  * If readers are active (`live.Length > 0`): executes a partial checkpoint up to `live[0]`, writes a retirement record, and reclaims obsolete unreachable WAL slots in place without truncating `-log.db`.

---

## 3. In-Process & Multi-Process Locking (Verified)

* **In-Process Locking (`LockService.cs`, `TransactionGate.cs:13-146`, `CollectionLock.cs:9-34`):**
  * `LockService._transaction` uses a custom `Monitor`-based `TransactionGate` (replacing `ReaderWriterLockSlim` so read leases can be released on a different thread during cursor disposal), enforcing writer-preference (`_waitingWriters != 0`) and rejecting reader-to-exclusive upgrades.
  * `WalIndexService._indexLock` still uses `ReaderWriterLockSlim` (`WalIndexService.cs:26`).
* **Multi-Process `SharedEngine` (`LiteDB/Client/Shared/SharedEngine.cs`, `SharedEngine.CoordinatedReads.cs`):**
  * Serializes mutations across processes via named OS `Mutex` + turnstile (`SharedMutexOwner`), opening and closing a `LiteEngine` per operation (reusing `SharedFileHandles` and deferring close-checkpoint up to `CLOSE_CHECKPOINT_PAGES = 50`).
  * On .NET 8+, adds mutex-free coordinated reads via a memory-mapped `SharedCoordinationPage` and OS-held reader lease files (`SharedReaderRegistry`).
* **Experimental `CoordinatedEngine` (`LiteDB/Client/Coordinated/CoordinatedEngine.cs`, `CoordinatorHost.cs`):**
  * Elects one host process (`CoordinatorMutex`) holding the sole writable `LiteEngine` and serving peer write requests over `NamedPipeServerStream` (one dedicated thread per pipe session to satisfy `ThreadLocal` transaction affinity), while peers execute local direct snapshot reads via `SharedReaderRegistry` and `WalIndexService.ExtendIndex()`.

---

## 4. Verified Limitations & OryqDB Improvements

1. **Thread-Bound `ThreadLocal` Transactions:** Forces LiteDB's `CoordinatedEngine` to dedicate one OS thread per client session and blocks cross-thread cursor/transaction movement. *OryqDB* uses explicit `ReadTransaction`/`WriteTransaction` values with movable `Send + !Sync` FFI handles (`docs/ffi.md`).
2. **Global Commit Serialization Without Group Commit:** Every durable LiteDB commit executes a synchronous `fsync`/`F_FULLFSYNC`/`FlushFileBuffers` under `lock (_header)` and `WalWriterLock`. *OryqDB* keeps one-sync explicit transactions clean in the M2 baseline and defines a decoupled `WriteCommand` batching port (`oryq-engine/src/commit_batch/`) for post-baseline `E3` (`docs/transactions.md`, `docs/designs/modular-architecture.md#94-engine-batch-commands-and-module-metrics`).
3. **Checkpoint Cache Wipe (`_disk.Cache.Clear()`):** Full checkpoints in LiteDB evict every cached page, causing a cold-cache latency spike immediately after checkpointing. *OryqDB* updates `PageVersionIndirectionMap` entries in place without purging clean cached pages.
4. **Multi-Process Complexity Isolated Behind Extension Gate:** LiteDB's `SharedEngine` and experimental `CoordinatedEngine` illustrate how fragile cross-process coordination is; *OryqDB* isolates `SharedShmCoordinator` behind `CoordinatorProvider` (`X02` / `E2`) and enforces a single exclusive OS file lock + process-global `(st_dev, st_ino)` registry in default `SingleProcess` mode (`docs/transactions.md`).
