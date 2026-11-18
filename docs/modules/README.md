# OryqDB Module Catalog

This catalog maps changes to one owner area and its contract tests. The [technical specification](../designs/modular-architecture.md) owns dependency and integration rules; the [module development plan](../MODULE_DEVELOPMENT_PLAN.md) owns ordered packages and their actual-result ledger. All modules are currently planned.

| Module guide | Implementation owner | Responsibility |
| :--- | :--- | :--- |
| [Foundation Contracts](foundation.md) | `oryq-types` | Own the small set of values shared across independent domain boundaries. Keep algorithm-specific state outside this crate. |
| [Document Values and Updates](document.md) | `oryq-document` | Own logical values, binary document representation, comparison, paths, JSON conversion and pure before/after update evaluation. |
| [Physical Storage and Transactions](engine.md) | `oryq-engine` | Own the physical durability domain: positioned I/O, checksummed pages/trees, cache versions, snapshots, savepoints, WAL, recovery, checkpoint and sealed files. |
| [Index Providers and Maintenance](index.md) | `oryq-index` | Own canonical key encoding and index behavior independent of collection API orchestration. Core chooses active descriptors; each provider owns complete read/write/drop behavior. |
| [Queries and Pipeline Stages](query.md) | `oryq-query` | Own parsing, filter semantics, access planning and bounded execution. New operator implementations register through owner-local factories rather than consumer-wide enums. |
| [Database Facade and Composition](core.md) | `oryq-core` | Provide the stable application API and coordinate transactions across otherwise independent document/engine/index/query owners. |
| [FFI and CLI Adapters](adapters.md) | `oryq-ffi and oryq-cli` | Keep language/transport ownership and presentation separate from database rules. New query/update syntax is forwarded through the common request model, not reimplemented by each adapter. |
| [Optional Providers and Removal](extensions.md) | `oryq-fts, oryq-vec; engine E2/E3 folders` | Make optional heavyweight implementations independently addable, testable and removable while preserving persisted-state safety. |

A capability fitting an existing port stays in its module folder plus tests/spec and registration. New cross-owner contracts, stable APIs or persistent bytes require the named owners to change together; do not hide that coupling in shared utilities. Source inventory/ownership manifests are planned in `architecture/`; no production crates or scaffolds are created by these documents.
