# Module: Physical Storage and Transactions

* **Owner:** `oryq-engine`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Own the physical durability domain: positioned I/O, checksummed pages/trees, cache versions, snapshots, savepoints, WAL, recovery, checkpoint and sealed files.

## 2. Architecture

oryq-types only. Engine must compile and recover byte records without document/index/query/core.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

ReadAccess exposes an immutable Send + Sync ReadProjection; write projections freeze current private buffers until query release. Public movable handles do not retain thread-affine MutexGuard values. ReadTxn resolves bytes under a pinned snapshot; WriteTxn exposes transactional byte trees/pages and nested statement savepoints. Only its owner can commit. A restricted provider storage view cannot commit, checkpoint or obtain file handles. RecordCompactor is a pure injected operation; engine supplies bytes/budget and core provides the document adapter.

## 4. Internal Data Structures and Owned Files

Physical handles/page guards, version map, private dirty state, savepoint log, snapshot token, coordinator, WAL and checkpoint state. Catalog-root metadata is an opaque page ID; core owns its logical schema.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/{lib,config,ports,inspect}.rs` | Export byte-level engine handles/options and pure compactor/coordinator ports. |
| `src/io/{mod,vfs,registry}.rs; src/io/platform/{mod,unix,windows}.rs` | I/O contract, file identity/handle ownership and isolated OS-specific adapters. |
| `src/format/{mod,header,page,wal}.rs` | Authoritative encoders/decoders for existing persistent-byte contracts. |
| `src/pager/{mod,allocator,freelist,overflow,guard}.rs` | Page ownership/allocation/reuse, whole-record overflow and RAII pins. |
| `src/btree/{mod,search,insert,delete,cursor,layout}.rs` | Byte-key/value tree, traversal and generic prefix-restart layout; no document-field interpretation. |
| `src/cache/{mod,pool,versions}.rs` | Bounded page/version metadata and retained-WAL reconstruction. |
| `src/transaction/{mod,read,write,savepoint,publish}.rs` | Transaction-private state, savepoint rollback and ordered committed publication. |
| `src/coordinator/{mod,single_process}.rs` | Provider boundary and default protected reader admission/writer ownership. |
| `src/recovery/{mod,scan,replay}.rs` | Bounded WAL scanning and restartable physical replay without domain callbacks. |
| `src/checkpoint/{mod,copy,reset,seal}.rs` | Data/header ordering, redundant reset intent and sealed/unsealed transitions. |
| `src/coordinator/shared_process/{mod,admission,locks,recovery}.rs` | E2-only cross-process provider; keep absent/disabled before its gate. |
| `src/commit_batch/{mod,queue,leader}.rs` | E3-only queued-command batching under existing ownership/failure rules. |
| `tests/{persistence,recovery,admission,checkpoint,locking}.rs; tests/support/{mod,fault_io,crash_process}.rs` | Owner-local byte persistence, adversarial schedule and crash-injection harness. |
| `tests/fixtures/README.md` | Record golden main-header/page/WAL/overflow/freelist fixtures and failure cuts. |

## 5. Invariants

Storage/recovery never calls a query/index/provider algorithm. Recover acknowledged physical state before opening domain catalogs. Snapshot pins outlive borrowed views; cancellation cannot advance reuse before release. Follow the canonical reset/unseal/sync order exactly.

## 6. Error Handling

Commit write/sync failure is uncertain and faults the engine. Complete invalid WAL records fail closed; incomplete final uncommitted records follow recovery rules. Hard WAL/cache/transaction caps reject work without recycling live storage.

## 7. Concurrency Behavior

One logical writer through sync/publication; admission mutex registers snapshot tokens atomically with reclamation. Compaction callbacks run on transaction-private immutable inputs and cannot recursively acquire database locks.

## 8. Performance Characteristics

Keep concrete page loops and bounded cache/version metadata. Trait dispatch at VFS/provider/compactor boundaries is allowed; no document deserialization during tree navigation or recovery.

## 9. Known Limitations and Change Boundary

Engine is mandatory. Replacing a pager/coordinator must preserve format and retention contracts. Shared processes and batching are separately gated extensions; their module listing does not enable them.

## 10. Testing Methodology

W03–W05/W12: golden bytes, file aliases/locking, crash at every reset/unseal write+sync, retry recovery twice, nested reader admission, in-flight cancellation, uncertain sync and savepoint allocation rollback.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
