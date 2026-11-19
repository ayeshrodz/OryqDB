# `oryq-engine`

Physical storage, positioned VFS/PAL, Page 0 dual shadow headers, 8192B slotted pager, freelist, overflow chains, bounded page cache, MVCC version metadata, logical-page byte B+ trees, and WAL binary codecs for **OryqDB** (`W03–W04`).

## Responsibilities (`W03–W04`)
- **`config`:** `PageSize` (`4096`, `8192` default, `16384`), `SyncMode`, and `EngineConfig`.
- **`io`:** `StorageIo` / `FileHandle` VFS traits, `OsVfs` with isolated Unix/Windows positioned I/O (`pread`/`pwrite` / `seek_read`/`seek_write`), exclusive OS locking (`fs4`), parent-directory fsync, and process-local `FileRegistry` / `FileLease` alias protection.
- **`format`:** Authoritative little-endian encoders/decoders and `XXH3_64` checksum validators for Page 0 dual shadow `StorageHeader`, 32-byte `PageHeader`, 16-byte `SlotEntry` (`SlottedPageView` / `SlottedPageMut`), `OverflowHeader` / `OverflowRef`, `FreelistTrunkHeader` / `FreelistEntry`, and `.oryq-wal` (`WalHeader`, `WalFrameHeader`, `CommitPayload`).
- **`pager`:** `Pager` (create/reopen, Slot A/B failover & alternating commits), `PageBuf` / `PageGuard` (RAII pins + `BudgetHandle`), `FreelistManager` (MVCC snapshot-horizon reuse gating + chain validation), `PageAllocator`, and `OverflowStore` (bounded chain read/write/free).
- **`cache`:** `PageCache` (pin-aware LRU cache bounded by `BudgetHandle`), `PinnedPage` (`Arc<[u8]>` pin handle), and `VersionIndex` (bounded per-page committed version chains with snapshot resolution and horizon pruning).
- **`btree`:** `BTree`, `BTreeCursor` (stack-based forward/reverse range scans with no on-disk sibling links), `PageStore` / `PageStoreMut` / `PagerPageStore`, leaf & internal splits, sibling borrow/merge rebalancing, and root collapse.

## Dependencies
- Internal: `oryq-types` only (zero dependencies on `oryq-document`, `oryq-index`, `oryq-query`, or `oryq-core`).
- External: `xxhash-rust` (XXH3-64), `fs4` (cross-platform exclusive file locking).
- Safety: `#![forbid(unsafe_code)]`.

## Verification
```bash
cargo test -p oryq-engine
cargo run -p xtask -- modules engine
```
