# OryqDB Query, Indexing & Aggregation Guide (`v0.11.0`)

## 1. Filter Expressions (`find` / `find_json`)

OryqDB supports MongoDB-style JSON filter predicates evaluated over canonical `OryqDoc` values (with exact cross-type ordering and exact integer/float numeric equality):

| Operator | Example | Semantics |
| :--- | :--- | :--- |
| Implicit equality | `{"status": "open"}` | Exact value match (or array element match) |
| `$eq`, `$ne` | `{"age": {"$ne": 0}}` | Exact equality / inequality |
| `$gt`, `$gte`, `$lt`, `$lte` | `{"score": {"$gte": 80, "$lt": 100}}` | Canonical range comparison |
| `$in`, `$nin` | `{"region": {"$in": ["nz", "au"]}}` | Set membership |
| `$exists` | `{"deleted_at": {"$exists": false}}` | Field presence check |
| `$and`, `$or`, `$not` | `{"$or": [{"vip": true}, {"score": {"$gte": 95}}]}` | Logical combinators |

## 2. Projections, Sorting & Cursors

```rust
let rows = col
    .find_json(
        Some(r#"{"active": true}"#),
        Some(r#"{"name": 1, "score": 1}"#), // inclusion projection (_id included unless "_id": 0)
        Some(r#"{"score": -1, "name": 1}"#), // multi-key sort
        10,                                  // skip
        Some(25),                            // limit
    )?
    .collect_all();
```

### Inspecting Query Plans (`explain_analyze`)

Use `ReadCollection::explain_analyze(&FindOptions { ... })` to inspect the chosen `AccessPlan` (`PrimaryIdLookup`, `SecondaryEqScan`, `SecondaryRangeScan`, or `CollectionScan`), rows scanned, and rows returned.

## 3. Indexes

| Index Kind | Creation API | Notes |
| :--- | :--- | :--- |
| **Primary (`_id_`)** | Automatic on `create_collection` | Mandatory unique `_id -> RecordId` index |
| **Secondary** | `col.create_index("email", SortOrder::Asc, unique, sparse, Some("email_idx"))` | Single or nested dot-path (`"addr.zip"`) |
| **Compound / Multi-Key** | `col.create_compound_index(&[("org", Asc), ("ts", Desc)], unique, sparse, Some("org_ts"))` | Up to 16 fields, `R=16` prefix compression, parallel-array prohibition |
| **Full-Text (`--features fts`)** | `col.create_fts_index(&["title", "body"], Some("doc_fts"))` | Okapi BM25 inverted index (`write_bits = [0]`), queried via `search_fts` |
| **Vector (`--features vec`)** | `col.create_vector_index("emb", opts, Some("emb_cos"))` | Cosine / L2 / DotProduct, `F32` or `Int8` quantization (`write_bits = [1]`), queried via `search_vector` |

> **Planner Safety:** Pluggable indexes (`index.fts`, `index.vec`) are never selected by the standard equality/range query planner (`plan_find`). Sparse indexes are never used for `$exists: false` or `null` equality predicates.

## 4. Aggregation Pipelines (`aggregate_json`)

OryqDB executes memory-bounded, spillable JSON aggregation pipelines within a pinned MVCC read snapshot:

- **`$match`:** Filters documents using the full filter AST.
- **`$project`:** Field inclusion/exclusion and computed renaming.
- **`$unwind`:** Expands array fields into one document per element.
- **`$lookup`:** Left outer join against another collection in the same snapshot (`from`, `localField`, `foreignField`, `as`).
- **`$group`:** Groups by `_id` expression with accumulators (`$sum`, `$avg`, `$min`, `$max`, `$count`, `$push`, `$addToSet`).
- **`$sort`:** Memory-bounded external merge sort that spills temporary runs via RAII `SpillManager` when exceeding the query memory budget (automatically cleaned up on completion or error).
- **`$skip` / `$limit`:** Deterministic pagination.

### Example Pipeline

```json
[
  {"$match": {"active": true}},
  {"$unwind": "$tags"},
  {"$group": {"_id": "$tags", "total_score": {"$sum": "$score"}, "members": {"$count": {}}}},
  {"$sort": {"total_score": -1}},
  {"$limit": 5}
]
```

See [docs/query-language.md](../query-language.md) and [docs/indexing.md](../indexing.md) for the complete formal specifications.
