# OryqDB Developer Quickstart (`v0.11.0`)

OryqDB is an embedded, serverless, single-file NoSQL document database written in Rust (`MIT` license) with ACID MVCC transactions, B+ tree indexing, aggregation pipelines, optional full-text (`oryq-fts`) & vector (`oryq-vec`) search, a native C ABI v1.1 (`include/oryq.h`), Python `ctypes` bindings, and the `oryq` CLI.

## 1. Rust (`oryq-core`)

Add `oryq-core` to your `Cargo.toml` (optional extensions `--features fts`, `--features vec`, or `--features fts,vec` are disabled by default):

```toml
[dependencies]
oryq-core = "0.11.0"
# Optional search extensions:
# oryq-core = { version = "0.11.0", features = ["fts", "vec"] }
```

### Basic CRUD, Indexing & Queries

```rust
use oryq_core::{Database, DatabaseConfig, SortOrder, Value};

fn main() -> oryq_core::Result<()> {
    let db = Database::open("app.oryq", DatabaseConfig::default())?;

    // Exclusive ACID write transaction (rolls back automatically if dropped without commit)
    let mut wtx = db.begin_write()?;
    wtx.create_collection("users")?;
    {
        let mut col = wtx.collection("users")?;
        col.create_index("email", SortOrder::Asc, true, false, Some("email_uniq"))?;
        col.insert_json(r#"{"_id": 1, "name": "Ada", "email": "ada@example.com", "score": 98.5}"#)?;
        col.insert_json(r#"{"_id": 2, "name": "Grace", "email": "grace@example.com", "score": 95.0}"#)?;
    }
    wtx.commit()?;

    // Snapshot-isolated read transaction (never blocks concurrent readers or writers)
    let rtx = db.begin_read()?;
    let col = rtx.collection("users")?;
    let rows = col
        .find_json(Some(r#"{"score": {"$gte": 95}}"#), None, Some(r#"{"score": -1}"#), 0, Some(10))?
        .collect_all();
    assert_eq!(rows.len(), 2);

    // Point lookup by _id
    let ada = col.find_by_id(&Value::Int64(1))?.expect("user 1 exists");
    println!("{}", oryq_core::to_json_string(&ada.view())?);
    Ok(())
}
```

### High-Throughput Group Commit (`commit_batch` & `submit_batched`)

```rust
// Explicit ordered batch: per-item savepoint rollback + single WAL fsync
let report = db.commit_batch(vec![
    Box::new(|wtx: &mut oryq_core::WriteTransaction<'_>| {
        wtx.collection("users")?.insert_json(r#"{"_id": 3, "email": "c@example.com"}"#)?;
        Ok(3)
    }) as Box<dyn FnOnce(&mut oryq_core::WriteTransaction<'_>) -> oryq_core::Result<i64>>,
])?;

// Concurrent threads: leader-follower group commit coalesces simultaneous callers into 1 fsync
let (doc_id, receipt) = db.submit_batched(|wtx| {
    wtx.collection("users")?.insert_json(r#"{"_id": 4, "email": "d@example.com"}"#)?;
    Ok(4i64)
})?;
```

### Optional Search Extensions (`--features fts,vec`)

- **Full-Text Search (`--features fts`):** `col.create_fts_index(&["title", "body"], Some("text_fts"))?` and `rtx.collection("docs")?.search_fts("text_fts", "embedded database", 10)?` (Okapi BM25).
- **Vector Similarity Search (`--features vec`):** `col.create_vector_index("embedding", VectorIndexOptions::new(384, DistanceMetric::Cosine)?, Some("emb_cos"))?` (or `VectorQuantization::Int8` for 4× smaller footprint) and `rtx.collection("docs")?.search_vector("emb_cos", &query_vec, 10)?`.

## 2. Python (`bindings/python`)

```bash
cargo build --release -p oryq-ffi
python3 -m unittest discover -s bindings/python/tests -v
```

See [bindings/python/README.md](../../bindings/python/README.md) for usage examples.

## 3. C99 & C++17 (`include/oryq.h`)

- C99 example: [bindings/c/quickstart.c](../../bindings/c/quickstart.c)
- C++17 RAII example: [bindings/cpp/quickstart.cpp](../../bindings/cpp/quickstart.cpp)
- Full ABI reference: [C ABI Guide](c-abi-guide.md)

## 4. CLI (`oryq`)

```bash
cargo build --release -p oryq-cli
./target/release/oryq info app.oryq
./target/release/oryq collections app.oryq
./target/release/oryq insert app.oryq users '{"_id": 10, "name": "CLI User"}'
./target/release/oryq find app.oryq users --filter '{"_id": {"$gte": 1}}'
./target/release/oryq integrity-check app.oryq
./target/release/oryq backup app.oryq app_backup.oryq
```

## Next Guides

- [Query, Indexing & Aggregation Guide](query-and-aggregation.md)
- [C ABI v1.1 Developer Guide](c-abi-guide.md)
- [Operations, Backup & Recovery Runbook](operations-and-recovery.md)
