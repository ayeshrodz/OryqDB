# Official C# (.NET P/Invoke) Bindings for OryqDB

Zero-NuGet-dependency .NET 8 P/Invoke bindings for **OryqDB** over **C ABI v1.2** (`include/oryq.h` + `liboryq`), featuring `SafeHandle`-managed lifetimes and automatic cross-platform library discovery (`ORYQ_LIB_PATH` or `target/{release,debug}`).

## Build & Run Tests

```bash
# 1. Build the native C ABI library (add `--features fts,vec` for hybrid search)
cargo build -p oryq-ffi --features fts,vec

# 2. Run .NET tests
ORYQ_EXPECT_HYBRID=1 dotnet run --project bindings/dotnet/OryqDb.Tests
```

## Quick Example

```csharp
using OryqDb;

using var db = OryqDatabase.OpenInMemory(); // or OryqDatabase.Open("app.oryq")
using (var wtx = db.BeginWrite())
{
    wtx.CreateCollection("users");
    wtx.InsertJson("users", """{"_id": 1, "name": "Ada"}""");
    wtx.Commit();
}
```

## License

MIT
