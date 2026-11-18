# OryqDB Subsystem Specification: System Architecture

* **Author:** Ayesh Rodrigo (@ayeshrodz)
* **Last Updated:** 2026-11-19
* **State:** Proposed modular source architecture; no production code exists
* **Technical authority:** [Modular architecture](designs/modular-architecture.md)

## 1. Purpose

Keep each feature's implementation, tests and specification in its owning module. Enforce domain boundaries through crate dependencies and narrow exports; retain a stable application facade and transactional storage behavior.

## 2. Architecture

The baseline has eight production crates. Optional FTS/vector crates are added only after their extension gate. The [dependency allowlist](designs/modular-architecture.md#1-dependency-graph) and [file inventory](designs/modular-architecture.md#2-proposed-file-structure) are canonical; do not maintain a second competing crate/file list here.

| Owner | Responsibility | Implementation guide |
| :--- | :--- | :--- |
| types | IDs, error envelopes, reservations, cancellation and capability identity | [Foundation](modules/foundation.md) |
| document | Binary values, pure comparison/path/update/compaction behavior | [Document](modules/document.md) |
| engine | Physical I/O/pages/trees/cache/transactions/WAL/recovery/checkpoint | [Engine](modules/engine.md) |
| index | Key encoding, provider descriptors and index maintenance/search/drop | [Index](modules/index.md) |
| query | Filter/planner/stage factories and bounded execution | [Query](modules/query.md) |
| core | Public Rust facade, catalog, atomic collection writes and composition | [Core](modules/core.md) |
| FFI / CLI | Language/process ownership and presentation, importing core only | [Adapters](modules/adapters.md) |
| optional providers | Search implementations and engine-local coordination/batching extensions | [Extensions](modules/extensions.md) |

Pure document work requires no engine. Engine stores/replays byte records without importing document/query/index code. Index/query consume downward interfaces; core supplies upward-facing adapters without reverse imports. Optional provider registration occurs only in core composition, so providers never depend on their host.

## 3. Public Interfaces

Applications use `oryq-core::Database`, collection/transaction/cursor wrappers and supported re-exported values/errors. Internal crates share the workspace version and do not promise independent API stability. The C ABI remains owned by [ffi.md](ffi.md).

Ports include engine StorageIo/ReadTxn/WriteTxn/Savepoint/RecordCompactor, document validated borrowed/owned values and normalized updates, index IndexProvider/IndexStorageContext, query QuerySource/operator/stage factories, and core DatabaseBuilder/ModuleRegistration. Their [boundary contracts](designs/modular-architecture.md#4-boundary-contracts) specify caller-owned snapshot, budget and transaction lifetimes.

## 4. Internal Data Structures

Engine owns page guards, physical version maps, WAL state and writer/snapshot synchronization. Document owns validated value/patch buffers. Index owns descriptors/keys/provider roots. Query owns AST/plans/execution/scratch. Core DatabaseInner owns their handles plus one catalog and immutable registries, rather than duplicating their algorithms/state.

Core's compactor adapter implements an engine-owned pure byte contract by invoking document compaction. Core's query-source adapter implements a query-owned port by resolving core catalog and engine/index reads at the caller snapshot. Domain crates cannot fetch arbitrary services from DatabaseInner.

## 5. Invariants

1. Production imports obey the allowlist, and no low-level crate imports the facade or an adapter.
2. All document/index/RecordId/catalog changes share a core statement savepoint and the same engine transaction. No correctness-critical best-effort event bus or independent provider commit exists.
3. Snapshot/guard retention, failed-sync uncertainty, reset/unseal ordering and persisted capability enforcement remain those in [transactions.md](transactions.md) and [file-format.md](file-format.md).
4. Registries freeze before open; IDs/feature bits do not depend on registration order and are never reused for unrelated meanings.
5. Normal implementation changes stay in their owner. Shared port/public API/persistent-format changes identify direct consumers and migration requirements explicitly.

## 6. Error Handling

Owners convert local errors to a module-tagged shared envelope. Core preserves its stable application errors; FFI owns stable C status mapping. Missing providers block required maintenance, and unknown operators/options fail deterministically. Panics and uncertain commit outcomes follow the existing faulted-database policy rather than allowing partially maintained state.

## 7. Concurrency Behavior

Engine serializes explicit writers through sync/publication and protects snapshot admission against reclamation. Core orchestrates domain calls within that ownership. QuerySource and providers use the supplied snapshot; registry changes require closing/reopening. FFI children retain parent transaction/database state. Optional coordinator/batching providers cannot weaken these contracts.

## 8. Performance Characteristics

Domain boundaries do not require serialization between Rust crates. Inline scalar reads remain borrowed; dynamic dispatch is concentrated at factories/providers and I/O boundaries. Shared reservations charge query/cache/transaction domains as already specified. No new latency, footprint or throughput result is asserted before measurements.

## 9. Known Limitations

Source modularity does not make durable formats, core identity or stable C symbols removable as local features. Existing persisted provider objects must be dropped/migrated with a capable binary before code removal. Internal crate APIs may expose lifetime pressure during implementation; adjust the port with its direct consumers rather than adding cycles.

## 10. Testing Methodology

Each owner has isolated tests and external-consumer conformance tests. The future architecture checker validates Cargo metadata, ownership and feature edges. Core integration tests prove cross-owner atomicity/recovery/removal; FFI and CLI validate their actual user boundaries. The [module development plan](MODULE_DEVELOPMENT_PLAN.md) defines gates and actual-result recording. Existing runtime/format/source-research gates remain open until executed.
