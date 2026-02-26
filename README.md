# wasbox

A small WebAssembly interpreter written in Rust for embedding into other applications.

This runtime is intentionally interpreter-first: simple control flow, explicit runtime state, and host-driven execution.
The primary goal is budget-controlled execution inside larger runtimes, where the host can run WASM in bounded slices and resume later.
It is not trying to be a high-performance JIT or a WASI runtime.

**Work in progress. Not production-ready.**

## Current Capabilities

- Core numeric execution: `i32`, `i64`, `f32`, `f64` arithmetic and comparisons.
- Control flow: `block`, `loop`, `if/else`, `br`, `br_if`, `br_table`, `return`, traps.
- Variables and functions: locals/globals, direct calls, `call_indirect` with signature checks.
- Memory: linear memory load/store, memory size/grow, and multi-memory indexing for 32-bit memories.
- Tables and references: `funcref`/`externref`, `table.get`, `table.set`, `ref.null`, `ref.func`, `ref.is_null`, `ref.eq`, `ref.as_non_null`.
- Bulk memory/table ops: `memory.init`, `data.drop`, `memory.copy`, `memory.fill`, `table.init`, `elem.drop`, `table.copy`, `table.grow`, `table.size`, `table.fill`.
- Module linking model: imports/exports, start function execution, multi-module execution state.
- Incremental execution API: `ExecutionState::run_for_ticks(ticks)` returns `Complete` or `Paused` and can be resumed.
- Shared imported runtime state: imported memories and tables share backing storage across modules.

## Test Status

Current branch status:

- `cargo test --lib` passes (`21` unit tests).
- WAST integration coverage is substantial but partial and still evolving.

Important scope note:

- The WAST harness currently runs a curated subset of the official suite.
- Repository has `257` `.wast` files under `tests/testsuite`.
- Harness currently wires `130` of those files as executable tests.

## Known Limitations

- No WASI support.
- Many proposals are not implemented: SIMD, threads, GC, exceptions, typed function references (`call_ref` family), and others.
- Memory64 is not supported.
- Runtime state is not designed for cross-thread `Send` usage in its current form.
- No stable serialization/deserialization API for execution state yet.

## WAST Expansion Focus

The current WAST expansion work is targeting:

- Core MVP behavior and parser robustness.
- Bulk memory/table semantics and edge cases.
- Reference-types behavior that does not require typed function references (`call_ref`) or GC.

WAST files tied to unsupported proposals (SIMD, GC, exceptions, typed-function-references/tail-call, memory64) are intentionally left out for now.

## Benchmarking

This repo includes two benchmark entry points:

- Cross-runtime runner (`wasbox` vs `wasmtime`):
  - `cargo run --release --bin bench_runner -- --engine both --warmup 250 --iters 2000`
  - Optional: `--case <name>` (repeatable), `--list`, and `--csv <path>`.
- Criterion benches (internal `wasbox` tuning):
  - `cargo bench --bench runtime`

The cross-runtime runner uses a small built-in suite of deterministic WAT programs and reports:

- module instantiate time
- total execute time over N iterations
- mean execute time per iteration

## License

GPL-3.0.

If you distribute modifications, you must share source under the same license terms.
