# OryqDB Subsystem Specification: Cross-Platform Portability & Compatibility Policy

* **Author:** Ayesh Rodrigo (@ayeshrodz, https://github.com/ayeshrodz)
* **Date:** 2026-11-18
* **Master Design Reference:** [docs/designs/oryqdb.md](./designs/oryqdb.md)

* **Last Reviewed:** 2026-11-19

---

> **Source ownership:** [Modular technical specification](designs/modular-architecture.md) and [module catalog](modules/README.md) own crate/file placement and extension boundaries. This subsystem document continues to own its behavioral/byte contract; earlier core-relative module paths map through the new ownership table.

## 1. Purpose

This document specifies OryqDB's cross-platform support matrix, OS-specific filesystem synchronization and locking abstractions (`storage/pal.rs`), binary file portability rules across CPU architectures, and forward/backward format compatibility guarantees (`required_read_features` and `required_write_features`).

## 2. Architecture

OryqDB isolates all operating-system-specific filesystem and locking behavior inside a single internal platform abstraction module (`crates/oryq-core/src/storage/pal.rs`) implementing the `StorageIo` VFS trait (`crates/oryq-core/src/storage/vfs.rs`). All higher subsystems (`storage/page.rs`, `storage/btree.rs`, `document/`, `transaction/`, `index/`, `query/`) are 100% platform-independent Rust.

### Target Platform Matrix

| Operating System | CPU Architecture | Tier | Build Artifacts Produced |
| :--- | :--- | :--- | :--- |
| **Linux** (glibc & musl) | `x86_64` | Tier 1 (Primary Dev & CI) | `liboryq.so`, `liboryq.a`, `oryq` |
| **Linux** (glibc & musl) | `aarch64` (ARM64) | Tier 1 | `liboryq.so`, `liboryq.a`, `oryq` |
| **macOS** | `aarch64` (Apple Silicon) & `x86_64` | Tier 1 | `liboryq.dylib`, `liboryq.a`, `oryq` |
| **Windows** (MSVC) | `x86_64` | Tier 1 | `oryq.dll`, `oryq.lib`, `oryq.exe` |
| **Windows** (MSVC) | `aarch64` (ARM64) | Tier 2 (Target; native verification pending) | `oryq.dll`, `oryq.lib`, `oryq.exe` |
| **Android / iOS / WASI** | `aarch64` / `wasm32` | Future Target (Phase 3+) | Static/cdylib artifacts via `StorageIo` VFS |

## 3. Public Interfaces

* `StorageIo` trait (`read_at`, `write_at`, `sync_data`, `sync_dir`, `file_len`, `set_len`, `lock_byte_range`, `try_lock_byte_range`, `unlock_byte_range`).
* Page 0 compatibility bitmasks: `format_version: u16`, `required_read_features: u64`, `required_write_features: u64`.

## 4. Internal Data Structures

### 4.1 Platform-Specific I/O & Durability Primitives (`storage/pal.rs`)

| Capability | Linux Implementation | macOS Implementation | Windows Implementation |
| :--- | :--- | :--- | :--- |
| **Positioned Read / Write** | `std::os::unix::fs::FileExt::{read_at, write_at}` (`pread64` / `pwrite64`) | `std::os::unix::fs::FileExt::{read_at, write_at}` (`pread` / `pwrite`) | `std::os::windows::fs::FileExt::{seek_read, seek_write}` (`ReadFile` / `WriteFile` with `OVERLAPPED` offset) |
| **Durable Data Flush (`sync_data`)** | `fdatasync(fd)` (plus parent directory `fsync` on initial `.oryq` / `.oryq-wal` file creation) | `fcntl(fd, F_FULLFSYNC)` in FullSync; reject unavailable guarantees with `UnsupportedFeature`. Use `fsync` only under explicitly selected `DurabilityMode::OsBuffered` | `FlushFileBuffers(handle)` |
| **Byte-Range Advisory Locking** | `fcntl(fd, F_OFD_SETLK)` (Open File Description locks on Linux $\ge 3.15$, falling back to process-scoped `F_SETLK` guarded by the in-process `(st_dev, st_ino)` registry) | `fcntl(fd, F_SETLK)` guarded by the in-process `(st_dev, st_ino)` handle registry | `LockFileEx(handle, LOCKFILE_FAIL_IMMEDIATELY \| LOCKFILE_EXCLUSIVE_LOCK, ...)` and `UnlockFileEx` |
| **Unique File Identity** | `(MetadataExt::dev(), MetadataExt::ino())` | `(MetadataExt::dev(), MetadataExt::ino())` | `(volume_serial_number, file_index_high, file_index_low)` via `GetFileInformationByHandle` |

### 4.2 Format Compatibility & Feature Bitmasks

Page 0 headers carry two 64-bit feature bitmasks to allow safe evolution across releases:
* **`required_read_features: u64`:** Bitmask of structural format features required to parse any page or catalog entry (e.g., future page-level compression or AEAD encryption). If `(header.required_read_features & !SUPPORTED_READ_FEATURES) != 0`, `Database::open()` rejects the file with `OryqError::UnsupportedFeature`.
* **`required_write_features: u64`:** Bitmask of index or write-path features active in the database (Bit 0 = `FEATURE_FTS`, Bit 1 = `FEATURE_VEC`). If a binary lacks a bit present in a collection's `required_write_features`, read transactions succeed while write transactions targeting that collection fail with `OryqError::UnsupportedFeature`.

FullSync is the default durable mode: successful commit requires the platform's documented durable file flush and durable directory publication on creation. OsBuffered explicitly offers weaker power-loss durability; process-crash visibility and power-loss persistence are separate claims. Never silently downgrade. The platform table is a target matrix, not a record of successful tests. Live file copies are unsupported: use backup or close/checkpoint before copying. Copying `.oryq` alone while a WAL is active can omit committed data. Shared-process mode remains disabled until its post-baseline protocol is verified.

## 5. Invariants

1. **Bit-Identical Cross-Platform File Portability:** A checkpointed `.oryq` file (or `.oryq` + `.oryq-wal` pair) copied from any supported OS/CPU (e.g., Linux `x86_64`) to any other supported OS/CPU (e.g., Windows `x86_64` or macOS `aarch64`) must open, pass `oryq integrity-check`, and return identical query results without conversion utilities.
2. **No Platform-Native Endianness or Struct Padding:** All on-disk fields use explicit `to_le_bytes` / `from_le_bytes` (or big-endian memcomparable key encoding) at fixed byte offsets defined in [docs/file-format.md](./file-format.md).
3. **POSIX `close()` Lock Protection:** No code path in OryqDB may open and close a temporary file descriptor to an already-open `.oryq` or `.oryq-shm` file inside the same process; all lock probes consult the process-global file identity registry first.

## 6. Error Handling

* Opening a file with `format_version > 1` or unknown `required_read_features` returns `OryqError::UnsupportedFeature` with the exact unsupported version or bitmask.
* Platform lock contention returns `OryqError::DatabaseLockedByProcess` rather than blocking indefinitely (unless a configurable `lock_timeout_ms` is specified).

## 7. Concurrency Behavior

* Positioned I/O (`pread` / `pwrite` on Unix; `OVERLAPPED` offset reads/writes on Windows) allows multiple reader threads and the group-commit writer thread to read from and write to the same OS file handle concurrently without mutating a shared kernel file seek cursor.

## 8. Performance Characteristics

* **macOS `F_FULLFSYNC` vs. `fsync`:** On macOS, standard `fsync()` does not flush the drive controller volatile write cache; `fcntl(F_FULLFSYNC)` guarantees physical media persistence (~1–3 ms per flush), which is amortized across concurrent writers by OryqDB's zero-delay leader-follower group-commit queue.
* **Windows File Handle Sharing:** Files are opened with `FILE_SHARE_READ | FILE_SHARE_WRITE` on Windows so byte-range locks via `LockFileEx` control concurrency uniformly across platforms.

## 9. Known Limitations

* Linking static `liboryq.a` / `oryq.lib` into a C application requires linking standard OS system libraries (`-lpthread -ldl -lm` on Linux; `ws2_32.lib userenv.lib advapi32.lib ntdll.lib` on Windows MSVC), which will be documented in `docs/ffi.md` and `pkg-config` / CMake metadata.
* 32-bit architectures (`i686`, `armv7`) are not Tier 1 targets; file offsets (`u64`) are converted via checked `usize::try_from` to prevent silent 32-bit truncation.

## 10. Testing Methodology

* **Multi-OS CI Matrix (`.github/workflows/ci.yml`):** Executes `cargo test --workspace` and the C ABI integration binary on `ubuntu-latest`, `macos-latest` (ARM64), and `windows-latest`.
* **Cross-Platform Golden Artifact Test (`tests/portability/endian_and_format_compat.rs`):** Commits a canonical `.oryq` + `.oryq-wal` binary test fixture into `tests/fixtures/` containing every `OryqTypeTag`, overflow chain, delta patch, compound index, and multi-key index, and verifies that Linux, macOS, and Windows builds read, validate, and mutate the fixture with identical page checksums.
