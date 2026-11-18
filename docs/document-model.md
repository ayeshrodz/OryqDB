# OryqDB Subsystem Specification: Document Model (`OryqDoc` & State-Normalized Deltas)

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document specifies OryqDB's document data model, type system, primary identifier (`_id`) semantics, deterministic JSON and Extended-JSON interoperability, zero-copy `OryqDocView<'a>` lookup mechanics, and state-normalized partial update delta patching (`$set`, `$unset`, `$inc`, `$push`, `$pull`).

## 2. Architecture

The `document/` subsystem separates document representation into two complementary types:
1. **`OryqDocView<'a>` (Zero-Copy Borrowed View):** Wraps a byte slice `&'a [u8]` pointing directly into a pinned buffer-pool `PageGuard` (with optional page-local delta patch slices). Provides $O(\log k)$ binary-search field extraction and $O(1)$ 64-bit bloom bitmask delta filtering with zero heap allocations.
2. **`OryqDocBuf` (Owned Immutable Buffer):** Wraps an `Arc<[u8]>` containing a contiguous, compacted `OryqDoc` byte buffer that can outlive transaction and cursor page pins or cross threads safely.

When a partial update executes, `document/patch.rs` evaluates the update operators against the current `OryqDocView`, computes the resulting post-update values for all modified field paths, and produces a **State-Normalized Value Patch** plus an updated 64-bit field-hash bloom bitmask. Low-level `storage/` pages store and compact these patches via the `RecordCompactor` trait without parsing update operators.

## 3. Public Interfaces

```rust
// Illustrative Rust Document Interfaces (Design Only)
pub struct OryqDocBuf { /* Arc<[u8]> */ }
pub struct OryqDocView<'a> { /* base: &'a [u8], deltas: SmallVec<[&'a [u8]; 4]>, bloom: u64 */ }

impl<'a> OryqDocView<'a> {
    pub fn get_path(&self, dotted_path: &str) -> Result<Option<OryqValueView<'a>>, OryqError>;
    pub fn get_str(&self, dotted_path: &str) -> Result<Option<&'a str>, OryqError>;
    pub fn get_i64(&self, dotted_path: &str) -> Result<Option<i64>, OryqError>;
    pub fn get_f64(&self, dotted_path: &str) -> Result<Option<f64>, OryqError>;
    pub fn get_bool(&self, dotted_path: &str) -> Result<Option<bool>, OryqError>;
    pub fn to_owned_doc(&self) -> Result<OryqDocBuf, OryqError>;
    pub fn to_json_string(&self) -> Result<String, OryqError>;
}
```

## 4. Internal Data Structures

### 4.1 Type System (`OryqTypeTag` & `OryqValueView<'a>`)

| Type Variant | Tag | Storage Width | JSON / Extended-JSON Representation |
| :--- | :--- | :--- | :--- |
| `Null` | `0x0` | 0B (inline in descriptor) | `null` |
| `Bool(bool)` | `0x1`/`0x2` | 0B (inline in descriptor) | `false` / `true` |
| `Int64(i64)` | `0x3` | 8B (`i64 LE`) | Standard JSON integer (or `{"$numberLong": "..."}` in strict mode) |
| `Float64(f64)` | `0x4` | 8B (`f64 LE`) | Standard JSON number (`NaN`/`±Inf` via `{"$numberDouble": "..."}`) |
| `DateTimeUtcMs(i64)` | `0x5` | 8B (`i64 LE`) | `{"$date": {"$numberLong": "<epoch_ms>"}}` or ISO-8601 string |
| `String(&'a str)` | `0x6` | 4B len + UTF-8 bytes | Standard JSON string |
| `Binary(&'a [u8], u8)` | `0x7` | 4B len + 1B subtype + bytes | `{"$binary": {"base64": "...", "subType": "00"}}` |
| `ObjectId([u8; 12])` | `0x8` | 12B | `{"$oid": "<24-hex-chars>"}` |
| `Uuid([u8; 16])` | `0x9` | 16B | `{"$uuid": "<canonical-uuid-string>"}` |
| `Decimal128([u8; 16])` | `0xA` | 16B | `{"$numberDecimal": "<exact-decimal-string>"}` |
| `Array(OryqArrayView<'a>)` | `0xB` | 8B header + $4N$ offset table + elements | `[ ... ]` |
| `Document(OryqDocView<'a>)` | `0xC` | Recursive `OryqDoc` | `{ ... }` |

### 4.2 Primary Identifier (`_id`) Semantics

* Every document stored in a collection must have a top-level `"_id"` field.
* Supported `_id` types: `ObjectId` (12 bytes), `Uuid` (16 bytes), `Int64` (8 bytes), and `String` (up to 1024 UTF-8 bytes). `Null`, `Boolean`, `Array`, and `Document` values are rejected as `_id` types with `OryqError::InvalidIdType`.
* **Automatic `_id` Generation:** If an inserted document omits `"_id"`, the engine automatically prepends a 12-byte monotonically sortable `ObjectId` (`[4-byte unix_seconds_be][5-byte process_random][3-byte atomic_counter_be]`), ensuring right-leaning sequential insertions in the primary B+ tree.

### 4.3 Update Operators & State-Normalized Patch Evaluation

| Operator | Syntax Example | Write-Time Normalization Behavior |
| :--- | :--- | :--- |
| `$set` | `{"$set": {"profile.country": "NZ", "active": true}}` | Emits `ReplaceValue` patch entries for `"profile.country"` and `"active"`; creates intermediate sub-documents if missing. |
| `$unset` | `{"$unset": {"temp_token": ""}}` | Emits `TombstoneUnset` (`op_kind = 0x02`) patch entry for `"temp_token"`. |
| `$inc` | `{"$inc": {"login_count": 1, "score": 2.5}}` | Reads current field value (treating missing as `0`), applies the checked arithmetic and precision rules in Section 4.4, and emits `ReplaceValue` with the computed result. Fails with `OryqError::TypeMismatch` if current value is non-numeric. |
| `$push` | `{"$push": {"tags": "rust"}}` | Reads current array (or initializes `[value]` if field is missing), appends element, and emits `ReplaceValue` containing the updated `OryqArray` slice. |
| `$pull` | `{"$pull": {"tags": "legacy"}}` | Reads current array, removes all elements matching value/predicate, and emits `ReplaceValue` containing the filtered `OryqArray` slice. |

### 4.4 Paths, Patch Overlap & Materialization

Field names are valid UTF-8 without NUL or `.`; empty names and empty path components are rejected in v1. Numeric array-position update paths are unsupported; `$push`/`$pull` operate on whole arrays. `_id` is immutable, including replacement documents (same ID required). Reject all ancestor/descendant overlaps across operators before modifying state.

Normalize nested updates to **top-level field replacements**: `$set: {"a.b": 1}` stores the complete post-update value of `a` in its patch. Bloom bits hash the top-level component of both stored patches and requested paths, so replacing `a` cannot produce a bloom miss for `a.b`. Within a patch resolve hash collisions by comparing full path bytes; search newest to oldest and let the first top-level replacement/tombstone win. A child lookup then traverses the winning replacement. This trades larger nested patches for deterministic zero-copy leaf lookup and avoids mixed ancestor/descendant reconstruction.

A fifth patch, insufficient space, slot relocation or any overflowing patch triggers full-document materialization and compaction before the operation completes. Large documents/values use owned cursor scratch buffers and full-page logging. Raw `OryqDoc` getters and owned clones always return the complete logical document, never stale base bytes or unresolved overflow pointers. Charge scratch bytes to the query budget and return a structured memory-limit error when unavailable.

`$inc` uses checked `i64` addition for integer operands (`NumericOverflow` on overflow); a floating operand produces `f64` only after lossless integer conversion, otherwise `NumericPrecisionLoss`. Decimal arithmetic is rejected with `UnsupportedFeature` until its rounding/context contract is implemented. Canonical Extended JSON preserves values and types, not arbitrary original NaN payload bits, decimal encodings, object field order or whitespace. Normalize NaNs and signed zero during ingestion; never claim bit-identical transport for noncanonical inputs.

## 5. Invariants

1. **Deterministic Numeric & Extended-JSON Fidelity:** Converting canonical `OryqDocBuf -> canonical Extended JSON -> OryqDocBuf` preserves values and type tags; noncanonical binary encodings are normalized on ingestion. Integers within `i64` bounds never silently coerce to lossy IEEE-754 floats during JSON parsing.
2. **Bloom Bitmask Completeness:** Physical patches replace top-level fields; the 2-hash bloom bits for `"a"` are set for every update beneath `a`. Readers hash that same top-level component for `a`, `a.b` and `a.b.c`, preventing ancestor and descendant false negatives.
3. **Unaligned Memory Safety:** Scalar getters (`get_i64`, `get_f64`, `get_datetime`) read fixed-size `[u8; 8]` arrays via `copy_from_slice` and `from_le_bytes`, never by casting unaligned `*const u8` pointers to `*const i64`.

## 6. Error Handling

* Inserting or updating a document with conflicting paths in the same update expression (e.g., modifying both `"profile"` and `"profile.age"` in one update) returns `OryqError::ConflictingUpdatePath`.
* Applying `$inc` to a string/boolean or `$push`/`$pull` to a non-array field returns `OryqError::UpdateTypeMismatch { path, expected, actual }`.
* Documents exceeding `Config::max_document_bytes` (default 16 MiB) return `OryqError::DocumentTooLarge`.

## 7. Concurrency Behavior

`OryqDocBuf` is immutable and `Send + Sync`. `OryqDocView<'a>` is an immutable read-only view bound by lifetime `'a` to a reader's `PageGuard`, allowing multiple threads to inspect fields on the same cached page simultaneously without locks or reference-count contention.

## 8. Performance Characteristics

* **Unmodified Field Lookup (`delta_count == 0` or Bloom Miss):** $O(1)$ bitwise AND on `field_bloom_mask` + $O(\log k)$ binary search over 12-byte directory entries (at most $\lceil \log_2(64) \rceil = 6$ integer comparisons for a 64-field document), zero heap allocations.
* **Modified Field Lookup (Bloom Hit):** Linear scan of at most `MAX_DELTA_CHAIN = 4` patch headers, returning a zero-copy slice of the normalized post-update value.
* **Array Element Lookup:** $O(1)$ random access via the array's 32-bit element offset table.

## 9. Known Limitations

* When a delta patch updates a single element of an array (`$push` / `$pull`), the normalized patch stores the updated array slice for that field rather than per-element array deltas.
* Nested changes replace the whole top-level value, so patch size grows with that value. Whole-document export and overflow reads may allocate bounded scratch storage.

## 10. Testing Methodology

* **Property-Based Round-Trip & Ordering (`proptest`):** Generate arbitrary nested JSON/Extended-JSON trees with mixed numeric, binary, array, and Unicode keys; verify lossless `OryqDocBuf` round-trips and identical lookup results between base `OryqDoc` documents and documents mutated through arbitrary sequences of 1 to 20 `$set`/`$unset`/`$inc`/`$push`/`$pull` deltas.
* **Miri Verification:** Run all `OryqDocView` scalar and slice accessors under `cargo +nightly miri test` at arbitrary byte offsets (`0..7` alignment shifts) to prove the absence of alignment or aliasing Undefined Behavior.

* **Ancestor/descendant gates:** Replace/unset `a` then read `a.b`; update `a.b` then replace `a`; repeat across four patches and compaction, with collisions, overflow, negative zero, immutable `_id` and numeric overflow/precision failures.
