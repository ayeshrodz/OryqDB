# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 1 — Persistent storage foundation (COMPLETE — `v0.1.0` / `W01–W03` verified)
STATUS: In Progress — M1 complete (`oryq-types`, `oryq-document`, `oryq-engine` io/format/pager); entering M2 (`W04` B+ tree & bounded cache, `W05` WAL/MVCC/recovery)

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`):
  - Eight-crate Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, `rustfmt.toml`, `.clippy.toml`, and `.github/workflows/ci.yml`.
  - Full implementation of `oryq-types` (IDs, boxed `ErrorEnvelope`, RAII `BudgetHandle`/`Reservation`, `CancellationToken`, `CapabilityId`/`FeatureBitmask`/`VersionRange`).
  - `architecture/{modules,capabilities,contract-changes}.toml` and `xtask architecture` / `xtask modules <id>` enforcement tool.
- Completed `W02` (`v0.1.0-alpha.2`):
  - Full implementation of `oryq-document` (`#![forbid(unsafe_code)]`, engine-independent): `OryqDoc` v1 binary codec & strict validator, zero-copy `DocumentView`/`ArrayView`, `ObjectId`/`Uuid`/`Decimal128` (IEEE 754-2008 BID), `FieldPath`, exact cross-type comparator (`compare_values`), and Extended-JSON conversion.
- Completed `W03` (`v0.1.0` / M1 Complete):
  - Implemented `oryq-engine` (`#![forbid(unsafe_code)]`): VFS/PAL (`StorageIo`, `FileHandle`, `OsVfs` Unix/Windows adapters, `FileRegistry` alias protection, `fs4` exclusive OS locking), persistent binary format codecs (Page 0 dual shadow `StorageHeader`, 32B `PageHeader`, 16B `SlotEntry`, `SlottedPageView`/`SlottedPageMut`, `OverflowHeader`/`OverflowRef`, `FreelistTrunkHeader`, `.oryq-wal` header/frame/commit codecs), and `Pager` / `PageAllocator` / `FreelistManager` / `OverflowStore`.
- Remaining 5 domain/adapter crates (`oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) and `oryq-engine` `btree`/`cache`/`transaction`/`recovery`/`checkpoint` are scaffolds or scheduled next.

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 31 tests passed (`oryq-types`: 7; `oryq-document`: 11; `oryq-engine`: 7 across `persistence` & `locking`; `xtask`: 6).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, `xtask modules document`, and `xtask modules engine`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 31 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml`.

KNOWN ISSUES:
- Byte B+ tree & bounded page cache (`W04`), WAL/MVCC/recovery/checkpoint (`W05`), and persistent catalog/CRUD (`W06–W08`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W04` (`v0.2.0-alpha.1`): `oryq-engine` logical-page byte B+ tree (`btree/`) and bounded page cache (`cache/`).
2. Implement `W05` (`v0.2.0` / M2 Complete): `oryq-engine` `.oryq-wal`, single-process MVCC snapshots/savepoints, checkpointing, and 7-point crash recovery.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
