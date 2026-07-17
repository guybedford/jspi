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

1. For `WebAssembly.promising` an `unsafe jspi::enter_promising(|| {})`
  scope which allows suspension. It is unsafe, because it may only be
  created as the first statement within a `WebAssembly.promising` function. It
  bounds the closure with `'static + Send` which guarantees that borrows
  outside the JSPI stack range are never held across suspension points.
  Returns `Result<R, jspi::EnterError>`: denied with `EnterError::Nested`
  inside an already-open activation, or `EnterError::Exclusive` while an
  exclusive activation owns promising.
1. For `WebAssembly.Suspending`, an `unsafe { jspi::blocking_call(foreign_suspending_fn, (args,...,))`
  wrapper fuction to handle saving and restoring the shadow stack before and
  after the call, including support for safe foreign unwinds. It can only be
  called from within a `jspi::enter_promising` closure, and is unsafe since
  like any other foreign function call.
1. `unsafe jspi::enter_promising_exclusive(|| {})` — an `enter_promising`
  that claims exclusive ownership of promising for the activation's whole
  extent: the exclusive bit stays raised across the owner's suspensions, so
  every other enter is denied with `EnterError::Exclusive` until the owner
  completes (released on completion or unwind). Granted only from
  quiescence: parked sibling activations deny it with `EnterError::Parked`.
1. `jspi::in_promising()` / `jspi::exclusive_promising()` — two distinct
  questions: is the currently executing code inside an open activation on
  this native stack, and does an exclusive activation own promising
  (including while it is parked). `jspi::jspi_enabled()` reports whether
  the module was linked with `-sJSPI` on a supporting host.
1. `unsafe jspi::sleep(duration)` — the one suspending import the crate
  ships itself: parks the calling activation on a host timer via
  `blocking_call`.

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
      .unwrap()
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

The suite is JS-driven: `tests-module/` builds a single `-sMODULARIZE`
Emscripten module of `t_*` exports, and `tests/driver.cjs` wraps them with
`WebAssembly.promising` from node to drive genuine host-side reentrancy —
overlapping parked entries, non-LIFO and same-tick wakes, plain-call
discipline, exclusive ownership, escaped panics as rejections, and
two-instance isolation — across debug, release, no-JSPI, and a
corruption-proof lane (virtualization disabled, the scenario must fail).
