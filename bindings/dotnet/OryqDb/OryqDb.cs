using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace OryqDb;

/// <summary>Status codes mirroring <c>oryq_status</c> in <c>include/oryq.h</c> (ABI v1.2).</summary>
public enum OryqStatus
{
    Ok = 0,
    CursorDone = 1,
    ErrInvalidArgument = 2,
    ErrIo = 3,
    ErrCorruption = 4,
    ErrNotFound = 5,
    ErrDuplicateKey = 6,
    ErrLockedByProcess = 7,
    ErrSnapshotTooOld = 8,
    ErrQueryMemoryLimit = 9,
    ErrParallelArrayIndex = 10,
    ErrUnsupportedFeature = 11,
    ErrTypeMismatch = 12,
    ErrBusy = 13,
    ErrCommitOutcomeUnknown = 14,
    ErrWalLimit = 15,
    ErrTooManyReaders = 16,
    ErrTransactionMemory = 17,
    ErrNumericOverflow = 18,
    ErrNumericPrecision = 19,
    ErrInvalidQuery = 20,
    ErrDocumentTooLarge = 21,
    ErrIndexKeyTooLarge = 22,
    ErrInvalidIdType = 23,
    ErrConflictingPath = 24,
    ErrIndexNotFound = 25,
    ErrInternalPanic = 255,
}

public enum VectorMetric
{
    Cosine = 0,
    L2 = 1,
    DotProduct = 2,
}

public enum VectorQuantization
{
    F32 = 0,
    Int8 = 1,
}

public sealed class OryqException : Exception
{
    public OryqStatus Status { get; }

    public OryqException(OryqStatus status, string message)
        : base($"{status}: {message}")
    {
        Status = status;
    }

    internal static void Check(int rawStatus)
    {
        var st = (OryqStatus)rawStatus;
        if (st == OryqStatus.Ok)
        {
            return;
        }
        var ptr = OryqNative.oryq_last_error_message();
        var msg = ptr != IntPtr.Zero
            ? Marshal.PtrToStringUTF8(ptr) ?? "unknown OryqDB error"
            : "unknown OryqDB error";
        throw new OryqException(st, msg);
    }
}

internal sealed class OryqDbHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    internal OryqDbHandle(IntPtr ptr) : base(true) => SetHandle(ptr);

    protected override bool ReleaseHandle()
    {
        var st = (OryqStatus)OryqNative.oryq_db_close(handle);
        return st == OryqStatus.Ok;
    }
}

internal sealed class OryqReadTxnHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    internal OryqReadTxnHandle(IntPtr ptr) : base(true) => SetHandle(ptr);

    protected override bool ReleaseHandle()
    {
        var st = (OryqStatus)OryqNative.oryq_read_txn_close(handle);
        return st == OryqStatus.Ok;
    }
}

internal sealed class OryqWriteTxnHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    internal OryqWriteTxnHandle(IntPtr ptr) : base(true) => SetHandle(ptr);

    internal void MarkFinalized() => SetHandleAsInvalid();

    protected override bool ReleaseHandle()
    {
        var st = (OryqStatus)OryqNative.oryq_write_txn_rollback(handle);
        return st == OryqStatus.Ok;
    }
}

internal sealed class OryqCursorHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    internal OryqCursorHandle(IntPtr ptr) : base(true) => SetHandle(ptr);

    protected override bool ReleaseHandle()
    {
        var st = (OryqStatus)OryqNative.oryq_cursor_close(handle);
        return st == OryqStatus.Ok;
    }
}

/// <summary>SafeHandle-managed OryqDB database instance.</summary>
public sealed class OryqDatabase : IDisposable
{
    private readonly OryqDbHandle _handle;

    private OryqDatabase(OryqDbHandle handle) => _handle = handle;

    public static string Version()
    {
        var ptr = OryqNative.oryq_version();
        return ptr != IntPtr.Zero ? Marshal.PtrToStringUTF8(ptr) ?? string.Empty : string.Empty;
    }

    public static (ushort Major, ushort Minor) AbiVersion()
    {
        uint packed = OryqNative.oryq_abi_version();
        return ((ushort)(packed >> 16), (ushort)(packed & 0xFFFF));
    }

