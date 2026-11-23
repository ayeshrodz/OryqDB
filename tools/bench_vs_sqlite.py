#!/usr/bin/env python3
"""Reproducible single-file benchmark comparing OryqDB (C ABI via Python ctypes) vs SQLite3.

Measures:
  1. Batched insert (N JSON documents + secondary index) in one ACID transaction
  2. Primary `_id` point lookups (N lookups)
  3. Secondary indexed equality queries (200 queries)
  4. Batched partial updates (N/4 updates)
  5. Single-file size on disk after clean close, after deleting 75% of docs, and after VACUUM

Usage:
  cargo build --release -p oryq-ffi
  python3 tools/bench_vs_sqlite.py --docs 2000
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import tempfile
import time
from pathlib import Path
from typing import Dict

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "bindings" / "python"))

from oryqdb import Database  # noqa: E402


def _make_doc(i: int) -> dict:
    return {
        "_id": i,
        "email": f"user{i}@example.com",
        "category": f"cat_{i % 20}",
        "score": float((i * 37) % 1000) / 10.0,
        "active": (i % 2) == 0,
        "bio": f"OryqDB vs SQLite embedded single-file document benchmark payload #{i:06d}",
    }


def bench_oryqdb(tmp_dir: Path, n_docs: int) -> Dict[str, float]:
    db_path = tmp_dir / "bench.oryq"
    docs = [_make_doc(i) for i in range(1, n_docs + 1)]
    metrics: Dict[str, float] = {}

    with Database.open(db_path) as db:
        # 1. Batched insert + secondary index
        t0 = time.perf_counter()
        with db.write_txn() as wtx:
            wtx.create_collection("users")
            wtx.create_index("users", "category", name="cat_idx")
            for doc in docs:
                wtx.insert("users", doc)
            wtx.commit()
        metrics["insert_ms"] = (time.perf_counter() - t0) * 1000.0

        # 2. Primary `_id` point lookups (zero-copy cursor getters)
        t0 = time.perf_counter()
        with db.read_txn() as rtx:
            for i in range(1, n_docs + 1):
                with rtx.find("users", {"_id": i}) as cur:
                    assert cur.step()
                    _ = cur.get_f64("score")
        metrics["pk_lookup_ms"] = (time.perf_counter() - t0) * 1000.0

        # 3. Secondary indexed lookups (200 queries)
        t0 = time.perf_counter()
        with db.read_txn() as rtx:
            for q in range(200):
                cat = f"cat_{q % 20}"
                with rtx.find("users", {"category": cat}) as cur:
                    count = 0
                    while cur.step():
                        count += 1
                    assert count > 0
        metrics["sec_lookup_ms"] = (time.perf_counter() - t0) * 1000.0

        # 4. Batched partial updates (n_docs // 4 updates)
        n_upd = max(1, n_docs // 4)
        t0 = time.perf_counter()
        with db.write_txn() as wtx:
            for i in range(1, n_upd + 1):
                wtx.update_by_id("users", i, {"$inc": {"score": 1.5}})
            wtx.commit()
        metrics["update_ms"] = (time.perf_counter() - t0) * 1000.0

        db.checkpoint()

    # Auto-sealed on close -> single `.oryq` file
    assert not Path(f"{db_path}-wal").exists(), "expected .oryq-wal removed on close"
    metrics["size_full_bytes"] = float(db_path.stat().st_size)

    # 5. Delete 75% of docs + VACUUM
    del_start = max(2, n_docs // 4 + 1)
    with Database.open(db_path) as db:
        with db.write_txn() as wtx:
            for i in range(del_start, n_docs + 1):
                wtx.delete_by_id("users", i)
            wtx.commit()
        db.checkpoint()
        t0 = time.perf_counter()
        db.vacuum()
        metrics["vacuum_ms"] = (time.perf_counter() - t0) * 1000.0

    assert not Path(f"{db_path}-wal").exists()
    metrics["size_vacuum_bytes"] = float(db_path.stat().st_size)
    return metrics


def bench_sqlite(tmp_dir: Path, n_docs: int) -> Dict[str, float]:
    db_path = tmp_dir / "bench.sqlite"
    docs = [_make_doc(i) for i in range(1, n_docs + 1)]
    metrics: Dict[str, float] = {}

    conn = sqlite3.connect(db_path, isolation_level=None)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")

    # 1. Batched insert + secondary index
    t0 = time.perf_counter()
    conn.execute("BEGIN")
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, category TEXT NOT NULL, score REAL NOT NULL, doc TEXT NOT NULL)"
    )
    conn.execute("CREATE INDEX cat_idx ON users(category)")
    conn.executemany(
        "INSERT INTO users(id, category, score, doc) VALUES (?, ?, ?, ?)",
        [(d["_id"], d["category"], d["score"], json.dumps(d, separators=(",", ":"))) for d in docs],
    )
    conn.execute("COMMIT")
    metrics["insert_ms"] = (time.perf_counter() - t0) * 1000.0

    # 2. Primary key point lookups
    t0 = time.perf_counter()
    cur = conn.cursor()
    for i in range(1, n_docs + 1):
        row = cur.execute("SELECT score FROM users WHERE id = ?", (i,)).fetchone()
        assert row is not None
    metrics["pk_lookup_ms"] = (time.perf_counter() - t0) * 1000.0

    # 3. Secondary indexed lookups (200 queries)
    t0 = time.perf_counter()
    for q in range(200):
        cat = f"cat_{q % 20}"
        rows = cur.execute("SELECT id FROM users WHERE category = ?", (cat,)).fetchall()
        assert len(rows) > 0
    metrics["sec_lookup_ms"] = (time.perf_counter() - t0) * 1000.0

    # 4. Batched partial updates (n_docs // 4 updates)
    n_upd = max(1, n_docs // 4)
    t0 = time.perf_counter()
    conn.execute("BEGIN")
    for i in range(1, n_upd + 1):
        conn.execute("UPDATE users SET score = score + 1.5 WHERE id = ?", (i,))
    conn.execute("COMMIT")
    metrics["update_ms"] = (time.perf_counter() - t0) * 1000.0

    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()
    metrics["size_full_bytes"] = float(db_path.stat().st_size)

    # 5. Delete 75% of docs + VACUUM
    del_start = max(2, n_docs // 4 + 1)
    conn = sqlite3.connect(db_path, isolation_level=None)
    conn.execute("BEGIN")
    conn.execute("DELETE FROM users WHERE id >= ?", (del_start,))
    conn.execute("COMMIT")
    t0 = time.perf_counter()
    conn.execute("VACUUM")
    metrics["vacuum_ms"] = (time.perf_counter() - t0) * 1000.0
    conn.close()
    metrics["size_vacuum_bytes"] = float(db_path.stat().st_size)
    return metrics


def main() -> None:
    parser = argparse.ArgumentParser(description="OryqDB vs SQLite3 single-file benchmark")
    parser.add_argument("--docs", type=int, default=2000, help="Number of documents (default: 2000)")
    args = parser.parse_args()
    n_docs = args.docs

    with tempfile.TemporaryDirectory(prefix="oryq_vs_sqlite_") as td:
        tmp = Path(td)
        oryq = bench_oryqdb(tmp, n_docs)
        sql = bench_sqlite(tmp, n_docs)

    print(f"# OryqDB vs SQLite3 Single-File Benchmark (N={n_docs}, Python {sys.version.split()[0]}, SQLite {sqlite3.sqlite_version})\n")
    print("| Metric | OryqDB (C ABI via Python) | SQLite3 (stdlib C) |")
    print("| :--- | ---: | ---: |")
    print(f"| Batched Insert ({n_docs} docs + 1 index) | `{oryq['insert_ms']:.2f} ms` ({n_docs / (oryq['insert_ms'] / 1000.0):,.0f} docs/s) | `{sql['insert_ms']:.2f} ms` ({n_docs / (sql['insert_ms'] / 1000.0):,.0f} docs/s) |")
    print(f"| Primary `_id` Point Lookups ({n_docs} ops) | `{oryq['pk_lookup_ms']:.2f} ms` ({n_docs / (oryq['pk_lookup_ms'] / 1000.0):,.0f} ops/s) | `{sql['pk_lookup_ms']:.2f} ms` ({n_docs / (sql['pk_lookup_ms'] / 1000.0):,.0f} ops/s) |")
    print(f"| Secondary Index Lookups (200 scans) | `{oryq['sec_lookup_ms']:.2f} ms` | `{sql['sec_lookup_ms']:.2f} ms` |")
    print(f"| Batched Partial Updates ({max(1, n_docs // 4)} ops) | `{oryq['update_ms']:.2f} ms` | `{sql['update_ms']:.2f} ms` |")
    print(f"| `VACUUM` after 75% delete | `{oryq['vacuum_ms']:.2f} ms` | `{sql['vacuum_ms']:.2f} ms` |")
    print(f"| Single-File Size (100% docs, sealed) | `{oryq['size_full_bytes'] / 1024.0:.1f} KiB` | `{sql['size_full_bytes'] / 1024.0:.1f} KiB` |")
    print(f"| Single-File Size (25% docs after `VACUUM`) | `{oryq['size_vacuum_bytes'] / 1024.0:.1f} KiB` | `{sql['size_vacuum_bytes'] / 1024.0:.1f} KiB` |")


if __name__ == "__main__":
    main()
