import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  Database,
  OryqStatus,
  VectorMetric,
  VectorQuantization,
} from "../index.mjs";

test("end-to-end CRUD, index, aggregation, vacuum, auto-seal & backup", () => {
  assert.ok(Database.version().length > 0);
  assert.deepEqual(Database.abiVersion(), [1, 2]);

  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "oryqdb_node_"));
  try {
    const dbPath = path.join(tmpDir, "app.oryq");
    const backupPath = path.join(tmpDir, "backup.oryq");

    const db = Database.open(dbPath);
    const wtx = db.beginWrite();
    wtx.createCollection("users");
    wtx.createIndex("users", "email", { unique: true, name: "email_idx" });
    wtx.insert("users", {
      _id: 1,
      name: "Ada",
      email: "ada@example.com",
      score: 98.5,
      active: true,
    });
    wtx.insert("users", {
      _id: 2,
      name: "Grace",
      email: "grace@example.com",
      score: 95.0,
      active: true,
    });
    wtx.commit();

    const rtx = db.beginRead();
    const cur = rtx.find("users", { _id: 1 });
    assert.equal(cur.step(), true);
    assert.equal(cur.getI64("_id"), 1);
    assert.equal(cur.getUtf8("name"), "Ada");
    assert.equal(cur.getF64("score"), 98.5);
    assert.equal(cur.getBool("active"), true);
    assert.equal(cur.step(), false);
    cur.close();

    const aggCur = rtx.aggregate("users", [
      { $group: { _id: null, total: { $sum: "$score" } } },
    ]);
    const aggRows = [...aggCur];
    aggCur.close();
    assert.equal(aggRows.length, 1);
    assert.equal(aggRows[0].total, 193.5);
    rtx.close();

    db.checkpoint();
    db.vacuum();
    assert.equal(db.integrityCheck(), true);
    db.backupTo(backupPath);
    db.close();

    assert.equal(fs.existsSync(`${dbPath}-wal`), false);

    const bdb = Database.open(backupPath);
    assert.equal(bdb.integrityCheck(), true);
    bdb.close();
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("in-memory database & hybrid FTS/Vector search", () => {
  const expectHybrid = process.env.ORYQ_EXPECT_HYBRID === "1";
  const db = Database.openInMemory();
  try {
    const wtx = db.beginWrite();
    wtx.createCollection("docs");
    wtx.insert("docs", {
      _id: 1,
      title: "Rust Embedded DB",
      emb: [1.0, 0.0, 0.0],
    });
    wtx.commit();

    const wtx2 = db.beginWrite();
    try {
      wtx2.createFtsIndex("docs", ["title"], "t_idx");
      wtx2.createVectorIndex("docs", "emb", 3, {
        metric: VectorMetric.COSINE,
        quantization: VectorQuantization.F32,
        name: "e_idx",
      });
      wtx2.commit();
    } catch (err) {
      wtx2.rollback();
      if (expectHybrid) throw err;
      assert.equal(err.status, OryqStatus.ERR_UNSUPPORTED_FEATURE);
      return;
    }

    const rtx = db.beginRead();
    try {
      const ftsCur = rtx.searchFts("docs", "t_idx", "rust", 5);
      assert.equal(ftsCur.step(), true);
      assert.equal(ftsCur.getI64("_id"), 1);
      assert.ok(ftsCur.getScore() > 0);
      ftsCur.close();

      const vecCur = rtx.searchVector("docs", "e_idx", [0.95, 0.05, 0.0], 1);
      assert.equal(vecCur.step(), true);
      assert.equal(vecCur.getI64("_id"), 1);
      assert.ok(vecCur.getScore() >= 0);
      vecCur.close();
    } finally {
      rtx.close();
    }
  } finally {
    db.close();
  }
});