    public static OryqDatabase Open(string path)
    {
        OryqException.Check(OryqNative.oryq_db_open(path, out var ptr));
        return new OryqDatabase(new OryqDbHandle(ptr));
    }

    public static OryqDatabase OpenInMemory() => Open(":memory:");

    private IntPtr RequireOpen()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "database is closed");
        }
        return _handle.DangerousGetHandle();
    }

    public OryqReadTxn BeginRead()
    {
        OryqException.Check(OryqNative.oryq_read_txn_begin(RequireOpen(), out var ptr));
        return new OryqReadTxn(new OryqReadTxnHandle(ptr));
    }

    public OryqWriteTxn BeginWrite()
    {
        OryqException.Check(OryqNative.oryq_write_txn_begin(RequireOpen(), out var ptr));
        return new OryqWriteTxn(new OryqWriteTxnHandle(ptr));
    }

    public void Checkpoint() => OryqException.Check(OryqNative.oryq_db_checkpoint(RequireOpen()));

    public void Vacuum() => OryqException.Check(OryqNative.oryq_db_vacuum(RequireOpen()));

    public void BackupTo(string destPath) =>
        OryqException.Check(OryqNative.oryq_db_backup_to(RequireOpen(), destPath));

    public bool IntegrityCheck()
    {
        OryqException.Check(OryqNative.oryq_db_integrity_check(RequireOpen(), out bool ok));
        return ok;
    }

    public void Close()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            return;
        }
        OryqException.Check(OryqNative.oryq_db_close(_handle.DangerousGetHandle()));
        _handle.SetHandleAsInvalid();
        _handle.Dispose();
    }

    public void Dispose()
    {
        if (!_handle.IsClosed && !_handle.IsInvalid)
        {
            _handle.Dispose();
        }
    }
}

public sealed class OryqWriteTxn : IDisposable
{
    private readonly OryqWriteTxnHandle _handle;

    internal OryqWriteTxn(OryqWriteTxnHandle handle) => _handle = handle;

    private IntPtr RequireOpen()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "write transaction is finalized");
        }
        return _handle.DangerousGetHandle();
    }

    public void CreateCollection(string name) =>
        OryqException.Check(OryqNative.oryq_create_collection(RequireOpen(), name));

    public void DropCollection(string name) =>
        OryqException.Check(OryqNative.oryq_drop_collection(RequireOpen(), name));

    public void CreateIndex(
        string collection,
        string fieldPath,
        int sortOrder = 1,
        bool unique = false,
        bool sparse = false,
        string? name = null) =>
        OryqException.Check(OryqNative.oryq_create_index(
            RequireOpen(), collection, fieldPath, sortOrder, unique, sparse, name));

    public void DropIndex(string collection, string indexName) =>
        OryqException.Check(OryqNative.oryq_drop_index(RequireOpen(), collection, indexName));

    public void CreateFtsIndex(string collection, IEnumerable<string> fields, string? name = null) =>
        OryqException.Check(OryqNative.oryq_create_fts_index(
            RequireOpen(), collection, string.Join(",", fields), name));

    public void CreateVectorIndex(
        string collection,
        string fieldPath,
        uint dimensions,
        VectorMetric metric = VectorMetric.Cosine,
        VectorQuantization quantization = VectorQuantization.F32,
        string? name = null) =>
        OryqException.Check(OryqNative.oryq_create_vector_index(
            RequireOpen(), collection, fieldPath, dimensions, (int)metric, (int)quantization, name));

    public void InsertJson(string collection, string docJson) =>
        OryqException.Check(OryqNative.oryq_insert_json(RequireOpen(), collection, docJson));

    public void UpdateByIdJson(string collection, string idJson, string updateJson) =>
        OryqException.Check(OryqNative.oryq_update_by_id_json(RequireOpen(), collection, idJson, updateJson));

    public void DeleteByIdJson(string collection, string idJson) =>
        OryqException.Check(OryqNative.oryq_delete_by_id_json(RequireOpen(), collection, idJson));

    public void Commit()
    {
        var ptr = RequireOpen();
        _handle.MarkFinalized();
        OryqException.Check(OryqNative.oryq_write_txn_commit(ptr));
        _handle.Dispose();
    }

    public void Rollback()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            return;
        }
        var ptr = _handle.DangerousGetHandle();
        _handle.MarkFinalized();
        OryqException.Check(OryqNative.oryq_write_txn_rollback(ptr));
        _handle.Dispose();
    }

    public void Dispose() => _handle.Dispose();
}

