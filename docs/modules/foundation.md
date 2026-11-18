# Module: Foundation Contracts

* **Owner:** `oryq-types`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Own the small set of values shared across independent domain boundaries. Keep algorithm-specific state outside this crate.

## 2. Architecture

No internal production dependencies. Standard allocation/synchronization utilities only; no I/O or domain imports.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

Reservation tokens transfer charges with ownership; borrowed use cannot duplicate them. ID types cannot be freely interchanged. The shared error envelope preserves module/location/source context while core owns public OryqError translation.

## 4. Internal Data Structures and Owned Files

IDs, BudgetHandle, Reservation, CancellationToken, CapabilityId and ErrorEnvelope. None contains DatabaseInner or a global registry.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/lib.rs` | Export the documented identity/error/budget/cancellation/capability contracts only. |
| `src/ids.rs` | Distinct PageId, TxId, CollectionId, IndexId, NamespaceId and RecordId newtypes; checked construction and counter overflow. |
| `src/error.rs` | Module-tagged error category, structured location and owned source-chain envelope; no feature enum. |
| `src/budget.rs` | Shared reservation tracker and nonduplicable reservation token; explicit budget domains. |
| `src/cancel.rs` | Cancellation request/token; a cancellation signal never releases a snapshot pin. |
| `src/capability.rs` | Stable string capability IDs and version/feature-bit types; no implementation registry. |
| `tests/{budget,identity,errors}.rs` | Reservation transfer/drop, quota exhaustion, ID separation and stable error conversion tests. |

## 5. Invariants

No documents, tree operations, OS paths or provider-specific variants enter this crate. A new abstraction needs at least two production consumers. A live Reservation releases its charge exactly once.

## 6. Error Handling

Quota exhaustion is deterministic and tagged with its budget domain. Overflow and invalid IDs return errors rather than wrap. Do not put every domain error into one growing enum.

## 7. Concurrency Behavior

Budget accounting supports shared owners; reservation ownership is explicit. Cancellation is observable across threads but does not imply operation quiescence.

## 8. Performance Characteristics

No dynamic dispatch for numeric ID comparison. Budget accounting is charged at allocation boundaries, not every byte inspected.

## 9. Known Limitations and Change Boundary

Changing ID widths, public error classifications or reservation semantics is a shared-contract change with direct consumer tests; it is not a local feature edit.

## 10. Testing Methodology

W01: budget transfer, drop, failure, cancellation and checked ID tests; external-consumer compilation from document/engine. No runtime database evidence is implied.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
