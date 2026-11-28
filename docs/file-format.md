# OryqDB Subsystem Specification: Binary File Format (`.oryq`, `OryqDoc`, `.oryq-wal`, `.oryq-shm`)

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document defines the exact on-disk and in-memory binary layouts for all persistent and sidecar artifacts managed by **OryqDB**:
1. The primary single-file database (`.oryq`), including Page 0 dual shadow headers, slotted B+ tree pages, overflow chains, and freelist pages.
2. The `OryqDoc` v1 binary document format and state-normalized slotted-page delta patches.
3. The Write-Ahead Log sidecar (`.oryq-wal`).
4. The Shared-Memory multi-process coordination sidecar (`.oryq-shm`).

## 2. Architecture

All multi-byte structural integers across `.oryq`, `OryqDoc`, `.oryq-wal`, and `.oryq-shm` are stored in **explicit little-endian (`LE`) byte order** (`u16::to_le_bytes`, `u32::to_le_bytes`, `u64::to_le_bytes`), with two explicit exceptions:
1. `name_hash_prefix: u32` in the `OryqDoc` field directory is stored big-endian so 32-bit integer comparison matches byte-prefix ordering.
2. Memcomparable B+ tree index keys (`IndexKey`) use big-endian order-preserving encoding so raw `memcmp` orders arbitrary typed values identically across x86-64 and ARM64.

No Rust struct is ever cast to or from raw bytes via `std::mem::transmute` or `#[repr(C)]` direct disk dumps.

## 3. Public Interfaces

* `StorageHeader::encode(&self, buf: &mut [u8; 256])` / `StorageHeader::decode(buf: &[u8; 256]) -> Result<Self, CorruptionError>`
* `SlottedPageView::parse(page_bytes: &[u8], expected_page_id: PageId) -> Result<Self, CorruptionError>`
* `OryqDocView::from_bytes(buf: &[u8]) -> Result<Self, CorruptionError>`
* `WalFrame::encode(&self, writer: &mut impl Write)` / `WalFrame::decode(bytes: &[u8], prev_checksum: u64) -> Result<Self, CorruptionError>`

## 4. Internal Data Structures

### 4.1 Section A: Main Database File (`.oryq`)

#### Page 0 Layout (8192 Bytes Default)

* **Bytes `0..256`:** Shadow Header Slot A (`StorageHeader`).
* **Bytes `256..512`:** Shadow Header Slot B (`StorageHeader`).
* **Bytes `512..page_size`:** Reserved zero; the catalog lives on ordinary logical pages.

| Byte Offset (Within 256B Slot) | Size | Type | Field | Normative Value / Rule |
| :--- | :--- | :--- | :--- | :--- |
| `0..8` | 8B | `[u8; 8]` | `magic` | Exact bytes `b"ORYQDB\x00\x01"` |
| `8..10` | 2B | `u16 LE` | `format_version` | `1` |
| `10..12` | 2B | `u16 LE` | `page_size_log2` | `12` (4096B), `13` (8192B default), or `14` (16384B) |
| `12..20` | 8B | `u64 LE` | `header_generation` | Monotonically increasing counter per checkpoint/header commit |
| `20..28` | 8B | `u64 LE` | `required_read_features` | Bitmask of features required to open for reading (`0` in v1 baseline) |
| `28..36` | 8B | `u64 LE` | `required_write_features` | Bit 0 = `FEATURE_FTS` (`0x01`), Bit 1 = `FEATURE_VEC` (`0x02`) |
| `36..52` | 16B | `[u8; 16]` | `database_uuid` | 128-bit UUID generated at database creation |
| `52..60` | 8B | `u64 LE` | `total_page_count` | Total logical pages allocated in `.oryq` ($\ge 1$) |
| `60..68` | 8B | `u64 LE` | `catalog_root_page_id` | Logical `PageId` of the system catalog B+ tree root |
| `68..76` | 8B | `u64 LE` | `freelist_trunk_page_id` | Logical `PageId` of the first freelist trunk page (`0` if empty) |
| `76..84` | 8B | `u64 LE` | `freelist_page_count` | Total free pages available across all freelist trunk/leaf pages |
| `84..92` | 8B | `u64 LE` | `checkpointed_tx_id` | Highest `TxId` checkpointed into `.oryq` |
| `92..100` | 8B | `u64 LE` | `checkpointed_wal_seq` | Last checkpointed WAL cycle sequence number |
| `100..108` | 8B | `u64 LE` | `reset_intent_seq` | `0` normally; next WAL sequence after a fully synced checkpoint, recorded before WAL reset |
| `108..109` | 1B | `u8` | `wal_required` | `0` sealed/standalone checkpoint; `1` WAL required for recovery |
| `109..248` | 139B | `[u8; 139]` | `reserved` | Must be all `0x00` in `format_version == 1` |
| `248..256` | 8B | `u64 LE` | `header_checksum` | `XXH3_64(slot_bytes[0..248])` |

