# `oryq-types`

Foundation contracts crate for **OryqDB** (see [docs/modules/foundation.md](../../docs/modules/foundation.md)).

## Responsibilities
* Checked, distinct identity newtypes (`PageId`, `TxId`, `SnapshotId`, `CollectionId`, `IndexId`, `NamespaceId`, `RecordId`).
* Module-tagged `ErrorEnvelope` and `ErrorCategory` with structured location and owned source chains.
* Shared `BudgetHandle` and non-duplicable RAII `Reservation` tokens across explicit `BudgetDomain`s.
* Thread-safe `CancellationSource` / `CancellationToken`.
* Stable `CapabilityId`, `FeatureBitmask`, `VersionRange`, and `CapabilityDescriptor`.

## Verification
```bash
cargo test -p oryq-types
```
