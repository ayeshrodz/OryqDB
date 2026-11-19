# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 3 — Document updates, primary index & first CRUD slice (COMPLETE — `v0.3.0` / `W01–W08` verified)
STATUS: In Progress — M1, M2 & M3 complete (first end-to-end embedded NoSQL CRUD database verified); entering M4–M5 (`W09` secondary indexes, `W10` streaming query engine)

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`): eight-crate Cargo workspace, `oryq-types`, `architecture/*.toml`, `xtask architecture`/`modules`, and multi-OS GitHub Actions CI.
- Completed `W02` & `W06` (`oryq-document`): `OryqDoc` v1 codec/views/comparator/JSON, `UpdateRegistry` (`$set`/`$unset`/`$inc`/`$push`/`$pull`), state-normalized physical patches, bloom resolution, and pure `compact_document`.
- Completed `W03–W05` (`v0.1.0` / `v0.2.0` — M1 & M2 Complete, `oryq-engine`): VFS/PAL (`OsVfs`, `FileRegistry`, `fs4`), persistent binary codecs, `Pager`/`PageAllocator`/`FreelistManager`/`OverflowStore`, `PageCache`/`VersionIndex`, byte `BTree`/`BTreeCursor`, `SingleProcessCoordinator`, WAL scan/replay, checkpoint/reset/seal, and `StorageEngine`/`ReadTxn`/`WriteTxn` with nested `SavepointId`.
- Completed `W07` (`v0.3.0-alpha.2`, `oryq-index`): exact memcomparable `key_codec`, `IndexDescriptor`, `IndexProvider`, `IndexProviderRegistry`, restricted `IndexReadContext`/`IndexWriteContext`, and `PrimaryIndexProvider`.
- Completed `W08` (`v0.3.0` — M3 Complete, `oryq-core`): `Database`, `DatabaseConfig`, `ReadTransaction`/`ReadCollection`, `WriteTransaction`/`WriteCollection`, frozen v1 `OryqDoc` catalog schema, atomic statement savepoint orchestration with auto-`ObjectId`, `CoreRegistries`, and `DocumentRecordCompactor`.
- Remaining 3 domain/adapter crates (`oryq-query`, `oryq-ffi`, `oryq-cli`) are minimal compiling scaffolds (scheduled for `W10`, `W11`, `W15`).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 53 tests passed (`oryq-types`: 7; `oryq-document`: 15; `oryq-engine`: 19; `oryq-index`: 3; `oryq-core`: 3; `xtask`: 6).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, `document`, `engine`, `index`, and `core`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 53 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml` (`actions/checkout@v6`).

KNOWN ISSUES:
- Secondary indexes (`W09`), query engine (`W10`), C ABI (`W11`), and CLI (`W15`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W09` (`v0.4.0-alpha.1`): `oryq-index` secondary indexes (single/nested field extraction, uniqueness, sparse, and differential transactional maintenance) + core `create_index`/`drop_index`/`list_indexes`.
2. Implement `W10` (`v0.4.0` / M4–M5 Complete): `oryq-query` filter AST, access planner, streaming lending cursors, and `explain_analyze()`.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