On `Database::open()`, both Slot A and Slot B are validated. If both pass `magic` and `header_checksum`, the slot with the higher `header_generation` is selected. If only one passes (e.g., power loss during a checkpoint header write), the valid slot is selected. If neither passes, `OryqError::Corruption` is returned.

#### Logical Page Header (`PageId >= 1`, 32 Bytes)

| Byte Offset | Size | Type | Field | Description |
| :--- | :--- | :--- | :--- | :--- |
| `0..8` | 8B | `u64 LE` | `page_id` | Logical `PageId` (must match requested `PageId`) |
| `8..9` | 1B | `u8` | `page_type` | `0x01`=ColLeaf, `0x02`=ColInternal, `0x03`=IdxLeaf, `0x04`=IdxInternal, `0x05`=Overflow, `0x06`=Freelist, `0x07`=PluginPage |
| `9..10` | 1B | `u8` | `page_flags` | Bit 0=`HAS_PREFIX_RESTARTS`, Bit 1=`IS_ROOT_PAGE` |
| `10..12` | 2B | `u16 LE` | `slot_count` | Number of directory slots, including patch/tombstone slots |
| `12..14` | 2B | `u16 LE` | `free_space_end` | Upper byte offset of unallocated free space (starts at `page_size`) |
| `14..16` | 2B | `u16 LE` | `fragmented_bytes` | Reclaimable dead payload bytes inside `[free_space_end..page_size]` |
| `16..24` | 8B | `u64 LE` | `lsn_tx_id` | `TxId` that last modified this page |
| `24..32` | 8B | `u64 LE` | `page_checksum` | `XXH3_64(page_bytes[0..24] || page_bytes[32..page_size])` |

#### Slotted-Page Directory Entry (16 Bytes per Slot, Starting at Offset 32)

| Byte Offset | Size | Type | Field | Description |
| :--- | :--- | :--- | :--- | :--- |
| `0..2` | 2B | `u16 LE` | `payload_offset` | Start byte offset of payload within the page (`>= free_space_end`) |
| `2..4` | 2B | `u16 LE` | `payload_len` | Byte length of payload (`payload_offset + payload_len <= page_size`) |
| `4..5` | 1B | `u8` | `slot_flags` | Bit 0=`BASE_DOC`, Bit 1=`DELTA_PATCH`, Bit 2=`HAS_OVERFLOW`, Bit 3=`TOMBSTONE` |
| `5..6` | 1B | `u8` | `delta_count` | Number of chained delta patches (`0..=4`; `0` on delta slots themselves) |
| `6..8` | 2B | `u16 LE` | `head_delta_slot` | Slot index of the newest delta patch (`0xFFFF` if `delta_count == 0`) |
| `8..16` | 8B | `u64 LE` | `field_bloom_mask` | 64-bit 2-hash bloom filter of all field paths modified in the delta chain |

#### B+ Tree, Overflow & Freelist Payloads

All tree kinds use the 32-byte page header and 16-byte slots. A slot is a stable directory position until a structural full-page rewrite; deleted slots count toward `slot_count` and have TOMBSTONE plus zero length. Payload intervals of live slots cannot overlap. Collection tree keys are `RecordId:u64 BE`; primary and secondary trees use the canonical key codec in [indexing.md](indexing.md). Collection and index leaf entries have the envelope `[key_len:u16 LE][key_bytes][value_len:u32 LE][value_bytes]`. Collection values are `[storage_kind:u8][body]`, where `0x00` is inline `OryqDoc`, `0x01` is an inline state-normalized delta-chain envelope (`MAX_DELTA_CHAIN = 4`), `0x02` is a 16-byte `OverflowRef` descriptor referencing chained `PageType::Overflow` pages (whose `original_tag` is `0x00` for raw `OryqDoc` or `0x03` for LZ4-compressed `[uncompressed_len:u32 LE][lz4_block_bytes]`), and `0x03` is an inline LZ4-compressed `OryqDoc` (`[uncompressed_len:u32 LE][lz4_block_bytes]`). Primary values are RecordId as `u64 BE`; secondary entries carry it as their key suffix and have empty values. Prefix-compressed index leaves use Section 4.2 of indexing.md in place of this plain leaf envelope, with `value_location` defined as `[value_len:u32 LE][value_bytes]`.

