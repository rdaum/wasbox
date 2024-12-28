### wasbox

This is a minimal and primitive WebAssembly virtual machine for running WASM modules in a sandboxed environment.

It is written from scratch in Rust and is designed to be as simple as possible, and to support a simplified API and
workflow for some specific use cases I have in mind for my own projects.

In particular, I need a wasm runtime that can do the following for another project I'm working on:

- Be suspended and resumed at will, and the contents of stack frames easily serialized.
- Be `Send` across threads.
- Cooperate / interoperate with other VMs co-hosted in the same runtime
- Easily bind to native functions and other services within that runtime.
- Be easy to understand and modify by _me_.
- No fancy stuff with funky lifetimes, just simple code being simple and living the simple life.

I don't need it to be JIT-blazing fast, just reasonable. I don't need a pile of extensions yet. I don't need `wasi` and
similar. I just want an embedded VM for running a subset of programs. And I banged my head against `wasmtime` and
`wasmer`
on and off for many months before just getting annoyed enough that I decided to try my hand at writing my own.

**But you shouldn't use it. At least not yet.**

### status

It can currently do the following

- Decode (but not link or run) the entirety of the 'wast' test
  suite (see `tests/testsuite`) into a Module structure
- Scan and decode the core WASM opcodes into a set of stack-based VM
  opcodes.
- Execute said opcodes in stack frames that run against a `Memory`
  which is for now a statically sized (un growable) slice.
- Execute the above well enough to run the included `itoa`
  function in `itoa.wat` / `itoa.wasm`. Produces the expected
  results.

Notably missing (and this is just the start of the list):

- Import binding. And likely a pile of of "link" time features.
- Proper error handling in a bunch of places -- panics all over the
  place in error conditions. (Needs a pile of error enums, etc.)
- Growable memory.
- Testing against anything other than "itoa". Will for _sure_ fail
  on anything but the simplest programs right now as it likely has a
  pile of bugs
- Does not support vectors/simd, reference types, tables, GC
  extensions, etc. etc. etc.
- Optimization.

### license

GPL 3.0.

Copyleft, free software.

If you want (for some crazy reason) to use this, you can, but you have to share your changes.
