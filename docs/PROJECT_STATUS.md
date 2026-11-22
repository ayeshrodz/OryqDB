# OryqDB Project Status

* **Last Updated:** 2026-11-22
* **Master Design:** [oryqdb.md](designs/oryqdb.md)

```text
MILESTONE: E1a — Optional Full-Text Search Provider (`v0.10.0` / Issue #15 / `X01a`)
STATUS: Active `0.10.x` Extensions — All 8 baseline crates + optional `oryq-fts` (`--features fts`) Okapi BM25 inverted-index provider implemented and verified.

IMPLEMENTED:
- `W00–W05` (`oryq-types`, `oryq-document`, `oryq-engine`): VFS/PAL, Page 0 dual shadow headers, slotted pages, overflow chains, freelist, pin-aware PageCache, VersionIndex, byte B+ tree, WAL, single-process MVCC transactions, savepoints, checkpointing & crash recovery.
- `W06–W09` (`oryq-document`, `oryq-index`, `oryq-core`): Update operators, state-normalized physical patches, memcomparable key codec, primary & secondary indexes, frozen v1 catalog, atomic statement orchestration.
- `W10` (`v0.4.0`, `oryq-query` & `oryq-core`): Filter/projection/sort AST & JSON parser, exact evaluator, rule-based planner with sparse-index safety, `QueryCursor`, `find`/`count`/`explain_analyze`.
- `W11` (`v0.5.0`, `oryq-ffi` & `include/oryq.h`): Panic-safe C ABI (`catch_unwind`), stable `OryqStatus` codes, thread-local diagnostics, child-tracked opaque handles, JSON CRUD/find & zero-copy cursor accessors.
- `W12` (`v0.6.0`, `oryq-core`): State-normalized delta-chain records (`MAX_DELTA_CHAIN = 4`) with automatic `RecordCompactor` compaction on 5th patch + release benchmark suite (`docs/BENCHMARK_RESULTS.md`).
- `W13a` (`v0.7.0-alpha.1`, `oryq-index` & `oryq-core`): Compound (`1..=16` fields, `R=16` prefix compression) & multi-key array indexes with parallel-array prohibition & dedup.
- `W13b` (`v0.7.0`, `oryq-query` & `oryq-core`): Spillable aggregation pipelines (`$match`, `$project`, `$sort` w/ RAII `SpillManager`, `$skip`, `$limit`, `$unwind`, `$lookup`, `$group`).
- `W14` (`v0.8.0-alpha.1`, `oryq-engine` & `oryq-core`): Read-only forensic `inspect_{page,wal,locks}`, `Database::integrity_check`, and sealed standalone online `Database::backup_to`.
- `W15` (`v0.8.0`, `oryq-cli`): Zero-extra-dependency `oryq` CLI utility (`info`, `collections`, CRUD, `find`, `aggregate`, `checkpoint`, `backup`, `integrity-check`, `inspect`).
- `W16` (`v0.9.0-rc.1`): M10 baseline format freeze, full 8-crate module/architecture/doc verification, and release binary verification.

VERIFIED:
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (0 warnings, MSRV 1.80.0 clean).
- `cargo test --workspace` and `--no-default-features`: 76 tests passed, 0 failed (`oryq-types`: 7; `oryq-document`: 16; `oryq-engine`: 23; `oryq-index`: 6; `oryq-query`: 6; `oryq-core`: 8; `oryq-ffi`: 3; `oryq-cli`: 1; `xtask`: 6).
- `cargo +nightly miri test` (`oryq-types`: 7, `oryq-document`: 16, `oryq-ffi`: 1): 24 passed, 0 failed (zero UB/leaks).
- `cargo audit`: 0 vulnerabilities across 49 crate dependencies (1,555 advisories scanned).
- `cargo run -p xtask -- modules` for all 8 crates and `xtask architecture --base origin/main`: passed.
- Native C99 FFI smoke test (`cc -std=c99 -Wall -Wextra -Werror -Iinclude ... -loryq`): passed.
- `python3 tools/validate_design_docs.py`: passed.
- `cargo build --release -p oryq-cli -p oryq-ffi`: `oryq` (1.6 MB), `liboryq.so` (1.5 MB), `liboryq.a` (30 MB).

PERFORMANCE (Linux x86_64, 8 KiB pages, SyncMode::Normal):
- Batched Insert (2,000 docs + 1 secondary index): 151.53 ms (13,199 docs/sec)
- Primary _id Point Lookup (2,000 lookups): 13.30 ms (150,359 ops/sec)
- Partial Delta Updates (1,200 ops, 6 rounds w/ compaction): 66.22 ms (18,121 ops/sec)
- Secondary Range Scan (100 scans x ~200 rows): 140.47 ms (712 scans/sec)
- WAL Checkpoint + Reset (117 pages copied): 0.60 ms

KNOWN ISSUES:
- Post-baseline extensions `X01b` (`oryq-vec`), `X02` (`.oryq-shm` multi-process), and `X03` (`commit_batch`) remain planned for `0.10.1–0.10.3`.

NEXT STEPS:
1. Merge `v0.10.0` (#15 / `oryq-fts`) after independent reviewer sign-off and green multi-OS CI.
2. Continue `v0.10.x` extensions: #16 (`oryq-vec` / `X01b`) → #17 (`.oryq-shm` / `X02`) → #18 (`commit_batch` / `X03`) → #19 (`v0.11.0` bindings & guides).
```

[Initial review report](reviews/initial-design/goldfish_review_report.md) · [Modular review report](reviews/modular-architecture/goldfish_review_report.md) · [Roadmap](ROADMAP.md) · [Risks](KNOWN_ISSUES.md)

[Modular specification](designs/modular-architecture.md) · [Module work packages](MODULE_DEVELOPMENT_PLAN.md) · [Module catalog](modules/README.md)
