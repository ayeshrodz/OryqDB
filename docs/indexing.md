# OryqDB Subsystem Specification: Indexing Engine & Pluggable Search Substrate

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document specifies OryqDB's persistent indexing subsystem (`crates/oryq-index/src/`), covering primary `_id` indexes, single-field and nested-path secondary indexes, order-preserving memcomparable key encoding, prefix-compressed compound indexes ($R=16$ restart points), multi-key array indexes, and the WAL-backed `PluggableIndex` substrate for feature-gated Full-Text Search (`crates/oryq-fts`) and Vector Search (`crates/oryq-vec`).

## 2. Architecture

Every collection in OryqDB maintains:
1. **Primary Index B+ Tree:** Maps the memcomparable encoding of user `_id` to internal `RecordId`; the collection B+ tree resolves RecordId to document payload (Section 4.5).
2. **Zero or More Secondary B+ Tree Indexes:** Store memcomparable keys encoded from one or more top-level or nested document paths (`"email"`, `"profile.country"`, `("status", "total")`, or array elements in `"tags"`), suffixed by `RecordId`.
3. **Zero or More Pluggable Indexes (`PluggableIndex` Trait):** Optional index engines (`oryq-fts`, `oryq-vec`) that store their inverted lists and vector graph nodes inside pages allocated from the same `.oryq` file and logged through `.oryq-wal` via `IndexStorageContext<'tx>`.

## 3. Public Interfaces

```rust
// Illustrative Rust Index Management API (Design Only)
pub struct IndexOptions {
    pub unique: bool,
    pub sparse: bool,
    pub name: Option<String>,
}

pub enum IndexSpec {
    Single { path: String, order: SortOrder },
    Compound { fields: Vec<(String, SortOrder)> },
    Pluggable { kind: String, paths: Vec<String>, params_json: String },
}

impl<'a, 'db> WriteCollection<'a, 'db> {
    pub fn create_index(&mut self, spec: IndexSpec, opts: IndexOptions) -> Result<String, OryqError>;
    pub fn drop_index(&mut self, index_name: &str) -> Result<(), OryqError>;
    pub fn list_indexes(&self) -> Result<Vec<IndexMetadata>, OryqError>;
}
```

## 4. Internal Data Structures

### 4.1 Canonical Order-Preserving Memcomparable Key Encoding (`key_codec.rs`)

Every indexed field component encodes into `[type_rank: u8][order_preserving_payload...]`:

* **`0x00` (`MinKey`):** 0 payload bytes.
* **`0x05` (`Missing` / `Null`):** 0 payload bytes. When `IndexOptions::sparse == true`, documents where all indexed fields are `Missing` omit the index entry entirely; when `sparse == false`, both `Missing` and explicit `Null` encode as `0x05`.
* **`0x10` (`Bool(false)`) / `0x11` (`Bool(true)`):** 0 payload bytes.
* **`0x1F` (`NumberNaN`):** Canonical representation of all `f64::NAN` bit patterns (sorted immediately before negative numbers).
* **`0x20` (`Number`):** Exact variable-width cross-type encoding, defined below. Never round an integer or decimal through `f64`.
* **`0x30` (`DateTimeUtcMs`):** 8-byte big-endian `u64` formed by `(epoch_ms as u64) ^ (1 << 63)`.
* **`0x40` (`String`):** UTF-8 bytes where any internal `0x00` byte is escaped as `0x00 0xFF`, terminated by `0x00 0x00`.
* **`0x50` (`Binary`):** One subtype byte followed by raw bytes where any internal `0x00` byte is escaped as `0x00 0xFF`, terminated by `0x00 0x00`.
* **`0x60` (`ObjectId`) / `0x61` (`Uuid`):** 12-byte or 16-byte raw big-endian bytes.
* **`0x70` (`Array`) / `0x80` (`Document`):** Recursive canonical components terminated by `0x00`. Arrays preserve element order. Documents sort field names by UTF-8 bytes and encode alternating string-name and value components; field-directory hash order is not semantic field order. Escape nesting through component parsing; no component begins with rank `0x00` except the internal MinKey sentinel, which is forbidden as a stored value.
* **`0xFF` (`MaxKey`):** 0 payload bytes.

For descending index components (`SortOrder::Desc`), every encoded byte of that component (including its rank byte and terminator) is bitwise inverted (`byte ^ 0xFF`).

#### Exact Numeric Payload

After rank `0x20`, emit a category byte: `0x00 = -Infinity`, `0x10 = negative finite`, `0x20 = zero`, `0x30 = positive finite`, `0x40 = +Infinity`. Zero and infinities have no following payload. NaNs use the separate canonical rank `0x1F`; all NaNs compare equal to each other for query/index purposes. Normalize signed zero.

