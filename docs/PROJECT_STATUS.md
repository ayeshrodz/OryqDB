# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 3 — Document updates, primary index & first CRUD slice (`W06` complete / `v0.3.0-alpha.1`)
STATUS: In Progress — `W01–W06` verified (`oryq-types`, full `oryq-document`, full `oryq-engine`); `W07` (`oryq-index` primary `_id` mapping) and `W08` (`oryq-core` catalog/CRUD) next to complete M3

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`):
  - Eight-crate Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, `rustfmt.toml`, `.clippy.toml`, and `.github/workflows/ci.yml`.
  - Full implementation of `oryq-types` (IDs, boxed `ErrorEnvelope`, RAII `BudgetHandle`/`Reservation`, `CancellationToken`, `CapabilityId`/`FeatureBitmask`/`VersionRange`).
  - `architecture/{modules,capabilities,contract-changes}.toml` and `xtask architecture` / `xtask modules <id>` enforcement tool.
- Completed `W02` (`v0.1.0-alpha.2`) & `W06` (`v0.3.0-alpha.1`):
  - Full implementation of `oryq-document` (`#![forbid(unsafe_code)]`, engine-independent): `OryqDoc` v1 binary codec & strict validator, zero-copy `DocumentView`/`ArrayView`, `ObjectId`/`Uuid`/`Decimal128` (IEEE 754-2008 BID), `FieldPath`, exact cross-type comparator (`compare_values`), Extended-JSON conversion, owner-local `UpdateRegistry` (`$set`, `$unset`, `$inc`, `$push`, `$pull`, `_id` immutability, conflict & overflow/precision checks), state-normalized binary physical patches, 64-bit bloom resolution, and pure `compact_document`.
- Completed `W03–W05` (`v0.1.0` / `v0.2.0` — M1 & M2 Complete):
  - Full implementation of `oryq-engine` (`#![forbid(unsafe_code)]`): VFS/PAL (`OsVfs`, `FileRegistry`, `fs4`), persistent binary codecs, `Pager`/`PageAllocator`/`FreelistManager`/`OverflowStore`, `PageCache`/`VersionIndex`, byte `BTree`/`BTreeCursor`, `SingleProcessCoordinator`, `scan_wal`/restartable replay, passive/reset/seal checkpointing, and `StorageEngine`/`ReadTxn`/`WriteTxn` with nested `SavepointId`.
- Remaining 5 domain/adapter crates (`oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) are minimal compiling scaffolds (scheduled for `W07–W15`).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 47 tests passed (`oryq-types`: 7; `oryq-document`: 15 across `roundtrip`, `ordering`, `malformed`, `updates`; `oryq-engine`: 19 across `persistence`, `locking`, `btree_cache`, `admission`, `checkpoint`, `recovery`; `xtask`: 6).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, `xtask modules document`, and `xtask modules engine`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 47 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml` (`actions/checkout@v6`).

KNOWN ISSUES:
- Primary/secondary indexes (`W07`, `W09`) and core catalog/CRUD (`W08`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W07` (`v0.3.0-alpha.2`): `oryq-index` primary `_id -> RecordId` mapping, exact key codec, descriptor, registry, and provider contracts.
2. Implement `W08` (`v0.3.0` / M3 Complete): `oryq-core` database/catalog/CRUD vertical slice.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
