# jspi

Shared spill-stack ("shadow stack") virtualization primitives for WebAssembly
stack switching, with a JSPI + emscripten backend. Prototype status; not
published.

## The problem

JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends and resumes
the *engine-managed* native wasm stack per activation. LLVM-compiled code
(Rust and C alike) additionally keeps a spill stack in linear memory,
addressed through the `__stack_pointer` global, which the engine knows
nothing about:

- All activations share one spill-stack region and one `__stack_pointer`.
- When activation A suspends at `SP = X`, its live frames occupy `[X, topA)`.
  Any other activation that runs meanwhile inherits the global `SP` and
  pushes frames down through A's live data.
- On resume, nothing restores `__stack_pointer` to A's value.

Both effects are silent memory corruption. The corruption fires only with two
or more concurrently suspended activations interleaving non-LIFO — a single
suspended stack with shallow JS callbacks is accidentally safe, which is why
naive JSPI use appears to work in simple tests. Emscripten does not solve
this for JSPI (`-sJSPI` / `ASYNCIFY=2` only wraps exports/imports; verified
against emsdk 6.0.4). The scheme implemented here is Hood Chatham's eager
slice virtualization ("Integrating JSPI with the WebAssembly C Runtime",
Pyodide blog, 2025-07-03), with the restore moved to the wasm side (see
invariant 2 below — the JS-side restore in the original write-up has a
resume-ordering race).

## The convention

Every suspension saves its own live spill slice `[sp, top)` to a side buffer;
every resumption restores that slice and the stack pointer at the true resume
boundary:

```rust
let _activation = jspi::wasm_enter();    // first call in every promising entry,
                                         // guard held for the activation's extent
...
let id = jspi::save_stack();             // atomic with the suspend
let ret = suspending_import(..., id);    // engine switches the native stack
jspi::restore_stack(ret);                // first action after resume
```

**Self-healing invariant** (the correctness core): with eager save at every
suspend and eager restore at every resume, arbitrary interleavings, non-LIFO
wake orders, and even overlapping slices are correct. Whatever a sibling
scribbles — or stale-restores — into your range while you are suspended, your
own resume-restore heals before you run; only one activation executes at a
time (resumes only fire from an empty native stack); and a *running*
activation's frames are never overwritten. Staleness is never detected, only
unconditionally overwritten — that is what makes independent users of the
convention compose without shared bookkeeping.

## Soundness invariants (do not weaken)

1. **Save is atomic with suspend**: it runs in wasm immediately before the
   suspending import; nothing can interleave.
