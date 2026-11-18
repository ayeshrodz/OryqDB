# LiteDB Storage Engine & Page Management Analysis

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-19
* **Work Package:** `W00` (Milestone 0 — Research & Architecture Baseline)
* **Pinned LiteDB Reference:** `https://github.com/litedb-org/LiteDB.git` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` (`dev`, MIT)

---

## 1. Verified File Organization & Page Layout

### 1.1 Physical Files & Page Sizing
* **Fixed page size:** `Constants.PAGE_SIZE = 8192` bytes; common header `Constants.PAGE_HEADER_SIZE = 32` bytes (`LiteDB/Utils/Constants.cs:27,32`).
* **Physical files:** Primary data file (`FileOrigin.Data`, `*.db`), WAL log file (`FileOrigin.Log`, `*-log.db`), and temporary external sort file (`SortDisk`). When AES encryption is enabled, physical page 0 is reserved for the encryption preamble/salt (`LiteDB/Engine/Disk/StreamFactory/FileStreamFactory.cs:102-104,155-156`).
* **Page types (`LiteDB/Engine/Pages/BasePage.cs:9`):** `Empty (0)`, `Header (1)`, `Collection (2)`, `Index (3)`, `Data (4)`, `VectorIndex (5)`, `Schema (6)`.
* **Format versions (`LiteDB/Engine/Pages/HeaderPage.cs:26-32`):** v8 (v5 stable) through v13 (`MVCC_FILE_VERSION`) on `dev`.

### 1.2 Common 32-Byte Page Header & Slotted Body (`BasePage.cs:25-44, 771-776`)
* **Header fields:** `PageID` (`uint`, `0..3`), `PageType` (`byte`, `4`), `PrevPageID` (`uint`, `5..8`), `NextPageID` (`uint`, `9..12`), `PageListSlot` (`byte`, `13`), `TransactionID` (`uint`, `14..17` — reused for CRC32C checksum on checkpointed checksummed data pages per `PageChecksum.cs:8-28`), `IsConfirmed` (`byte`, `18`), `ColID` (`uint`, `19..22`), `ItemsCount` (`byte`, `23`), `UsedBytes` (`ushort`, `24..25`), `FragmentedBytes` (`ushort`, `26..27`), `NextFreePosition` (`ushort`, `28..29`), `HighestIndex` (`byte`, `30`), `PageFormat` (`byte`, `31`: `0x00` legacy, `0xA5` checksummed).
* **Slotted footer:** Segments grow upward from offset `32`; 4-byte slot entries (`2B len + 2B pos`) grow downward from offset `8192`.
* **In-page `Defrag()` (`BasePage.cs:350-358, 562-634`):** When contiguous free space is smaller than an incoming insert but total `FreeBytes` suffices, `Defrag()` builds a `SortedList<ushort, byte>` of active segments, compacts them from offset `32` via `Buffer.BlockCopy`, and preserves slot indices so external `PageAddress` pointers remain valid.

### 1.3 HeaderPage (`PageID = 0`) & CollectionPage
* **`HeaderPage` (`HeaderPage.cs:45-56`):** Stores magic `"** This is a LiteDB file **"` (`32..58`), `FileVersion` (`59`), `FreeEmptyPageList` (`60..63`), `LastPageID` (`64..67`), `CreationTime` (`68..75`), 115-byte `EnginePragmas` block (`76..190`, including WAL CRC salt and `CRC1` marker), `InvalidDatafileState` (`191`), and an 8000-byte BSON document (`192..8191`) mapping collection names to `CollectionPage` IDs.
* **`CollectionPage` (`CollectionPage.cs:16-80, 234`):** Stores 5 `FreeDataPageList` heads (`32..51`) and an 8096-byte serialized index catalog (`96..8191`, hard cap of 255 indexes).

---

## 2. Free-Space Management, Overflow Chains & `Rebuild()` (Verified)

### 2.1 Free-List Buckets
* **`DataPage` 5-bucket free list (`DataPage.cs:88-128`, `Snapshot.cs:265-295`):**
  * Bucket `0`: `>= 7344` bytes free (90–100%); Bucket `1`: `>= 6120` (75–90%); Bucket `2`: `>= 4896` (60–75%); Bucket `3`: `>= 2448` (30–60%); Bucket `4`: `0..2447` (0–30%).
  * `GetMinimumIndexSlot(len)` returns `FreeIndexSlot(len) - 1`, so `GetFreeDataPage()` checks only bucket heads without scanning linked lists. However, any document/block `> 7344` bytes returns `-1` and bypasses all buckets, always allocating a new page.
  * Emptied `DataPage`s (`ItemsCount == 0`) convert to `PageType.Empty` and link onto `_header.FreeEmptyPageList` at commit (`TransactionService.cs:201-212`).
* **`IndexPage` 2-state free list (`IndexPage.cs:78-88`):** Listed in Slot `0` only if `freeBytes >= 1400` (`MAX_INDEX_LENGTH`); otherwise unlisted, stranding up to 1,399 bytes (~17%) per `IndexPage` until deletes reopen it.

### 2.2 Overflow Chains (`DataBlock`) & Full-Document Rewrite on Update
* **`DataBlock` (`DataBlock.cs:11-16`):** 6-byte prefix (`Extend` 1B + `NextBlock` 5B `PageAddress`) limiting single-page payload to `8150` bytes (`DataService.cs:13-17`; `MAX_DOCUMENT_SIZE = 2047 * 8150 ≈ 15.91 MiB`).
* **Insert (`DataService.cs:36-82`):** Chunks encoded document bytes into `<= 8150`-byte `DataBlock`s linked via `NextBlock`.
* **Update (`DataService.cs:87-162`, `Update.cs:98-171`):** Re-serializes the **entire** document and overwrites blocks in place starting at `pkNode.DataBlock`, spilling excess bytes into newly allocated extend blocks or freeing trailing blocks if the document shrank.

### 2.3 File Growth & Stop-the-World `Rebuild()`
* Normal deletes recycle empty pages via `_header.FreeEmptyPageList` and never shrink the `.db` file.
* `Rebuild()` (`Rebuild.cs:35-66`, `RebuildService.cs:67-330`) acquires `_locker.EnterExclusive()`, closes the engine, streams every document into a brand-new `-temp` `LiteEngine`, recreates all indexes, checkpoints, swaps files on disk using a `RebuildRecovery` marker, and reopens.

---

## 3. Buffer Pool (`MemoryCache`) & Durable Disk I/O (Verified)

* **Segmented CLOCK `MemoryCache` (`MemoryCache.cs`, `PageFramePool.cs`, `CacheReclaimer.cs`):**
  * Default `64 MiB` soft limit (`Constants.DEFAULT_CACHE_SIZE`), allocated in contiguous `byte[8192 * segmentSize]` arrays (`MEMORY_SEGMENT_SIZES = { 8, 128 }` pages).
  * `SharedPageReads.cs` adds a 4096-slot lock-free CAS hint array for already-pinned concurrent reads; cache misses, evictions, idle-to-busy transitions, and writable acquisitions serialize on a single monitor (`MemoryCache._sync`).
  * Under heavy pinning (`_idleReadablePages == 0`), the pool allocates elastic overflow segments beyond `LimitPagesRounded` (`CacheReclaimer.cs:55-59`) and trims idle segments back on transaction release.
* **Disk I/O & Sync (`StreamPool.cs`, `NativeFileSync.cs`, `DiskService.DurableFlush.cs`):**
  * One writer `FileStream` per origin + pooled read-only `FileStream`s per `DiskReader`.
  * `NativeFileSync.FlushToDisk()` calls `FlushFileBuffers` on Windows, `fcntl(F_FULLFSYNC)` on macOS, and `fsync(fd)` + first-commit parent-directory `fsync` on Linux/BSD, explicitly bypassing `.NET`'s `SystemNative_FSync` error-swallowing bug.

---

## 4. Verified Limitations & OryqDB Design Improvements

1. **255-Item Page Cap (`byte` `ItemsCount`/`HighestIndex`) & Fixed 8000B Collection / 8096B Index Header Ceilings:**
   * *OryqDB improvement:* Uses `u16` `slot_count` (`docs/file-format.md`) and stores collection/index catalogs in a dedicated logical-page B+ tree (`catalog_root_page_id`) with no 8 KiB metadata ceiling.
2. **Full-Document Rewrite on Partial Updates & Chained `DataBlock` Overflow:**
   * *OryqDB improvement:* Uses write-time state-normalized slotted-page delta patches (`MAX_DELTA_CHAIN = 4`, 64-bit bloom bitmask) and `SlotDeltaFrame` WAL records (`docs/document-model.md`, `docs/file-format.md`), with bounded whole-record overflow chains.
3. **Single-Header Page 0 Without Dual Shadow Slots:**
   * *OryqDB improvement:* Page 0 uses dual 256-byte shadow headers (`0..256` and `256..512`) with monotonic `header_generation` and XXH3-64 checksums, paired with ordered data-before-header checkpoint sync and redundant WAL reset intent (`docs/transactions.md`).
4. **Full Cache Flush on Full Checkpoint (`_disk.Cache.Clear()`):**
   * *OryqDB improvement:* Logical-page `PageVersionIndirectionMap` updates committed version pointers without evicting clean cached data pages on checkpoint.
