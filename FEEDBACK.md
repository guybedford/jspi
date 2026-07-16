# Review feedback: naming pass (pre-publish)

The API rework landed well — boundary is right, tokio now integrates with
zero JavaScript (745/0 on the suite). One final naming pass before publish,
settled with Guy:

## The three-word convention: `stack` / `suspend` / `resume`

The crate's domain is the linear-memory spill stack — the engine already
owns execution (native stack switching); this crate owns the stack the
engine doesn't know about. Every public verb should take that stack as its
object:

| current | new |
|---|---|
| `wasm_enter()` | `stack()` |
| `wasm_enter_exact(top)` | `stack_with_top(top)` |
| `ActivationGuard` | `Stack` |
| `save_stack()` | `suspend()` |
| `restore_stack(ret)` | `resume(ret)` |
| `suspend(pid)` | private (pid plumbing behind `suspend_on`) |

The raw ABI bracket then reads as the mental model:

```rust
let _stack = jspi::stack();           // register this activation's stack
...
let id = jspi::suspend();             // atomic with the suspending import
let ret = my_suspending_import(..., id);
jspi::resume(ret);                    // first action after the engine resumes
```

## Doc framing (use this language)

- Execution never stops — the engine switches the native stack; what
  suspends is *the stack*: `suspend()` **snapshots** the live slice
  `[sp, top)` into a side buffer and leaves the region in place. What
  changes is authority: the snapshot becomes the truth, and the in-memory
  region is legally clobberable by interleaving activations (sibling
  growth, stale restores).
- `resume(ret)` **reinstates** the snapshot and the stack pointer,
  unconditionally — staleness is never detected, only overwritten, which is
  what lets independent users of the convention compose with no shared
  bookkeeping.
- Not "detach/move": in the uncontended case the region is never touched
  and the copy-back is semantically a no-op.

## Surface trim

Public: `linked`, `stack`, `stack_with_top`, `Stack`, `suspend`, `resume`,
`Deferred::{new, resolve, ptr_eq}`, `suspend_on`, `Rejected`, `spawn`,
`JoinHandle`. Everything else (`stack_current`, `stack_base`,
`STACK_TOP_PAD`, `stack_top`, em_js macros, metrics) `#[doc(hidden)]` or
feature-gated as already done.

Note `suspend_on(&Deferred)` keeps its name: the low-level `suspend()` is
the bracket for integrators writing their own suspending imports;
`suspend_on` is the complete high-level operation. Related layers, related
names — intentional.

## Follow-up

Tokio (`emscripten-jspi-spawn`, `tokio/src/runtime/jspi.rs`) uses
`wasm_enter_exact`, `stack_base`, `suspend_on`, `Deferred`, `spawn`,
`linked` — I'll do the trivial rename there once this lands (`stack_base`
staying accessible, even doc(hidden), is enough). README's convention
snippet also needs the rename (it currently shows `wasm_enter`).
