# OryqDB Official Language Bindings & Native Examples (`v0.14.0`)

All official language bindings target the stable, panic-safe **OryqDB C ABI v1.2** (`include/oryq.h` + `liboryq`), supporting single-file `.oryq` persistence, auto-seal on close, `:memory:` ephemeral databases, `VACUUM`, online backup, and optional hybrid FTS + Vector search (`--features fts,vec`).

| Language | Directory | Requirements | Status |
| :--- | :--- | :--- | :--- |
| **Python** (`ctypes` package) | [bindings/python](python/README.md) | Python 3.9+ (stdlib only, zero third-party deps) | Tested in CI (Linux, macOS, Windows) |
| **Go** (`cgo` package) | [bindings/go](go/README.md) | Go 1.21+ & C compiler (`cgo`) | Tested in CI (Linux, macOS) |
| **Node.js** (Node-API addon + TypeScript) | [bindings/node](node/README.md) | Node.js 18+ & C99 compiler (zero npm deps) | Tested in CI (Linux, macOS) |
| **C# / .NET** (P/Invoke + `SafeHandle`) | [bindings/dotnet](dotnet/README.md) | .NET 8 SDK (zero NuGet deps) | Tested in CI (Linux, macOS, Windows) |
| **C99** native example | [bindings/c/quickstart.c](c/quickstart.c) | Any C99 compiler (`cc`, `clang`, `gcc`, MSVC) | Compiled & executed in CI |
| **C++17** RAII example | [bindings/cpp/quickstart.cpp](cpp/quickstart.cpp) | Any C++17 compiler | Compiled & executed in CI |

## Building `liboryq`

```bash
# Minimal core build
cargo build --release -p oryq-ffi

# Hybrid build with Full-Text Search (BM25) + Vector Similarity Search
cargo build --release -p oryq-ffi --features fts,vec
```

This produces:
- **Linux:** `target/release/liboryq.so` (and `liboryq.a`)
- **macOS:** `target/release/liboryq.dylib` (and `liboryq.a`)
- **Windows:** `target/release/oryq.dll` (and `oryq.dll.lib`)

For full ABI documentation and building custom bindings in additional languages (Zig, Swift, Java/JNI), see [docs/guides/c-abi-guide.md](../docs/guides/c-abi-guide.md).
