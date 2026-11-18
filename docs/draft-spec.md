# PROJECT: Build a High-Performance, Cross-Platform, Embedded NoSQL Database Engine in Rust

> **Review status (2026-11-18):** Historical requirements and brainstorming input. Reviewed subsystem contracts and delivery gates take precedence; see [specification authority](designs/oryqdb.md#specification-authority--readiness). This document is not evidence of implemented or verified behavior.

## 0. YOUR ROLE AND RESPONSIBILITIES

You are acting as a team of highly experienced systems engineers, database architects, Rust developers, storage-engine specialists, performance engineers, and software security researchers.

Your combined expertise includes:

- Database internals and storage-engine architecture.
- Rust systems programming and memory safety.
- C interoperability and application binary interfaces.
- Embedded database design.
- B-tree and LSM-tree storage architectures.
- Write-ahead logging and crash recovery.
- Transaction processing and ACID guarantees.
- Concurrency control and transaction isolation.
- Binary serialization formats.
- Document-oriented database systems.
- Query parsing, planning, and optimization.
- Memory allocation and cache management.
- Filesystem durability and platform-specific I/O.
- Performance profiling and benchmarking.
- Cross-platform compilation and packaging.
- Fuzz testing, property-based testing, and fault injection.
- Open-source software architecture and maintenance.

Your task is to research, architect, design, implement, and thoroughly test a new embedded NoSQL database engine written primarily in Rust.

This is intended to become a serious, production-quality open-source database project.

**This must not be a toy database, a JSON file wrapper, an in-memory HashMap with serialization, or an experimental prototype presented as production-ready software.**

Build a genuine database engine with persistent storage, indexing, transactions, query processing, and recovery mechanisms.

However, do not sacrifice correctness or maintainability by attempting to implement everything at once.

You must prioritize engineering quality, incremental delivery, and demonstrable correctness.

---

# 1. PROJECT VISION

Build a database that combines three fundamental characteristics:

> SQLite's portability and simplicity, MongoDB-style document storage, and a high-performance Rust engine.

The database must be:

1. Lightweight.
2. Embedded.
3. Serverless.
4. Portable.
5. Persistent.
6. Transactional.
7. Memory-efficient.
8. High-performance.
9. Cross-platform.
10. Accessible from multiple programming languages.
11. Easy to distribute and integrate.
12. Suitable for real applications.

The database should operate as a native library that applications can embed directly.

It must not require:

- A database server.
- A separate operating-system service.
- A Java Virtual Machine.
- A .NET runtime.
- A JavaScript runtime.
- A garbage-collected runtime.
- An external database.
- A network connection.
- Docker or another container runtime.

The database must work fully offline.

The long-term goal is to establish a credible alternative to traditional embedded relational databases for applications that benefit from a document-oriented data model.

The database is not required to implement SQL.

It should instead provide a clean, powerful, and expressive document API.

---

# 2. PRIMARY REFERENCE IMPLEMENTATION

Study the following open-source project in depth:

Repository:

https://github.com/litedb-org/LiteDB

Project:

LiteDB — A .NET NoSQL Document Store in a Single Data File.

License:

MIT.

Official documentation:

https://www.litedb.org/docs/

LiteDB will serve as the primary reference implementation for understanding the functionality, storage architecture, and developer experience of an embedded document database.

## 2.1 Repository preparation

Clone the official repository into a dedicated research directory.

Example:

```bash
git clone https://github.com/litedb-org/LiteDB.git references/LiteDB
```

Before conducting the analysis:

1. Verify the repository URL and upstream identity.
2. Identify the latest stable release.
3. Identify the primary development branch.
4. Record the exact commit SHA being analyzed.
5. Identify important differences between the stable release and development branch.
6. Read the repository README.
7. Read the license.
8. Examine project documentation.
9. Inspect the source code.
10. Examine tests.
11. Inspect build configuration.
12. Identify external dependencies.
13. Review known architectural limitations.
14. Look for existing benchmarks and performance discussions.
15. Identify features that are stable, experimental, incomplete, or deprecated.

Do not assume all features mentioned in documentation exist in the same branch or release.

Use the source code as the primary evidence of actual functionality.

Keep the reference repository separate from the new implementation.

Do not modify LiteDB source files.

## 2.2 Analyze LiteDB thoroughly

Investigate the following subsystems.

### Storage engine

Identify:

- Database file organization.
- File headers.
- Page layout.
- Page types.
- Document storage.
- Free-page management.
- Overflow-page handling.
- Allocation strategies.
- File growth.
- Storage fragmentation.
- Compaction mechanisms.
- Memory mapping, if used.
- Buffer management.
- Disk I/O mechanisms.

### Transaction system

Investigate:

- Transaction boundaries.
- Transaction isolation.
- Lock acquisition.
- Commit behavior.
- Rollback behavior.
- Journaling or write-ahead logging.
- Recovery after interruption.
- Durability guarantees.
- Reader and writer concurrency.
- Multiple-thread behavior.
- Multiple-process behavior.

### Document model

Investigate:

- BSON support.
- Supported data types.
- Nested documents.
- Arrays.
- Document identifiers.
- Serialization.
- Deserialization.
- Maximum document sizes.
- Numeric comparison semantics.
- Null and missing-field behavior.
- Document validation.

### Indexing

Investigate:

- Index data structures.
- Primary indexes.
- Secondary indexes.
- Unique indexes.
- Compound indexes, if supported.
- Index creation.
- Index deletion.
- Index updates.
- Query execution using indexes.
- Index storage overhead.
- Index consistency after crashes.

### Query engine

Investigate:

- Query parsing.
- Query representation.
- Filtering.
- Comparison operators.
- Logical operators.
- Sorting.
- Pagination.
- Projection.
- Aggregation.
- Query planning.
- Query execution.
- Explain-plan support.
- Full-text search, if available.

### Public APIs

Investigate:

- Database opening.
- Collection creation.
- Document insertion.
- Document retrieval.
- Document updates.
- Deletion.
- Transactions.
- Index management.
- Configuration.
- Error handling.

### Testing

Analyze how LiteDB tests:

- Data correctness.
- Transactions.
- Concurrent operations.
- Recovery.
- Serialization.
- Index integrity.
- Query semantics.
- File persistence.

Use these findings to identify both useful features and architectural opportunities.

---

# 3. RESEARCH DELIVERABLES

Before implementing the storage engine, produce these documents:

```text
docs/research/
├── litedb-architecture.md
├── litedb-feature-inventory.md
├── litedb-storage-analysis.md
├── litedb-query-analysis.md
├── litedb-concurrency-analysis.md
├── litedb-performance-analysis.md
├── competitor-comparison.md
├── architecture-decisions.md
└── implementation-roadmap.md
```

The documents must distinguish between:

- Observed behavior.
- Verified implementation details.
- Measured limitations.
- Hypotheses.
- Proposed improvements.

Do not present guesses as established facts.

For important findings, reference specific source files, functions, or commit identifiers.

## 3.1 Competitor research

Also investigate the architecture and publicly documented capabilities of:

1. SQLite.
2. SQLite JSON functions.
3. SQLite JSONB.
4. UnQLite.
5. LiteDB.
6. RocksDB.
7. LMDB.
8. redb.
9. ObjectBox.
10. Other relevant embedded document databases discovered during research.

The purpose is not to copy their implementations.

The purpose is to understand established storage techniques and identify opportunities for differentiation.

Pay special attention to SQLite.

SQLite provides extremely mature transactional storage and JSON capabilities.

Our database must therefore offer meaningful advantages beyond accepting JSON documents.

---

# 4. FUNDAMENTAL ARCHITECTURAL REQUIREMENT

The core engine must be implemented in Rust.

The public integration layer must expose a stable C-compatible ABI.

Do not build two separate database engines.

The intended architecture is:

```text
                 USER APPLICATIONS
                         |
        +----------------+----------------+
        |                |                |
      Rust             Python             Go
        |                |                |
        +----------------+----------------+
                         |
                 Public API Layer
                         |
                  C-Compatible ABI
                         |
               +-------------------+
               | Rust Database Core|
               +-------------------+
                         |
       +-----------------+-----------------+
       |                 |                 |
   Document           Query            Index
    Engine            Engine           Engine
       |                 |                 |
       +-----------------+-----------------+
                         |
                 Transaction Manager
                         |
                  Storage Manager
                         |
                    Database File
```

Important:

Native Rust consumers should be able to use an ergonomic Rust API directly.

C and other foreign-language consumers should use the C ABI.

Do not unnecessarily route native Rust calls through the C ABI.

The implementation must not require a managed runtime.

Keep platform-specific logic isolated behind clearly defined interfaces.

---

# 5. STORAGE ENGINE DESIGN

The storage engine is the most important subsystem.

Prioritize:

1. Correctness.
2. Durability.
3. Crash safety.
4. Consistency.
5. Predictable performance.
6. Memory efficiency.
7. Portability.

Performance must never be achieved by silently weakening durability or transaction guarantees.

## 5.1 Storage architecture investigation

Evaluate at least three possible designs:

### Option A: B+ tree with write-ahead logging

Advantages may include:

- Efficient indexed lookups.
- Ordered iteration.
- Predictable transactional behavior.
- Familiarity from traditional databases.

### Option B: Log-structured merge tree

Advantages may include:

- Efficient sequential writes.
- Write-heavy workload optimization.
- Batched storage operations.

Disadvantages to investigate include:

- Write amplification.
- Compaction overhead.
- Read amplification.
- Storage amplification.

### Option C: Copy-on-write B+ tree

Advantages may include:

- Snapshot-friendly architecture.
- Potentially simpler multi-version reads.
- Atomic root transitions.

Disadvantages to investigate include:

- Page rewrite overhead.
- Free-space reclamation.
- Long-lived snapshot behavior.

Evaluate these approaches using:

- Expected workload characteristics.
- Implementation complexity.
- Recovery complexity.
- Read performance.
- Write performance.
- Memory consumption.
- Storage efficiency.
- Concurrent access requirements.
- Cross-platform filesystem behavior.

Provide a written architecture decision record.

**The initial preference is a page-oriented B+ tree with write-ahead logging, but this is not mandatory if the research demonstrates a better design.**

Do not combine several competing storage architectures without a clear reason.

## 5.2 Single-file persistence

The database should store committed user data in one portable database file.

Example:

```text
application.db
```

Temporary files may be used when necessary for safe transactions, locking, recovery, or maintenance.

A write-ahead log or journal sidecar is acceptable if required.

Do not incorrectly claim that a single-file database can never create temporary files.

The desired user experience is that the primary persistent database can be copied, backed up, and moved between supported systems.

Define a safe backup procedure, particularly when the database is open or transactions are active.

## 5.3 Database file format

Create an explicitly versioned binary format.

Include:

- Magic bytes.
- Format version.
- Feature flags.
- Page size.
- Database identifier.
- Root-page references.
- Metadata references.
- Integrity information.
- Reserved compatibility fields.

Do not serialize Rust structs directly to disk using their in-memory layout.

Define every on-disk field explicitly.

Specify:

- Integer widths.
- Byte order.
- String encoding.
- Alignment.
- Null representation.
- Page identifiers.
- Checksums.
- Maximum supported sizes.
- Format compatibility rules.

The database must behave consistently across:

- Linux.
- Windows.
- macOS.
- x86-64.
- ARM64.

Do not rely on platform-native structure layouts.

Provide a file-format specification:

```text
docs/file-format.md
```

## 5.4 Page management

Implement:

- Page allocation.
- Page deallocation.
- Free-page tracking.
- Page reads.
- Page writes.
- Overflow handling.
- Page metadata.
- Page validation.
- Page checksums where appropriate.

Support configurable page sizes only if justified by benchmarks and format design.

Otherwise, begin with one carefully selected page size.

## 5.5 Storage integrity

The engine must detect malformed or corrupted structures.

Validate:

- Page boundaries.
- Record lengths.
- Pointer references.
- Page types.
- File headers.
- Tree invariants.
- Checksums.
- Allocation metadata.

Never blindly trust bytes read from disk.

The engine must return meaningful corruption errors rather than invoking undefined behavior or silently accepting invalid data.

---

# 6. DOCUMENT MODEL

The database must be document-oriented.

The developer experience should resemble familiar MongoDB-style document operations while remaining lightweight and embedded.

Example document:

```json
{
  "_id": 1001,
  "name": "Alice",
  "email": "alice@example.com",
  "profile": {
    "country": "New Zealand",
    "age": 30
  },
  "tags": ["developer", "rust"],
  "active": true
}
```

## 6.1 Required document capabilities

Support:

- Nested documents.
- Arrays.
- Strings.
- Boolean values.
- Null values.
- Integer types.
- Floating-point types.
- Binary values.
- Dates and timestamps.
- Unique document identifiers.

Define the document type system explicitly.

Decide whether the internal representation uses:

- BSON.
- JSONB-style binary encoding.
- A custom typed binary document format.

Research the trade-offs before deciding.

The internal format should aim to provide:

- Efficient serialization.
- Efficient deserialization.
- Compact representation.
- Fast field lookup.
- Predictable memory usage.
- Forward compatibility.

## 6.2 JSON interoperability

Users must be able to insert and retrieve ordinary JSON documents.

Example:

```rust
let db = Database::open("app.db")?;

let users = db.collection("users")?;

users.insert(json!({
    "name": "Alice",
    "age": 30,
    "active": true
}))?;
```

The engine must provide deterministic conversion between JSON-compatible values and its internal document representation.

For BSON-specific types that JSON cannot represent directly, define an extended JSON representation.

Do not silently lose numeric precision, binary data, or date information.

## 6.3 Document identifiers

Support a unique primary identifier:

```json
{
  "_id": "user-1001"
}
```

Investigate support for:

- Strings.
- Signed integers.
- UUIDs.
- Automatically generated identifiers.

Do not introduce incompatible identifier semantics without documenting them.

## 6.4 Document updates

Support complete document replacement and partial updates.

Example:

```rust
users.update(
    query!({"_id": "user-1001"}),
    update!({"$set": {"active": false}})
)?;
```

Initial update operators should include:

- `$set`
- `$unset`
- `$inc`

Additional operators can be introduced after the basic implementation is correct.

Document how indexes are maintained when indexed fields change.

---

# 7. INDEXING ENGINE

Indexes are essential for competitive performance.

Implement a genuine persistent indexing system.

Do not simulate indexes using repeated collection scans.

## 7.1 Primary indexes

Every collection must provide efficient document lookup by its primary identifier.

Primary-key uniqueness must be enforced transactionally.

## 7.2 Secondary indexes

Support indexes on document fields.

Example:

```rust
users.create_index("email", IndexOptions::unique())?;
```

Support:

- Indexed equality searches.
- Ordered range searches.
- Unique constraints.
- Index deletion.
- Index persistence.
- Index recovery.
- Index consistency during updates.

## 7.3 Nested-field indexes

Support indexed access to nested fields.

Example:

```rust
users.create_index(
    "profile.country",
    IndexOptions::default()
)?;
```

Document how missing fields, null values, and arrays are represented in indexes.

## 7.4 Compound indexes

Evaluate compound indexes after single-field indexing is stable.

Example:

```text
(country, age)
```

Do not claim compound-index support until it is fully implemented and tested.

## 7.5 Index integrity

Test:

- Duplicate key rejection.
- Insert rollback.
- Update rollback.
- Delete rollback.
- Index rebuilds.
- Interrupted index creation.
- Crash recovery.
- Concurrent index reads.
- Index corruption detection.

Every index must remain consistent with committed document data.

---

# 8. QUERY ENGINE

Build a document-oriented query engine.

The initial API should feel natural to developers familiar with document databases.

Example:

```rust
let results = users.find(
    query!({
        "age": {"$gte": 18},
        "active": true
    })
)?;
```

## 8.1 Query operators

Implement in stages.

### Initial operators

- Equality.
- Inequality.
- Greater than.
- Greater than or equal.
- Less than.
- Less than or equal.
- Logical AND.
- Logical OR.

### Later operators

- `$in`
- `$nin`
- `$exists`
- `$not`
- Array membership.
- Nested-field expressions.
- Pattern matching.

## 8.2 Query functionality

Support:

- Filtering.
- Projection.
- Sorting.
- Limit.
- Offset.
- Counting.
- Collection iteration.
- Cursor-based results.

Do not load entire result sets into memory unnecessarily.

Use streaming or cursor-based APIs where appropriate.

## 8.3 Query planning

The engine should distinguish between:

- Primary-key lookup.
- Secondary-index lookup.
- Indexed range scan.
- Full collection scan.

Introduce a query planner that selects available access paths.

Avoid building a highly complex optimizer before simpler query execution is correct.

Provide an explain facility.

Example:

```rust
let plan = users.explain(
    query!({"email": "alice@example.com"})
)?;
```

The explanation should identify the selected index and execution strategy.

## 8.4 Aggregation

Aggregation is a later milestone.

Investigate:

- Count.
- Sum.
- Average.
- Minimum.
- Maximum.
- Grouping.

Do not prioritize advanced aggregation over transaction correctness or persistence.

---

# 9. TRANSACTION ENGINE

The database must support genuine ACID transactions.

Define precisely what ACID means for this implementation.

## 9.1 Atomicity

A transaction must either commit entirely or have no visible effect.

Partial updates must not become visible following failure.

## 9.2 Consistency

Committed transactions must preserve:

- Primary-key uniqueness.
- Index invariants.
- Valid page references.
- Valid document records.
- Storage metadata consistency.

## 9.3 Isolation

Define and implement a documented isolation model.

For the initial architecture, a reasonable target is:

**Multiple concurrent readers with one active writer.**

Evaluate snapshot isolation or other mechanisms based on the selected storage architecture.

Do not claim serializable isolation unless the implementation actually guarantees it.

Document:

- Reader visibility.
- Writer locking.
- Transaction conflicts.
- Lock timeouts.
- Thread safety.
- Multiple-process access.

## 9.4 Durability

Committed transactions must survive supported failure scenarios, subject to documented filesystem and hardware guarantees.

Implement:

- A defined commit protocol.
- Correct write ordering.
- Appropriate synchronization calls.
- Recovery logic.
- Transaction-log validation.
- Interrupted-commit handling.

Do not equate flushing language-level buffers with durable persistence.

Investigate platform-specific requirements for:

- Linux.
- Windows.
- macOS.

## 9.5 Public transaction API

Target an API similar to:

```rust
let mut tx = db.begin_transaction()?;

tx.collection("accounts")?.update(
    query!({"_id": "account-1"}),
    update!({"$inc": {"balance": -100}})
)?;

tx.collection("accounts")?.update(
    query!({"_id": "account-2"}),
    update!({"$inc": {"balance": 100}})
)?;

tx.commit()?;
```

The exact API may differ depending on Rust borrowing and transaction lifetime requirements.

Prefer safe ownership models over superficially convenient syntax.

---

# 10. WRITE-AHEAD LOGGING AND CRASH RECOVERY

If the selected architecture uses write-ahead logging, implement it as a first-class component.

Requirements:

- Explicit WAL record format.
- Transaction identifiers where needed.
- Record validation.
- Checksums.
- Commit markers.
- Recovery ordering.
- Checkpointing.
- Incomplete-record handling.
- Recovery after partial writes.

Test failures at critical points:

1. Before a transaction begins.
2. During data-page modification.
3. During WAL writing.
4. Before a commit marker.
5. After a commit marker.
6. During a checkpoint.
7. During database reopening.

Recovery must not depend on happy-path assumptions.

Build deterministic fault-injection facilities so failures can be reproduced.

The database must never acknowledge a successful durable commit if its documented durability conditions have not been satisfied.

---

# 11. CONCURRENCY AND THREAD SAFETY

Embedded databases are frequently accessed by multithreaded applications.

Support safe concurrent access within the same process.

Initially target:

- Multiple readers.
- One writer.
- Safe connection sharing.
- Explicit transaction isolation.
- Predictable locking.

Avoid unnecessary global locks for operations that can safely run concurrently.

However:

**Do not weaken correctness merely to achieve higher concurrent throughput.**

Multi-process database access must be explicitly addressed.

Either:

1. Implement and test cross-process locking correctly, or
2. Clearly reject unsupported simultaneous access.

Never silently allow unsafe concurrent writers.

Investigate platform-specific locking semantics before claiming multi-process support.

---

# 12. MEMORY MANAGEMENT AND PERFORMANCE

The database should have a small and predictable memory footprint.

Important optimization areas include:

- Buffer allocation.
- Page caching.
- Document decoding.
- Index traversal.
- Memory reuse.
- Temporary allocations.
- Query result iteration.
- Write batching.

## 12.1 Page cache

Design a bounded cache.

Evaluate:

- LRU.
- Clock-based eviction.
- Other suitable replacement algorithms.

Cache size should be configurable.

Document the effect of cache configuration on performance.

## 12.2 Zero-copy and memory mapping

Investigate zero-copy techniques where they provide genuine benefits.

Evaluate memory-mapped I/O against explicit buffered I/O.

Consider:

- Safety.
- File truncation.
- Memory pressure.
- Cache interactions.
- Cross-platform behavior.
- Lifetime management.
- Concurrent writes.

Do not introduce unsafe memory access solely to achieve theoretical performance improvements.

## 12.3 Rust memory safety

Prefer safe Rust.

Any `unsafe` code must:

- Be isolated.
- Have documented invariants.
- Explain why it is necessary.
- Have dedicated tests.
- Receive additional review.

Avoid unsafe code where an acceptable safe alternative exists.

---

# 13. C-COMPATIBLE API

This is a mandatory component.

The database must be embeddable in C applications and accessible from other programming languages through a stable C ABI.

Provide:

- A public C header.
- A static library.
- A shared library.
- Stable opaque handles.
- Explicit ownership rules.
- Error codes.
- Version information.

Example proposed interface:

```c
#ifndef PORTABLE_NOSQL_H
#define PORTABLE_NOSQL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct pndb_database pndb_database;
typedef struct pndb_collection pndb_collection;

typedef enum {
    PNDB_OK = 0,
    PNDB_ERR_INVALID_ARGUMENT,
    PNDB_ERR_IO,
    PNDB_ERR_CORRUPTION,
    PNDB_ERR_NOT_FOUND,
    PNDB_ERR_CONFLICT,
    PNDB_ERR_INTERNAL
} pndb_status;

pndb_status pndb_open(
    const char *path,
    pndb_database **out_database
);

void pndb_close(
    pndb_database *database
);

pndb_status pndb_insert_json(
    pndb_database *database,
    const char *collection,
    const char *json,
    size_t json_length
);

const char *pndb_version(void);

#ifdef __cplusplus
}
#endif

#endif
```

This is an illustrative starting point.

Refine the ABI through design review.

## 13.1 ABI rules

Follow these requirements:

- Use opaque handles.
- Never expose Rust-specific types.
- Never expose Rust struct layouts.
- Use fixed-width integers where appropriate.
- Document pointer ownership.
- Document thread-safety guarantees.
- Define explicit buffer lengths.
- Avoid ambiguous string ownership.
- Provide dedicated functions for releasing returned buffers.
- Define API and ABI versioning.
- Keep error handling explicit.

## 13.2 Error handling

Rust panics must never unwind across the C ABI.

Use appropriate panic containment where possible.

Do not rely on panic catching as a replacement for correct error handling.

Document the behavior of unrecoverable failures and panic-abort configurations.

Do not leak Rust error objects directly into C.

Provide stable error codes and human-readable diagnostics.

## 13.3 Integration testing

Create a real C program that:

1. Opens a database.
2. Creates or accesses a collection.
3. Inserts a document.
4. Retrieves the document.
5. Updates it.
6. Deletes it.
7. Closes the database.
8. Reopens it.
9. Verifies persistence.

Compile and run this program against the actual library.

Do not consider the C API complete simply because the Rust library builds.

---

# 14. CROSS-PLATFORM PORTABILITY

Target the following initial platforms:

| Platform | Architecture |
|---|---|
| Linux | x86-64 |
| Linux | ARM64 |
| Windows | x86-64 |
| Windows | ARM64 |
| macOS | x86-64 |
| macOS | ARM64 |

Prioritize Linux x86-64 as the primary development target.

Expand platform support after tests demonstrate correctness.

Future targets may include:

- Android.
- iOS.
- Embedded Linux.
- WebAssembly, where technically practical.

Do not claim support for a target that has not been built and tested.

## 14.1 Portability requirements

Avoid unnecessary dependencies on:

- Linux-only system calls.
- POSIX-only file locking.
- Windows-only APIs.
- Architecture-specific memory layouts.
- Native-endian serialization.
- Platform-specific path assumptions.

Provide platform-specific implementations only where required.

## 14.2 Build artifacts

Where supported, produce:

```text
Linux:
    libpndb.so
    libpndb.a

Windows:
    pndb.dll
    pndb.lib

macOS:
    libpndb.dylib
    libpndb.a
```

The exact artifact names may change after project naming.

Native library packages must include any required linking documentation and dependencies.

Do not claim that a static library is universally dependency-free merely because the database core is written in Rust.

---

# 15. DEVELOPER EXPERIENCE

The database must be exceptionally easy to integrate.

A developer should be able to open a database and store documents with minimal configuration.

Target an API similar to:

```rust
use portable_nosql::{Database, Document};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::open("application.db")?;

    let users = db.collection("users")?;

    users.insert(Document::from_json(
        r#"{"name":"Alice","age":30}"#
    )?)?;

    let user = users.find_one(
        r#"{"name":"Alice"}"#
    )?;

    println!("{user:?}");

    Ok(())
}
```

The exact API should be designed for idiomatic Rust.

The initial user experience should require:

- No server configuration.
- No external service.
- No database credentials.
- No network ports.
- No separate migration server.

Configuration should be optional unless necessary.

Defaults must be safe and documented.

---

# 16. CLI APPLICATION

Build a lightweight command-line application as a separate component.

Example:

```bash
pndb open application.db
```

Possible commands:

```text
pndb info
pndb collections
pndb collection create users
pndb collection drop users
pndb insert users document.json
pndb find users '{"active":true}'
pndb indexes users
pndb integrity-check
pndb backup
pndb version
```

The CLI must use the actual database engine.

Do not create a separate mock implementation.

A CLI is useful for:

- Development.
- Debugging.
- Inspection.
- Maintenance.
- Testing.
- Backups.

Do not prioritize an elaborate graphical interface before the storage engine is reliable.

---

# 17. PERFORMANCE ENGINEERING

Performance is a primary product objective.

However, performance claims must be based on reproducible measurements.

Do not assume that Rust is automatically faster than C#, C, or C++.

The goal is to identify actual bottlenecks and improve algorithms, data structures, I/O behavior, and memory management.

## 17.1 Benchmark competitors

Benchmark against:

1. LiteDB.
2. SQLite.
3. SQLite using JSON/JSONB where relevant.
4. At least one suitable embedded key-value database.
5. Other comparable document engines where practical.

Use native APIs for each database.

Do not impose an artificial serialization or wrapper disadvantage on competitors.

## 17.2 Benchmark workloads

Create workloads covering:

### Insertion

- 1,000 documents.
- 10,000 documents.
- 100,000 documents.
- 1,000,000 documents.

### Point lookups

- Primary-key lookups.
- Indexed-field lookups.
- Missing-key lookups.

### Document queries

- Equality.
- Range queries.
- Nested-field queries.
- Multi-condition queries.

### Updates

- Full replacement.
- Partial updates.
- Indexed-field updates.

### Deletion

- Single-document deletion.
- Bulk deletion.
- Deletion followed by reinsertion.

### Transactions

- Single-document transactions.
- Batched transactions.
- Rollbacks.
- Durable commits.

### Concurrency

- One reader.
- Multiple readers.
- One writer.
- Mixed reads and writes.

### Storage efficiency

- File size.
- Index overhead.
- Fragmentation.
- Cache efficiency.

## 17.3 Required measurements

Measure:

- Operations per second.
- Average latency.
- p50 latency.
- p95 latency.
- p99 latency.
- Peak memory consumption.
- Allocations where measurable.
- Database file size.
- CPU utilization.
- Write amplification where measurable.
- Recovery duration.

## 17.4 Benchmark fairness

Ensure:

- Equivalent datasets.
- Comparable indexes.
- Equivalent durability settings.
- Comparable transaction sizes.
- Same hardware.
- Documented software versions.
- Warm-cache and cold-cache tests.
- Multiple repetitions.
- Recorded variability.
- Clearly defined measurement methodology.

Do not compare fully durable writes in one engine against unsafe writes in another.

Do not disable journaling or synchronization for competitors unless the comparison explicitly targets nondurable configurations.

Provide:

```text
benchmarks/
├── datasets/
├── workloads/
├── runners/
├── results/
└── README.md
```

## 17.5 Optimization policy

Follow this cycle:

1. Measure.
2. Profile.
3. Identify bottlenecks.
4. Form a hypothesis.
5. Make a focused change.
6. Rerun correctness tests.
7. Rerun benchmarks.
8. Compare results.
9. Keep or revert the change.
10. Document the evidence.

Never optimize purely on intuition.

---

# 18. TESTING REQUIREMENTS

Testing is mandatory from the beginning.

The database must never be considered production-ready solely because its happy-path tests pass.

## 18.1 Unit tests

Test individual components:

- Binary encoding.
- Page parsing.
- Page allocation.
- Tree operations.
- Serialization.
- Query parsing.
- Index updates.
- Transaction state transitions.

## 18.2 Integration tests

Test complete workflows:

- Create database.
- Insert documents.
- Close database.
- Reopen database.
- Retrieve documents.
- Update documents.
- Delete documents.
- Verify indexes.
- Execute transactions.
- Recover after interruption.

## 18.3 Property-based tests

Use property-based testing for invariants such as:

- Serialization round trips.
- Document comparison ordering.
- Index consistency.
- B-tree ordering.
- Transaction atomicity.
- Record visibility.
- Storage allocation correctness.

## 18.4 Fuzz testing

Fuzz:

- Document decoders.
- Query parsers.
- Database headers.
- Page parsing.
- WAL parsing.
- Binary record decoding.
- C API argument handling.

Malformed input must never cause memory unsafety.

## 18.5 Crash testing

Build a subprocess-based crash-testing harness.

Inject termination at controlled storage operations.

After every simulated crash:

1. Reopen the database.
2. Execute recovery.
3. Validate file structures.
4. Validate committed records.
5. Confirm incomplete transactions are not partially visible.
6. Validate indexes.
7. Confirm the engine remains usable.

Distinguish process termination tests from actual power-loss durability testing.

Process termination alone does not prove resistance to every filesystem or hardware failure.

## 18.6 Cross-platform tests

Run tests on:

- Linux.
- Windows.
- macOS.

Include ARM64 testing where execution infrastructure is available.

Record skipped or unsupported targets explicitly.

## 18.7 Memory-safety checks

Use appropriate Rust analysis tools.

Include:

- Clippy.
- Rustfmt.
- Dependency auditing.
- Fuzzing.
- Sanitizers where supported.
- Miri where applicable.
- Unsafe-code review.

Do not claim that these tools prove complete memory safety or correctness.

---

# 19. DATA CORRUPTION PREVENTION

Data integrity is more important than benchmark performance.

Never implement optimizations that risk silent data loss.

Specific scenarios to test:

- Interrupted writes.
- Partial WAL records.
- Invalid file headers.
- Invalid page references.
- Unexpected EOF.
- Corrupted index pages.
- Corrupted document payloads.
- Out-of-space conditions.
- Failed synchronization.
- Invalid user input.
- Concurrent access attempts.
- Database reopening after a crash.

Return explicit errors where recovery is impossible.

Do not silently discard damaged data.

Do not automatically rewrite a corrupted database unless the user explicitly invokes a documented recovery procedure.

---

# 20. BACKUP AND RESTORE

Provide a safe mechanism for creating a consistent backup.

Investigate:

- Online backup.
- Snapshot-based backup.
- Checkpointing.
- Offline file copying.
- Backup verification.

Initially, an offline backup procedure is acceptable if documented.

Later, implement online backup if supported by the transaction architecture.

Backups must be restorable and verifiable.

A backup command that merely copies a live, inconsistent database file is not acceptable.

---

# 21. DEPENDENCY POLICY

Keep the core dependency footprint small.

Prefer mature, actively maintained Rust crates.

Use dependencies when they provide a meaningful correctness, security, or maintenance advantage.

Avoid:

- Unnecessary frameworks.
- Heavy web stacks in the database core.
- Async runtimes without demonstrated need.
- Duplicate serialization frameworks.
- Excessive abstraction layers.
- Runtime dependencies unrelated to embedded storage.

Do not write custom cryptographic primitives.

Do not introduce encryption until there is a clear design with established cryptographic libraries and appropriate security review.

Database encryption is a later feature.

---

# 22. PROJECT STRUCTURE

Use a Cargo workspace.

A proposed structure is:

```text
portable-nosql/
│
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── SECURITY.md
│
├── crates/
│   ├── pndb-core/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── config.rs
│   │       ├── storage/
│   │       ├── transaction/
│   │       ├── document/
│   │       ├── index/
│   │       ├── query/
│   │       └── cache/
│   │
│   ├── pndb-ffi/
│   │   └── src/
│   │
│   └── pndb-cli/
│       └── src/
│
├── include/
│   └── pndb.h
│
├── bindings/
│   ├── python/
│   ├── go/
│   ├── node/
│   └── dotnet/
│
├── examples/
│   ├── rust/
│   └── c/
│
├── tests/
│   ├── integration/
│   ├── recovery/
│   ├── concurrency/
│   ├── corruption/
│   └── portability/
│
├── fuzz/
│
├── benchmarks/
│
├── docs/
│   ├── architecture.md
│   ├── file-format.md
│   ├── transactions.md
│   ├── query-language.md
│   ├── ffi.md
│   ├── compatibility.md
│   └── research/
│
└── .github/
    └── workflows/
```

This structure is a proposal.

Adjust it if a simpler architecture is better.

Do not create empty crates or directories merely to make the repository appear complete.

Implement components when they become necessary.

---

# 23. IMPLEMENTATION MILESTONES

Follow the milestones below.

Each milestone must produce working software, executable tests, documentation, and an accurate assessment of remaining work.

Do not skip ahead to advanced features while foundational components are unstable.

## MILESTONE 0 — Research and architecture

Deliver:

- LiteDB source analysis.
- Feature inventory.
- Competitor comparison.
- Architecture decision records.
- Initial file-format proposal.
- Storage-engine selection.
- Implementation plan.
- Risk register.
- Initial benchmark design.

Acceptance criteria:

- Important claims have evidence.
- Major architectural trade-offs are documented.
- Unresolved decisions are identified.
- The proposed architecture is internally consistent.

## MILESTONE 1 — Persistent storage foundation

Implement:

- Cargo workspace.
- Core library.
- Error handling.
- Configuration.
- Database file creation.
- File header.
- Page abstraction.
- Page allocation.
- Page reads.
- Page writes.
- File reopening.
- Basic integrity validation.

Acceptance criteria:

- The database creates a real file.
- Stored pages survive process restart.
- Invalid headers are rejected.
- Basic corruption tests pass.

Do not implement collection APIs using an in-memory HashMap as a substitute for persistent storage.

## MILESTONE 2 — Transactional storage

Implement:

- Selected persistent tree structure.
- Transaction boundaries.
- Commit.
- Rollback.
- Journaling or WAL.
- Recovery.
- Initial locking.

Acceptance criteria:

- Committed values survive reopening.
- Rolled-back changes are invisible.
- Interrupted transactions do not partially commit.
- Storage invariants survive recovery tests.

## MILESTONE 3 — Document storage

Implement:

- Document encoding.
- Document decoding.
- Collections.
- Primary keys.
- Insert.
- Get by ID.
- Replace.
- Delete.

Acceptance criteria:

- Real documents are persisted through the storage engine.
- Nested documents survive serialization.
- Duplicate identifiers are rejected.
- CRUD operations remain correct after reopening.

## MILESTONE 4 — Indexing

Implement:

- Primary-index integration.
- Secondary indexes.
- Unique indexes.
- Indexed lookups.
- Index persistence.
- Transactional index updates.

Acceptance criteria:

- Indexed lookups return correct results.
- Unique constraints are enforced.
- Updates maintain indexes.
- Index recovery passes fault-injection tests.

## MILESTONE 5 — Query engine

Implement:

- Query representation.
- Filtering.
- Comparison operators.
- Logical operators.
- Nested-field lookup.
- Sorting.
- Pagination.
- Query execution.
- Basic planning.

Acceptance criteria:

- Queries produce correct results.
- Available indexes are used where appropriate.
- Results can be retrieved without unnecessary full materialization.
- Query behavior is documented.

## MILESTONE 6 — C API

Implement:

- Shared library.
- Static library.
- C header.
- Opaque database handles.
- Document APIs.
- Error handling.
- Buffer ownership.
- C integration tests.

Acceptance criteria:

- A real C application can open, write, read, and close the database.
- The C application can reopen the file and read persisted data.
- Rust panics cannot unwind across the ABI.
- Ownership and thread-safety rules are documented.

## MILESTONE 7 — Performance optimization

Implement:

- Repeatable benchmark suite.
- Profiling.
- Page-cache optimization.
- Allocation optimization.
- Document serialization optimization.
- Index traversal optimization.
- Write batching where appropriate.

Acceptance criteria:

- Benchmarks can be reproduced.
- Results are recorded.
- Performance improvements are measurable.
- Correctness and durability tests remain green.

## MILESTONE 8 — Portability

Implement:

- Linux builds.
- Windows builds.
- macOS builds.
- Platform-specific file I/O handling.
- Cross-platform C API examples.
- Portability testing.

Acceptance criteria:

- Supported platforms compile successfully.
- Native tests pass on available target systems.
- Database files produced on one platform can be opened on another where tested.
- Unsupported environments are documented.

## MILESTONE 9 — Developer tooling

Implement:

- CLI.
- Database inspection.
- Collection listing.
- Integrity checking.
- Backup functionality.
- Documentation examples.

Acceptance criteria:

- Every CLI operation uses the actual engine.
- Integrity checks detect deliberately corrupted structures.
- Backups can be restored and verified.

## MILESTONE 10 — Release candidate

Before declaring a release candidate:

- Complete regression testing.
- Run crash-recovery tests.
- Run corruption tests.
- Run concurrency tests.
- Run C ABI tests.
- Run cross-platform builds.
- Audit dependencies.
- Review unsafe code.
- Document known limitations.
- Publish reproducible benchmarks.
- Verify license compliance.
- Define compatibility policy.
- Establish security reporting procedures.

Do not describe the engine as production-ready until substantial real-world testing supports that claim.

---

# 24. OPEN-SOURCE LICENSING AND ATTRIBUTION

LiteDB is MIT-licensed.

You may study its implementation and reuse appropriately licensed material, subject to the license conditions.

However, the new project should have its own architecture and identity.

Do not blindly translate C# source files into Rust.

Prefer implementing behavior from documented requirements and architectural understanding.

If LiteDB source code or substantial derived material is reused:

- Preserve required copyright notices.
- Preserve the applicable MIT license notices.
- Document the provenance.
- Identify the relevant upstream commits.
- Preserve any additional applicable notices.

Audit third-party dependencies and bundled assets separately.

Do not assume the entire dependency tree has the same license.

For the new project, prefer dual licensing under:

```text
MIT OR Apache-2.0
```

However, do not assume that a new license choice eliminates attribution requirements for reused upstream code.

The project must use its own branding.

Do not imply official endorsement or affiliation with LiteDB.

---

# 25. IMPORTANT NON-GOALS

The initial version is NOT intended to provide:

- A MongoDB-compatible server.
- A distributed database.
- Network replication.
- Horizontal sharding.
- A cloud service.
- A web administration dashboard.
- Full-text search.
- Vector search.
- SQL compatibility.
- Full MongoDB query compatibility.
- Binary compatibility with LiteDB files.
- Advanced distributed transactions.
- Automatic multi-node failover.

These may be considered later.

The first objective is a reliable, high-performance, embedded document database.

Do not introduce unnecessary complexity to support hypothetical future features.

---

# 26. FUTURE FEATURE OPPORTUNITIES

After the core engine reaches a stable release, investigate:

### Advanced document capabilities

- Compound indexes.
- Aggregation pipelines.
- Full-text search.
- Change streams.
- Document schemas.
- Partial document updates.
- Compression.

### Developer capabilities

- Additional language bindings.
- Interactive CLI shell.
- Database browser.
- Safe migration tools.
- Import/export.
- Query profiling.

### Advanced storage

- MVCC.
- Snapshot reads.
- Online compaction.
- Online backup.
- Incremental backup.
- Improved concurrent write handling.

### Commercial opportunities

- Cloud backup.
- Cloud synchronization.
- Optional replication.
- Managed database hosting.
- Enterprise management tools.

Keep the commercial services separate from the core embedded library.

The core should remain useful without subscriptions, network access, or proprietary infrastructure.

---

# 27. DOCUMENTATION STANDARDS

Every major subsystem must have documentation explaining:

1. Purpose.
2. Architecture.
3. Public interfaces.
4. Internal data structures.
5. Invariants.
6. Error handling.
7. Concurrency behavior.
8. Performance characteristics.
9. Known limitations.
10. Testing methodology.

Maintain architecture decision records.

Explain why significant engineering decisions were made.

Do not write documentation that describes features that have not yet been implemented.

---

# 28. DEVELOPMENT QUALITY REQUIREMENTS

You must follow professional software engineering practices.

### Code quality

- Use idiomatic Rust.
- Prefer explicit error handling.
- Avoid unnecessary allocations.
- Avoid premature abstractions.
- Keep modules focused.
- Document public APIs.
- Avoid unexplained unsafe code.
- Keep dependencies minimal.
- Use consistent naming.

### Repository quality

- Maintain a clean Cargo workspace.
- Use Rustfmt.
- Use Clippy.
- Include unit tests.
- Include integration tests.
- Include examples.
- Maintain accurate documentation.
- Record architectural decisions.

### Engineering honesty

Never:

- Claim benchmarks were run when they were not.
- Claim tests passed when they were not executed.
- Claim cross-platform support based only on compilation assumptions.
- Claim ACID compliance without recovery tests.
- Claim SQLite compatibility without implementing it.
- Claim superior performance without comparative evidence.
- Mark unfinished modules as complete.
- Hide important limitations.
- Replace real persistence with mocked behavior.

If a feature is incomplete, document it explicitly.

---

# 29. AUTONOMOUS DEVELOPMENT WORKFLOW

You are authorized to perform the engineering work within the project workspace, subject to the tools and permissions available to you.

You should:

1. Inspect the environment.
2. Inspect the existing repository, if any.
3. Clone the reference repository.
4. Analyze the implementation.
5. Produce the architecture documents.
6. Create the new Rust project.
7. Implement the smallest working vertical slice.
8. Run tests.
9. Fix failures.
10. Document progress.
11. Continue through the agreed milestone sequence.

Do not ask unnecessary questions when a sensible engineering default exists.

However, do not make irreversible architectural decisions without recording their reasoning.

Keep changes incremental and reviewable.

Do not modify unrelated files.

Do not overwrite user work.

Do not perform destructive operations outside the project workspace.

Do not publish repositories, release packages, or deploy services without authorization.

## 29.1 Continuous verification

After meaningful implementation changes, run the relevant checks.

Typical commands:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Additional checks should be introduced as components become available.

Do not ignore failing tests.

Fix the underlying problem rather than disabling the test.

## 29.2 Iteration discipline

Use the following loop:

```text
Research
   |
   v
Design
   |
   v
Implement
   |
   v
Test
   |
   v
Review
   |
   v
Benchmark
   |
   v
Improve
```

Do not generate the entire database engine in one unreviewed implementation pass.

Complete coherent components.

Make each milestone independently verifiable.

---

# 30. PROGRESS TRACKING

Maintain the following files:

```text
docs/PROJECT_STATUS.md
docs/ROADMAP.md
docs/KNOWN_ISSUES.md
docs/ARCHITECTURE_DECISIONS.md
docs/BENCHMARK_RESULTS.md
```

After every meaningful development stage, update:

- Completed work.
- Tests executed.
- Tests passed.
- Tests failed.
- Known bugs.
- Remaining risks.
- Performance observations.
- Next implementation priority.

Use objective language.

For every milestone, report:

```text
MILESTONE:
STATUS:

IMPLEMENTED:
- ...

VERIFIED:
- ...

TEST RESULTS:
- ...

KNOWN ISSUES:
- ...

PERFORMANCE:
- ...

NEXT STEPS:
- ...
```

The project status must reflect actual repository state.

---

# 31. SUCCESS CRITERIA

The initial project is successful when a developer can:

1. Download a small native database library.
2. Embed it in an application.
3. Open a database file.
4. Create collections.
5. Insert JSON documents.
6. Retrieve documents by identifier.
7. Query documents using indexed fields.
8. Update documents.
9. Delete documents.
10. Execute transactions.
11. Close the application.
12. Reopen the database.
13. Retrieve the same committed data.
14. Recover safely from supported interruption scenarios.
15. Use the engine from Rust and C.
16. Move database files between verified compatible platforms.
17. Run reproducible benchmarks.

The engine must meet these requirements without running a separate database server.

A later performance milestone should demonstrate advantages over at least one relevant existing engine on clearly identified workloads.

Do not treat universally outperforming SQLite as a prerequisite or an established outcome.

---

# 32. FIRST EXECUTION INSTRUCTIONS

Begin working immediately.

Follow this exact sequence.

### Step 1 — Inspect the workspace

Determine:

- Current directory.
- Existing files.
- Git status.
- Rust toolchain availability.
- C compiler availability.
- Available operating system.
- Available disk space.
- Available test tools.

Preserve existing user files.

### Step 2 — Clone LiteDB

Clone the official LiteDB repository into a separate reference directory.

Record:

- Repository URL.
- Branch.
- Commit SHA.
- License.
- Source layout.

### Step 3 — Conduct a genuine code review

Inspect actual source files.

Identify:

- Storage engine.
- Page management.
- Transaction manager.
- Document serialization.
- Index implementation.
- Query execution.
- Locking model.
- Recovery design.

Do not rely solely on the README.

### Step 4 — Produce research documentation

Create the research documents described above.

Include a feature-parity matrix.

For every LiteDB feature, classify it as:

```text
Required for MVP
Required for v1
Optional
Deferred
Not planned
```

### Step 5 — Create the architecture proposal

Present:

- Selected storage architecture.
- Alternatives considered.
- File-format proposal.
- Transaction design.
- Document representation.
- Index design.
- Concurrency model.
- C ABI design.
- Benchmark methodology.
- Key risks.

Explain decisions with evidence.

### Step 6 — Establish the baseline

Create the Cargo workspace.

Add:

- Formatting configuration.
- Linting.
- Basic tests.
- Core crate.
- Error types.
- Initial documentation.
- CI configuration.

### Step 7 — Implement the first real vertical slice

Implement the smallest meaningful persistent-storage functionality.

The first implementation must genuinely write data to disk and read it back after reopening the database.

Do not simulate persistence.

### Step 8 — Verify

Run the applicable build and tests.

Fix errors.

Document the results.

### Step 9 — Report

Provide a concise engineering report:

```text
REFERENCE REPOSITORY:
REFERENCE COMMIT:

ARCHITECTURAL FINDINGS:

ARCHITECTURE SELECTED:

IMPLEMENTED COMPONENTS:

TESTS EXECUTED:

TEST RESULTS:

KNOWN LIMITATIONS:

NEXT MILESTONE:
```

### Step 10 — Continue incrementally

Proceed to subsequent milestones when their dependencies and acceptance criteria are satisfied.

If an architectural blocker is encountered, investigate and document it rather than inventing a workaround that compromises data integrity.

---

# 33. FINAL ENGINEERING PRINCIPLES

Throughout development, follow these principles:

**Correctness before performance.**

**Durability before throughput.**

**Simplicity before unnecessary abstraction.**

**Portability before platform-specific optimization.**

**Evidence before performance claims.**

**Memory safety before unsafe micro-optimizations.**

**Verified functionality before feature-count expansion.**

**Small, maintainable components before unnecessary complexity.**

**A reliable database engine is more valuable than a fast database engine that can lose data.**

The ultimate goal is to create a genuine open-source database that developers can trust.

It should be small enough to embed easily, powerful enough for real applications, portable across major platforms, and architecturally capable of delivering excellent document-oriented performance.

Start with LiteDB research, but do not let its implementation constrain better architectural decisions.

Build an independent database engine with a clear technical identity.

**BEGIN WITH THE REPOSITORY ANALYSIS AND ARCHITECTURE RESEARCH. THEN IMPLEMENT AND VERIFY MILESTONE 1 BEFORE EXPANDING THE ENGINE.**