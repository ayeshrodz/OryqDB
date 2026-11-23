# OryqDB

An open-source (`MIT`), embedded, serverless, single-file NoSQL document database engine written in Rust with a native C ABI (`include/oryq.h`) and CLI (`oryq`).

> **Current Version:** `0.12.4` (Pre-1.0 Feature Complete — `1.2 MiB` `liboryq` / `954 KiB` CLI, Single-File Auto-Seal & `VACUUM`, `:memory:`, Hybrid FTS & Vector Search, Official Python/Go/Node.js/C#/C99/C++17 Bindings, `.oryq-shm`, Group Commit; see [PRE_1_0_READINESS_CHECKLIST.md](docs/PRE_1_0_READINESS_CHECKLIST.md).)

## Design Overview

OryqDB combines single-file embedded portability with MongoDB-style document capabilities:
* **`OryqDoc` Binary Format:** Little-endian binary document encoding with a sorted field-offset directory for $O(\log k)$ zero-copy field extraction and state-normalized slotted-page delta patches guarded by a 64-bit bloom bitmask.
* **Logical-Page Slotted B+ Tree + `.oryq-wal` + MVCC:** Single-file `.oryq` persistence with checksummed Write-Ahead Logging, snapshot isolation, statement savepoints, and deterministic crash recovery.
* **Native Document Indexing & Pipelines:** Exact variable-width memcomparable key encoding, primary/secondary/prefix-compressed compound ($R=16$ restart points) and multi-key array indexes, streaming lending cursors, and memory-bounded spillable aggregation pipelines.
* **Compile-Enforced Modular Architecture:** Eight baseline crates (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) with owner-local feature registries and optional post-baseline E1 search provider crates (`oryq-fts`, `oryq-vec`).

## Documentation Map

* **Production Developer Guides & Bindings (`v0.11.0`):**
  * [docs/guides/quickstart.md](docs/guides/quickstart.md) · [docs/guides/query-and-aggregation.md](docs/guides/query-and-aggregation.md) · [docs/guides/c-abi-guide.md](docs/guides/c-abi-guide.md) · [docs/guides/operations-and-recovery.md](docs/guides/operations-and-recovery.md)
  * [bindings/README.md](bindings/README.md) (Python `ctypes`, C99, C++17) · [docs/PRE_1_0_READINESS_CHECKLIST.md](docs/PRE_1_0_READINESS_CHECKLIST.md)
* **Master Design & Modularity:**
  * [docs/designs/modular-architecture.md](docs/designs/modular-architecture.md) — Crate dependency allowlist, file ownership tree, boundary traits, and `xtask` architecture checker contracts.
  * [docs/designs/oryqdb.md](docs/designs/oryqdb.md) — Master engine architecture specification.
  * [docs/MODULE_DEVELOPMENT_PLAN.md](docs/MODULE_DEVELOPMENT_PLAN.md) — Ordered work packages (`W00–W16`, `X01–X03`), `0.x.x` version phasing, and progress ledger.
  * [docs/modules/README.md](docs/modules/README.md) — Per-module ownership guides.
* **Subsystem Specifications:**
  * [docs/architecture.md](docs/architecture.md) · [docs/file-format.md](docs/file-format.md) · [docs/document-model.md](docs/document-model.md) · [docs/transactions.md](docs/transactions.md) · [docs/indexing.md](docs/indexing.md) · [docs/query-language.md](docs/query-language.md) · [docs/ffi.md](docs/ffi.md) · [docs/compatibility.md](docs/compatibility.md)
* **Governance, Decisions & Reviews:**
  * [docs/ROADMAP.md](docs/ROADMAP.md) · [docs/ARCHITECTURE_DECISIONS.md](docs/ARCHITECTURE_DECISIONS.md) · [docs/PROJECT_STATUS.md](docs/PROJECT_STATUS.md) · [docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md) · [docs/BENCHMARK_RESULTS.md](docs/BENCHMARK_RESULTS.md)
  * [docs/reviews/initial-design/goldfish_review_report.md](docs/reviews/initial-design/goldfish_review_report.md) · [docs/reviews/modular-architecture/goldfish_review_report.md](docs/reviews/modular-architecture/goldfish_review_report.md)

## Versioning & Pre-1.0 Phasing (`0.x.y`)

OryqDB stays on `0.x.y` versions throughout pre-release development. All workspace crates share the workspace version in `VERSION` / root `Cargo.toml`, and every work-package completion updates [CHANGELOG.md](CHANGELOG.md) and the ledger in [docs/MODULE_DEVELOPMENT_PLAN.md](docs/MODULE_DEVELOPMENT_PLAN.md):

| Version | Work Packages | Product Gate | Scope |
| :--- | :--- | :--- | :--- |
| `0.0.1` | Design Baseline | M0 (Design) | Converged EGM specs, modular architecture, module guides, ADRs 001–008, design validator |
| `0.0.2` | `W00` | M0 (Complete) | Pinned LiteDB reference audit (`docs/research/litedb-*.md`) |
| `0.1.0` | `W01–W03` | M1 | Eight-crate workspace, `oryq-types`, `xtask architecture`, `oryq-engine` persistent slotted pager & VFS |
| `0.2.0` | `W04–W05` | M2 | Logical-page B+ tree, bounded cache, `.oryq-wal`, single-process MVCC, checkpointing & 7-point recovery |
| `0.3.0` | `W02, W06–W08` | M3 | `oryq-document` (`OryqDoc` + updates), `oryq-index` primary mapping, `oryq-core` catalog & first CRUD slice |
| `0.4.0` | `W09–W10` | M4–M5 | Exact secondary indexes, filter AST, access planner, lending cursors & `explain_analyze()` |
| `0.5.0` | `W11` | M6 | Panic-safe C ABI (`oryq-ffi`, `include/oryq.h`) & native C/C++ lifecycle tests |
| `0.6.0` | `W12` | M7 | State-normalized slotted-page deltas (`RecordCompactor`) & comparative benchmark suite |
| `0.7.0` | `W13a–W13b` | M8 (Core) | Prefix-compressed compound ($R=16$), multi-key array indexes & spillable aggregation pipelines |
| `0.8.0` | `W14–W15` | M8–M9 | Core integrity/inspect API, snapshot-consistent online backup & `oryq` CLI |
| `0.9.0-rc.1` | `W16` | M10 | Baseline format freeze, native multi-OS tests, fuzzing, Miri/ASan/TSan & release candidate |
| `0.10.x` | `X01–X03` | E1–E3 | Optional `oryq-fts` / `oryq-vec` search crates, `.oryq-shm` multi-process mode & E3 batching |
| `1.0.0` | Release | Stable | First public stable release after all M10 native verification gates pass |

Patch versions (`0.x.1`, `0.x.2`, ...) are used for bug fixes, test additions, and specification refinements within an active minor phase.

## Validation

Run the design-level link, numeric ordering, prefix-bound, and C/C++ header syntax checks:

```bash
python3 tools/validate_design_docs.py
```

## License

Licensed under the [MIT License](LICENSE).
