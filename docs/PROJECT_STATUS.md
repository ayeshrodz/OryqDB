# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 1 — Persistent storage foundation (`W02` complete / `v0.1.0-alpha.2`)
STATUS: In Progress — `W01` (`oryq-types`, workspace, `xtask`, CI) and `W02` (`oryq-document` codec, views, comparator, JSON) verified; `W03` (`oryq-engine` pager/VFS) next to complete M1

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`):
  - Eight-crate Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, `rustfmt.toml`, `.clippy.toml`, and `.github/workflows/ci.yml`.
  - Full implementation of `oryq-types` (IDs, boxed `ErrorEnvelope`, RAII `BudgetHandle`/`Reservation`, `CancellationToken`, `CapabilityId`/`FeatureBitmask`/`VersionRange`).
  - `architecture/{modules,capabilities,contract-changes}.toml` and `xtask architecture` / `xtask modules <id>` enforcement tool.
- Completed `W02` (`v0.1.0-alpha.2`):
  - Full implementation of `oryq-document` (`#![forbid(unsafe_code)]`, engine-independent): `OryqDoc` v1 binary codec & strict validator, zero-copy `DocumentView`/`ArrayView`, `ObjectId`/`Uuid`/`Decimal128` (IEEE 754-2008 BID), `FieldPath`, exact cross-type comparator (`compare_values`), and Extended-JSON conversion.
- Remaining 6 domain/adapter crates (`oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) are minimal compiling scaffolds only (not marked implemented).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings).
- `cargo test --workspace` and `--no-default-features`: 24 tests passed (`oryq-types`: 7 integration tests; `oryq-document`: 11 integration tests across `roundtrip`, `ordering`, `malformed`; `xtask`: 6 architecture-checker self-tests).
- `cargo run -p xtask -- architecture --base origin/main`, `xtask modules types`, and `xtask modules document`: passed.
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 24 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml`.

KNOWN ISSUES:
- Persistent storage (`W03`), transactions/WAL/MVCC (`W04–W05`), and persistent catalog/CRUD (`W06–W08`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W03` (`v0.1.0` / M1 Complete): `oryq-engine` VFS/PAL, Page 0 dual shadow headers, 8192B slotted pager, freelist, overflow chains, and golden byte fixtures.
2. Implement `W04` (`v0.2.0-alpha.1`): `oryq-engine` logical-page B+ tree and bounded page cache.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
