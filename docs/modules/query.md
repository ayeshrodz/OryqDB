# Module: Queries and Pipeline Stages

* **Owner:** `oryq-query`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Own parsing, filter semantics, access planning and bounded execution. New operator implementations register through owner-local factories rather than consumer-wide enums.

## 2. Architecture

oryq-types, oryq-document, oryq-engine and oryq-index. No core or concrete optional-provider crates.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

QuerySource resolves collection/index descriptors and probes/scans/get operations under the caller snapshot. ProviderSearchFactory invokes provider matches for ExactCandidates and can scan unranked index-optional predicates without an index; it does not add concrete-provider switches. StageFactory validates operands and constructs execution with a shared context. Core implements the source adapter; query never calls a core Database API or begins a new snapshot.

## 4. Internal Data Structures and Owned Files

Core logical predicate primitives plus registry-backed extensions, immutable plans, capability declarations, stage instances, shared budget/cancellation and scratch ownership. No global list of every possible plugin type.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/{lib,ports,context,ast,parse,cursor,explain}.rs` | Supported query boundary, source port, one shared query context, AST/factory parse dispatch, lending cursor and diagnostics. |
| `src/predicates/{mod,registry,comparison,logical,array,string}.rs` | Predicate factories and exact semantic evaluation. |
| `src/planner/{mod,capabilities,bounds,rank}.rs` | Completeness checks, exact encoded bounds and deterministic access ranking. |
| `src/stages/{mod,registry,match_stage,project,limit,skip,unwind,lookup}.rs` | Stage registry and one local implementation per streaming stage. |
| `src/stages/sort/{mod,heap,external}.rs` | Bounded top-K and external sort. |
| `src/stages/group/{mod,accumulator,partition}.rs` | Arithmetic/aggregation state and bounded partition spill. |
| `src/spill/{mod,files,quota,cleanup}.rs` | Temporary-file creation, per-query spill cap and cleanup ownership. |
| `tests/{filters,planner,pipeline,budget,spill}.rs` | Differential semantics/planning, full-query reservations and all failure cleanup cases. |

## 5. Invariants

Capabilities must prove index completeness; otherwise scan. Negative predicates are complements, Missing differs from Null, and array correlation needs elemMatch. One context charges all stages/candidates/materializations; lookup retains the same snapshot.

## 6. Error Handling

Unknown/unsupported operations and malformed operands fail before execution; memory/spill/IO errors clean scratch and release reservations. Unspillable state returns a quota error. No silently unbounded bitmap or large group.

## 7. Concurrency Behavior

A Send, non-Sync cursor borrows/retains a Send + Sync immutable ReadProjection of the transaction. The writer cannot mutate while this query projection lives; FFI mirrors Rust borrowing with BUSY on mutation with child cursors. Plans can be shared when immutable but each execution has its own charged state; operator registration freezes before database open.

## 8. Performance Characteristics

Factory dispatch occurs at parse/plan construction. Keep hot predicate loops concrete where practical. A local sort/group algorithm change must not require FFI, CLI or core dispatch branches.

## 9. Known Limitations and Change Boundary

New primitives changing canonical comparison/identity must update their domain owner first. Arbitrary plugin planner rewrites are not allowed to bypass completeness, residual filtering or memory limits.

## 10. Testing Methodology

W10/W13b: scan/index oracle, Unicode/NUL prefix bounds, sparse/negative/array cases, representation-preserving projection, rational average and quota/spill cancellation/error cleanup. Use fake QuerySource in unit tests and core adapter in integration.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
