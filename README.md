# JSPI

JSPI spill-stack primitives for `wasm32-unknown-emscripten`: safe blocking
calls to async JavaScript from Rust, without stack corruption.

Experimental; not published.

## The Problem

JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends the
engine-managed native wasm stack per activation. LLVM-compiled code also
uses a spill ("shadow") stack in linear memory through the `__stack_pointer`
global, which JSPI knows nothing about. This can cause silent stack corruption.

See Hood Chatham's blog post on this topic for more information -
[Integrating JSPI with the WebAssembly C Runtime](https://blog.pyodide.org/posts/jspi-with-c-runtime/).

We adopt Chatham's solution with safe Rust primitives here to provide
the following invariants:

1. A `jspi::blocking_call(foreign_suspending_fn, (args,...,))` which will
  handle storing and restoring the stack around a blocking JSPI foreign JS
  function, including safe support for unwinds.
2. A `jspi::spawn(|| {})` context which asserts `'static + Send`, so that
  we know mutable borrows are not held outside JSPI stack range across
  suspension points.

## Usage

```rust
// in a promising-entered function (glue entry, or main itself —
// -sJSPI auto-wraps main):
jspi::spawn(|| {
    // ordinary Rust; blocking_call saves the stack, executes the call,
    // then restores the stack on completion or exception.
    jspi::blocking_call(glue_fetch, (url_ptr as usize, url_len));
})
```

Link with `-C link-args=-sJSPI`. Run on a JSPI-enabled host (Node ≥ 26).

Supports nesting fine - blocked calls may in turn re-enter the runtime,
execute their own `jspi::spawn` and `jspi::blocking_call` and these stacks
will compose logically. Safety invariants are maintained.

## Caveats

- All `jspi::spawn` "threads" still share the same `ThreadId`. While still
  formally safe, blocking calls will interact with shared TLS, for example
  `RefCell` guards.
- Deadlocks are possible and compute exhaustion are possible.

## Testing

`chomp test` (https://chompbuild.com/) runs all tests.
