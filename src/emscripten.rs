use std::cell::{Cell, RefCell};
use std::mem::MaybeUninit;
use std::ptr;

use crate::{BlockingArgs, STACK_TOP_PAD};

const SENTINEL: usize = 0;

/// Save-state for one parked bracket. Lives on the heap inside the static
/// registry: statics and heap are never part of any saved slice, so both
/// survive arbitrary sibling scribbling while the owner is parked.
struct Meta {
    sp: usize,
    len: usize,
    top: usize,
    buf: Box<[MaybeUninit<u8>]>,
}

thread_local! {
    /// Parity counter: every parked activation holds root + bracket
    /// = +2, so parity always describes the running code. Even: `stack_root`
    /// permitted. Odd: `blocking_call` permitted.
    static COUNTER: Cell<usize> = const { Cell::new(0) };
    /// The running activation's stack top, written by `stack_root`, written
    /// back per-activation by each bracket's restore.
    static TOP: Cell<usize> = const { Cell::new(SENTINEL) };
    /// Parked bracket save-state, keyed by the bracket's frame-derived
    /// probe address.
    static REGISTRY: RefCell<Vec<(usize, Meta)>> = const { RefCell::new(Vec::new()) };
    /// Holds the resuming bracket's `Meta` across the restore instant: the
    /// restore rewrites `blocking_call`'s own frame slots, so nothing may
    /// transit the restore in a local. Occupied only between the registry
    /// removal and the post-restore take — no suspension point in between.
    static IN_FLIGHT: Cell<Option<Meta>> = const { Cell::new(None) };
}

unsafe extern "C" {
    fn emscripten_stack_get_current() -> usize;
    fn emscripten_stack_get_base() -> usize;
    // compiler-rt stack_ops.S, hand-written and frame-free at every opt level
    fn _emscripten_stack_restore(sp: usize);
}

crate::em_js_data!(
    __em_js__jspi_linked,
    "()<::>{ return (typeof Asyncify !== 'undefined' && !!Asyncify.makeAsyncFunction && typeof WebAssembly.promising === 'function') ? 1 : 0; }"
);

// The true-resume-boundary restore. Runs zero wasm frames (wasm restore
// code would clobber or be clobbered by the region it rewrites): sets the
// stack pointer through the passed compiler-rt funcref, then copies the
// slice back. HEAPU8 is module-scope in emscripten glue and refreshed on
// memory growth.
crate::em_js_data!(
    __em_js__jspi_restore,
    "(buf, sp, len, setSp)<::>{ wasmExports.__indirect_function_table.get(setSp)(sp); HEAPU8.copyWithin(sp, buf, buf + len); }"
);

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn jspi_linked() -> i32;
    fn jspi_restore(buf: usize, sp: usize, len: usize, set_sp: usize);
}

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__jspi_linked.as_ptr(),
        __em_js__jspi_restore.as_ptr(),
    ));
}

/// Whether JSPI suspension is available: binary linked with `-sJSPI` and
/// host support present.
pub fn linked() -> bool {
    anchor();
    unsafe { jspi_linked() != 0 }
}

fn enter_root<R>(top: usize, f: impl FnOnce() -> R) -> R {
    let c = COUNTER.get();
    assert!(
        c & 1 == 0,
        "jspi: activation root nested inside an active root"
    );
    COUNTER.set(c + 1);
    // Decrement + top reset on both exit paths: a panic unwinding out of
    // the closure fully unwinds the root instead of poisoning every
    // subsequent activation.
    struct RootExit;
    impl Drop for RootExit {
        fn drop(&mut self) {
            COUNTER.set(COUNTER.get() - 1);
            TOP.set(SENTINEL);
        }
    }
    let _exit = RootExit;
    TOP.set(top);
    let r = f();
    assert!(
        COUNTER.get() & 1 == 1,
        "jspi: parity corrupted during activation root"
    );
    r
}

