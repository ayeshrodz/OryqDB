package oryqdb

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEndToEndCRUDIndexAggregateVacuumAndBackup(t *testing.T) {
	if Version() == "" {
		t.Fatal("expected non-empty version")
	}
	maj, min := ABIVersion()
	if maj != 1 || min != 2 {
		t.Fatalf("expected ABI 1.2, got %d.%d", maj, min)
	}

	dir := t.TempDir()
	dbPath := filepath.Join(dir, "go_test.oryq")
	backupPath := filepath.Join(dir, "go_backup.oryq")

	db, err := Open(dbPath)
	if err != nil {
		t.Fatalf("Open failed: %v", err)
	}

	wtx, err := db.BeginWrite()
	if err != nil {
		t.Fatalf("BeginWrite failed: %v", err)
	}
	if err := wtx.CreateCollection("users"); err != nil {
		t.Fatalf("CreateCollection failed: %v", err)
	}
	if err := wtx.CreateIndex("users", "email", 1, true, false, "email_idx"); err != nil {
		t.Fatalf("CreateIndex failed: %v", err)
	}
	if err := wtx.InsertJSON("users", `{"_id": 1, "name": "Ada", "email": "ada@example.com", "score": 98.5, "active": true}`); err != nil {
		t.Fatalf("InsertJSON 1 failed: %v", err)
	}
	if err := wtx.InsertJSON("users", `{"_id": 2, "name": "Grace", "email": "grace@example.com", "score": 95.0, "active": true}`); err != nil {
		t.Fatalf("InsertJSON 2 failed: %v", err)
	}
	if err := wtx.Commit(); err != nil {
		t.Fatalf("Commit failed: %v", err)
	}

	rtx, err := db.BeginRead()
	if err != nil {
		t.Fatalf("BeginRead failed: %v", err)
	}
	cur, err := rtx.FindJSON("users", `{"_id": 1}`)
	if err != nil {
		t.Fatalf("FindJSON failed: %v", err)
	}
	ok, err := cur.Next()
	if err != nil || !ok {
		t.Fatalf("Next failed: ok=%v err=%v", ok, err)
	}
	id, err := cur.GetI64("_id")
	if err != nil || id != 1 {
		t.Fatalf("GetI64: got %d err=%v", id, err)
	}
	name, err := cur.GetUTF8("name")
	if err != nil || name != "Ada" {
		t.Fatalf("GetUTF8: got %q err=%v", name, err)
	}
	score, err := cur.GetF64("score")
	if err != nil || score != 98.5 {
		t.Fatalf("GetF64: got %f err=%v", score, err)
	}
	active, err := cur.GetBool("active")
	if err != nil || !active {
		t.Fatalf("GetBool: got %v err=%v", active, err)
	}
	_ = cur.Close()

	// Aggregation
	aggCur, err := rtx.AggregateJSON("users", `[{"$group": {"_id": null, "total": {"$sum": "$score"}}}]`)
	if err != nil {
		t.Fatalf("AggregateJSON failed: %v", err)
	}
	rawJSON, hasRow, err := aggCur.NextJSON()
	if err != nil || !hasRow || rawJSON == "" {
		t.Fatalf("NextJSON failed: hasRow=%v err=%v json=%q", hasRow, err, rawJSON)
	}
	_ = aggCur.Close()
	_ = rtx.Close()

	if err := db.Checkpoint(); err != nil {
		t.Fatalf("Checkpoint failed: %v", err)
	}
	if err := db.Vacuum(); err != nil {
		t.Fatalf("Vacuum failed: %v", err)
	}
	icOK, err := db.IntegrityCheck()
	if err != nil || !icOK {
		t.Fatalf("IntegrityCheck failed: ok=%v err=%v", icOK, err)
	}
	if err := db.BackupTo(backupPath); err != nil {
		t.Fatalf("BackupTo failed: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("Close failed: %v", err)
	}

	// Single-file check: auto-seal removes .oryq-wal
	if _, err := os.Stat(dbPath + "-wal"); !os.IsNotExist(err) {
		t.Fatalf("expected %s-wal to be removed on clean close", dbPath)
	}

	bdb, err := Open(backupPath)
	if err != nil {
		t.Fatalf("Open backup failed: %v", err)
	}
	defer bdb.Close()
	if icOK, err := bdb.IntegrityCheck(); err != nil || !icOK {
		t.Fatalf("backup IntegrityCheck failed: ok=%v err=%v", icOK, err)
	}
}

func TestInMemoryAndHybridSearch(t *testing.T) {
	expectHybrid := os.Getenv("ORYQ_EXPECT_HYBRID") == "1"

	db, err := OpenInMemory()
	if err != nil {
		t.Fatalf("OpenInMemory failed: %v", err)
	}
	defer db.Close()

	wtx, err := db.BeginWrite()
	if err != nil {
		t.Fatalf("BeginWrite failed: %v", err)
	}
	if err := wtx.CreateCollection("docs"); err != nil {
		t.Fatalf("CreateCollection failed: %v", err)
	}
	if err := wtx.InsertJSON("docs", `{"_id": 1, "title": "Rust DB", "emb": [1.0, 0.0, 0.0]}`); err != nil {
		t.Fatalf("InsertJSON failed: %v", err)
	}
	if err := wtx.Commit(); err != nil {
		t.Fatalf("Commit failed: %v", err)
	}

	wtx2, err := db.BeginWrite()
	if err != nil {
		t.Fatalf("BeginWrite 2 failed: %v", err)
	}
	ftsErr := wtx2.CreateFtsIndex("docs", []string{"title"}, "t_idx")
	if ftsErr != nil {
		_ = wtx2.Rollback()
		if expectHybrid {
			t.Fatalf("expected hybrid FTS+Vec build, got %v", ftsErr)
		}
		if !IsStatus(ftsErr, StatusErrUnsupportedFeature) {
			t.Fatalf("expected ErrUnsupportedFeature, got %v", ftsErr)
		}
		return
	}
	if err := wtx2.CreateVectorIndex("docs", "emb", 3, VectorMetricCosine, VectorQuantF32, "e_idx"); err != nil {
		_ = wtx2.Rollback()
		t.Fatalf("CreateVectorIndex failed: %v", err)
	}
	if err := wtx2.Commit(); err != nil {
		t.Fatalf("Commit 2 failed: %v", err)
	}

	rtx, err := db.BeginRead()
	if err != nil {
		t.Fatalf("BeginRead failed: %v", err)
	}
	defer rtx.Close()

	ftsCur, err := rtx.SearchFts("docs", "t_idx", "rust", 5)
	if err != nil {
		t.Fatalf("SearchFts failed: %v", err)
	}
	defer ftsCur.Close()
	if ok, err := ftsCur.Next(); err != nil || !ok {
		t.Fatalf("FTS Next failed: ok=%v err=%v", ok, err)
	}
	if score, err := ftsCur.GetScore(); err != nil || score <= 0 {
		t.Fatalf("FTS GetScore failed: score=%f err=%v", score, err)
	}

	vecCur, err := rtx.SearchVector("docs", "e_idx", []float32{0.95, 0.05, 0.0}, 1)
	if err != nil {
		t.Fatalf("SearchVector failed: %v", err)
	}
	defer vecCur.Close()
	if ok, err := vecCur.Next(); err != nil || !ok {
		t.Fatalf("Vec Next failed: ok=%v err=%v", ok, err)
	}
	if id, err := vecCur.GetI64("_id"); err != nil || id != 1 {
		t.Fatalf("Vec GetI64 failed: id=%d err=%v", id, err)
	}
}
