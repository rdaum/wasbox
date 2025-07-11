### wasbox

A minimal WebAssembly virtual machine designed to be embedded within other Rust applications and runtimes.

Written from scratch in Rust for scenarios where you need to run WASM modules as part of a larger system:

- Suspend and resume execution with serializable stack frames
- `Send` across threads for multi-threaded host applications
- Co-host multiple VMs within the same runtime
- Easy binding to native functions and host services
- Simple, understandable codebase without complex lifetimes

I'm not focused on amazing performance, JITting, running big programs, WASI, fancy extensions or generally for running
your dumb "smart contracts" - I'm just aiming for a simple solid VM for integrating bits of WASM execution into existing
tools and applications.

**Work in progress - not ready for production use.**

### status

Recent work has fixed major execution engine issues and expanded test coverage. Several fundamental WASM test
suites now pass:

- ✅ **i32/i64 operations** - arithmetic, comparison, bitwise ops
- ✅ **f32/f64 operations** - arithmetic with proper IEEE 754 compliance
- ✅ **local variables** - get/set/tee operations
- ✅ **constants** - all constant value operations
- ✅ **control flow** - if/else, block, loop, br/br_if/br_table
- ✅ **function calls** - direct calls and call_indirect with type checking
- ✅ **memory ops** - load/store for modules with memory
- ✅ **reference types** - complete implementation
- ✅ **WAST test support** - comprehensive test runner with trap handling

Still missing (partial list):

- Import/export binding and linking features
- SIMD, GC extensions, some others
- Many edge cases and complex control flow scenarios
- Optimization and performance tuning

### license

GPL 3.0.

Copyleft, free software.

If you want (for some crazy reason) to use this, you can, but you have to share your changes.
