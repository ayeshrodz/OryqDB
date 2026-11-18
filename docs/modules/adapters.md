# Module: FFI and CLI Adapters

* **Owner:** `oryq-ffi and oryq-cli`
* **State:** Planned; interfaces are design contracts, not compiled Rust
* **Date:** 2026-11-19
* **Authority:** [Modular technical specification](../designs/modular-architecture.md)

## 1. Purpose

Keep language/transport ownership and presentation separate from database rules. New query/update syntax is forwarded through the common request model, not reimplemented by each adapter.

## 2. Architecture

Each imports oryq-core only among production workspace crates. Core re-exports the supported request/value/error types they need.

## 3. Public Interfaces

Normative receiver/lifetime, registration and request details are in [the contract appendix](../designs/modular-architecture.md#9-normative-interface-and-registration-appendix); this guide owns local responsibilities and file inventory.

FFI keeps stable symbol/status/lifecycle semantics from ffi.md. CLI exposes core APIs with explicit offline/default exclusive access. Both pass versioned payloads/options to core; neither selects a concrete index/query stage.

## 4. Internal Data Structures and Owned Files

FFI handle state plus retained core ownership; CLI argument and rendering types. No duplicate AST evaluators, file I/O backend, catalog state or provider registry in adapters.

Unless qualified with a crate name, paths are relative to the owning crate. Each production crate also owns `Cargo.toml` (allowlisted dependencies), `README.md` (ports and local commands), and `src/lib.rs` or `src/main.rs` (entrypoint).

| File or file set | Responsibility |
| :--- | :--- |
| `oryq-ffi/src/{lib,guard,status,diagnostics}.rs` | Exported functions, panic boundaries, fixed status mapping and TLS diagnostics. |
| `oryq-ffi/src/handles/{mod,database,transaction,cursor,document}.rs` | Opaque handle state, child counts, ownership and pointer lifetime checks. |
| `oryq-ffi/src/api/{mod,database,collection,transaction,document,cursor,admin}.rs` | Group C functions by user-facing API; call only core behavior. |
| `oryq-ffi/build.rs; include/oryq.h` | Generate/check the stable C header from its owned declarations; never hand-edit a generated header as a second source. |
| `oryq-ffi/tests/{header,lifecycle,panic}.rs; tests/c/{api,lifecycle}.c` | Compile/link native C/C++, execute ownership/error scenarios and verify panic return-type fallback. |
| `oryq-cli/src/{main,app,output}.rs` | Process entry, argument parsing and presentation. |
| `oryq-cli/src/commands/{mod,registry,info,collections,crud,indexes,backup,integrity,inspect}.rs` | Owner-local command registration and one workflow area per command. |
| `oryq-cli/tests/{commands,locking,output}.rs` | CLI behavior, output/error stability and exclusive access conflicts. |

## 5. Invariants

Cursors retain transaction state, completion or transaction mutation with children returns BUSY, txn_close frees the handle, and getters invalidate on next/close. Stable symbols remain even when feature support is absent. Direct dependency on internal domain crates is rejected.

## 6. Error Handling

Status translation lives in status.rs, TLS messages in diagnostics.rs; getters/destructors have return-type-specific panic fallbacks. CLI presents core errors without claiming unsupported live access.

## 7. Concurrency Behavior

FFI follows database sharing and transaction/child external serialization rules. Closing a database handle does not invalidate retained child state; arbitrary forged C pointers are outside the ABI preconditions.

## 8. Performance Characteristics

Counted UTF-8/binary slices and lending getters avoid forced JSON on scalar reads; complex raw/overflow export uses charged cursor-owned storage.

## 9. Known Limitations and Change Boundary

A new stable C symbol requires FFI/header/test changes even when its underlying algorithm is modular. A new CLI command may need a core API if it exposes new behavior; that is an explicit shared boundary change.

## 10. Testing Methodology

W11/W15: native linked lifecycle and panic tests, failed-open diagnostics, output initialization, stable exports across feature builds, CLI owner-lock refusal and offline backup.

[Development packages and ledger](../MODULE_DEVELOPMENT_PLAN.md) · [Product acceptance gates](../ROADMAP.md)
