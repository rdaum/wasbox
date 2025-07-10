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

Recent work has implemented structured control flow and fixed major execution issues. Several fundamental WASM test
suites now pass:

- ✅ **i32/i64 operations** - arithmetic, comparison, bitwise ops
- ✅ **local variables** - get/set/tee operations
- ✅ **constants** - all constant value operations
- ✅ **control flow** - if/else, block, loop, br/br_if/br_table
- ✅ **function calls** - direct calls and basic call_indirect
- ✅ **memory ops** - load/store for modules with memory

Still missing (partial list):

- Import/export binding and linking features
- Reference types, tables, SIMD, GC extensions
- Many edge cases and complex control flow scenarios
- F32 NaN handling and IEEE 754 compliance
- Optimization and performance tuning

### license

GPL 3.0.

Copyleft, free software.

If you want (for some crazy reason) to use this, you can, but you have to share your changes.
