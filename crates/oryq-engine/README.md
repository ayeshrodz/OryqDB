# `oryq-engine`

Physical storage, positioned VFS/PAL, Page 0 dual shadow headers, 8192B slotted pager, freelist, overflow chains, bounded page cache, MVCC version metadata, logical-page byte B+ trees, `.oryq-wal`, single-process MVCC transactions, nested savepoints, checkpoint/reset, and restartable crash recovery for **OryqDB** (`W03–W05` / Milestone 2 Complete).

## Responsibilities (`W03–W05`)
- **`config`:** `PageSize` (`4096`, `8192` default, `16384`), `SyncMode`, and `EngineConfig` (`max_cache_bytes`, `max_version_entries`, `max_wal_hard_bytes`, `max_readers`).
- **`ports`:** `RecordCompactor` pure compaction port, `CommitReceipt`, and `CheckpointOutcome`.
- **`io`:** `StorageIo` / `FileHandle` VFS traits, `OsVfs` with isolated Unix/Windows positioned I/O, exclusive OS locking (`fs4`), parent-directory fsync, and process-local `FileRegistry` / `FileLease`.
- **`format`:** Authoritative little-endian encoders/decoders and `XXH3_64` checksum validators for Page 0 dual shadow `StorageHeader`, 32B `PageHeader`, 16B `SlotEntry`, `OverflowHeader`/`OverflowRef`, `FreelistTrunkHeader`/`FreelistEntry`, and `.oryq-wal` (`WalHeader`, chained `WalFrameHeader`, `CommitPayload`).
- **`pager`:** `Pager`, `PageBuf`/`PageGuard`, `FreelistManager`, `PageAllocator`, and `OverflowStore`.
- **`cache`:** Pin-aware `PageCache`/`PinnedPage` (`BudgetHandle`) and `VersionIndex`.
- **`btree`:** `BTree`, stack-based forward/reverse `BTreeCursor`, `PageStore`/`PageStoreMut`/`PagerPageStore`, splits, borrow/merge rebalancing, and root collapse.
- **`coordinator`:** `SingleProcessCoordinator`, `SnapshotGuard`, and `WriterPermit` (atomic snapshot admission, serialized writer ownership, and uncertain-commit poisoning).
- **`recovery`:** `scan_wal` (chained `XXH3_64` verification, incomplete-tail truncation, fail-closed corruption) and restartable `replay_into_version_index` without domain callbacks.
- **`checkpoint`:** Passive horizon checkpoint (`copy_committed_pages_to_main`), durable unseal (`unseal_for_first_write`), `reset_intent_seq` WAL reset (`reset_wal_after_full_checkpoint`), and `complete_interrupted_reset`.
- **`transaction`:** `StorageEngine`, `ReadTxn`, `WriteTxn` (nested `SavepointId` rollback/release, auto-rollback on `Drop`, hard WAL cap, uncertain-sync poisoning).

## Dependencies
- Internal: `oryq-types` only (zero dependencies on `oryq-document`, `oryq-index`, `oryq-query`, or `oryq-core`).
- External: `xxhash-rust` (XXH3-64), `fs4` (cross-platform exclusive file locking).
- Safety: `#![forbid(unsafe_code)]`.

## Verification
```bash
cargo test -p oryq-engine
cargo run -p xtask -- modules engine
```
