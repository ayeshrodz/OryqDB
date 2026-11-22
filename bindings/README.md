# OryqDB Official Language Bindings & Native Examples (`v0.11.0`)

All language bindings target the stable, panic-safe **OryqDB C ABI v1.1** (`include/oryq.h` + `liboryq`).

| Language | Directory | Requirements | Status |
| :--- | :--- | :--- | :--- |
| **Python** (`ctypes` package) | [bindings/python](python/README.md) | Python 3.9+ (stdlib only, zero third-party deps) | Tested in CI (Linux, macOS, Windows) |
| **C99** native example | [bindings/c/quickstart.c](c/quickstart.c) | Any C99 compiler (`cc`, `clang`, `gcc`, MSVC) | Compiled & executed in CI |
| **C++17** RAII example | [bindings/cpp/quickstart.cpp](cpp/quickstart.cpp) | Any C++17 compiler | Compiled & executed in CI |

## Building `liboryq`

```bash
cargo build --release -p oryq-ffi
```

This produces:
- **Linux:** `target/release/liboryq.so` (and `liboryq.a`)
- **macOS:** `target/release/liboryq.dylib` (and `liboryq.a`)
- **Windows:** `target/release/oryq.dll` (and `oryq.dll.lib`)

For full ABI documentation and building bindings in additional languages (Go `cgo`, Node.js `napi`/`ffi-napi`, C# P/Invoke, Zig), see [docs/guides/c-abi-guide.md](../docs/guides/c-abi-guide.md).