Represent a nonzero finite absolute value exactly as decimal digits `D` with no leading or trailing zero and adjusted exponent `E = floor(log10(abs(value)))`, so value equals `integer(D) * 10^(E - len(D) + 1)`. Do not compute `E` with floating-point logarithms: derive it from the exact coefficient and scale. Encode magnitude as `[E:i32 biased by XOR 0x80000000, 4 bytes BE][digits: each 0x10 + digit][terminator:0x00]`. For negative finite numbers invert every magnitude byte, including the terminator; positive finite magnitude is unchanged. Compare encoded bytes lexicographically. This orders shorter equal-prefix significands correctly and reverses all negative magnitudes.

For `i64`, use exact absolute integer arithmetic with `i64::MIN` handled in unsigned/wider arithmetic. For finite `f64`, decode sign/exponent/significand to an exact rational `m * 2^e`; if `e < 0`, form integer `m * 5^(-e)` with decimal scale `e`, then trim zeroes. Decimal128 finite values use their exact coefficient and decimal exponent; reject noncanonical/reserved decimal encodings on ingestion. This codec needs bounded wide-integer scratch (at most 1074 fractional binary exponent steps for f64) and can produce hundreds of digits; the normal key-size limit still applies. Numerically equal values of different types encode identically, e.g. `10_i64` and `10.0_f64`; decimal `0.1` differs from binary floating `0.1` because their exact values differ.

Golden cases must cover signed zero, NaNs/infinities, adjacent integers around `2^53` and `2^62`, `i64` limits, subnormals, and decimal exponent/precision limits. The exact comparator must be independently implemented as a rational-number oracle. Decoding yields a canonical numeric value, not its original storage type; covering projections cannot infer that type.

### 4.2 Prefix-Compressed

### 4.2 Prefix-Compressed Index Leaf Pages with $R = 16$ Restart Intervals (`compound.rs`)

Index Leaf Pages (`page_type = 0x03`, `page_flags & HAS_PREFIX_RESTARTS != 0`) group slots into blocks of $R = 16$ entries:
* **Restart Slot ($i \bmod 16 == 0$):** Stores `[shared_prefix_len: u16 = 0][full_key_len: u16][full_key_bytes...][value_location]`.
* **Delta Slot ($i \bmod 16 \neq 0$):** Stores `[shared_prefix_len: u16][unshared_suffix_len: u16][unshared_suffix_bytes...][value_location]`, where `shared_prefix_len` is always relative to the immediately preceding reconstructed key. Lengths are `u16 LE`; reject a prefix longer than that key, an over-limit reconstructed key, or a non-increasing key. Insert/delete/split recompress the affected leaf and regenerate restart slots at positions divisible by 16; the slot directory supplies restart offsets.
* **Binary Search:** Searching a leaf page with $N$ keys performs binary search over the $\lceil N / 16 \rceil$ Restart Slots in $O(\log(N/16))$ comparisons, then sequentially scans at most 15 delta entries within the matching block.

### 4.3 Multi-Key (Array) Index Maintenance (`multikey.rs`)

