# OryqDB Module Development Plan

* **Author:** Ayesh Rodrigo (@ayeshrodz)
* **Date:** 2026-11-19
* **State:** Planned; no production workspace or module implementation exists
* **Source ownership:** [Modular technical specification](designs/modular-architecture.md)
* **Product acceptance:** [Milestones and extension gates](ROADMAP.md)

## How to Use the Plan

Develop one work package at a time. A package may implement part of a domain crate; it must still deliver a testable boundary and a useful integration slice. “Module by module” does not mean finishing every future engine capability before testing document CRUD. Pure document work can prepare independently of I/O, but dependent integration starts only after its prerequisites pass.

Statuses are `Planned`, `In progress`, `Blocked`, and `Verified`. Move to Verified only with the command, platform, fixture/source revision and actual result recorded in the package ledger. Keep unimplemented optional crates out of Cargo members **and dependency paths**: even an inactive optional path dependency needs a real manifest. Add an optional crate, its feature edge and its registration together when E1 starts.

Each package owns the source files listed in the technical-spec tree and its module guide; tests and fixtures stay with that owner. Normal implementation changes update that module's README/spec and ledger entry. Change global architecture/roadmap only when a dependency, public boundary, delivery gate or compatibility rule changes.

## Ordered Work Packages

| ID | Prerequisites | Owned files / module | Deliverable and acceptance | Product gate |
| :--- | :--- | :--- | :--- | :--- |
| W00 | none | Existing M0 research docs and pinned reference inspection | Finish the six missing LiteDB source reports, record commit and evidence; confirm baseline format/API constraints. No invented inspection claims. | M0 |
| W01 | W00 | Root Cargo/lint/CI files; `architecture/modules.toml`, `architecture/capabilities.toml`, `architecture/contract-changes.toml`; types crate; xtask architecture/module command scaffolds | Establish eight baseline crates with narrow exports and dependency allowlist; implement Section 11 metadata/change-note schema and checker self-tests; types budget/ID/error tests pass. Empty domain scaffolds are not marked implemented. | M1 setup |
| W02 | W01 | document codec/view/array/path/json/compare and local fixture/roundtrip/ordering/malformed tests | In-memory encode/decode, exact comparator, bounded validation, canonical JSON and scalar getters work without engine dependencies; malformed/deep inputs fail safely. | M3 preparation |
| W03 | W01 | engine io/format/pager; persistence/locking tests and fault I/O support | Real headers/pages/overflow/freelist survive reopen; golden bytes and checked parsing pass; file identity and flush behavior have explicit platform evidence. | M1 |
| W04 | W03 | engine btree/cache and related engine tests | Byte-key/value trees split/merge/scan; pinned cache and version metadata obey quotas; test without document/query imports. | M2 foundation |
| W05 | W04 | engine transaction/coordinator/recovery/checkpoint; engine ports | One-writer snapshots/savepoints, WAL redo, publication, sealed/reset state and uncertain sync outcomes pass adversarial schedules; no domain callbacks in recovery. Define compactor port but do not require deltas before W12. | M2 |
| W06 | W02 | document updates/patch and update tests | `_id` immutability, conflicting paths, numeric errors and before/after document normalization pass; expose pure compaction for later delta storage. Core initially stores full replacements. | M3 |
| W07 | W02, W05 | index descriptor/key_codec/registry/provider/primary; codec and conformance tests | Primary `_id` to RecordId mapping works through engine transaction ports; supported identifier encoding is exact and stable; duplicates rollback; provider create on an empty collection yields valid roots and reopens/drops without a synthetic document mutation. External-consumer provider/context signatures and generic search-binding conformance from Section 9 compile; basic index contracts exist before core CRUD, even though complete secondary indexing is M4. | M3 prerequisite |
| W08 | W05, W06, W07 | core database/config/API/catalog/write/composition/adapters; vertical-slice and atomicity tests | Open, create collection, insert/update/delete, commit/rollback, close/reopen; core catalog fixture schema frozen. Documents, primary index, RecordId counter and requirements commit atomically. This is the first document database slice. | M3 |
| W09 | W08 | index secondary/extract/read/write, full key codec and uniqueness/completeness tests | Secondary indexes create/drop/maintain within the same savepoint and transaction; exact comparator and scan oracle cover all indexed types. Core enumerates registry entries without new per-index branches. Test A-root-change/B-failure followed by successful commit in the same transaction. | M4 |
| W10 | W09 | query ports/context/AST/parse/predicates/planner/cursor/explain and baseline stages; core query-source adapter | Find/count/project/sort/limit/skip and explain work with one snapshot, safe bounds and shared quotas; generic candidate-superset residual matching and unranked no-index fallback, registered factory dispatch and object-safe lending StageFactory/QuerySource external-consumer tests pass. Unsupported later pipeline operations fail explicitly. | M5 |
| W11 | W08, W10 | FFI handles/API/guard/status/diagnostics/build/header; C lifecycle tests | Stable C header and native C/C++ consumer exercise BUSY completion, parent close ordering, failures and TLS diagnostics; no engine/document/index direct dependencies. Aggregation symbols return UnsupportedFeature until W13b. | M6 |
| W12 | W08, W10, W11 | document patch owner, engine slotted-page/WAL delta owner, core compactor adapter; benchmark harness/workloads | Integrate pure compaction through engine port; nested/ancestor delta oracle and crash recovery pass. Record benchmark methodology and measurements honestly. This is an intentionally cross-owner storage feature with three explicit ports, not a hidden import. | M7 |
| W13a | W09, W12 | index compound/multikey and conformance/completeness tests | Prefix restarts, single-array guards, exact RecordId dedup and sparse/negative predicate completeness pass. Query planner consumes declared capability facts; update bounds only when shared semantics genuinely change. | M8 indexes |
| W13b | W10, W12, W13a | query stage folders/group/sort/spill; core/API aggregation dispatch | Aggregation/lookup use one snapshot; rational average, oversized state, spill cap, cancellation and cleanup pass. FFI/CLI forward the existing pipeline payload without stage-specific switches. | M8 pipelines |
| W14 | W08, W13a, W13b | core admin integrity/backup/inspect and engine inspect.rs; native platform fixtures/tests | Core integrity API/harness and bounded read-only page/WAL/lock inspection port are available for M8 portability; CLI-only consumers compile through core. Snapshot-consistent sealed backup is complete before M9 CLI exposure. Backup safely copies opaque optional pages without search code. | M8 integrity; M9 backup |
| W15 | W11, W14 | CLI app/output/command registry and commands/tests | Offline CLI CRUD/info/backup/integrity work through core; live exclusive-owner conflict produces the documented error. No engine/query business logic in commands. | M9 |
| W16 | all W00–W15 | Release/CI, fixture inventories, architecture checks, all owner test selections | Default and no-default-features baseline, C ABI, native portability, fault schedules, fuzzing and audits pass; freeze baseline formats and report real coverage/limitations. | M10 |

