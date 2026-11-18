# Changelog

All notable changes to **OryqDB** will be documented in this file.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (`0.x.y` pre-1.0 phasing until the Milestone 10 baseline release candidate passes all native verification gates).

## [Unreleased]

### Planned
- `0.0.2` (`W00` / M0): Pinned LiteDB reference audit and `docs/research/litedb-*.md` reports.
- `0.1.0` (`W01–W03` / M1): Eight-crate Cargo workspace, `oryq-types`, `xtask architecture` checker, and `oryq-engine` persistent slotted-page foundation.

## [0.0.1] - 2026-11-19

### Added
- Converged EGM master engine design specification ([docs/designs/oryqdb.md](docs/designs/oryqdb.md)) and modular technical specification ([docs/designs/modular-architecture.md](docs/designs/modular-architecture.md)).
- Eight per-module ownership and contract guides ([docs/modules/README.md](docs/modules/README.md)) and ordered module work-package plan ([docs/MODULE_DEVELOPMENT_PLAN.md](docs/MODULE_DEVELOPMENT_PLAN.md)).
- Eight normative subsystem specifications ([docs/architecture.md](docs/architecture.md), [docs/file-format.md](docs/file-format.md), [docs/document-model.md](docs/document-model.md), [docs/transactions.md](docs/transactions.md), [docs/indexing.md](docs/indexing.md), [docs/query-language.md](docs/query-language.md), [docs/ffi.md](docs/ffi.md), [docs/compatibility.md](docs/compatibility.md)).
- Architecture Decision Records ADR-001 through ADR-008 ([docs/ARCHITECTURE_DECISIONS.md](docs/ARCHITECTURE_DECISIONS.md)), product roadmap ([docs/ROADMAP.md](docs/ROADMAP.md)), risk register ([docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md)), benchmark specification ([docs/BENCHMARK_RESULTS.md](docs/BENCHMARK_RESULTS.md)), and competitor comparison ([docs/research/competitor-comparison.md](docs/research/competitor-comparison.md)).
- EGM Goldfish review reports under `docs/reviews/initial-design/` and `docs/reviews/modular-architecture/`.
- Automated design-doc validator ([tools/validate_design_docs.py](tools/validate_design_docs.py)) covering local link/fragment verification, 26,809-case exact numeric key ordering, 100-case prefix bounds, and C/C++ syntax checks of the proposed ABI header.
