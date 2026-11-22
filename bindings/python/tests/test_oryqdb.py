"""End-to-end integration tests for the official Python `oryqdb` ctypes bindings."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

# Allow running directly from repo root without editable install
PYTHON_BINDING_ROOT = Path(__file__).resolve().parents[1]
if str(PYTHON_BINDING_ROOT) not in sys.path:
    sys.path.insert(0, str(PYTHON_BINDING_ROOT))

from oryqdb import Database, OryqError, OryqStatus  # noqa: E402


class TestOryqDbPythonBindings(unittest.TestCase):
    def test_crud_indexes_zero_copy_cursor_aggregate_backup_and_rollback(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oryqdb_py_") as tmpdir:
            db_path = Path(tmpdir) / "app.oryq"
            backup_path = Path(tmpdir) / "backup.oryq"

            with Database.open(db_path) as db:
                self.assertTrue(db.version())
                self.assertEqual(db.abi_version(), (1, 1))

                # 1. Create collection, unique index, and insert documents
                with db.write_txn() as wtx:
                    wtx.create_collection("users")
                    wtx.create_index("users", "email", unique=True, name="email_uniq")
                    wtx.insert(
                        "users",
                        {
                            "_id": 1,
                            "name": "Ada",
                            "email": "ada@example.com",
                            "score": 98.5,
                            "active": True,
                            "team": "core",
                        },
                    )
                    wtx.insert(
                        "users",
                        {
                            "_id": 2,
                            "name": "Grace",
                            "email": "grace@example.com",
                            "score": 95.0,
                            "active": True,
                            "team": "core",
                        },
                    )
                    wtx.insert(
                        "users",
                        {
                            "_id": 3,
                            "name": "Linus",
                            "email": "linus@example.com",
                            "score": 88.0,
                            "active": False,
                            "team": "infra",
                        },
                    )
                    wtx.commit()

                # 2. Duplicate unique key raises OryqError(ERR_DUPLICATE_KEY) and auto-rolls back
                with self.assertRaises(OryqError) as ctx:
                    with db.write_txn() as wtx:
                        wtx.insert(
                            "users",
                            {"_id": 4, "name": "Temp", "email": "temp@example.com"},
                        )
                        wtx.insert(
                            "users",
                            {"_id": 5, "name": "Dup", "email": "ada@example.com"},
                        )
                        wtx.commit()
                self.assertEqual(ctx.exception.status, OryqStatus.ERR_DUPLICATE_KEY)

                # 3. Zero-copy cursor accessors (step + get_i64/get_f64/get_bool/get_utf8)
                with db.read_txn() as rtx:
                    with rtx.find("users", {"_id": 1}) as cur:
                        self.assertTrue(cur.step())
                        self.assertEqual(cur.get_i64("_id"), 1)
                        self.assertEqual(cur.get_utf8("name"), "Ada")
                        self.assertAlmostEqual(cur.get_f64("score"), 98.5, places=6)
                        self.assertTrue(cur.get_bool("active"))
                        self.assertFalse(cur.step())

                    # JSON iterator verifies rollback left only 3 documents
                    with rtx.find("users") as cur:
                        docs = list(cur)
                        self.assertEqual(len(docs), 3)

                    # 4. Aggregation pipeline
                    with rtx.aggregate(
                        "users",
                        [
                            {"$match": {"active": True}},
                            {"$group": {"_id": "$team", "total": {"$sum": "$score"}, "n": {"$count": {}}}},
                        ],
                    ) as cur:
                        agg = list(cur)
                        self.assertEqual(len(agg), 1)
                        self.assertEqual(agg[0]["_id"], "core")
                        self.assertEqual(agg[0]["n"], 2)
                        self.assertAlmostEqual(agg[0]["total"], 193.5, places=6)

                # 5. Update, delete, drop index, checkpoint, integrity check, and online backup
                with db.write_txn() as wtx:
                    wtx.update_by_id("users", 1, {"$set": {"score": 100.0}})
                    wtx.delete_by_id("users", 3)
                    wtx.drop_index("users", "email_uniq")
                    wtx.commit()

                db.checkpoint()
                self.assertTrue(db.integrity_check())
                db.backup_to(backup_path)

            # 6. Verify backup opens cleanly and contains exact updated state
            with Database.open(backup_path) as bdb:
                self.assertTrue(bdb.integrity_check())
                with bdb.read_txn() as rtx:
                    with rtx.find("users", {"_id": 1}) as cur:
                        self.assertTrue(cur.step())
                        self.assertAlmostEqual(cur.get_f64("score"), 100.0, places=6)
                    with rtx.find("users") as cur:
                        self.assertEqual(len(list(cur)), 2)

    def test_string_id_utf8_edge_cases_busy_close_retry_and_use_after_close(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oryqdb_py_edge_") as tmpdir:
            db_path = Path(tmpdir) / "edge.oryq"
            db = Database.open(db_path)

            # 1. String _id insert, update_by_id, delete_by_id + empty & multi-byte UTF-8 get_utf8
            with db.write_txn() as wtx:
                wtx.create_collection("items")
                wtx.insert(
                    "items",
                    {"_id": "item-alpha", "empty": "", "label": "日本語 🚀 café"},
                )
                wtx.update_by_id("items", "item-alpha", {"$set": {"updated": True}})
                wtx.commit()

            with db.read_txn() as rtx:
                with rtx.find("items", {"_id": "item-alpha"}) as cur:
                    self.assertTrue(cur.step())
                    self.assertEqual(cur.get_utf8("_id"), "item-alpha")
                    self.assertEqual(cur.get_utf8("empty"), "")
                    self.assertEqual(cur.get_utf8("label"), "日本語 🚀 café")
                    self.assertTrue(cur.get_bool("updated"))

            with db.write_txn() as wtx:
                wtx.delete_by_id("items", "item-alpha")
                wtx.commit()

            with db.read_txn() as rtx:
                with rtx.find("items") as cur:
                    self.assertEqual(len(list(cur)), 0)

            # 2. Database.close() while a ReadTxn is open raises ERR_BUSY without leaking handle;
            #    closing the ReadTxn and retrying db.close() succeeds cleanly.
            rtx = db.read_txn()
            with self.assertRaises(OryqError) as ctx:
                db.close()
            self.assertEqual(ctx.exception.status, OryqStatus.ERR_BUSY)
            rtx.close()
            db.close()

            # 3. Double-close is idempotent; use-after-close raises ERR_INVALID_ARGUMENT
            db.close()
            with self.assertRaises(OryqError) as ctx:
                db.read_txn()
            self.assertEqual(ctx.exception.status, OryqStatus.ERR_INVALID_ARGUMENT)


if __name__ == "__main__":
    unittest.main()