W02 is independent preparation and may run before W03; the default sequential implementation order above keeps integration evidence explicit. W14 contains two named acceptance checkpoints so the M8 integrity gate never depends on the M9 CLI. Adding a primary mapping at W07 refines the old M3/M4 source schedule without changing their product behavior.

## Post-Baseline Work Packages

| ID | Prerequisites | Owned area | Required acceptance |
| :--- | :--- | :--- | :--- |
| X01 | W16; index provider/context conformance | FTS or vector provider crate, one core provider registration and Cargo dependency/feature edge; capability manifest entry | Each provider independently passes pending/merged snapshot visibility, update/delete/drop, quota and crash conformance; alone/both/no-provider builds pass. Never introduce core-to-provider-to-core cycles. Corresponds to E1. |
| X02 | W16; reviewed cross-process protocol | engine `coordinator/shared_process/`, owner registry and option validator | Admission, initialization, slot minima/reuse, reset exclusion, peer recovery and owner death pass native multiprocess schedules. Before then the advertised request returns UnsupportedFeature. Corresponds to E2. |
| X03 | W16; queued-command contract | engine `commit_batch/`, owner registration and core queued-command API adapter | Dependent command order, validation rollback, one-sync batching and unknown batch outcomes pass. Explicit write transactions retain the established baseline. Corresponds to E3. |

These packages do not make optional implementations part of the M10 baseline. No duration estimates are claimed before module sizes and platform evidence exist.

