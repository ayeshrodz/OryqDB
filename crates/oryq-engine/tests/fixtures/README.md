# `oryq-engine` Test Fixtures & Persistent Format Verification

This directory documents the golden binary layout assertions and fault-injection scenarios used to verify `oryq-engine` (`W03` / M1 Complete).

## Test Suites

- `tests/persistence.rs`:
  - **Create / Reopen & Page 0 Shadow Header Failover:** Exercises `PageSize::Size4096`, `Size8192`, and `Size16384`; verifies `sync_all` and `sync_parent_dir` on creation, alternating `commit_header()` across Slot A (`0..256`) and Slot B (`256..512`), failover when either slot is corrupted, and `ErrorCategory::Corruption` when both slots are corrupted.
  - **Slotted Page Persistence & Corruption Detection:** Exercises `SlottedPageMut` insert, tombstone, payload compaction, reopen verification via `SlottedPageView::parse`, and 1-bit flip detection via `XXH3_64`.
  - **Overflow Chains & Freelist Reuse:** Exercises multi-page 25 KiB overflow chains (`OverflowStore::write_blob`/`read_blob`/`free_blob`), `OverflowRef` 16-byte round-trip, `BudgetHandle` quota enforcement, cycle rejection, MVCC snapshot-horizon reuse gating (`retired_tx_id < min_active_snapshot_tx_id`), and `FreelistManager::validate_chain` across reopen.
  - **Golden Binary Layout Verification:** Verifies exact byte offsets and `XXH3_64` / chained `XXH3_64_with_seed` checksums for `StorageHeader`, `WalHeader`, `WalFrameHeader`, and `CommitPayload`.
- `tests/locking.rs`:
  - **OS File Identity & Exclusive Locking (`OsVfs`):** Verifies that opening the same `.oryq` file via identical path, relative alias (`./`), or Unix symlink returns `ErrorCategory::LockedByProcess`, and that dropping the `Pager` releases both the `FileRegistry` lease and OS lock for clean reopen.
  - **Fault-Injected Sync Failures (`FaultVfs`):** Verifies that sync failures during `commit_header` propagate `ErrorCategory::Io` without advancing `active_slot`.
- `tests/btree_cache.rs` (`W04`):
  - **Pin-Aware Bounded `PageCache`:** Verifies `BudgetHandle` quota enforcement, pinned-page protection (`QuotaExceeded` when all resident pages are pinned), and LRU unpinned eviction.
  - **MVCC `VersionIndex`:** Verifies snapshot visibility (`tx_id <= snapshot_id`), horizon-safe pruning (`prune_below`), monotonicity checks, and `max_entries` quota.
  - **Multi-Level Byte `BTree` & `BTreeCursor`:** 400-key deterministic shuffled oracle test exercising leaf/internal/root splits, 100 upserts, point lookups, forward/range/reverse stack-based scans, persistence across reopen, sibling borrow/merge deletions, freelist chain validation, and full root collapse back to an empty leaf.
- `tests/admission.rs`, `tests/checkpoint.rs`, `tests/recovery.rs` (`W05`):
  - **MVCC Snapshot Admission & Savepoints:** Nested same-thread readers, `max_readers` / single-writer `Busy` contention, nested `SavepointId` rollback with page-allocation rollback, auto-rollback on `Drop`, and hard `max_wal_hard_bytes` rejection.
  - **Sealed/Unsealed Checkpointing & WAL Reset:** Durable dual-slot unseal, passive horizon checkpoint under active readers, full WAL reset & seal when idle, standalone sealed reopen without `.oryq-wal`, and interrupted `reset_intent_seq` completion on reopen.
  - **Crash Recovery & Uncertain Sync Poisoning:** Incomplete trailing frame truncation, restartable double-reopen recovery, fail-closed corruption on bit-flipped complete WAL frames or missing unsealed WAL, and `CommitOutcomeUnknown` poisoning on sync failure.

## Regenerating / Adding Fixtures

All W03 golden byte assertions are generated deterministically inside `tests/persistence.rs`. Frozen cross-version `.oryq` and `.oryq-wal` binary artifacts will be stored in this directory at the M10 format freeze (`W16`).
