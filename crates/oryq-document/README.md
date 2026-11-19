# `oryq-document`

`OryqDoc` v1 binary codec, zero-copy views, canonical cross-type comparator, field path validator, Extended-JSON conversion, update operators, and state-normalized physical delta patches for **OryqDB** (`W02`, `W06`).

## Responsibilities
- **`types` & `path`:** `OryqTypeTag`, `ObjectId`, `Uuid`, IEEE 754-2008 BID `Decimal128`, `Value`, `ValueView<'a>`, `DocumentBuf`, and `FieldPath` (`MAX_PATH_DEPTH = 128`).
- **`codec`, `view` & `array`:** `OryqDoc` v1 encoder/validator (`encode_document`, `validate_document`), `O(log k)` `DocumentView<'a>`, and `O(1)` `ArrayView<'a>`.
- **`compare` & `json`:** Exact rational cross-type `compare_values` and Extended-JSON (`$oid`, `$uuid`, `$date`, `$binary`, `$numberLong`, `$numberDouble`, `$numberDecimal`) conversion.
- **`updates`:** Owner-local `UpdateRegistry`, `$set`, `$unset`, `$inc`, `$push`, `$pull`, `_id` immutability, ancestor/descendant path-conflict checks, checked numeric overflow/precision rules, and before/after `NormalizedMutation`.
- **`patch`:** State-normalized top-level binary delta patches (`encode_physical_patch` / `decode_physical_patch`), 2-hash 64-bit bloom filtering (`resolve_path_with_patches`), and pure stateless `compact_document`.

## Dependencies
- Internal: `oryq-types` only (completely independent of `oryq-engine`).
- External: `serde_json`, `xxhash-rust`.
- Safety: `#![forbid(unsafe_code)]`.

## Verification
```bash
cargo test -p oryq-document
cargo run -p xtask -- modules document
```
