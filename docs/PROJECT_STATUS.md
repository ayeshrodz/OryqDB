# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 0 — Research and architecture (`W00` / `v0.0.2`)
STATUS: Complete — converged architecture & pinned LiteDB source audit verified; ready for W01 (M1 setup)

IMPLEMENTED:
- Documentation only. No Cargo workspace, production engine or executable Rust tests exist.
- Authored master and eight subsystem specifications plus roadmap and trackers.
- Added the modular technical specification, eight module guides and ordered module work packages.
- Completed pinned LiteDB source audit (`dev` @ `b3c4406231908c6ca29f6d4b2769aa08351d6fc2`, MIT) across all six `docs/research/litedb-*.md` deliverables (`W00` / `v0.0.2`).
- Applied EGM review revisions across initial design and modular architecture (both converged).

VERIFIED:
- `python3 tools/validate_design_docs.py` passes (links, fragments, fences, 26,809 numeric ordering cases, 100 prefix-bound cases, C/C++ header syntax).
- See initial and modular Goldfish review reports for reviewer completion, evidence and dispositions.
- Specification review and reference audit are not runtime validation or format freeze.

TEST RESULTS:
- Documentation/reference checks and design-level experiments passed.
- No production Rust, C ABI implementation, durability or benchmark tests executed yet.

KNOWN ISSUES:
- Persistent catalog schema/fixtures must be frozen at the M3 gate.
- Shared-process synchronization and optional search are post-baseline extensions.
- Complete-length corrupted WAL tails fail closed and may require manual diagnosis.

PERFORMANCE:
- No measured OryqDB performance results. All design figures are unverified targets.

NEXT STEPS:
1. Start `W01` (`v0.1.0`): create eight-crate Cargo workspace, `architecture/*.toml` manifests, `oryq-types`, and `xtask architecture` self-tests.
2. Implement `W02`/`W03` (M1 pager/VFS + document codec) with golden fixtures and parser limits.
3. Implement `W04`/`W05` (M2 B+ tree, WAL, transaction protocol and crash/admission schedules).
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
