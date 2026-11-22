# `oryqdb` — Official Python Bindings

Zero-dependency Python `ctypes` bindings for **OryqDB** over the stable C ABI v1.1 (`liboryq` + `include/oryq.h`).

## Quickstart

```python
from oryqdb import Database

with Database.open("app.oryq") as db:
    with db.write_txn() as wtx:
        wtx.create_collection("users")
        wtx.create_index("users", "email", unique=True)
        wtx.insert("users", {"_id": 1, "name": "Ada", "email": "ada@example.com", "score": 98.5, "active": True})
        wtx.commit()

    with db.read_txn() as rtx:
        with rtx.find("users", {"active": True}) as cur:
            for doc in cur:
                print(doc)

    assert db.integrity_check()
```

## Library Discovery

`oryqdb` locates `liboryq` automatically in the following order:
1. Explicit `lib_path` passed to `Database.open(..., lib_path=...)` or `load_library(path)`.
2. `ORYQ_LIB_PATH` environment variable.
3. Workspace `target/release/` and `target/debug/` directories (for local source builds).
4. Standard OS shared-library search paths (`liboryq.so`, `liboryq.dylib`, `oryq.dll`).

## Running Tests

```bash
cargo build -p oryq-ffi
python3 -m unittest discover -s bindings/python/tests -v
```