Internal slots encode `[separator_len:u16 LE][separator_bytes][right_child:u64 LE]`; slot 0 is a sentinel with zero separator length whose child is the leftmost subtree. All later separators are nonempty and strictly increasing, with equality descending right. Sibling links are not stored: cursors retain a traversal stack to advance leaves. Page splits/merges and any change in slot ordering emit full-page WAL images. Catalog entries use the plain leaf envelope: keys are UTF-8 catalog names and values are versioned canonical OryqDoc records, including collection/primary/index roots, RecordId high watermark, and per-index feature requirements. Freeze their schema/golden fixtures before the M3 catalog gate.

Overflow pages (`0x05`) have no slot directory (`slot_count=0`): bytes `32..40` next PageId (`0` ends chain), `40..44` chunk length (`u32 LE`), `44..48` reserved zero, `48..page_size` payload and zero unused bytes. A reference contains `[first_page:u64 LE][total_len:u32 LE][original_tag:u8][reserved:3 zero]`. Reject cycles, out-of-file IDs, shared ownership, chunks exceeding page capacity and total-length mismatches; bound traversal by declared length and page count. Overflow allocation/free is transactional and reader-retained. Ordinary exported OryqDoc has no storage pointers; resolve chains into owned buffers before returning binary documents.

Freelist pages (`0x06`) are trunks: `32..40` next trunk (`u64 LE`, 0 end), `40..44` entry count (`u32 LE`), `44..48` zero; from byte 48 store `[page_id:u64 LE][retired_tx_id:u64 LE]` entries. Entries represent retired pages until the retention horizon permits reuse. The header freelist count counts reusable entries only; count limits follow the page size. Never list Page 0, a live reachable page or a trunk itself. Reuse creates a full-page image with the new owner's TxId. Unused page bytes are zero and checksummed. The earlier trunk/leaf freelist terminology is superseded by this single trunk-chain encoding.

Page 0 bytes `512..page_size` are reserved zero, never an inline catalog. `catalog_root_page_id` names an ordinary page. Root changes live in WAL commit metadata until checkpoint. A new file initializes a catalog root and two identical logical header states, syncs database/WAL and directory entries before reporting creation success.

### 4.2 Section B: `OryqDoc` v1 & State-Normalized Delta Patch Encoding

#### Base `OryqDoc` Layout

1. **Document Header (8 Bytes):**
   * `version: u8` (`0x01`)
   * `doc_flags: u8` (Bit 0 = `HAS_EXTENDED_TYPES`, Bit 1 = `HAS_OVERFLOW_POINTERS`)
   * `field_count: u16 LE` (Number of fields $k$ at this document level)
   * `total_byte_len: u32 LE` (Total byte length of this `OryqDoc` slice, $\ge 8 + 12k$)
