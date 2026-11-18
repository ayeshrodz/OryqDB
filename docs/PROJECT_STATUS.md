# OryqDB Project Status

* **Last Updated:** 2026-11-19
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: 0 — Research and architecture
STATUS: In Progress — reviewed architecture draft; source audit outstanding

IMPLEMENTED:
- Documentation only. No Cargo workspace, production engine or executable Rust tests exist.
- Authored master and eight subsystem specifications plus roadmap and trackers.
- Added the modular technical specification, eight module guides and ordered module work packages.
- Source placement now follows compile-enforced domain boundaries; no production scaffolds created.
- Applied EGM review revisions to storage/recovery, numeric indexes, query semantics,
  memory budgets, FFI lifecycle, release scope and acceptance gates.

VERIFIED:
- See goldfish_review_report.md for reviewer completion, evidence and dispositions.
- Specification review is not runtime validation or format freeze.

TEST RESULTS:
- Documentation/reference checks and design-level experiments are recorded in the report.
- No production Rust, C ABI implementation, durability or benchmark tests executed.

KNOWN ISSUES:
- Six LiteDB source-analysis reports and pinned source commit remain outstanding.
- Persistent catalog schema/fixtures must be frozen at the M3 gate.
- Shared-process synchronization and optional search are post-baseline extensions.
- Complete-length corrupted WAL tails fail closed and may require manual diagnosis.

PERFORMANCE:
- No measured OryqDB performance results. All design figures are unverified targets.

NEXT STEPS:
1. Complete pinned LiteDB source audit and M0 research acceptance.
2. Implement M1 pager/VFS with golden fixtures and parser limits.
3. Implement M2 transaction protocol and crash/admission schedules before optimization.
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
