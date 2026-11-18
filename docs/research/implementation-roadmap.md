# Milestone 0 Research Deliverable: Implementation Roadmap

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Last Reviewed:** 2026-11-19

> **Canonical Source Link:** To prevent duplicate-source documentation drift across milestones, product milestones (M0–M10) and extension acceptance gates (E1–E3) are maintained in [docs/ROADMAP.md](../ROADMAP.md), modular crate/file ownership is specified in [docs/designs/modular-architecture.md](../designs/modular-architecture.md), and the ordered module work packages (`W00–W16`, `X01–X03`) are maintained in [docs/MODULE_DEVELOPMENT_PLAN.md](../MODULE_DEVELOPMENT_PLAN.md).

## Milestone & Work-Package Mapping Summary

| Phase | Milestone / Gate | Work Packages | Core Focus | Key Verification Gate |
| :--- | :--- | :--- | :--- | :--- |
| **Phase 1: Core Transactional Engine** | **M0** | `W00` | LiteDB Reference Audit & Subsystem Specifications | All `docs/research/*.md` and `docs/*.md` specifications complete and cross-checked. |
| | **M1** | `W01`, `W03` | Eight-Crate Baseline Setup, `oryq-types`, Architecture Checker & Slotted Pager/VFS (`oryq-engine`) | `xtask architecture` self-tests pass; pages persist across restarts; bit-flip corruption tests return structured `CorruptionError`. |
| | **M2** | `W04`, `W05` | Logical-Page B+ Tree, Bounded Cache, `.oryq-wal`, `SingleProcessCoordinator` MVCC & Recovery (`oryq-engine`) | Subprocess fault-injection harness passes all 7 WAL/checkpoint failure points and reader-admission schedules. |
| | **M3** | `W02`, `W06`, `W07`, `W08` | `OryqDoc` Codec/Updates (`oryq-document`), Primary Mapping (`oryq-index`), Catalog & First CRUD Slice (`oryq-core`) | Miri passes unaligned scalar reads; documents, primary index, `RecordId` counter, and catalog requirements commit atomically. |
| | **M4** | `W09` | Exact Memcomparable Key Codec & Single/Nested-Field Secondary Indexes (`oryq-index`) | `proptest` index equivalence matches collection scan oracle; multi-index savepoint rollback verified. |
| | **M5** | `W10` | Filter AST, Rule-Based Query Planner, Step-Unpinning `DocCursor` & Baseline Stages (`oryq-query`) | Queries over datasets exceeding `max_cache_bytes` stream within bounded RAM; generic provider search binding verified. |
| | **M6** | `W11` | Panic-Safe C ABI (`oryq-ffi`, `include/oryq.h`) & Native C/C++ Lifecycle Tests (`crates/oryq-ffi/tests/c/{api,lifecycle}.c`) | Compiled C/C++ tests pass CRUD, `BUSY` child-handle rules, reopen persistence, and panic-guard checks under ASan/Valgrind. |
| **Phase 2: Deltas, Advanced Indexing, Pipelines & CLI** | **M7** | `W12` | State-Normalized Slotted-Page Deltas (64-Bit Bloom Mask, `MAX_DELTA_CHAIN = 4`, `RecordCompactor` Adapter) & Benchmark Suite | Reproducible benchmarks against SQLite, SQLite + JSONB, LiteDB, and `redb` recorded in `docs/BENCHMARK_RESULTS.md`. |
| | **M8** | `W13a`, `W13b`, `W14` (Integrity) | Prefix-Compressed Compound Indexes ($R=16$), Multi-Key Array Indexes, Spillable Aggregation Pipeline, Core Integrity/Inspect API & Multi-OS Portability | Golden `.oryq` fixtures pass cross-platform checks and `Database::integrity_check()` on Linux, Windows, and macOS (x86-64/ARM64). |
| | **M9** | `W14` (Backup), `W15` | `oryq` CLI (`oryq-cli`), Forensic Triage Subcommands & Snapshot-Consistent Sealed Backup (`Database::backup_to`) | `oryq integrity-check`, `page-inspect`, `wal-inspect`, `inspect-locks`, and live in-process / offline CLI `backup` verified. |
| **Phase 3: Baseline Release** | **M10** | `W16` | Baseline Format Freeze, Full Owner Test Selections, Fuzzing, Safety Audits & v1.0 RC | All `W00–W15` gates, `cargo-fuzz`, native ABI checks, and license audits pass; optional extensions remain gated. |
| **Post-Baseline Extensions** | **E1 / E2 / E3** | `X01`, `X02`, `X03` | Optional Search Crates (`oryq-fts`, `oryq-vec`), Shared Multi-Process Coordinator (`.oryq-shm`), Queued-Command Group Commit | Independent extension conformance, capable-drop/reduced-reopen drills, and native multi-process `SIGKILL` schedules pass. |
