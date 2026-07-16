# jspi

JSPI spill-stack primitives for `wasm32-unknown-emscripten`: safe blocking
calls to async JavaScript from Rust, without stack corruption.

Experimental; not published.

## The problem

JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends the
engine-managed native wasm stack per activation. LLVM-compiled code also
uses a spill ("shadow") stack in linear memory through the `__stack_pointer`
global, which JSPI knows nothing about: concurrently suspended activations
share one spill stack and one stack pointer, and silently corrupt each
other. The corruption needs two or more suspended activations interleaving
non-LIFO — single-suspension programs are accidentally safe, which is why
naive JSPI appears to work.

The scheme here is Hood Chatham's eager save/restore convention
[Integrating JSPI with the WebAssembly C Runtime](https://blog.pyodide.org/posts/jspi-with-c-runtime/)
reduced to primitives: every suspension saves its live stack slice to the
heap, every resumption restores it, wasm-initiated at the true resume
boundary. **The foreign call is the Suspending thing** — you declare your
own async imports and call them directly; the crate supplies only the stack
discipline that makes this sound.

## Usage

```rust
// in a promising-entered function (glue entry, or main itself —
// -sJSPI auto-wraps main):
jspi::spawn(|| {
    // ordinary Rust; blocking_call parks this activation until the
    // import's promise settles
    jspi::blocking_call(glue_fetch, (url_ptr as usize, url_len));
    let result = glue_take_result(); // plain import, post-restore
})
```

Link with `-C link-args=-sJSPI`. Run on a JSPI-enabled host (Node ≥ 26).

## API

```rust
pub fn spawn<R>(f: impl FnOnce() -> R) -> R;               // safe, full capture
pub unsafe fn stack_root<R>(f: impl FnOnce() -> R) -> R;   // optimized, marked top
pub fn blocking_call<A: BlockingArgs>(f: A::Fn, args: A);  // safe
pub fn linked() -> bool;             // -sJSPI + host support probe
pub const STACK_TOP_PAD: usize;
```

Both roots are **synchronous scopes**, not schedulers: they mark this
activation's stack extent and run the closure immediately. Every
`blocking_call` inside saves the live slice, calls `f(args)` (your
suspending import), and restores slice and stack pointer when it returns.

- `spawn` captures from the save point to the absolute stack base. Nothing
  can be under-saved, so it has no contract: any placement, any call depth,
  fat frames welcome. Cost is `O(sp → base)` bytes copied per park
  (bounded by `-sSTACK_SIZE`).
- `stack_root` captures only up to a mark measured at the root
  (`SP + STACK_TOP_PAD`, clamped to the base) — `O(live slice)` copies.
  Unsafe contract: it must be the first statement of the
  promising-entered function, the entry frame above it must stay thinner
  than the pad (over-save heals; under-save corrupts), and no nesting.

`BlockingArgs` is sealed, implemented for tuples of arity 0–4 over the wasm
ABI scalars (`u32`, `i32`, `usize`, `f64`), mapping to the unit-returning
`extern "C-unwind" fn(...)` pointer types. Scalars only, deliberately:
nothing borrowed can be smuggled into the call (a pointer is an explicit
`as usize` act), everything is `Copy` and consumed before the suspension,
and the unit return is type-enforced (results are fetched by a plain import
after the restore).

`blocking_call` is safe because the obligations rest with the import's
author at its declaration site (edition-2024 `unsafe extern` blocks allow
`safe fn` declarations). Per import:

- a genuine `__asyncjs__` suspending import — or any non-suspending call,
  for which the bracket degrades to a benign identical-bytes no-op;
- returns unit; never throws into the resume site (catch rejections in JS
  and record them for post-restore retrieval);
- panics only before its suspension point — such panics (the denial class)
  unwind cleanly through the bracket; a panic after the import returns is a
  contract violation, denied best-effort by abort.

Misplaced `blocking_call`s — outside a root, from a plain host callback,
reentrant — are denied by an internal parity counter as a catchable panic
before anything runs. On non-emscripten targets everything compiles,
`linked()` returns false, and the operations panic. `-pthread` is a
`compile_error!`.

## How it stays sound

One spill stack, shared by all activations. Function prologues push the
stack pointer down and epilogues restore it; suspension leaves it wherever
it was; whatever runs next pushes down from there and unwinds fully before
any resume fires. The single explicit SP write in the system is the restore
at a resume boundary.

The healing invariant: a running activation's frames are never written by
anyone else (single-threaded, resumes only fire from an empty native
stack), and a parked activation's slice may hold arbitrary garbage — its
heap snapshot is the truth, and its own restore is the first thing that
runs at its resume. Under eager save and eager restore, arbitrary
interleavings, non-LIFO wake orders, and overlapping slices are all
correct.

## Writing glue

`tests-glue` is a reference consumer: fiber dispatch through a
promising-wrapped trampoline, a suspending sleep, resolvable/rejectable
promises with post-restore value fetch, and plain host callbacks. The em_js
convention from Rust:

```rust
jspi::em_js_data!(
    __em_js____asyncjs__glue_sleep,
    "(ms)<::>{ return Asyncify.handleAsync(async () => { await new Promise((r) => setTimeout(r, ms)); }); }"
);

#[link(wasm_import_module = "env")]
unsafe extern "C-unwind" {
    #[link_name = "__asyncjs__glue_sleep"]
    safe fn glue_sleep_import(ms: f64);
}
pub const glue_sleep: extern "C-unwind" fn(f64) = glue_sleep_import;

pub fn sleep(ms: f64) {
    jspi::blocking_call(glue_sleep, (ms,));
}
```

Delivery notes (empirically established):

- em_js statics must live in a **lib** crate, be referenced from linked
  code (an `#[inline(never)]` anchor using `black_box`), and never use
  `#[link_section]`.
- The `__asyncjs__` name prefix on an `env` import receives
  `WebAssembly.Suspending` wrapping under `-sJSPI`; `Asyncify.handleAsync`
  in the body provides runtime keepalive across the suspension.
- Hang per-instance JS state off `Module`, never `globalThis` — two wasm
  instances in one JS context cross-wire shared state.

## Sharp edges (by design)

- All activations share one `ThreadId` and one set of `thread_local!`
  instances. Treat a `blocking_call` as a blocking syscall on a thread that
  shares TLS and thread identity with every other activation: hold nothing
  across it that you would not hold across `epoll_wait`. `RefCell` guards
  held across a park fail loud; identity-keyed reentrancy
  (`parking_lot::ReentrantMutex`) is unsound in dependent code.
- The restore writes memory behind live `&mut` borrows (byte-identical or
  healed) — observationally correct, formally the same class as
  emscripten's long-shipped ASYNCIFY.
- Cooperative only: a compute-bound activation starves the loop.
- Relies on emscripten internals (`Asyncify.handleAsync`, `wasmExports`,
  `HEAPU8`, `_emscripten_stack_restore`, keepalive hooks); validated
  against emsdk 6.0.4.

## Development

`chomp test` runs all lanes (Node ≥ 26, emsdk ≥ 6.x, `npm i -g chomp`):
debug + release suites (debug is load-bearing: `-O0` spills all locals),
a proof lane asserting the corruption regression bites with the restore
disabled, escaped-panic fatality, `-sEXIT_RUNTIME=1` keepalive, a no-JSPI
build, and two instances in one process.
