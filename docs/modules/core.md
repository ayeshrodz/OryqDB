# Module: Database Facade and Composition

* **Owner:** `oryq-core`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Provide the stable application API and coordinate transactions across otherwise independent document/engine/index/query owners.

## 2. Architecture

All five baseline domain crates; optional FTS/vector provider crates at E1. Never FFI/CLI. Providers do not import core.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

Database/Collection/ReadTransaction/WriteTransaction/Cursor remain application-facing types. Builder assembles immutable owner registries and validates them once. Adapter implementations bridge engine compaction and query collection resolution without upward dependencies.

## 4. Internal Data Structures and Owned Files

DatabaseInner holds engine, catalog and immutable registries; core does not duplicate page caches or provider algorithms. Catalog records point to provider-owned roots/options with stable IDs and data versions.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/{lib,database,config,error,stats}.rs` | Facade exports, open/builder, shared limits, public error translation and stats aggregation. |
| `src/api/{mod,collection,read,write,cursor}.rs` | Application handles and safe wrappers around byte transactions and query cursors. |
| `src/catalog/{mod,collection,index,capabilities,codec}.rs` | Versioned collection catalog schema, provider references and transactional feature requirements. |
| `src/write/{mod,statement,mutation,maintenance}.rs` | One statement savepoint and atomic before/after/index/catalog orchestration. |
| `src/composition/{mod,builtins,providers,options}.rs` | The sole cross-owner registration/typed-option assembly area. |
| `src/adapters/{mod,record_compactor,query_source}.rs` | Implement lower-layer ports using other domain owners, closing dependency inversions safely. |
| `src/admin/{mod,integrity,backup,inspect}.rs` | Logical catalog/index validation, sealed backup, and re-exported read-only Page/WAL/Locks inspection workflows. |
| `tests/{vertical_slice,atomic_maintenance,feature_lifecycle,backup}.rs` | Cross-owner persistence/rollback, unsupported provider, safe removal and copy correctness. |

Facade inspection and exact source/API ownership are defined in [adapter operations](../designs/modular-architecture.md#10-facade-operations-required-by-adapters); CLI cannot bypass this boundary for raw pages/WAL/locks.

## 5. Invariants

All collection/index/RecordId/catalog effects share a transaction and statement rollback. Required capabilities are checked using WAL-visible catalog state. Only composition knows concrete provider implementations; ordinary database/write/query code uses ports.

## 6. Error Handling

Translate domain envelopes to public errors without losing module/location/source. FFI owns numeric C-code mapping. A missing required provider blocks affected mutations and schema changes. Failed sync propagates the uncertain outcome without retrying internally.

## 7. Concurrency Behavior

Database is shareable; engine owns actual snapshot/writer synchronization. Registry changes require reopening; public handles/FFI retain database/transaction state until children release it.

## 8. Performance Characteristics

Core orchestrates once per statement/plan, not every byte lookup. Integrity/backup use explicit snapshot traversal, including opaque allocated provider pages.

## 9. Known Limitations and Change Boundary

Facade and catalog are mandatory; changing stable Rust signatures/catalog versions is a contract change. A new feature fitting an existing port must not add another condition in database/write APIs.

## 10. Testing Methodology

W08/W14/X01: first CRUD/reopen slice, partial-statement rollback after one index succeeds and another fails, provider fault behavior, catalog feature unions, capable-drop/reduced-reopen and concurrent snapshot backup.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
