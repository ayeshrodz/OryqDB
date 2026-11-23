using OryqDb;

static void Assert(bool cond, string message)
{
    if (!cond)
    {
        throw new InvalidOperationException($"Assertion failed: {message}");
    }
}

Assert(!string.IsNullOrEmpty(OryqDatabase.Version()), "version should be non-empty");
var (maj, min) = OryqDatabase.AbiVersion();
Assert(maj == 1 && min == 2, $"expected ABI 1.2, got {maj}.{min}");

// 1. Disk database: CRUD, index, zero-copy getters, aggregation, vacuum, auto-seal & backup
var tmpDir = Path.Combine(Path.GetTempPath(), $"oryqdb_dotnet_{Environment.ProcessId}_{DateTime.UtcNow.Ticks}");
Directory.CreateDirectory(tmpDir);
try
{
    var dbPath = Path.Combine(tmpDir, "app.oryq");
    var backupPath = Path.Combine(tmpDir, "backup.oryq");

    using (var db = OryqDatabase.Open(dbPath))
    {
        using (var wtx = db.BeginWrite())
        {
            wtx.CreateCollection("users");
            wtx.CreateIndex("users", "email", sortOrder: 1, unique: true, name: "email_idx");
            wtx.InsertJson("users", """{"_id": 1, "name": "Ada", "email": "ada@example.com", "score": 98.5, "active": true}""");
            wtx.InsertJson("users", """{"_id": 2, "name": "Grace", "email": "grace@example.com", "score": 95.0, "active": true}""");
            wtx.Commit();
        }

        using (var rtx = db.BeginRead())
        {
            using (var cur = rtx.Find("users", """{"_id": 1}"""))
            {
                Assert(cur.Next(), "expected matching doc");
                Assert(cur.GetInt64("_id") == 1, "id mismatch");
                Assert(cur.GetUtf8("name") == "Ada", "name mismatch");
                Assert(Math.Abs(cur.GetDouble("score") - 98.5) < 1e-6, "score mismatch");
                Assert(cur.GetBool("active"), "active mismatch");
                Assert(!cur.Next(), "expected end of cursor");
            }

            using (var agg = rtx.Aggregate("users", """[{"$group": {"_id": null, "total": {"$sum": "$score"}}}]"""))
            {
                var row = agg.NextJson();
                Assert(row != null && row.Contains("193.5"), $"unexpected agg row: {row}");
            }

            // Close while ReadTxn is open must fail with ErrBusy
            bool busyCaught = false;
            try
            {
                db.Close();
            }
            catch (OryqException ex) when (ex.Status == OryqStatus.ErrBusy)
            {
                busyCaught = true;
            }
            Assert(busyCaught, "expected ErrBusy when closing db with open ReadTxn");
        }

        // Duplicate unique key + Rollback
        using (var wtxDup = db.BeginWrite())
        {
            wtxDup.InsertJson("users", """{"_id": 3, "name": "Temp", "email": "temp@example.com"}""");
            bool dupCaught = false;
            try
            {
                wtxDup.InsertJson("users", """{"_id": 4, "name": "Dup", "email": "ada@example.com"}""");
            }
            catch (OryqException ex) when (ex.Status == OryqStatus.ErrDuplicateKey)
            {
                dupCaught = true;
            }
            Assert(dupCaught, "expected ErrDuplicateKey");
            wtxDup.Rollback();
            bool finalizedCaught = false;
            try
            {
                wtxDup.InsertJson("users", """{"_id": 5}""");
            }
            catch (OryqException ex) when (ex.Status == OryqStatus.ErrInvalidArgument)
            {
                finalizedCaught = true;
            }
            Assert(finalizedCaught, "expected ErrInvalidArgument after Rollback");
        }

        // Update, delete, drop index, drop collection
        using (var wtxMut = db.BeginWrite())
        {
            wtxMut.UpdateByIdJson("users", "1", """{"$set": {"score": 100.0}}""");
            wtxMut.DeleteByIdJson("users", "2");
            wtxMut.DropIndex("users", "email_idx");
            wtxMut.CreateCollection("temp_col");
            wtxMut.DropCollection("temp_col");
            wtxMut.Commit();
        }

        db.Checkpoint();
        db.Vacuum();
        Assert(db.IntegrityCheck(), "integrity check failed");
        db.BackupTo(backupPath);
        db.Close();

        bool closedCaught = false;
        try
        {
            db.BeginRead();
        }
        catch (OryqException ex) when (ex.Status == OryqStatus.ErrInvalidArgument)
        {
            closedCaught = true;
        }
        Assert(closedCaught, "expected ErrInvalidArgument after Close");
    }

    Assert(!File.Exists(dbPath + "-wal"), "expected .oryq-wal removed on clean close");
    using (var bdb = OryqDatabase.Open(backupPath))
    {
        Assert(bdb.IntegrityCheck(), "backup integrity check failed");
    }
}
finally
{
    if (Directory.Exists(tmpDir))
    {
        Directory.Delete(tmpDir, recursive: true);
    }
}

// 2. In-memory database & hybrid FTS/Vector search
bool expectHybrid = Environment.GetEnvironmentVariable("ORYQ_EXPECT_HYBRID") == "1";
using (var memDb = OryqDatabase.OpenInMemory())
{
    using (var wtx = memDb.BeginWrite())
    {
        wtx.CreateCollection("docs");
        wtx.InsertJson("docs", """{"_id": 1, "title": "Rust Embedded DB", "emb": [1.0, 0.0, 0.0]}""");
        wtx.Commit();
    }

    bool hybridReady = true;
    using (var wtx = memDb.BeginWrite())
    {
        try
        {
            wtx.CreateFtsIndex("docs", new[] { "title" }, "t_idx");
            wtx.CreateVectorIndex("docs", "emb", 3, VectorMetric.Cosine, VectorQuantization.F32, "e_idx");
            wtx.Commit();
        }
        catch (OryqException ex) when (ex.Status == OryqStatus.ErrUnsupportedFeature)
        {
            if (expectHybrid)
            {
                throw;
            }
            hybridReady = false;
        }
    }

    if (hybridReady)
    {
        using var rtx = memDb.BeginRead();
        using (var fts = rtx.SearchFts("docs", "t_idx", "rust", 5))
        {
            Assert(fts.Next(), "expected FTS hit");
            Assert(fts.GetInt64("_id") == 1, "FTS id mismatch");
            Assert(fts.GetScore() > 0.0, "expected positive BM25 score");
        }
        using (var vec = rtx.SearchVector("docs", "e_idx", new[] { 0.95f, 0.05f, 0.0f }, 1))
        {
            Assert(vec.Next(), "expected vector hit");
            Assert(vec.GetInt64("_id") == 1, "vector id mismatch");
            Assert(vec.GetScore() >= 0.0, "expected non-negative distance");
        }
    }
}

Console.WriteLine("OryqDb .NET P/Invoke tests passed!");