2. **Restore is wasm-initiated at the true resume boundary.** Restoring from
   the JS continuation (e.g. an async function's `finally`) is unsound: V8
   resumes the wasm activation on a later tick, so when two suspensions wake
   in the same microtask drain, B's restore runs between A's restore and A's
   engine-resume — A resumes with B's stack pointer and stale slices. This
   race exists in the original blog scheme (both the simple and the
   lazy/evicting variant).
3. **The save id round-trips through the suspending import's return
   channel.** Post-resume, pre-restore, the activation's own spill frame may
   hold sibling scribbles: no value read from memory written before the
   suspension may be trusted. Engine-delivered values (wasm locals, return
   values) are safe; at `-O0` Rust locals are frame slots, so this is the
   only sound transport.
4. **`restore_stack` is `#[inline(always)]`** and must be called with nothing
   between the suspending import returning and the call. A non-inlined callee
   allocates its frame at the stale stack pointer and its epilogue rewrites
   the stack pointer after the restore fixed it. The restore itself performs
   no wasm frame work: one plain JS import sets `SP` (via a funcref to
   compiler-rt's frame-free `_emscripten_stack_restore`) and copies the slice
   back.
5. **The suspending import never throws into the resume boundary.**
   Unwinding delivered at the import call site would run landing pads against
   a scribbled, un-restored stack. Rejections are recorded JS-side and
   signalled via flag bits on the returned id; `suspend` surfaces `Err` (and
   invalid-pid panics) only after the restore completes. Consumer-generated
   suspending imports must follow the same rule.

## Activation tops

The slice extent `[sp, top)` needs the activation's *top*: the stack pointer
value when the promising entry was entered. This is the one cross-system
datum — a suspending wrapper (e.g. wasm-bindgen) generally did not create the
current activation (e.g. a tokio fiber) — hence the shared thread-local:

- `wasm_enter()` — first call in a promising entry body; measures inside the
  entry and pads by `STACK_TOP_PAD` (4096, clamped to the stack base) to
  cover the entry's own frame. Overshoot is safe under the healing invariant;
  undershoot is not. Entry wrappers must be thin. Returns an RAII
  `ActivationGuard` that must be held for the activation's full extent: on
  drop (including unwinds) it clears the registration to the sentinel, so a
  later entry that forgets `wasm_enter` fails loudly instead of silently
  reusing a dead activation's top (which can under-save and corrupt in
  release).
- `wasm_enter_exact(top)` — preferred when entry glue measures SP immediately
  before the promising call; `jspi::spawn`'s internal trampoline does this,
  so it only concerns entries the crate does not create.
- Without any top, a conservative save up to `stack_base()` is also correct
  (superset restores heal), just proportionally more copying.

Top correctness across every control transfer into wasm:

| transfer | shared top handled by |
|---|---|
| fresh promising entry | `wasm_enter` / `wasm_enter_exact` |
| JSPI resumption | writeback inside `restore_stack` (no hook needed) |
| return to JS caller when a callee suspends/completes | entry-glue save/restore (below) |
| plain JS→wasm callback | must not suspend; guard sentinel makes misuse loud |

The third row is load-bearing: calling a promising export is *synchronous*
up to its first suspension, so JS invoked from running wasm can dispatch a
nested promising activation (the "sandwich"). When that callee suspends,
control returns to the JS frame — and into the still-running caller — with
no callee-side wasm running again, so no wasm-side hook can restore the
caller's top. Entry glue that can be invoked this way must save/restore the
shared top around the promising call using the `getTop`/`setTop` funcrefs
registered on the per-instance `Module.__jspi`:

```js
const savedTop = J.getTop();
WebAssembly.promising(entry)(...);   // runs callee to first suspend
J.setTop(savedTop);
```

This is strictly LIFO (synchronous JS nesting), which is why a simple
save/restore suffices; the callee's async legs are covered by the
`restore_stack` writeback.

## How general is this?

The primitives and invariants are not JSPI-specific: they constitute a
shadow-stack virtualization convention for any *asymmetric* (run-to-
suspension, scheduler-style) stack switching mechanism that switches the
native wasm stack but not linear memory — JSPI today, the core wasm
stack-switching proposal (WasmFX / typed continuations) tomorrow, or a host
fiber API. A different backend would replace only the delivery layer (the
suspending import and its JS half); `save_stack` / `restore_stack` /
top-tracking carry over unchanged.

Known generality boundary: *symmetric* switching — a parent that resumes a
child continuation while remaining live on the native stack — breaks the
"only suspended activations get scribbled" premise. Covering it needs one
additional rule (save around `resume`, not just at suspend), expressible with
these same primitives but not stated or tested here.

Also deliberately out of scope: wasm threads (each thread has its own spill
stack; JSPI+threads interplay untested), and the lazy/evicting optimization
(below).

## The lazy/evicting optimization (not implemented; measure first)

Hood's optimized scheme inverts the eager convention: the stack memory stays
primary and copies happen only on conflict, at resume time. Suspend just
registers `(bottom, top)` in a global position-sorted list of "residents"
(no copy). Resuming T first evicts everything below `T.top` — T's future
write zone, since a running activation pushes arbitrarily deep below its
bottom — fully evicting residents whose top is below, partially evicting the
one straddling `T.top` up to that line, then restores T's own previously
evicted bytes and sets SP. Eviction always eats a resident's range bottom-up
(the evictor's top is the cutline), so each suspended activation is exactly
"evicted prefix in buffer + resident suffix on stack": one cutline, no
fragment bookkeeping. A corollary invariant — everything below the current
SP is evicted or dead — is what makes fresh entries and plain JS→wasm calls
safe without healing.

Notes against adopting it prematurely:

- The win is workload-shaped. Bottom-active patterns (short-lived work
  entering and completing below quiet parked activations) approach zero
  copies. But a high-parked, frequently-woken activation — exactly a tokio
  main parked in `block_on`, waking per batch — evicts all residents below
  it on every wake: the same bytes move as under the eager scheme, just
  later, plus list-management overhead.
- It concentrates new proof obligations in ~150 lines of subtle JS
  (sorted-list maintenance, cutline arithmetic, partial-evict edge cases)
  versus ~10 lines eager.
- It requires the global residency registry — the one tier that structurally
  cannot be implemented independently by multiple tools.
- The `save_stack`/`restore_stack` API and all soundness invariants are
  unchanged by it (and our wasm-initiated restore architecture removes the
  resume-ordering race Hood's JS-side restore has), so it can be adopted
  later without breaking integrators.

To decide on data: the `metrics` feature exposes cumulative copy load —
`jspi::metrics()` returns saves/restores counts and bytes copied each way
(`jspi::reset_metrics()` to window it). Run the real consumer (tokio
integration) with metrics on; if per-park slices stay small under exact
tops, eager wins by simplicity.

## API

```rust
jspi::linked() -> bool                      // -sJSPI linked and host support present
jspi::wasm_enter() -> ActivationGuard       // padded; first call in promising entries
jspi::wasm_enter_exact(top: usize) -> ActivationGuard
jspi::stack_top() -> Option<usize>

jspi::Deferred::new(timeout_ms: Option<f64>) -> Deferred   // Clone; resolvable promise
jspi::Deferred::resolve(&self)              // no-op once settled; disarms the timeout
jspi::suspend_on(&Deferred) -> Result<(), Rejected>        // park with save/restore

jspi::spawn(f: FnOnce() -> T + 'static) -> JoinHandle<T>   // fresh fiber activation
jspi::JoinHandle::join(self) -> Result<T, Box<dyn Any + Send>>  // parks; Err = fiber panic
```

`Deferred` registrations are single-use (one `suspend_on` per deferred;
suspending twice panics) and cheap — construct a fresh one per park.
`Deferred` is `Clone` (reference-counted) since the waiting and resolving
parties are typically different code; dropping the last handle before
suspension clears the registration and disarms the timer so a stale timer
never pins the host event loop. Timeout resolution is indistinguishable from
`resolve()`; track deadlines consumer-side if the distinction matters.
`spawn`'s trampoline is crate-internal: it holds the `ActivationGuard`
(exact glue-measured top) and catches unwinds, so a fiber panic never
crosses the promising boundary and surfaces only through `join`; dropping
the `JoinHandle` detaches. The crate never marshals JS values.

The underlying primitives (`save_stack`/`restore_stack`, `suspend(pid)`
against the raw registry, `em_js_data!`, `stack_current`/`stack_base`) are
`#[doc(hidden)]`: they carry contracts consumers can easily get wrong (id
transport, inline restore, registry shape) and exist for backend
integration — the wasm-bindgen backend (M3) will consume them, at which
point their visibility gets revisited.

## Integrator contract

- Call `wasm_enter` (or `_exact`) first in every function entered through
  `WebAssembly.promising` — including `main` under emscripten, which is
  auto-wrapped when linked with `-sJSPI` — and hold the guard for the full
  activation. Entry glue that can be invoked synchronously from a JS frame
  above live wasm must save/restore the shared top around the promising call
  (see "Activation tops").
- Route every suspension through `suspend`, or replicate its exact
  save/import/restore sequence under invariants 1–5.
- Plain (non-promising) host callbacks must never suspend; the engine trap is
  the only backstop. They may freely `resolve` promises.
- Panics escaping a promising entry reject its JS promise with a
  `WebAssembly.Exception` (unhandled rejection in node): `catch_unwind` at
  fiber entries. Suspension *inside* drops during unwinding works (tested).
- Embedders with their own per-activation thread-local state (e.g. tokio's
  runtime context) must save it to locals before `suspend` and write it back
  after, exactly as the crate does for the top thread-local.
- No JS frame may be live on the stack at suspension (engine-level
  `SuspendError: trying to suspend JS frames`): do not suspend from inside a
  JS-invoked synchronous callback (wasm → JS → wasm → suspend), and beware C
  dependencies compiled with JS-based exceptions or old setjmp/longjmp, whose
  `invoke_*` trampolines are JS frames. Rust's emscripten target uses native
  wasm EH: panics and unwinding introduce no JS frames (empirically proven by
  the suspend-inside-drop-during-unwind test).

## Emscripten backend notes

The JS half is delivered via the EM_JS object-file convention replicated from
Rust (`em_js_data!`): a `#[no_mangle] #[used] pub static __em_js__<name>`
containing `"(args)<::>{ body }"` lands as a wasm data-symbol export that
emscripten extracts into glue. Requirements discovered empirically:

- Must live in a `lib` crate: rustc internalizes `#[no_mangle]` statics when
  compiling a `bin` (binding=local), and wasm-ld drops the export.
- The static must be referenced from linked code (see `anchor()`) or the
  archive member is never pulled in.
- `#[link_section]` must *not* be used: on wasm, rustc emits custom sections
  (not data segments), which also rules out replicating `EM_JS_DEPS` — hence
  no JS library deps: shims use `wasmExports.__indirect_function_table`
  (exported by default) and funcrefs registered at init instead of
  `wasmTable`/`stackSave`.
- Suspending marking is the `__asyncjs__` import-name prefix
  (`DEFAULT_ASYNCIFY_IMPORTS`); the body just returns a promise.
  `Asyncify.handleAsync` supplies runtime keepalive across suspensions.

All JS-side registry state (saves, promise registrations, funcrefs,
metrics) hangs off the per-instance `Module.__jspi`, never `globalThis`: two
wasm instances in one JS context would otherwise share one registry, and the
last-initialized instance's `setSp`/`getSp`/`getTop`/`setTop` funcrefs would
clobber the others' — instance A's restore writing instance B's stack
pointer. Covered by the two-instance test lane.

The only consumer-facing link requirement is `-C link-args=-sJSPI`.

Internal emscripten APIs relied on (validated against emsdk 6.0.4, node 26;
re-verify on emsdk upgrades): `Asyncify.handleAsync` /
`Asyncify.makeAsyncFunction` (also the `linked()` probe),
`_emscripten_stack_restore` / `emscripten_stack_get_current` /
`emscripten_stack_get_base` (compiler-rt), `wasmExports`, `HEAPU8`,
`runtimeKeepalivePush/Pop` (guarded).

## Testing

All link flags are supplied inline by the chomp tasks (no `.cargo/config.toml`:
cargo merges config rustflags with env overrides, which leaks `-sJSPI` into
builds that must not have it). Bare `cargo test` will not link `-sJSPI`; use
chomp. `chomp test` runs:

- `test:debug` / `test:release` — `tests/corruption.rs` (Hood's victim
  reproduction: sibling exit resets SP above a suspended victim, plain
  activation scribbles through it), `tests/suspend.rs` (basic, same-tick
  double-wake, non-LIFO wake with re-occupation, memory growth while
  suspended), `tests/panics.rs` (rejection after restore, panic unwinding
  through suspension frames with drop-time frame assertions, suspension
  inside a drop mid-unwind, invalid/consumed pid panics). Debug profile
  matters: `-O0` makes Rust locals frame slots, the worst case for
  invariants 3–4.
- `test:proof` — rebuilds with `--cfg jspi_disable_virtualization` (restore
  skipped) and asserts the corruption test fails: proves the tests bite.
- `test:exit-runtime` — `-sEXIT_RUNTIME=1`: fibers parked at main-return
  complete under runtime keepalive before the process exits.
- `test:no-jspi` — built without `-sJSPI`: `linked()` is false, `suspend_on`
  panics with the targeted message, nothing else breaks.
- `test:two-instance` — `-sMODULARIZE` build instantiated twice in one node
  process with interleaved suspensions across both instances (verified to
  crash if the registry moves back to `globalThis`).

## Status / roadmap

- Emscripten backend: working, tested (this prototype).
- Next: tokio integration (branch `emscripten-jspi-spawn`) replacing its
  hand-rolled fiber/park FFI with this crate.
- Later: wasm32-unknown-unknown backend via wasm-bindgen (needs suspending-
  import wrapping and `__stack_pointer` access in bindgen glue), release
  hygiene (CI with pinned emsdk), possibly the lazy/evicting scheme.