/// The safe full-capture activation root: like [`stack_root`], but every
/// bracket inside saves and restores the **entire** stack from the save
/// point to the absolute stack base. Under-save is thereby impossible, so
/// every `stack_root` contract disappears: no placement requirement, no
/// thin-entry-frame requirement, any call depth. Everything above the save
/// point at a resume belongs to parked activations (their own restore
/// heals the stale rewrite) or to dead frames (harmless) — the same
/// healing invariant, applied conservatively.
///
/// The price is copy volume: `O(sp → stack base)` bytes per park, both
/// directions (bounded by `-sSTACK_SIZE`), instead of `stack_root`'s
/// `O(live activation slice)`.
///
/// A synchronous scope, not a scheduler: runs `f` immediately. Parking in
/// a non-promising activation still traps at the engine (fatal and loud,
/// never corruption). Residual to know about: all activations share one
/// `ThreadId` and one set of `thread_local!` instances — treat a bracketed
/// foreign call as a blocking syscall on a thread that shares TLS and
/// thread identity with every other activation.
pub fn spawn<R>(f: impl FnOnce() -> R) -> R {
    anchor();
    enter_root(unsafe { emscripten_stack_get_base() }, f)
}

/// Mark the top of this activation's spill stack and run `f` immediately:
/// a synchronous scope, not a scheduler. Every [`blocking_call`] made
/// (transitively) inside `f` saves and restores the live slice up to this
/// mark. The optimized root — [`spawn`] is the safe full-capture variant.
///
/// # Safety
///
/// 1. Must be the first statement of the top-level function of a
///    suspendable activation — one entered through `WebAssembly.promising`.
/// 2. The entry frame above it stays thin: `stack_root` records
///    `SP + STACK_TOP_PAD` (clamped to the stack base) as the activation
///    top; entry frames (including the closure's captures, which live
///    there) fatter than the pad are a contract violation. Over-save is
///    healed; under-save is corruption.
/// 3. No nesting: one root per activation (denied by the parity counter as
///    a panic).
/// 4. By entering it, the caller asserts the dependency tree contains no
///    thread-identity-keyed reentrancy assumptions — all activations share
///    one `ThreadId` and one set of `thread_local!` instances, and
///    interleaving under one `ThreadId` is what the root makes possible.
pub unsafe fn stack_root<R>(f: impl FnOnce() -> R) -> R {
    anchor();
    let top = unsafe {
        (emscripten_stack_get_current() + STACK_TOP_PAD).min(emscripten_stack_get_base())
    };
    enter_root(top, f)
}