2. **Sorted Field Directory (`12 * field_count` Bytes, Offsets `8 .. 8 + 12k`):**
   Entries are sorted in strictly ascending order by `(name_hash_prefix: u32, utf8_field_name: &[u8])`:
   * `name_hash_prefix: u32 BE`: High 32 bits of `XXH3_64(utf8_field_name)`.
   * `name_offset: u32 LE`: Relative offset from the start of this `OryqDoc` to `[name_len: u16 LE][utf8_bytes...]`.
   * `val_descriptor: u32 LE`:
     * High 4 bits (`bits 28..31`): `OryqTypeTag`:
       * `0x0` = `Null` (0 payload bytes; `val_offset` unused)
       * `0x1` = `BoolFalse` (0 payload bytes)
       * `0x2` = `BoolTrue` (0 payload bytes)
       * `0x3` = `Int64` (8-byte `i64 LE` at `val_offset`)
       * `0x4` = `Float64` (8-byte `f64 LE` at `val_offset`)
       * `0x5` = `DateTimeUtcMs` (8-byte `i64 LE` Unix epoch milliseconds at `val_offset`)
       * `0x6` = `String` (`[byte_len: u32 LE][utf8_bytes...]` at `val_offset`)
       * `0x7` = `Binary` (`[byte_len: u32 LE][subtype: u8][raw_bytes...]` at `val_offset`)
       * `0x8` = `ObjectId` (12 raw bytes at `val_offset`)
       * `0x9` = `Uuid` (16 raw bytes at `val_offset`)
       * `0xA` = `Decimal128` (16 IEEE 754-2008 decimal128 bytes at `val_offset`)
       * `0xB` = `Array` (`[elem_count: u32 LE][byte_len: u32 LE][elem_descriptors: u32 LE * elem_count][elem_heap...]` at `val_offset`)
       * `0xC` = `Document` (Recursive embedded `OryqDoc` starting at `val_offset`)
       * `0xF` = `OverflowRef` (16-byte descriptor `[first_overflow_page_id: u64 LE, total_uncompressed_len: u32 LE, original_type_tag: u8, reserved: [u8; 3]]` at `val_offset`)
     * Low 28 bits (`bits 0..27`): `val_offset` relative to the start of this `OryqDoc` (supporting up to 256 MiB per single document buffer).

#### State-Normalized Delta Patch Layout (`slot_flags & DELTA_PATCH != 0`)

```text
+-----------------------------------------------------------------------+ 0
| [prev_delta_slot: u16 LE] (0xFFFF if tail of delta chain)             |
| [patch_entry_count: u16 LE]                                           |
+-----------------------------------------------------------------------+ 4
| Patch Entries (Sorted by `path_hash64: u64`)                          |
| Entry j:                                                              |
|   [path_hash64: u64 LE]   (XXH3_64 of canonical dotted field path)    |
|   [op_kind: u8]           (0x01 = ReplaceValue, 0x02 = TombstoneUnset)|
|   [type_tag: u8]          (OryqTypeTag of replacement value)          |
|   [path_len: u16 LE]      (Byte length of UTF-8 dotted path)          |
|   [val_byte_len: u32 LE]  (Byte length of normalized value payload)   |
|   [utf8_dotted_path...]                                               |
|   [normalized_value_payload...]                                       |
+-----------------------------------------------------------------------+
```

### 4.3 Section C: Write-Ahead Log Sidecar (`.oryq-wal`)

* **WAL File Header (Bytes `0..64`):**
  * `0..8`: `magic` = `b"ORYQWAL\x01"`
  * `8..10`: `format_version: u16 LE` = `1`
  * `10..12`: `page_size_log2: u16 LE`
  * `12..20`: `checkpoint_seq: u64 LE`
  * `20..36`: `database_uuid: [u8; 16]` (Must match `.oryq` Page 0 `database_uuid`)
  * `36..44`: `salt1: u64 LE` (Incremented on every WAL reset/checkpoint)
  * `44..52`: `salt2: u64 LE` (Random 64-bit nonce generated on every WAL reset)
  * `52..56`: `reserved: u32 LE` (`0`)
  * `56..64`: `header_checksum: u64 LE` (`XXH3_64(bytes[0..56])`)
* **WAL Frame Header (48 Bytes Preceding Each Frame Payload):**
  * `0..8`: `frame_seq: u64 LE` (1-based sequential frame number)
  * `8..16`: `tx_id: u64 LE`
  * `16..24`: `logical_page_id: u64 LE`
  * `24..25`: `frame_kind: u8` (`0x01` = `FullPageFrame`, `0x02` = `SlotDeltaFrame`, `0x03` = `CommitFrame`)
  * `25..28`: `frame_flags: [u8; 3]` (`0`)
  * `28..32`: `payload_byte_len: u32 LE`
  * `32..40`: `salt1: u64 LE`
  * `40..48`: `cumulative_checksum: u64 LE` (`XXH3_64_with_seed(frame_header[0..40] || payload_bytes, prev_frame_cumulative_checksum)`)

#### WAL Payloads & Validation

All frame byte counts are bounded and checked before allocation; transaction frames are contiguous, transaction IDs strictly increase, and `CommitFrame.logical_page_id = 0`. `prev_checksum` for frame 1 is the WAL header checksum. Full-page payload length equals the selected `page_size`, not a fixed 8192.

