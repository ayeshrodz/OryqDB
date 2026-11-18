# `oryq-document`

Binary `OryqDoc` v1 codec, zero-copy `DocumentView` / `ArrayView`, exact rational comparator, canonical dotted `FieldPath`, and Extended-JSON conversion (see [docs/modules/document.md](../../docs/modules/document.md)).

## Verification
```bash
cargo test -p oryq-document
cargo run -p xtask -- modules document
```
