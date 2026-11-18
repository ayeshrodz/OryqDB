# Module: Document Values and Updates

* **Owner:** `oryq-document`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Own logical values, binary document representation, comparison, paths, JSON conversion and pure before/after update evaluation.

## 2. Architecture

oryq-types only. Never engine, index, query or core.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

Validated DocumentView borrows its caller-owned bytes; DocumentBuf owns bytes. Encoding/update/compaction that allocates accepts a caller budget. Updates produce normalized logical mutation bytes and touched top-level paths, never engine page writes.

## 4. Internal Data Structures and Owned Files

Value tags, array/document views, canonical paths, operator factories, normalized mutation and physical patch representation. Preserve the formats in document-model.md/file-format.md; do not create a second schema here.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `src/{lib,types}.rs` | Public borrowed/owned document/value types and narrow exports. |
| `src/{codec,view,array}.rs` | Encode/decode validated bytes, checked recursive slices and array descriptors. |
| `src/path.rs` | Validate canonical paths; use the same rules for filters and updates. |
| `src/json.rs` | Canonical JSON/Extended JSON conversions and numeric normalization. |
| `src/compare.rs` | Exact cross-type numeric and canonical value order consumed by predicates and index encoding. |
| `src/updates/{mod,registry,set,unset,inc,push,pull}.rs` | Common update validation plus one file per operator; owner-local factory registration. |
| `src/patch/{mod,normalize,resolve,compact}.rs` | Top-level physical replacement patches, newest-first shadowing and pure compaction. |
| `tests/{roundtrip,ordering,updates,malformed}.rs` | Oracle tests for value fidelity/order, paths, delta equivalence, depth and malformed inputs. |
| `tests/fixtures/README.md` | Inventory the exact document/array/patch golden fixture cases and generation rules. |

## 5. Invariants

One comparator defines value semantics used by scans and indexes. No page IDs or unresolved overflow references in exported OryqDoc. Nested updates replace top-level values and hash those same names for bloom checks.

## 6. Error Handling

Return path/type/overflow/precision/malformed-value errors before effects. Reserved tags, invalid UTF-8, self-reference and excessive depth fail validation.

## 7. Concurrency Behavior

Owned buffers are immutable and shareable; a borrowed view remains valid only while its owner lives. This crate cannot extend an engine pin or widen a Rust lifetime.

## 8. Performance Characteristics

Inline field access remains borrowed and does not need global context. Owned full-document materialization reserves bytes through the provided budget.

## 9. Known Limitations and Change Boundary

Adding a type/tag or changing canonical order affects codecs, indexes and fixtures. A new update operator fitting the normalized mutation contract stays in updates/ plus tests/spec.

## 10. Testing Methodology

W02/W06/W12: independent numeric comparator oracle, roundtrip and malformed fixtures, ancestor/descendant patch sequences, immutable _id, overflow/precision errors and Miri scalar reads. Engine compaction adapter gets separate core integration tests.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