* **FullPageFrame:** exactly one validated logical page image. The page ID and page `lsn_tx_id` match the frame. Structural changes (split, merge, slot renumbering, compaction, allocation/free) always use full images.
* **SlotDeltaFrame:** `[base_frame_seq:u64 LE][base_page_checksum:u64 LE][result_page_checksum:u64 LE][image_count:u16 LE][reserved:u16 LE=0]`, followed by `image_count` entries `[offset:u16 LE][len:u16 LE][replacement_bytes:len]`. Entries are sorted, nonoverlapping, wholly inside the page and contain every changed byte including the resulting page header/checksum and slot directory. Apply to the exact preceding committed page version identified by `base_frame_seq`; verify both checksums. A same-cycle committed full image must anchor the chain. Cap reconstruction at 32 WAL deltas per page by emitting a new full image; this is separate from the 4-patch document chain limit.
* **CommitFrame:** fixed 48-byte payload `[first_frame_seq:u64 LE][mutation_frame_count:u64 LE][db_total_pages:u64 LE][catalog_root:u64 LE][freelist_root:u64 LE][freelist_page_count:u64 LE]`. These root/allocation changes are transactional metadata, not direct Page 0 writes. Feature requirements live in the transactional catalog and Page 0 stores their checkpointed union. The frame's cumulative checksum covers the commit payload and all preceding contiguous transaction frames.

