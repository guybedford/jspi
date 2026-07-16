# JSPI

JSPI spill-stack primitives for `wasm32-unknown-emscripten`: blocking
calls to async JavaScript from Rust, without stack corruption.

Experimental; not published.

## The Problem

JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends the
engine-managed native wasm stack per activation. LLVM-compiled code also
uses a spill ("shadow") stack in linear memory through the `__stack_pointer`
global, which JSPI knows nothing about. This can cause silent stack corruption.

See Hood Chatham's blog post on this topic for more information -
[Integrating JSPI with the WebAssembly C Runtime](https://blog.pyodide.org/posts/jspi-with-c-runtime/).

We adopt Chatham's solution with Rust primitives here to provide the
following invariants:

1. A `jspi::blocking_call(foreign_suspending_fn, (args,...,))` which will
  handle storing and restoring the stack around a blocking JSPI foreign JS
  function, including safe support for unwinds. Safe, because misuse is
  denied dynamically before anything runs.
2. An `unsafe jspi::enter_promising(|| {})` activation scope, called as the
  first statement of every promising-entered function. Its `'static + Send`
  bound guarantees mutable borrows from outside the JSPI stack range are
  never held across suspension points; its `unsafe` carries what types
  cannot see — that the activation really is promising-entered, and that
  the dependency tree tolerates multiple parked activations interleaving
  under one shared `ThreadId` (no thread-identity-keyed reentrancy
  assumptions).

## Usage

```rust
// first statement of a promising-entered function (glue entry, or main
// itself — -sJSPI auto-wraps main):
unsafe {
    jspi::enter_promising(|| {
        // ordinary Rust; blocking_call saves the stack, executes the call,
        // then restores the stack on completion or exception.
        jspi::blocking_call(glue_fetch, (url_ptr as usize, url_len));
    })
}
```

Link with `-C link-args=-sJSPI`. Run on a JSPI-enabled host (Node ≥ 26).

Supports nesting fine - blocked calls may in turn re-enter the runtime,
execute their own `jspi::enter_promising` and `jspi::blocking_call` and
these stacks will compose logically. Safety invariants are maintained.

## Caveats

- All `jspi::enter_promising` "threads" still share the same `ThreadId`.
  Blocking calls will interact with shared TLS, for example `RefCell`
  guards held across a suspension fail loud on contention;
  thread-identity-keyed reentrancy (e.g. `parking_lot::ReentrantMutex`) is
  unsound — this is what `enter_promising`'s `unsafe` asserts against.
- Deadlocks and compute exhaustion are possible.

## Testing

`chomp test` (https://chompbuild.com/) runs all tests.
