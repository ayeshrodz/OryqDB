# Module: Optional Providers and Removal

* **Owner:** `oryq-fts, oryq-vec; engine E2/E3 folders`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Make optional heavyweight implementations independently addable, testable and removable while preserving persisted-state safety.

## 2. Architecture

Search providers import types/document/engine/index only. Engine-local coordinator/batching providers stay inside engine and cannot import query/core.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

IndexProvider includes all write/read/descriptor/drop/integrity duties under one scoped transaction. Queries use capabilities and generic search requests; no planner imports of provider crates. E2/E3 preserve engine snapshot/commit invariants.

## 4. Internal Data Structures and Owned Files

Provider-local option schema/data version and catalog namespace roots; stable feature identity is independent of build registration order. Core hosts factories without a reverse dependency.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `oryq-fts/src/{lib,descriptor,provider,tokenizer,postings,bm25,options}.rs` | One exported registration factory and FTS-owned extraction/storage/search/options. |
| `oryq-vec/src/{lib,descriptor,provider,distance,graph,options}.rs` | One exported registration factory and vector-owned data/distance/graph/options. |
| `Each provider tests/{conformance,visibility,removal}.rs` | Run shared index conformance plus provider-specific snapshot/delete/drop/reduced-build fixtures. |
| `oryq-engine/src/coordinator/shared_process/{mod,admission,locks,recovery}.rs` | Keep the complete E2 synchronization protocol within one owner area. |
| `oryq-engine/src/commit_batch/{mod,queue,leader}.rs` | Keep E3 batch ownership/queue/durability implementation within one owner area. |
| `oryq-core/src/composition/providers.rs; provider Cargo.toml; core Cargo.toml` | Single host registration and optional dependency/feature edge, added only when manifests exist. |
| `architecture/capabilities.toml; docs/compatibility.md` | Allocate stable persistent IDs/bits and document compatibility when a persisted capability is added. |

A shared provider can serve multiple option sets: bind each search to its selected snapshot descriptor and test two differently configured indexes against explicit no-index semantics. Provider initialization works without any documents, and matches removes candidate-superset false positives through the generic query factory. Generic search bindings and exact/approximate result semantics are index-owned. Core binds query's generic ProviderSearchFactory once per declared binding. Missing or incompatible data/options versions block dependent operations before mutation, even when a feature bit is available.

## 5. Invariants

An absent provider never skips maintenance of existing data. Drop provider objects with a capable binary before source removal; preserve pages for old snapshots. Bits/IDs are never reused. No independent sidecar storage or provider commit.

## 6. Error Handling

Duplicate registration/options mismatch fail builder validation. Required-read bits reject open; missing optional write support blocks affected collection writes/schema operations while permitted baseline reads/unrelated writes retain existing semantics.

## 7. Concurrency Behavior

Registries freeze per database instance. Provider state uses the caller snapshot/writer and scoped storage; remove registrations by reopen, never by unloading active pinned code/state.

## 8. Performance Characteristics

Optional dependency footprints stay out of default builds. Pending/merged records remain transaction-visible and charged; merges are normal write transactions, not checkpoint side effects.

## 9. Known Limitations and Change Boundary

E1/E2/E3 remain post-baseline acceptance gates. Dynamic native loading/unloading is not supported. Removing format-level parsing requires a migration/API compatibility design.

## 10. Testing Methodology

X01–X03: each-provider/combined/reduced build matrices, interrupted drop/merge recovery, old snapshot retention, incompatible options/data versions, candidate quotas and cross-process/batch adversarial schedules.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
