# OryqDB Known Issues, Limitations & Risk Register

* **Last Updated:** 2026-11-19
* **State:** Design only; no engine implementation or performance evidence.

| ID | Risk / remaining limitation | Contract or closure gate |
| :--- | :--- | :--- |
| R01 | A leaked snapshot can retain WAL indefinitely. | Request cancellation but retain history until guard release; reject writes at hard WAL cap. M2 interleaving/quota tests. |
| R02 | File-identity races and POSIX process lock behavior can invalidate locking. | PAL must specify race-safe registry/open identity handling, alias/symlink tests and descriptor ownership before enabling process-scoped locks. M1 gate; no claim of proven immunity. |
| R03 | Shared-process ABI/admission/death/recovery protocol is not frozen. | Return UnsupportedFeature until E2 native tests pass. Publish per-process minimum across local tokens; raw pointers alone do not prove safety. |
| R04 | Nested replacement patches can be large and overflow. | Normalize top-level values; materialize on overflow/5th patch; M3/M7 oracle and scratch-budget tests. |
| R05 | Multikey, sparse and negative predicates can omit matches. | Restrict index eligibility and recheck; fall back to collection scans. M4/M8 differential tests. |
| R06 | Missing optional index implementations could corrupt write maintenance. | Transactional catalog feature masks and checkpointed header union; reject affected collection writes. E1. |
| R07 | Concurrent queries, writer buffers and caller documents exceed a simple cache budget. | Shared per-query reservations, separate transaction cap and spill cap. Component budgets are not a process RSS bound. M5/M8. |
| R08 | Failed commit sync can persist an unknown subset. | CommitOutcomeUnknown, faulted database, reopen recovery and idempotent application IDs. M2 failure schedules. |
| R09 | Complete-length invalid WAL frame may be a torn tail or corruption of committed history. | Fail closed, preserve artifacts, diagnose manually; do not silently truncate acknowledged data. Recovery availability is conditional, not guaranteed for all corruption. |
| R10 | Format is unfrozen; catalog schema/fixtures and native portability are unverified. | Freeze at M1–M3/M10 gates only after fixtures and tests. Cross-compilation is not native verification. |
| R11 | Reference research remains incomplete. | Six `docs/research/litedb-*.md` reports require pinned source inspection; existing competitor descriptions are unverified hypotheses. |
| R12 | Strict power-loss durability depends on supported filesystem/device flush semantics. | FullSync rejects unavailable required flush guarantees; weaker mode must be explicitly selected and labelled. M8 native tests. |

[Review dispositions](reviews/initial-design/goldfish_review_report.md) · [Delivery and verification gates](ROADMAP.md)
