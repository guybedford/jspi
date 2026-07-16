# JSPI

Safe JSPI primitives for Rust.

_Manages the Rust shadow stack for blocking JS operations._

## Background

In JSPI WebAssembly function exports wrapped with `WebAsembly.promising`
will suspend execution when calling a WebAssembly function import wrapped
with `WebAssembly.Suspending`. The Wasm engine manages the WebAssembly
stack being saved and restored to permit reentrancy while the suspended
stack remains waiting.

LLVM-compiled code also uses a spill "shadow" stack in linear memory managed
via the `__stack_pointer` global, which the Wasm engine doesn't know about.
This stack thus gets silently corrupted.

See Hood Chatham's blog post on the Pyodid blog on this topic for more information -
[Integrating JSPI with the WebAssembly C Runtime](https://blog.pyodide.org/posts/jspi-with-c-runtime/).

## API

1. An `unsafe jspi::blocking_call(foreign_suspending_fn, (args,...,))`
  which will handle storing and restoring the stack around a blocking JSPI
  foreign JS function, including safe support for unwinds. Misplacement is
  denied dynamically before anything runs; the `unsafe` is the caller
  vouching for the foreign function itself — a genuine suspending import
  (or non-suspending call), unit return, no throw into the resume site, no
  panic after its suspension point.
2. An `unsafe jspi::enter_promising(|| {})` activation scope, called as the
  first statement of every promising-entered function. Its `'static + Send`
  bound guarantees mutable borrows from outside the JSPI stack range are
  never held across suspension points; its `unsafe` carries what types
  cannot see — that the activation really is promising-entered, and that
  the dependency tree tolerates multiple parked activations interleaving
  under one shared `ThreadId` (no thread-identity-keyed reentrancy
  assumptions).

1. For `WebAssembly.promising` an `unsafe jspi::enter_promising(|| {})`
  scope which allows suspension. It is unsafe, because it may only be
  created as the first statement within a `WebAssembly.promising` function. It
  bounds the closure with `'static + Send` which guarantees that borrows
  outside the JSPI stack range are never held across suspension points.
1. For `WebAssembly.Suspending`, an `unsafe { jspi::blocking_call(foreign_suspending_fn, (args,...,))`
  wrapper fuction to handle saving and restoring the shadow stack before and
  after the call, including support for safe foreign unwinds. It can only be
  called from within a `jspi::enter_promising` closure, and is unsafe to
  ensure that it is indeed a `

## Example

```rust
// Imported function wrapped in JS with WebAssembly.Suspending
#[link(wasm_import_module = "env")]
extern "C-unwind" {
    fn foreign_suspending_fn(x: i32);
}

// Exported function wrapped in JS with WebAssembly.promising
#[no_mangle]
pub extern "C" fn promising_function(a: i32, b: i32) -> i32 {
  // This must be the first and last statement of the WebAssembly.promising function
  unsafe {
      jspi::enter_promising(|| {
          // ... safe Rust code...
      
          unsafe {
            jspi::blocking_call(foreign_suspending_fn, (5,));
          };

          // ... safe Rust code...
      })
  }
}
```

Link with `-C link-args=-sJSPI`.

Fully supports nesting - blocked calls may in turn re-enter the runtime,
execute their own `jspi::enter_promising` and `jspi::blocking_call` and
these stacks will compose logically. Safety invariants are maintained.

## Caveats

- Currently only supports `wasm32-unknown-emscripten`, with wassm-bindgen support planned.
- All `jspi::enter_promising` "threads" still share the same `ThreadId`.
  Blocking calls will interact with shared TLS, for example `RefCell` guards.
- Deadlocks and compute exhaustion are possible.

## Testing

`chomp test` (https://chompbuild.com/) runs all tests.