public sealed class OryqReadTxn : IDisposable
{
    private readonly OryqReadTxnHandle _handle;

    internal OryqReadTxn(OryqReadTxnHandle handle) => _handle = handle;

    private IntPtr RequireOpen()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "read transaction is closed");
        }
        return _handle.DangerousGetHandle();
    }

    public OryqCursor Find(string collection, string? filterJson = null)
    {
        OryqException.Check(OryqNative.oryq_find_json(RequireOpen(), collection, filterJson, out var ptr));
        return new OryqCursor(new OryqCursorHandle(ptr));
    }

    public OryqCursor Aggregate(string collection, string pipelineJson)
    {
        OryqException.Check(OryqNative.oryq_aggregate_json(RequireOpen(), collection, pipelineJson, out var ptr));
        return new OryqCursor(new OryqCursorHandle(ptr));
    }

    public OryqCursor SearchFts(string collection, string indexName, string queryText, int limit = 10)
    {
        if (limit < 0)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "limit must be >= 0");
        }
        OryqException.Check(OryqNative.oryq_search_fts(
            RequireOpen(), collection, indexName, queryText, (nuint)limit, out var ptr));
        return new OryqCursor(new OryqCursorHandle(ptr));
    }

    public OryqCursor SearchVector(string collection, string indexName, float[] queryVec, int topK = 10)
    {
        if (queryVec is null || queryVec.Length == 0)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "queryVec must not be empty");
        }
        if (topK < 0)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "topK must be >= 0");
        }
        OryqException.Check(OryqNative.oryq_search_vector(
            RequireOpen(), collection, indexName, queryVec, (nuint)queryVec.Length, (nuint)topK, out var ptr));
        return new OryqCursor(new OryqCursorHandle(ptr));
    }

    public void Dispose() => _handle.Dispose();
}

public sealed class OryqCursor : IDisposable
{
    private readonly OryqCursorHandle _handle;

    internal OryqCursor(OryqCursorHandle handle) => _handle = handle;

    private IntPtr RequireOpen()
    {
        if (_handle.IsClosed || _handle.IsInvalid)
        {
            throw new OryqException(OryqStatus.ErrInvalidArgument, "cursor is closed");
        }
        return _handle.DangerousGetHandle();
    }

    public bool Next()
    {
        int raw = OryqNative.oryq_cursor_next(RequireOpen());
        if ((OryqStatus)raw == OryqStatus.CursorDone)
        {
            return false;
        }
        OryqException.Check(raw);
        return true;
    }

    public string? NextJson()
    {
        int raw = OryqNative.oryq_cursor_next_json(RequireOpen(), out var ptr);
        if ((OryqStatus)raw == OryqStatus.CursorDone)
        {
            return null;
        }
        OryqException.Check(raw);
        if (ptr == IntPtr.Zero)
        {
            return null;
        }
        try
        {
            return Marshal.PtrToStringUTF8(ptr);
        }
        finally
        {
            OryqNative.oryq_free_string(ptr);
        }
    }

    public long GetInt64(string field)
    {
        OryqException.Check(OryqNative.oryq_cursor_get_i64(RequireOpen(), field, out long val));
        return val;
    }

    public double GetDouble(string field)
    {
        OryqException.Check(OryqNative.oryq_cursor_get_f64(RequireOpen(), field, out double val));
        return val;
    }

    public bool GetBool(string field)
    {
        OryqException.Check(OryqNative.oryq_cursor_get_bool(RequireOpen(), field, out bool val));
        return val;
    }

    public string GetUtf8(string field)
    {
        OryqException.Check(OryqNative.oryq_cursor_get_utf8(RequireOpen(), field, out var ptr, out var len));
        if (len == 0)
        {
            return string.Empty;
        }
        int byteLen = checked((int)len);
        byte[] buf = new byte[byteLen];
        Marshal.Copy(ptr, buf, 0, byteLen);
        return Encoding.UTF8.GetString(buf);
    }

    public double GetScore()
    {
        OryqException.Check(OryqNative.oryq_cursor_get_score(RequireOpen(), out double score));
        return score;
    }

    public void Dispose() => _handle.Dispose();
}
