# Official Go (`cgo`) Bindings for OryqDB

Zero-dependency Go `cgo` bindings for **OryqDB** over **C ABI v1.2** (`include/oryq.h` + `liboryq`).

## Build `liboryq` & Run Tests

```bash
# 1. Build the native C ABI library (add `--features fts,vec` for hybrid search)
cargo build -p oryq-ffi --features fts,vec

# 2. Run Go tests
cd bindings/go
ORYQ_EXPECT_HYBRID=1 go test -v ./...
```

## Quick Example

```go
db, err := oryqdb.OpenInMemory() // or oryqdb.Open("app.oryq")
if err != nil {
    panic(err)
}
defer db.Close()

wtx, _ := db.BeginWrite()
_ = wtx.CreateCollection("users")
_ = wtx.InsertJSON("users", `{"_id": 1, "name": "Ada"}`)
_ = wtx.Commit()
```

## License

MIT
