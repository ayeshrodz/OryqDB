# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 4–5 — Secondary indexes & streaming query engine (`W09` M4 complete / `v0.4.0-alpha.1`)
STATUS: In Progress — `W01–W09` verified (M1–M4 complete: persistent engine, document codec/updates, primary & secondary indexes, core CRUD/DDL); `W10` (`oryq-query` filter AST, planner & cursors) next

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`): eight-crate Cargo workspace, `oryq-types`, `architecture/*.toml`, `xtask architecture`/`modules`, and multi-OS GitHub Actions CI.
- Completed `W02` & `W06` (`oryq-document`): `OryqDoc` v1 codec/views/comparator/JSON, `UpdateRegistry` (`$set`/`$unset`/`$inc`/`$push`/`$pull`), state-normalized physical patches, bloom resolution, and pure `compact_document`.
- Completed `W03–W05` (`v0.1.0` / `v0.2.0` — M1 & M2 Complete, `oryq-engine`): VFS/PAL, persistent binary codecs, `Pager`/`PageAllocator`/`FreelistManager`/`OverflowStore`, `PageCache`/`VersionIndex`, byte `BTree`/`BTreeCursor`, `SingleProcessCoordinator`, WAL scan/replay, checkpoint/reset/seal, and `StorageEngine`/`ReadTxn`/`WriteTxn` with nested `SavepointId`.
- Completed `W07` & `W09` (`v0.3.0-alpha.2` / `v0.4.0-alpha.1`, `oryq-index`): exact memcomparable `key_codec`, `IndexDescriptor`, `IndexProvider`, `IndexProviderRegistry`, `IndexReadContext`/`IndexWriteContext`, `PrimaryIndexProvider`, and `SecondaryIndexProvider` (single/nested extraction, `unique`/`sparse`, differential `on_update`, `Asc`/`Desc` equality & range scans).
- Completed `W08` (`v0.3.0` — M3 Complete) + `W09` DDL (`oryq-core`): `Database`, `DatabaseConfig`, `ReadTransaction`/`ReadCollection`, `WriteTransaction`/`WriteCollection` (including `create_index`/`drop_index`/`list_indexes`), frozen v1 catalog, atomic statement savepoints, `CoreRegistries`, and `DocumentRecordCompactor`.
- Remaining 3 domain/adapter crates (`oryq-query`, `oryq-ffi`, `oryq-cli`) are minimal compiling scaffolds (scheduled for `W10`, `W11`, `W15`).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 55 tests passed (`oryq-types`: 7; `oryq-document`: 15; `oryq-engine`: 19; `oryq-index`: 4; `oryq-core`: 4; `xtask`: 6).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, `document`, `engine`, `index`, and `core`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 55 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml` (`actions/checkout@v6`).

KNOWN ISSUES:
- Query engine (`W10`), C ABI (`W11`), delta compaction wiring (`W12`), compound/multikey indexes & aggregation (`W13a–W13b`), and CLI (`W15`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W10` (`v0.4.0` / M4–M5 Complete): `oryq-query` filter AST, predicate registry, access planner, streaming lending cursors, `explain_analyze()`, and `oryq-core` `find`/`count` integration.
2. Implement `W11` (`v0.5.0` / M6 Complete): `oryq-ffi` panic-safe C ABI and `include/oryq.h`.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