Provider search/version/arena conformance and facade contracts are normative in Sections 9–10 of the technical spec. The checker schemas, commands, comparison input and module selections are normative in Section 11. Include selected-index semantic binding, two differently configured indexes, candidate-superset residual evaluation and explicit index-free options; include newer-data/older-provider and newer-options/older-provider rejection before mutation, plus rollback/rebuild fixtures in X01.

## Ownership-Based Change Procedure

1. Identify the capability's owner in [the module catalog](modules/README.md). Write the behavior change and affected public/persistent contract before editing.
2. For a contract-preserving change, edit the feature folder, owner tests and owner specification only. Keep unrelated consumers compiling unchanged.
3. For an existing extension port, add its factory/provider registration in the owner registry; add a core composition entry only for a new separately linked provider. Update manifests when adding a crate or persistent capability.
4. For a changed boundary, enumerate direct consumers from the dependency allowlist, change only those adapters and tests, and record compatibility/migration in the decision log. Never solve a boundary mismatch with a reverse dependency.
5. Run owner tests, the architecture check and selected downstream integration tests. Persisted feature removal additionally runs capable-build drop and reduced-build reopen/write fixtures.
6. Record actual results in this ledger and the project status. Do not mark downstream packages Verified because a dependency now compiles.

## Verification Commands to Implement

The following are planned commands for the future Rust workspace, not executed results from this documentation task:

```text
cargo run -p xtask -- architecture
cargo run -p xtask -- modules document
cargo test -p oryq-document
cargo test -p oryq-engine
cargo test -p oryq-index
cargo test -p oryq-query
cargo test -p oryq-core --test atomic_maintenance
cargo test -p oryq-core --test feature_lifecycle
cargo test -p oryq-ffi
cargo test -p oryq-cli
cargo test --workspace
cargo test --workspace --no-default-features
```

When E1 crates exist and are included in the extension workspace selection, run `cargo test -p oryq-core --features fts`, `--features vec`, and `--features fts,vec`, plus reduced-build fixtures. Tests must link C against the actual release library when that library exists; syntax-checking a proposed header is not ABI execution.

## Progress Ledger

### Pre-1.0 Versioning Policy (`0.x.y`)

OryqDB uses Semantic Versioning (`0.x.y`) synchronized across `VERSION`, `CHANGELOG.md`, and the future workspace `Cargo.toml`:
- `0.0.1`: M0 converged design & modular architecture baseline.
- `0.0.2`: `W00` (pinned LiteDB reference audit complete).
- `0.1.0`: `W01–W03` (M1 workspace, `oryq-types`, `xtask`, `oryq-engine` pager/VFS).
- `0.2.0`: `W04–W05` (M2 B+ tree, cache, WAL, MVCC, recovery).
- `0.3.0`: `W02, W06–W08` (M3 `oryq-document`, primary mapping, `oryq-core` first CRUD slice).
- `0.4.0`: `W09–W10` (M4–M5 secondary indexes & streaming query engine).
- `0.5.0`: `W11` (M6 panic-safe C ABI & native C/C++ tests).
- `0.6.0`: `W12` (M7 state-normalized slotted deltas & benchmarks).
- `0.7.0`: `W13a–W13b` (M8 compound/multi-key indexes & aggregation pipelines).
- `0.8.0`: `W14–W15` (M8/M9 core integrity/inspect, online backup & `oryq` CLI).
- `0.9.0-rc.1`: `W16` (M10 baseline format freeze & release candidate).
- `0.10.x`: `X01–X03` (post-baseline E1–E3 extensions); `1.0.0` follows full M10 native release verification.
- Patch bumps (`0.x.1`, `0.x.2`, ...) track fixes or documentation refinements within an active minor phase.