When an index is defined on path `P` (e.g., `"tags"` or `"items.sku"`):
* **Array Extraction:** If `P` evaluates to an array (or traverses an array of objects), the key extractor collects the set of distinct encoded memcomparable keys $K(\text{doc})$.
* **Parallel Array Prohibition:** In a compound index `(P_1, P_2, ..., P_m)`, at most one path $P_j$ may evaluate to an array for any document. If two or more paths evaluate to arrays in the same document, the write transaction aborts with `OryqError::ParallelArrayIndex`.
* **Transactional Diff on Update:** When a document is updated (either by full replacement or by a state-normalized delta patch whose modified paths intersect the index's paths), the writer computes $K_{\text{old}}$ (from the pre-update document view) and $K_{\text{new}}$ (from the post-update view), deletes every key in $K_{\text{old}} \setminus K_{\text{new}}$, and inserts every key in $K_{\text{new}} \setminus K_{\text{old}}$.

### 4.4 WAL-Backed `PluggableIndex` Substrate (`plugin.rs`)

* **`IndexStorageContext<'tx>`:** Grants a `PluggableIndex` access to:
  * A dedicated **Pending-Delta B+ Tree** (identified by `pending_root_page_id` in the index catalog entry).
  * A dedicated **Compacted Segment B+ Tree / Page Pool** (`segment_root_page_id`, using `page_type = 0x07`).
* **Two-Tier Write Flow:**
  1. During individual document inserts/updates/deletes (`on_doc_write`), `oryq-fts` and `oryq-vec` append a single compact delta entry `(doc_id, operation, payload)` into their Pending-Delta B+ Tree (1 localized B+ tree write).
  2. During `on_commit_batch` (or when pending deltas exceed a configurable batch threshold, e.g., 256 documents), the plugin folds the pending entries into compressed BM25 posting lists (`oryq-fts`) or paged HNSW graph nodes (`oryq-vec`) inside the same WAL transaction.
* **Feature-Bitmask Enforcement:** Creating an `oryq-fts` index sets Bit 0 (`0x01`) of `required_write_features` in Page 0; creating an `oryq-vec` index sets Bit 1 (`0x02`).

### 4.5 Identity, Catalog & Planner Completeness

`_id` is the user-visible primary key; **`RecordId`** is a separate internal unsigned 64-bit identity, monotonically allocated from a transactional catalog counter (start 1, reject overflow). Never reuse a RecordId during the file's lifetime. The collection tree maps RecordId to document bytes; the primary `_id` index maps its exact key to RecordId. Secondary entries append RecordId as an 8-byte BE tie-breaker and resolve document locations through the collection tree, so slot relocation does not invalidate indexes. Persist the counter and all mappings in the same transaction. Deduplication and plugin candidate filters use `RoaringTreemap` over RecordId, never hashes/truncated user IDs. Rollback may leave gaps but never committed collisions.

Sparse indexes omit documents only when all indexed components are Missing. Missing and Null share the non-sparse key, so existence/type predicates always recheck document bytes. A sparse index is usable only when the planner proves every possible match has an entry; negative predicates, `$exists:false`, unfiltered count/sort and whole-array equality use a collection scan unless completeness is independently proven. Multikey element keys support positive scalar equality/membership/ranges only. Recheck the full predicate, and do not intersect bounds from independent array conditions as if they referred to one element; `$elemMatch` supplies that correlation. Empty arrays and whole/nested-array equality require a complete scan. Index-only counts require no unresolved residual predicate and one counted entry per document. Covering projection is disabled for normalized numeric values, Missing/Null, multikey values or any original representation erased by the codec.

Index creation takes the writer lock, builds from its transaction snapshot, and publishes the root/catalog entry only at commit; failure rolls back its pages and feature requirements. Plugin queries merge the snapshot-visible pending tree with compacted segments, applying newest operation per RecordId, including tombstones. Plugin merge is a normal write transaction, never an unlogged checkpoint side effect. `required_write_features` is the checkpointed union of catalog requirements; enforce the WAL-visible per-collection mask before every write/schema change.

## 5. Invariants

1. **Index-Document Transaction Equivalence:** A committed document exists at `(LogicalPageId, SlotIdx)` if and only if its primary index entry and all applicable secondary, compound, multi-key, and pluggable index entries exist in the same committed snapshot `TxId`.
2. **Unique Constraint Enforcement:** For any index with `unique == true`, attempting to insert or update a document such that its index key (excluding the trailing `DocId` tie-breaker) collides with an existing live document's key aborts the operation with `OryqError::DuplicateKey { index_name, key_debug }`.
3. **Feature-Gate Write Protection:** If a collection has an active pluggable index whose `required_feature_bit` is not registered in the running binary, any write operation (`insert`, `update`, `delete`) targeting that collection fails immediately with `OryqError::UnsupportedFeature` before mutating any pages.

## 6. Error Handling

* Duplicate keys on unique indexes return `OryqError::DuplicateKey` without corrupting partially updated index trees (any prior index insertions in the failed statement are rolled back via transaction savepoints).
* Corrupted index pages or broken B+ tree key orderings detected during traversal return `OryqError::Corruption(CorruptionError)`.

## 7. Concurrency Behavior

Index B+ trees share the same `BufferPool`, `PageVersionIndirectionMap`, and `.oryq-wal` as collection B+ trees. Multiple readers traverse index pages lock-free at their `SnapshotTxId` while the writer updates index pages in its transaction workspace.

## 8. Performance Characteristics

* **Point & Prefix Lookups:** $O(\log_B N)$ page traversals + $O(\log(M/16) + 16)$ in-page comparisons per level.
* **Unmodified Indexed Field Bypass on Partial Update:** When a partial update (`$inc`, `$set`) modifies only unindexed fields, the writer checks the update's modified paths against the collection's indexed paths in $O(1)$ bitmask/set time and skips secondary index B+ tree lookups and writes completely.

## 9. Known Limitations

* Compound indexes support up to 16 component fields per index.
* Maximum complete encoded index key (including RecordId) is `min(2048, floor((page_size - 64) / 4) - 32)` bytes, leaving room for at least three entries and internal pointers at the supported 4096-byte minimum. Oversized keys return `IndexKeyTooLarge`; never silently truncate.

## 10. Testing Methodology

* **Property-Based Oracle Equivalence (`proptest`):** Compare the results of every indexed equality, range, prefix, compound, and multi-key query against a brute-force full collection scan oracle across random sequences of inserts, partial updates, deletes, and transaction rollbacks.
* **Crash-Consistent Index Recovery (`tests/recovery/crash_fault_injection.rs`):** Inject faults mid-index-creation and mid-multi-index-update, reopen the database, and run `db.integrity_check()` to verify exact 1-to-1 bijection between collection documents and all index entries.
