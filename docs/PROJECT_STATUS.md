# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 1 — Persistent storage foundation (`W01` M1 setup complete / `v0.1.0-alpha.1`)
STATUS: In Progress — `W01` workspace, `oryq-types`, `xtask` architecture checker, and CI verified; `W02` (`oryq-document`) and `W03` (`oryq-engine`) next

IMPLEMENTED:
- Completed `W00` (`v0.0.2`): converged EGM design/modular specs and pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`).
- Completed `W01` (`v0.1.0-alpha.1`):
  - Eight-crate Cargo workspace (`oryq-types`, `oryq-document`, `oryq-engine`, `oryq-index`, `oryq-query`, `oryq-core`, `oryq-ffi`, `oryq-cli`) + `xtask`, `rustfmt.toml`, `.clippy.toml`, and `.github/workflows/ci.yml`.
  - Full implementation of `oryq-types` (IDs, boxed `ErrorEnvelope`, RAII `BudgetHandle`/`Reservation`, `CancellationToken`, `CapabilityId`/`FeatureBitmask`/`VersionRange`).
  - `architecture/{modules,capabilities,contract-changes}.toml` and `xtask architecture` / `xtask modules <id>` enforcement tool.
- Remaining 7 domain/adapter crates are minimal compiling scaffolds only (not marked implemented).

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings).
- `cargo test --workspace` and `--no-default-features`: 13 tests passed (`oryq-types`: 7 integration tests; `xtask`: 6 architecture-checker self-tests).
- `cargo run -p xtask -- architecture` and `cargo run -p xtask -- modules types`: passed (7 downstream consumers checked in topological order).
- `python3 tools/validate_design_docs.py`: passed.

TEST RESULTS:
- Linux `x86_64` (`rustc 1.99.1`): 13 passed, 0 failed.
- Multi-OS CI matrix (`ubuntu-latest`, `macos-latest`, `windows-latest`) configured in `.github/workflows/ci.yml`.

KNOWN ISSUES:
- Persistent storage (`W03`), document codec (`W02`), transactions (`W05`), and persistent catalog fixtures (`W08`) are not yet implemented.
- Shared-process synchronization (`X02`) and optional search (`X01`) remain post-baseline extensions.

PERFORMANCE:
- No measured OryqDB database benchmarks yet (scheduled for `W12` / M7).

NEXT STEPS:
1. Implement `W02` (`v0.1.0-alpha.2`): `oryq-document` binary codec, zero-copy `DocumentView`, exact numeric comparator, path validator, and JSON conversion.
2. Implement `W03` (`v0.1.0` / M1 Complete): `oryq-engine` VFS/PAL, Page 0 dual shadow headers, 8192B slotted pager, freelist, overflow chains, and golden byte fixtures.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
