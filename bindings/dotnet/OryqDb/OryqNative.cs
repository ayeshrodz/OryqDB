using System.Reflection;
using System.Runtime.InteropServices;

namespace OryqDb;

internal static class OryqNative
{
    private const string LibName = "oryq";

    static OryqNative()
    {
        NativeLibrary.SetDllImportResolver(typeof(OryqNative).Assembly, ResolveLibrary);
    }

    private static IntPtr ResolveLibrary(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != LibName)
        {
            return IntPtr.Zero;
        }

        var envPath = Environment.GetEnvironmentVariable("ORYQ_LIB_PATH");
        if (!string.IsNullOrEmpty(envPath) && NativeLibrary.TryLoad(envPath, out var envHandle))
        {
            return envHandle;
        }

        string[] names = OperatingSystem.IsWindows()
            ? new[] { "oryq.dll", "liboryq.dll" }
            : OperatingSystem.IsMacOS()
                ? new[] { "liboryq.dylib" }
                : new[] { "liboryq.so" };

        // Walk up from assembly directory to find workspace target/{release,debug}
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        for (int depth = 0; depth < 10 && dir != null; depth++, dir = dir.Parent)
        {
            var targetDir = Path.Combine(dir.FullName, "target");
            if (!Directory.Exists(targetDir))
            {
                continue;
            }
            var candidates = new List<FileInfo>();
            foreach (var profile in new[] { "release", "debug" })
            {
                foreach (var name in names)
                {
                    var full = Path.Combine(targetDir, profile, name);
                    if (File.Exists(full))
                    {
                        candidates.Add(new FileInfo(full));
                    }
                }
            }
            candidates.Sort((a, b) => b.LastWriteTimeUtc.CompareTo(a.LastWriteTimeUtc));
            foreach (var cand in candidates)
            {
                if (NativeLibrary.TryLoad(cand.FullName, out var h))
                {
                    return h;
                }
            }
        }

        return IntPtr.Zero;
    }

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern IntPtr oryq_version();

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint oryq_abi_version();

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern IntPtr oryq_last_error_message();

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void oryq_free_string(IntPtr ptr);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_open([MarshalAs(UnmanagedType.LPUTF8Str)] string pathUtf8, out IntPtr outDb);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_close(IntPtr db);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_checkpoint(IntPtr db);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_vacuum(IntPtr db);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_backup_to(IntPtr db, [MarshalAs(UnmanagedType.LPUTF8Str)] string destPathUtf8);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_db_integrity_check(IntPtr db, [MarshalAs(UnmanagedType.I1)] out bool outOk);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_read_txn_begin(IntPtr db, out IntPtr outTxn);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_read_txn_close(IntPtr txn);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_write_txn_begin(IntPtr db, out IntPtr outTxn);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_write_txn_commit(IntPtr txn);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_write_txn_rollback(IntPtr txn);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_create_collection(IntPtr txn, [MarshalAs(UnmanagedType.LPUTF8Str)] string name);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_drop_collection(IntPtr txn, [MarshalAs(UnmanagedType.LPUTF8Str)] string name);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_create_index(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string fieldPath,
        int sortOrder,
        [MarshalAs(UnmanagedType.I1)] bool unique,
        [MarshalAs(UnmanagedType.I1)] bool sparse,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? nameOrNull);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_drop_index(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string indexName);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_create_fts_index(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string fieldsCsv,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? nameOrNull);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_create_vector_index(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string fieldPath,
        uint dimensions,
        int metric,
        int quantization,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? nameOrNull);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_insert_json(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string docJson);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_update_by_id_json(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string idJson,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string updateJson);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_delete_by_id_json(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string idJson);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_find_json(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? filterJson,
        out IntPtr outCursor);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_aggregate_json(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string pipelineJson,
        out IntPtr outCursor);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_search_fts(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string indexName,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string queryText,
        nuint limit,
        out IntPtr outCursor);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_search_vector(
        IntPtr txn,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string collection,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string indexName,
        [In] float[] queryVec,
        nuint queryLen,
        nuint topK,
        out IntPtr outCursor);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_next(IntPtr cursor);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_next_json(IntPtr cursor, out IntPtr outJson);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_get_i64(
        IntPtr cursor,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string field,
        out long outVal);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_get_f64(
        IntPtr cursor,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string field,
        out double outVal);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_get_bool(
        IntPtr cursor,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string field,
        [MarshalAs(UnmanagedType.I1)] out bool outVal);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_get_utf8(
        IntPtr cursor,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string field,
        out IntPtr outPtr,
        out nuint outLen);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_get_score(IntPtr cursor, out double outScore);

    [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
    internal static extern int oryq_cursor_close(IntPtr cursor);
}