| Packages | State | Evidence | Next action |
| :--- | :--- | :--- | :--- |
| W00 | Verified | LiteDB `dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2` inspected; all 6 `docs/research/litedb-*.md` reports authored; `python3 tools/validate_design_docs.py` passed (Linux x86_64) | Unblocked `W01` (`v0.1.0`) |
| W01 | Verified | Eight-crate workspace, `architecture/*.toml`, `oryq-types` (7 integration tests), `xtask` (6 self-tests), `xtask modules types` (7 downstream checks), `cargo clippy -- -D warnings`, and `.github/workflows/ci.yml` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W02` (`oryq-document` codec/views/comparator) or `W03` (`oryq-engine` io/format/pager) |
| W02 | Verified | `oryq-document` (`OryqDoc` v1 codec, zero-copy `DocumentView`/`ArrayView`, IEEE 754 BID `Decimal128`, exact cross-type comparator, `FieldPath`, Extended-JSON) implemented; 11 integration tests (`roundtrip`, `ordering`, `malformed`), `xtask modules document` (5 downstream checks), `xtask architecture --base origin/main` (`CC-0002`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Implement `W03` (`oryq-engine` io/format/pager + golden fixtures) |
| W03 | Verified | `oryq-engine` (`io/` VFS/PAL/FileRegistry, `format/` Page 0 dual shadow headers/slotted pages/overflow/freelist/WAL codecs, `pager/` Pager/PageAllocator/FreelistManager/OverflowStore) implemented; 7 integration tests (`persistence`, `locking` + `FaultVfs`), `xtask modules engine` (5 downstream checks), `xtask architecture --base origin/main` (`CC-0003`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W04` (`v0.2.0-alpha.1` — `oryq-engine` byte B+ tree & bounded page cache) |
| W04 | Verified | `oryq-engine` (`cache/` pin-aware `PageCache` + `VersionIndex`, `btree/` byte-key/value `BTree` + stack-based `BTreeCursor` with splits, merges & root collapse) implemented; 4 new integration tests in `btree_cache.rs` (11 total in `oryq-engine`), `xtask modules engine` (5 downstream checks), `xtask architecture --base origin/main` (`CC-0004`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W05` (`v0.2.0` / M2 Complete — WAL, MVCC snapshots/savepoints, recovery & checkpoint) |
| W05 | Verified | `oryq-engine` (`ports.rs`, `coordinator/` single-process MVCC, `recovery/` bounded WAL scan & restartable replay, `checkpoint/` passive/reset/seal, `transaction/` `StorageEngine`/`ReadTxn`/`WriteTxn`/`SavepointId`) implemented; 8 new integration tests (`admission`, `checkpoint`, `recovery` — 19 total in `oryq-engine`), `xtask modules engine` (5 downstream checks), `xtask architecture --base origin/main` (`CC-0005`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start Milestone 3 with `W06` (`v0.3.0-alpha.1` — `oryq-document` update operators & patch normalization) |
| W06 | Verified | `oryq-document` (`updates/` `UpdateRegistry`, `$set`/`$unset`/`$inc`/`$push`/`$pull`, `_id` immutability, path-conflict & numeric overflow/precision validation; `patch/` binary state-normalized patches, 64-bit bloom resolution, pure `compact_document`) implemented; 4 new integration tests in `updates.rs` (15 total in `oryq-document`), `xtask modules document` (5 downstream checks), `xtask architecture --base origin/main` (`CC-0006`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W07` (`v0.3.0-alpha.2` — `oryq-index` primary `_id -> RecordId` mapping & key codec) |
| W07 | Verified | `oryq-index` (`key_codec.rs` exact memcomparable codec across all 12 types, `descriptor.rs`, `provider.rs`, `registry.rs`, `plugin/context.rs`, `primary/` `_id -> RecordId` provider) implemented; 3 integration tests (`codec` 2,209-pair oracle + `conformance` create/reopen/drop/CRUD on `StorageEngine`), `xtask modules index` (4 downstream checks), `xtask architecture --base origin/main` (`CC-0007`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W08` (`v0.3.0` / M3 Complete — `oryq-core` database, catalog & first CRUD vertical slice) |
| W08 | Verified | `oryq-core` (`database.rs`, `config.rs`, `api/` Read/Write transactions & collections, `catalog/` frozen v1 `OryqDoc` catalog schema, `write/` statement savepoint CRUD/DDL orchestration, `composition/` `CoreRegistries`, `adapters/` `DocumentRecordCompactor`) implemented; 3 end-to-end integration tests (`vertical_slice`, `atomic_maintenance` — 53 total across workspace), `xtask modules core` (2 downstream checks), `xtask architecture --base origin/main` (`CC-0008`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start Milestone 4–5 with `W09` (`v0.4.0-alpha.1` — `oryq-index` secondary indexes & differential maintenance) |
| W09 | Verified | `oryq-index` (`secondary/` `SecondaryIndexProvider`, nested extraction, `unique`/`sparse`, differential `on_update`, `Asc`/`Desc` equality & range scans) and `oryq-core` (`create_index`/`drop_index`/`list_indexes` with backfill & savepoint rollback) implemented; 2 new integration tests (`uniqueness.rs` + multi-index A-succeeds/B-fails rollback in `atomic_maintenance.rs` — 55 total across workspace), `xtask modules index` & `core`, `xtask architecture --base origin/main` (`CC-0009`), and `cargo clippy -- -D warnings` passed on Linux x86_64 (`rustc 1.99.1`) | Start `W10` (`v0.4.0` / M4–M5 Complete — `oryq-query` filter AST, access planner, cursors & `explain_analyze`) |
| W10 | Verified | `oryq-query` (`ports.rs`, `context.rs`, `ast.rs`, `parse.rs`, `predicates/`, `planner/`, `explain.rs`, `cursor.rs`) + `oryq-core` `CoreQuerySource` & `find`/`count_matching`/`explain_analyze` implemented; `filters.rs`, `planner.rs`, `vertical_slice.rs`, `xtask modules query` & `core`, `CC-0010` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W11` (`v0.5.0` — `oryq-ffi`) |
| W11 | Verified | `oryq-ffi` (`status.rs`, `diagnostics.rs`, `guard.rs`, `handles.rs`, `api.rs`) + `include/oryq.h` implemented; `c_api.rs`, native `cc -std=c99 -Wall -Wextra -Werror` smoke test, `xtask modules ffi`, `CC-0011` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W12` (`v0.6.0` — delta compaction & benchmarks) |
| W12 | Verified | `RECORD_KIND_DELTA_CHAIN` (`MAX_DELTA_CHAIN = 4`) + automatic `RecordCompactor` compaction on 5th patch in `oryq-core`; `delta_compaction.rs` + release `bench_suite` (`docs/BENCHMARK_RESULTS.md`) verified (Linux x86_64, `rustc 1.99.1`) | Proceed to `W13a` (`v0.7.0-alpha.1` — compound & multi-key indexes) |
| W13a | Verified | `oryq-index` `CompoundIndexProvider` (`1..=16` fields, `R=16` prefix codec, multi-key dedup, parallel-array rejection) + `oryq-core` `create_compound_index`; `compound_multikey.rs`, `xtask modules index`, `CC-0013` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W13b` (`v0.7.0` — spillable aggregation pipelines) |
| W13b | Verified | `oryq-query` `pipeline.rs` (`$match`/`$project`/`$sort`/`$skip`/`$limit`/`$unwind`/`$lookup`/`$group`) + `spill.rs` RAII `SpillManager` + `oryq-core` `aggregate_json`; `pipeline.rs` tests, `xtask modules query`, `CC-0014` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W14` (`v0.8.0-alpha.1` — integrity, inspect & backup) |
| W14 | Verified | `oryq-engine` read-only `inspect_{page,wal,locks}` + `oryq-core` `Database::{integrity_check, backup_to, inspect_*}`; `backup_integrity.rs` & `CC-0015` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W15` (`v0.8.0` — `oryq` CLI) |
| W15 | Verified | `oryq-cli` (`oryq` binary: `info`, `collections`, CRUD, `find`, `aggregate`, `checkpoint`, `backup`, `integrity-check`, `inspect`); `commands.rs` end-to-end binary test, `xtask modules cli`, `CC-0016` passed (Linux x86_64, `rustc 1.99.1`) | Proceed to `W16` (`v0.9.0-rc.1` — M10 format freeze & RC1) |
| W16 | Verified | Full M10 `v0.9.0-rc.1` verification passed: `cargo fmt --check`, `cargo clippy -- -D warnings`, 69 tests (default + `--no-default-features`), all 8 `xtask modules`, `xtask architecture --base origin/main`, `validate_design_docs.py`, and release builds (`oryq` 1.6 MB, `liboryq.so` 1.5 MB) | Raise consolidated PR #12 into `main` for automated `v0.9.0-rc.1` release packaging |
| X01–X03 | Planned; post-baseline | Extension protocols/implementation not verified | Keep disabled until their prerequisite gates pass |

When work begins, split the range rows into individual package rows. Each completed row must record commands, platform, fixture revision, failures/limitations and the next unblocked package. Keep this ledger as the work-package tracker; ROADMAP remains the product gate tracker.
