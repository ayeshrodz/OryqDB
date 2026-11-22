# OryqDB Operations, Backup & Recovery Runbook (`v0.11.0`)

## 1. On-Disk File Layout

An OryqDB database at `<path>.oryq` consists of:
- **`<path>.oryq`** — Primary database file (Page 0 dual shadow headers + logical-page slotted B+ trees + overflow chains + freelist).
- **`<path>.oryq-wal`** — Companion checksummed Write-Ahead Log (present when unsealed; absent or empty after a sealed checkpoint).
- **`<path>.oryq-shm` (+ `.wlock`, `.rlock`)** — Ephemeral multi-process coordination files (created only when using `SharedProcessCoordinator` / `X02`). Safe to delete when zero processes have the database open.

> **Never copy an unsealed `.oryq` file with `cp` while writes are active** without copying its matching `.oryq-wal` or using online `Database::backup_to`.

## 2. Durability (`SyncMode`) & Checkpointing

- **`SyncMode::Full` (default):** Calls `sync_all()` on WAL commits and syncs parent directories on creation/rename. Recommended for production ACID durability.
- **`SyncMode::Normal`:** Calls `sync_data()` on WAL commits. Faster on filesystems where metadata syncs are expensive while still durable against process crashes.
- **`SyncMode::Off`:** Skips OS fsync calls. Use only for ephemeral caches, tests, or bulk offline loads followed by `checkpoint(true)`.

### Checkpointing & Sealing

- `db.checkpoint(false)`: Copies committed WAL pages up to the active MVCC reader retention horizon into `.oryq` without blocking readers.
- `db.checkpoint(true)` (or `oryq checkpoint <db>`): When zero active readers remain, copies all committed WAL pages into `.oryq`, atomically executes the `reset_intent_seq` WAL reset, and seals the database (`wal_required = false`) so `<path>.oryq` is completely self-contained.

## 3. Online Backups & Integrity Verification

### Sealed Online Backup (`backup_to`)

```bash
oryq backup /var/lib/app/prod.oryq /var/backups/prod-2026-11-22.oryq
```

`Database::backup_to(dest)` (and `oryq_db_backup_to` in C ABI):
1. Pins a single MVCC read snapshot (concurrent readers remain unblocked).
2. Copies all collections, indexes (including FTS & Vector indexes when compiled with those features), and documents into a temporary file.
3. Checkpoints, seals (`wal_required = false`), runs `integrity_check()`, and atomically renames into `dest`.
4. Cleans up temporary files on any failure and fails closed (`UnsupportedFeature`) if a pluggable index's provider is not compiled into the binary.

### Integrity Checks

```bash
oryq integrity-check /var/lib/app/prod.oryq
```

Verifies every document's `_id`, primary index resolution, payload equality, and collection count consistency within a single MVCC snapshot.

## 4. Crash Recovery & Incident Procedures

### Normal Process Crash / Power Loss
Simply reopen the database (`Database::open`). OryqDB automatically runs deterministic, restartable 7-point recovery:
1. Selects the highest valid Page 0 shadow header slot (failing over automatically if one slot was torn mid-write).
2. Completes any interrupted `reset_intent_seq` WAL reset cleanly.
3. Scans `.oryq-wal`, validates cumulative xxHash3 frame checksums, truncates any incomplete/uncommitted tail, and replays all committed transactions.

### `ErrorCategory::CommitOutcomeUnknown` (Engine Poisoned)
If an OS write or `fsync` fails mid-commit, OryqDB cannot know whether the commit reached durable media. It **poisons** the in-memory `StorageEngine` handle and rejects all subsequent reads/writes with `CommitOutcomeUnknown`.
- **Procedure:** Close all handles to the database, resolve the underlying storage/disk issue (e.g., disk full, I/O error), and reopen the database to run deterministic WAL recovery.

### Forensic Read-Only Inspection (Non-Mutating)
Use the `oryq inspect` commands on a suspect or locked database without mutating or replaying it:
```bash
oryq inspect prod.oryq page 0
oryq inspect prod.oryq wal
oryq inspect prod.oryq locks
```

### Multi-Process `.oryq-shm` Stale Peer Recovery
If a peer process using `SharedProcessCoordinator` crashes abruptly (`SIGKILL` / `TerminateProcess`), the OS kernel immediately releases its `.oryq-shm.wlock` / `.oryq-shm.rlock` handles. Any surviving peer calling `coord.reclaim_stale_peers()` deterministically clears the dead writer, dead reader slots, and stale WAL-reset markers so `retention_horizon()` and WAL resets resume immediately.

### Reduced-Build Feature-Bit Behavior
If a database contains an optional pluggable index (`index.fts` bit `0`, `index.vec` bit `1`) and is opened by a binary compiled **without** that Cargo feature:
- **Reads (`find_by_id`, `find`, `aggregate`) continue to work.**
- **Mutations (`insert`, `update`, `delete`, `drop_index`) and `backup_to` fail closed with `ErrorCategory::UnsupportedFeature`** to prevent silent index desynchronization or index loss.