/// Bracket one foreign blocking call: save the live slice `[sp, top)` to
/// the heap, call `f(args)` (the consumer's `__asyncjs__` Suspending
/// import, which parks the activation until its promise settles), restore
/// slice and stack pointer at the wasm resume boundary.
///
/// Safe: the obligations rest with `f`'s author at its declaration site
/// (edition-2024 `unsafe extern` blocks allow `safe fn` import
/// declarations) — a genuine suspending import (or any non-suspending safe
/// call, for which the bracket degrades to a benign identical-bytes no-op),
/// returning unit (type-enforced), never throwing into the resume site, and
/// panicking only before its suspension point (`C-unwind`: such a panic
/// unwinds through the bracket, whose RAII close discards the untouched
/// snapshot; a panic after the foreign import returns is a contract
/// violation, denied best-effort by abort). Misplacement — outside a root,
/// from a plain host callback, reentrantly — is denied by the parity
/// counter as a catchable panic before anything runs.
pub fn blocking_call<A: BlockingArgs>(f: A::Fn, args: A) {
    let c = COUNTER.get();
    assert!(
        c & 1 == 1,
        "jspi: blocking_call requires an open stack_root with no bracket in \
         flight (denied: outside any root, from a plain host callback, or \
         reentrant)"
    );
    let top = TOP.get();
    debug_assert_ne!(top, SENTINEL, "jspi: parity odd but no activation top");
    let sp = unsafe { emscripten_stack_get_current() };
    assert!(
        sp <= top,
        "jspi: stack pointer above the recorded activation top (stack_root \
         contract violation)"
    );
    let len = top - sp;
    let mut buf = Box::new_uninit_slice(len);
    // Save is atomic with the suspension: nothing interleaves between this
    // copy and the engine suspending inside the foreign import.
    unsafe { ptr::copy_nonoverlapping(sp as *const MaybeUninit<u8>, buf.as_mut_ptr(), len) };
    // The registry key: the address of a local in this frame. Frame
    // addresses derive from the frame-pointer wasm local — engine-preserved
    // per activation — so recomputing this after the resume yields the
    // identical value without any memory or SP-global read (the SP global
    // holds whatever the last runner left at that point). The probe sits
    // inside the saved slice and heals with everything else.
    let probe: u8 = 0;
    let key = &raw const probe as usize;
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        assert!(
            !r.iter().any(|(k, _)| *k == key),
            "jspi: bracket key collision: another parked activation's \
             bracket frame occupies this frame address; alter the caller's \
             frame geometry to avoid"
        );
        r.push((key, Meta { sp, len, top, buf }));
    });
    COUNTER.set(c + 1);

    // Unwind path: `f` is `extern "C-unwind"` so pre-suspension panics
    // (denial panics, consumer shim validation) unwind through the bracket
    // with destructors. This guard closes the bracket during such an unwind
    // *without* the restore: pre-suspension nothing else has run, so live
    // memory is the truth and the snapshot is discardable — and the SP
    // write would be a geometry error mid-unwind (the unwinder is executing
    // in frames below the save point; SP := sp would let landing-pad
    // callees allocate on top of them). A panic after the foreign import
    // returns is a contract violation: the key field read from this frame
    // is untrusted, so it is validated against the registry — best-effort
    // abort on a miss, loud rather than corrupt.
    struct UnwindClose {
        key: usize,
    }
    impl Drop for UnwindClose {
        fn drop(&mut self) {
            let Some(meta) = registry_remove(self.key) else {
                eprintln!(
                    "jspi: bracket unwound with unrecognized save-state \
                     (panic after the foreign import returned?)"
                );
                std::process::abort();
            };
            TOP.set(meta.top);
            drop(meta);
            COUNTER.set(COUNTER.get() - 1);
        }
    }
    let unwind_close = UnwindClose { key };

    A::call(f, args); // parks here; siblings run; engine resumes with an
    // empty native stack, our wasm locals intact, and the SP global and
    // every byte of [sp, top) untrusted.

    std::mem::forget(unwind_close);
    // The resume-boundary close, inlined (load-bearing): the restore
    // rewrites [sp, top) and sets SP := sp, so it must execute in the
    // bracket frame itself — sp is this frame's own base, callees after the
    // restore allocate below it. A separate call frame would sit below sp
    // and be popped mid-execution by the SP write.
    let key = &raw const probe as usize;
    let Some(meta) = registry_remove(key) else {
        // pre-restore: this frame is still sibling-scribbled, so unwinding
        // here would run landing pads over garbage — abort
        eprintln!("jspi: bracket save-state missing at resume boundary");
        std::process::abort();
    };
    let (buf_ptr, sp, len) = (meta.buf.as_ptr() as usize, meta.sp, meta.len);
    // Nothing may transit the restore in a local: the restore rewrites this
    // frame's own slots with their pre-suspension bytes, so Meta parks in a
    // static across the instant and is re-taken from it afterwards.
    IN_FLIGHT.set(Some(meta));
    #[cfg(not(jspi_disable_virtualization))]
    unsafe {
        let set_sp: unsafe extern "C" fn(usize) = _emscripten_stack_restore;
        jspi_restore(buf_ptr, sp, len, set_sp as usize);
    }
    #[cfg(jspi_disable_virtualization)]
    let _ = (buf_ptr, sp, len);
    let meta = IN_FLIGHT
        .take()
        .expect("jspi: in-flight save-state missing");
    TOP.set(meta.top);
    drop(meta);
    COUNTER.set(COUNTER.get() - 1);
}

fn registry_remove(key: usize) -> Option<Meta> {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.iter()
            .position(|(k, _)| *k == key)
            .map(|i| r.swap_remove(i).1)
    })
}
