# `oryq-document` Test Fixtures & Invariant Coverage

This directory documents the test cases and deterministic binary/JSON fixtures used to verify the `oryq-document` module (`W02`).

## Test Suites

- `tests/roundtrip.rs`:
  - Encodes and decodes all 12 v1 `OryqTypeTag` variants (`Null`, `Bool`, `Int64`, `Float64`, `Decimal128`, `String`, `Binary`, `ObjectId`, `Uuid`, `DateTimeMillis`, `Document`, `Array`).
  - Verifies `DocumentView` `O(log k)` field lookup, dotted `FieldPath` resolution across nested documents and array indices, `ArrayView` `O(1)` random access, and unaligned byte-offset reads (`&padded[1..]`).
  - Verifies Extended JSON (`$oid`, `$uuid`, `$date`, `$binary`, `$numberLong`, `$numberDouble`, `$numberDecimal`) round-tripping and `Decimal128` special values (`NaN`, `+Infinity`, `-Infinity`, `-0.00`).
  - Verifies `BudgetHandle` reservation accounting and `ResourceExhausted` rollback.
- `tests/ordering.rs`:
  - Verifies total canonical cross-type ordering across all 12 types.
  - Verifies exact numeric comparisons across `Int64`, `Float64`, and `Decimal128`: `-0.0 == +0.0 == 0 == Decimal("-0.00")`, `NaN == NaN < -Inf`, `10 == 10.0 == Decimal("10.0")`, `Decimal("0.1") < 0.1_f64`, precision preservation above `2^53` and at `i64::MIN`/`i64::MAX`, and subnormal `f64` ordering.
- `tests/malformed.rs`:
  - Verifies strict `validate_document` rejection of truncated buffers, unknown format versions, mismatched `total_len`, `HAS_OVERFLOW_POINTERS` (`Corruption`), `0xF` `OverflowRef` (`Corruption`), reserved flags/tags (`Corruption`), corrupted `xxh3_64` hash prefixes, unsorted directory entries, overlapping heap spans, invalid UTF-8, NUL bytes in field names, duplicate keys, nesting depth `> 128`, and invalid `FieldPath` strings.

## Regenerating / Adding Fixtures

All W02 fixtures are currently synthesized deterministically inside the integration test files so there are no opaque binary blobs in Git. When cross-version wire-compatibility fixtures are added in later milestones (`W09+`), store frozen `.oryqdoc` binaries in this directory and document their schema version and generator command here.
