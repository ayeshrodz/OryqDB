# Module: Index Providers and Maintenance

* **Owner:** `oryq-index`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Own canonical key encoding and index behavior independent of collection API orchestration. Core chooses active descriptors; each provider owns complete read/write/drop behavior.

## 2. Architecture

oryq-types, oryq-document and oryq-engine. Never query/core/FFI/CLI.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

IndexProvider initializes empty indexes through create, validates descriptors, declares capabilities, extracts/validates keys, applies insert/update/delete, opens snapshot scans and drops/validates its persisted roots. All mutation receives the caller transaction/savepoint; no independent commit. Generic provider requests bind immutable snapshot-selected field/options/version context before validation; scan and matches use that same context. Generic provider predicates use index-owned matches for residual evaluation; unranked index-optional search falls back to collection scan with that evaluator. Read capabilities state exactness, ordering, multikey status and supported predicates explicitly.

## 4. Internal Data Structures and Owned Files

Immutable descriptors/factories, encoded keys, provider-specific roots/data versions and candidate iterators. RecordId identity is distinct from arbitrary user _id; bitmaps use full u64 identities.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/{lib,descriptor,key_codec}.rs` | Provider descriptor/options schema and exact memcomparable encoding. |
| `src/{registry,provider,maintenance}.rs` | Immutable factories, provider interface and scoped maintenance dispatch. |
| `src/primary/{mod,read,write}.rs` | Unique user _id to internal RecordId mapping. |
| `src/secondary/{mod,extract,read,write}.rs` | Single/nested-field extraction and transactional differential maintenance. |
| `src/compound/{mod,keys,bounds}.rs` | Compound key construction and bound semantics; engine owns the generic byte-tree prefix restart codec. |
| `src/multikey/{mod,extract,dedup}.rs` | Array keys, guards, exact RecordId candidate dedup and quota checks. |
| `src/plugin/{mod,context,pending}.rs` | Restricted storage namespace, pending updates and shared provider substrate. |
| `tests/{codec,conformance,uniqueness,completeness}.rs` | Shared provider contracts, exact ordering and indexed-versus-scan completeness. |

Registered providers are transaction-independent; all mutable pending/root state stays in statement arenas/private pages. Core stages returned MaintenanceResult roots into its transactional catalog. Provider read/write/options version ranges are validated after recovery and at operation time.

## 5. Invariants

Every applicable provider maintains the same before/after logical document inside one statement transaction. Primary uniqueness is mandatory. A provider cannot claim a scan is complete when Missing/Null, sparse or array semantics can omit matches.

## 6. Error Handling

Descriptor/key-size/unique/quota failures propagate to core statement rollback. Missing provider capability blocks required writes/drop instead of skipping maintenance. Query-unusable indexes may be bypassed by a complete scan when semantics permit.

## 7. Concurrency Behavior

Reads use the caller snapshot and include committed pending updates. Writes use the existing engine writer. Namespace allocations and roots participate in rollback and reader-retained deletion.

## 8. Performance Characteristics

Extraction and maintenance stay provider-local. Provider capability metadata lets planners choose access without importing concrete implementation types.

## 9. Known Limitations and Change Boundary

Changing a key encoding, RecordId semantics or provider data version needs fixtures/migration review. New provider internals stay in one folder/crate plus registration and capability allocation.

## 10. Testing Methodology

W07/W09/W13a/X01: conformance across create/insert/update/delete/rollback/drop/reopen, exact numerical limits, null/sparse/multikey oracle equivalence, pending deletion visibility and reduced-build removal fixtures.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
