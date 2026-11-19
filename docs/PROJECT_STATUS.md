# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 2 — B+ tree, cache, WAL, MVCC & recovery (COMPLETE — `v0.2.0` / `W01–W05` verified)
STATUS: In Progress — M1 & M2 complete (`oryq-types`, `oryq-document`, full `oryq-engine`); entering M3 (`W06` document updates, `W07` primary index, `W08` core CRUD vertical slice)

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`):
  - Eight-crate Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, `rustfmt.toml`, `.clippy.toml`, and `.github/workflows/ci.yml`.
  - Full implementation of `oryq-types` (IDs, boxed `ErrorEnvelope`, RAII `BudgetHandle`/`Reservation`, `CancellationToken`, `CapabilityId`/`FeatureBitmask`/`VersionRange`).
  - `architecture/{modules,capabilities,contract-changes}.toml` and `xtask architecture` / `xtask modules <id>` enforcement tool.
- Completed `W02` (`v0.1.0-alpha.2`):
  - Full implementation of `oryq-document` (`#![forbid(unsafe_code)]`, engine-independent): `OryqDoc` v1 binary codec & strict validator, zero-copy `DocumentView`/`ArrayView`, `ObjectId`/`Uuid`/`Decimal128` (IEEE 754-2008 BID), `FieldPath`, exact cross-type comparator (`compare_values`), and Extended-JSON conversion.
- Completed `W03` (`v0.1.0` / M1 Complete):
  - Implemented `oryq-engine` VFS/PAL (`StorageIo`, `FileHandle`, `OsVfs`, `FileRegistry`, `fs4` locking), persistent binary format codecs, and `Pager`/`PageAllocator`/`FreelistManager`/`OverflowStore`.
- Completed `W04` (`v0.2.0-alpha.1` / M2 Foundation):
  - Implemented `oryq-engine` `cache/` (pin-aware `PageCache`, `VersionIndex`) and `btree/` (`BTree`, stack-based forward/reverse `BTreeCursor`, splits, borrow/merge rebalancing, root collapse).
- Completed `W05` (`v0.2.0` / M2 Complete):
  - Implemented `oryq-engine` `ports.rs` (`RecordCompactor`, `CommitReceipt`, `CheckpointOutcome`), `coordinator/` (`SingleProcessCoordinator`, `SnapshotGuard`, `WriterPermit`), `recovery/` (`scan_wal`, restartable `replay_into_version_index`), `checkpoint/` (passive checkpoint, durable dual-slot unseal, `reset_intent_seq` WAL reset & interrupted-reset recovery), and `transaction/` (`StorageEngine`, `ReadTxn`, `WriteTxn` with nested `SavepointId` rollback, hard WAL cap, and `CommitOutcomeUnknown` poisoning).
- Remaining 5 domain/adapter crates (`oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) are minimal compiling scaffolds (scheduled for `W07–W15`).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 43 tests passed (`oryq-types`: 7; `oryq-document`: 11; `oryq-engine`: 19 across `persistence`, `locking`, `btree_cache`, `admission`, `checkpoint`, `recovery`; `xtask`: 6).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, `xtask modules document`, and `xtask modules engine`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 43 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml` (`actions/checkout@v6`).

KNOWN ISSUES:
- Document updates (`W06`), primary/secondary indexes (`W07`, `W09`), and core catalog/CRUD (`W08`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W06` (`v0.3.0-alpha.1`): `oryq-document` update operators (`$set`, `$unset`, `$inc`, `$rename`, etc.), `_id` immutability, path-conflict validation, and pure compaction helper.
2. Implement `W07` (`v0.3.0-alpha.2`): `oryq-index` primary `_id -> RecordId` mapping and key codec.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