A valid main-header `reset_intent_seq` permits rebuilding the WAL during an interrupted reset. A sealed header (`wal_required=0`) separately permits creating a missing empty WAL; it does not authorize silently replacing a present corrupt WAL. See the ordered checkpoint protocol in [transactions.md](transactions.md#41-snapshot-admission-checkpoint--recovery-protocol). Outside reset intent or the explicit sealed state, refuse missing WAL state rather than assuming an empty log. Before any post-seal append, durably mark both main-header slots `wal_required=1` as specified in transactions.md.

### 4.4 Section D:

### 4.4 Section D: Shared-Memory Coordination Sidecar (`.oryq-shm`)

This sidecar is an **unfrozen extension**, not part of the baseline format. Persistent LE rules do not apply to live synchronization atomics: supported peers must share host endianness, atomic width and SHM ABI version. The `.oryq` and WAL remain portable.

* **Control block `0..128`:** sixteen naturally 8-byte-aligned `u64` words; this does not mean each word occupies a cache line. Writer and checkpoint ownership require kernel locks, not kill-unsafe spinlocks.
* **Process slots `128..8320`:** 128 slots of 64 bytes: `pid:u32`, `generation:u32`, `min_snapshot_tx_id:u64`, `heartbeat:u64`, padding 40 bytes. Each process publishes the minimum of all its local reader tokens, not its latest reader. Slot generation prevents stale-owner reuse.
* **Lock namespace:** byte-range lock offsets are a separate lock namespace; locking bytes does not write their mapped contents. Offset `16+i` identifies a process lifetime lock; distinct writer, checkpoint and admission locks must be specified before extension implementation.
* **Frame lookup:** the proposed fixed 64 KiB hash table is removed as a correctness dependency. Each process rebuilds a bounded local map from committed WAL and scans retained frames on cache miss. Never pack 64-bit page IDs or frame sequences into 32-bit halves.

Before enabling `SharedMultiProcess`, specify and test cross-process admission/publication/reset exclusion, multiple local readers, initialization races, owner-death recovery and ABI compatibility. Until then, requesting this mode returns `UnsupportedFeature`. Raw-pointer access alone is not a synchronization proof.

## 5. Invariants

1. **Page Boundary Containment:** For every slot $s$ on a page, `32 + 16 * slot_count <= free_space_end <= s.payload_offset` and `s.payload_offset + s.payload_len <= page_size`. Any overlap or out-of-bounds offset triggers `CorruptionError`.
2. **Sorted Directory Invariant:** In every `OryqDoc`, directory entry $i$ must satisfy `(entry[i].name_hash_prefix, name[i]) < (entry[i+1].name_hash_prefix, name[i+1])`. Duplicate field names at the same document level are rejected during encoding and validation.
3. **Full-Page Write (FPW) Baseline Invariant:** A `SlotDeltaFrame` (`0x02`) for `logical_page_id = P` is valid in `.oryq-wal` only if a `FullPageFrame` (`0x01`) for `P` precedes it in the same `checkpoint_seq` cycle.

### Parser Limits & Canonical Payload Rules

Before exposing any view, validate total length, type tags, field hashes/names, strict ordering, UTF-8, every descriptor range and recursion depth (default/max v1 128). Use checked arithmetic before computing directory sizes, offsets, lengths or allocations. Maximum document bytes is `min(Config::max_document_bytes, 2^28 - 1)`; reject names exceeding `u16::MAX`. Reserved tags/flags and nonzero reserved bytes are rejected. Zero-payload descriptors have offset zero. Array `byte_len` includes its 8-byte header/table/heap; each element descriptor has the same high-4-bit type / low-28-bit offset layout as a document value descriptor, with offsets relative to the **array start**, including nested arrays. Embedded documents start their own offset origin. Require all names and nonempty value payloads to start after the owning header/directory/table, never inside it. Embedded containers have positive lengths, start in the parent heap and lie wholly inside the containing slice, strictly reducing available nesting span. Validate all nested bounds against their containing value, not the outer file; reject self-references/overlapping payload ranges. Depth limits apply before recursion, not after.

Only the storage envelope uses overflow references in baseline v1: user-visible/exported OryqDoc must have `HAS_OVERFLOW_POINTERS=0` and cannot contain tag `0xF`. Oversized documents are stored as complete overflow chains, materialized into budgeted owned storage for reads; selective field overflow is deferred. Decimal128 uses IEEE decimal128 BID with low-order bytes first, with normalization/parser fixtures required at M3; no host struct casts.

Physical document patch paths are top-level names only (document-model.md Section 4.4). Sort entries by `(path_hash64, full path bytes)`; reject duplicates and inconsistent hashes. Replacement payloads are standalone type payloads: nested array/document offsets have their own origins, not the page/patch origin. Tombstones have type Null and zero value length. The bloom mask ORs two bits from XXH3-64 of the top-level UTF-8 name: positions `hash & 63` and `(hash >> 6) & 63` (the bits may coincide). Hash collisions always require full name comparison. Patch chains are acyclic, page-local, visit exactly `delta_count` entries and terminate at `0xFFFF`; directory slots include base, patch and tombstone entries, not just live documents.

## 6. Error Handling

Malformed headers, mismatched `database_uuid` between `.oryq` and `.oryq-wal`, invalid UTF-8 field names, out-of-order `OryqDoc` directories, or XXH3-64 checksum mismatches abort ordinary reads and return `OryqError::Corruption(CorruptionError)` containing the exact file source, page/frame ID, byte offset, and checksum values.

Truncated final WAL records are handled only by the explicit recovery-tail rules in transactions.md; a complete invalid frame fails closed. This conservative policy can require manual diagnosis after a torn complete-length tail and must be tested/documented rather than claimed as unconditional recovery.

## 7. Concurrency Behavior

* `.oryq` Page 0 shadow headers are written only by the active checkpointer while holding the checkpoint lock.
* `.oryq-wal` frames are appended sequentially by the single active group-commit leader; concurrent readers read committed frames at offsets `< committed_wal_tail` using positioned `pread` / `seek_read` without mutating file offsets.
* `.oryq-shm` memory is accessed exclusively via raw pointer atomic instructions (`core::sync::atomic`), never via Rust `&[u8]` or `&mut [u8]` references.

## 8. Performance Characteristics

* **Header Verification:** Hashes two 256-byte slots; latency is unmeasured and belongs in the benchmark results.
* **Page Verification:** Hashes one configured-size page on load; validated immutable cache hits need no repeat hash. No nanosecond claim is verified.
* **WAL Write Reduction:** Delta savings depend on changed byte ranges, frame overhead and full-image frequency; measure actual encoded bytes rather than assuming a fixed 64-byte frame or 99% reduction.

## 9. Known Limitations

* Changing `page_size` after database creation requires exporting/compacting into a new `.oryq` file.
* `.oryq-shm` supports up to 128 concurrent OS processes per database file.

## 10. Testing Methodology

* **Deterministic Round-Trip & Golden File Tests:** `tests/portability/endian_and_format_compat.rs` verifies byte-for-byte identity of encoded headers, pages, `OryqDoc` buffers, and WAL frames against golden byte fixtures.
* **Bit-Flip & Truncation Fuzzing:** `tests/corruption/bitflip_and_forensics.rs` and `fuzz/fuzz_targets/fuzz_page_and_wal.rs` mutate arbitrary bits and truncate files at every byte boundary to verify zero panics and 100% checksum/bounds detection.
