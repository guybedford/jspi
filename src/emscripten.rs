use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::STACK_TOP_PAD;

const SENTINEL: usize = 0;

thread_local! {
    static TOP: Cell<usize> = const { Cell::new(SENTINEL) };
    static INIT: Cell<bool> = const { Cell::new(false) };
}

extern "C" {
    fn emscripten_stack_get_current() -> usize;
    fn emscripten_stack_get_base() -> usize;
    // compiler-rt stack_ops.S, hand-written and frame-free at every opt level
    fn _emscripten_stack_restore(sp: usize);
}

crate::em_js_data!(
    __em_js__jspi_linked,
    "()<::>{ return (typeof Asyncify !== 'undefined' && !!Asyncify.makeAsyncFunction && typeof WebAssembly.promising === 'function') ? 1 : 0; }"
);

crate::em_js_data!(
    __em_js__jspi_init,
    concat!(
        "(setSp, getSp, getTop, setTop)<::>{ ",
        crate::jspi_js_prelude!(),
        " const t = wasmExports.__indirect_function_table; \
         J.setSp = t.get(setSp); J.getSp = t.get(getSp); \
         J.getTop = t.get(getTop); J.setTop = t.get(setTop); }"
    )
);

crate::em_js_data!(
    __em_js__jspi_save,
    concat!(
        "(sp, top)<::>{ ",
        crate::jspi_js_prelude!(),
        " const id = J.nsave++; J.saves.set(id, { sp, top, buf: HEAPU8.slice(sp, top) }); \
         const m = J.m ??= { saves: 0, restores: 0, savedBytes: 0, restoredBytes: 0 }; \
         m.saves++; m.savedBytes += top - sp; return id; }"
    )
);

// Suspending import (via the __asyncjs__ name prefix, which emscripten's
// DEFAULT_ASYNCIFY_IMPORTS matches under -sJSPI). Returns the save id through
// the engine so the resumed wasm never reads it from possibly-scribbled
// memory. Rejections must not throw into the resume boundary (unwinding
// before the restore would run landing pads on a scribbled stack), so they
// are recorded in the registry and signalled via the id's high bit.
// Asyncify.handleAsync provides runtime keepalive across the suspension.
// An unknown or already-consumed pid returns synchronously (no promise, so
// WebAssembly.Suspending performs no suspension): the save entry is dropped
// and the invalid bit signals a loud Rust panic instead of a silent
// immediate-resume misbehavior.
crate::em_js_data!(
    __em_js____asyncjs__jspi_suspend,
    concat!(
        "(pid, id)<::>{ ",
        crate::jspi_js_prelude!(),
        " if (!J.promises.has(pid)) { J.saves.delete(id); return id | 0x40000000; } \
         const p = J.promises.get(pid); J.promises.delete(pid); \
         return Asyncify.handleAsync(async () => { \
           try { await p; return id; } \
           catch (e) { (J.rejections ??= new Map()).set(pid, e); return id | 0x80000000; } \
         }); }"
    )
);

// Plain import: the true-resume-boundary restore. Runs no wasm frames: sets
// the stack pointer via the registered compiler-rt funcref, then copies the
// slice back. Returns the activation top for the thread-local writeback.
crate::em_js_data!(
    __em_js__jspi_restore,
    concat!(
        "(id)<::>{ ",
        crate::jspi_js_prelude!(),
        " const e = J.saves.get(id); J.saves.delete(id); J.setSp(e.sp); HEAPU8.set(e.buf, e.sp); \
         const m = J.m; if (m) { m.restores++; m.restoredBytes += e.buf.length; } return e.top; }"
    )
);

crate::em_js_data!(
    __em_js__jspi_deferred_new,
    concat!(
        "(ms)<::>{ ",
        crate::jspi_js_prelude!(),
        " const pid = J.npromise++; let resolve; const p = new Promise(r => resolve = r); \
         const D = J.deferred ??= new Map(); const e = { resolve, timer: 0 }; \
         if (!Number.isNaN(ms)) e.timer = setTimeout(() => { D.delete(pid); e.resolve(); }, ms); \
         D.set(pid, e); J.promises.set(pid, p); return pid; }"
    )
);

crate::em_js_data!(
    __em_js__jspi_deferred_resolve,
    concat!(
        "(pid)<::>{ ",
        crate::jspi_js_prelude!(),
        " const D = J.deferred; const e = D && D.get(pid); if (!e) return; \
         if (e.timer) clearTimeout(e.timer); D.delete(pid); e.resolve(); }"
    )
);

// Drop of an unconsumed Deferred removes the registration and disarms the
// timer (a stale timer must not pin the host event loop). A consumed entry
// (suspender parked) keeps its resolver and timer: they are the only ways
// the suspender ever wakes.
crate::em_js_data!(
    __em_js__jspi_deferred_drop,
    concat!(
        "(pid)<::>{ ",
        crate::jspi_js_prelude!(),
        " const D = J.deferred; const e = D && D.get(pid); \
         if (!e || !J.promises.has(pid)) return; J.promises.delete(pid); \
         if (e.timer) clearTimeout(e.timer); D.delete(pid); }"
    )
);

// Fresh suspendable activation entered from the event loop with an exactly
// measured pre-entry top. The shared top is saved/restored around the
// synchronous segment, and runtime keepalive is held across the pending
// entry and its full activation (EXIT_RUNTIME=1 support).
crate::em_js_data!(
    __em_js__jspi_spawn,
    "(fnptr, data)<::>{ const t = wasmExports.__indirect_function_table; \
     const entry = WebAssembly.promising(t.get(fnptr)); \
     const J = Module.__jspi; \
     const ka = typeof runtimeKeepalivePush == 'function'; \
     if (ka) runtimeKeepalivePush(); \
     setTimeout(() => { \
       const savedTop = J.getTop(); \
       entry(data, J.getSp()).finally(() => { if (ka) runtimeKeepalivePop(); }); \
       J.setTop(savedTop); \
     }, 0); }"
);

#[cfg(feature = "metrics")]
crate::em_js_data!(
    __em_js__jspi_metric,
    concat!(
        "(idx)<::>{ ",
        crate::jspi_js_prelude!(),
        " const m = J.m; if (!m) return 0; \
         return [m.saves, m.restores, m.savedBytes, m.restoredBytes][idx]; }"
    )
);

#[cfg(feature = "metrics")]
crate::em_js_data!(
    __em_js__jspi_metrics_reset,
    concat!(
        "()<::>{ ",
        crate::jspi_js_prelude!(),
        " J.m = { saves: 0, restores: 0, savedBytes: 0, restoredBytes: 0 }; }"
    )
);

#[link(wasm_import_module = "env")]
extern "C" {
    fn jspi_linked() -> i32;
    fn jspi_init(set_sp: usize, get_sp: usize, get_top: usize, set_top: usize);
    fn jspi_save(sp: usize, top: usize) -> u32;
    #[link_name = "__asyncjs__jspi_suspend"]
    fn jspi_suspend(pid: u32, id: u32) -> u32;
    fn jspi_restore(id: u32) -> usize;
    fn jspi_deferred_new(timeout_ms: f64) -> u32;
    fn jspi_deferred_resolve(pid: u32);
    fn jspi_deferred_drop(pid: u32);
    fn jspi_spawn(fnptr: extern "C" fn(*mut c_void, usize), data: *mut c_void);
    #[cfg(feature = "metrics")]
    fn jspi_metric(idx: u32) -> f64;
    #[cfg(feature = "metrics")]
    fn jspi_metrics_reset();
}

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__jspi_linked.as_ptr(),
        __em_js__jspi_init.as_ptr(),
        __em_js__jspi_save.as_ptr(),
        __em_js____asyncjs__jspi_suspend.as_ptr(),
        __em_js__jspi_restore.as_ptr(),
        __em_js__jspi_deferred_new.as_ptr(),
        __em_js__jspi_deferred_resolve.as_ptr(),
        __em_js__jspi_deferred_drop.as_ptr(),
        __em_js__jspi_spawn.as_ptr(),
    ));
    #[cfg(feature = "metrics")]
    std::hint::black_box((
        __em_js__jspi_metric.as_ptr(),
        __em_js__jspi_metrics_reset.as_ptr(),
    ));
}

// Registered as funcrefs so promising-entry glue can save/restore the shared
// top around synchronous dispatch (see ActivationGuard docs).
extern "C" fn tl_get_top() -> usize {
    TOP.with(|t| t.get())
}

extern "C" fn tl_set_top(top: usize) {
    TOP.with(|t| t.set(top))
}

fn ensure_init() {
    INIT.with(|i| {
        if !i.get() {
            anchor();
            let set_sp: unsafe extern "C" fn(usize) = _emscripten_stack_restore;
            let get_sp: unsafe extern "C" fn() -> usize = emscripten_stack_get_current;
            let get_top: extern "C" fn() -> usize = tl_get_top;
            let set_top: extern "C" fn(usize) = tl_set_top;
            unsafe {
                jspi_init(
                    set_sp as usize,
                    get_sp as usize,
                    get_top as usize,
                    set_top as usize,
                )
            };
            i.set(true);
        }
    });
}

/// Whether JSPI suspension is available: binary linked with `-sJSPI` and
/// host support present.
pub fn linked() -> bool {
    ensure_init();
    unsafe { jspi_linked() != 0 }
}

/// Current spill stack pointer.
#[doc(hidden)]
pub fn stack_current() -> usize {
    unsafe { emscripten_stack_get_current() }
}

/// High end of the spill stack region.
#[doc(hidden)]
pub fn stack_base() -> usize {
    unsafe { emscripten_stack_get_base() }
}

/// RAII activation-top registration; see [`wasm_enter`]. Dropping restores
/// the sentinel (not any previous value: by the time an async activation
/// completes, the synchronous context it entered from is long gone —
/// synchronous nesting is the entry glue's responsibility via the registered
/// `getTop`/`setTop` funcrefs). The sentinel-on-drop makes a forgotten
/// `wasm_enter` in a later entry fail loudly instead of silently reusing a
/// dead activation's top, which can under-save and corrupt in release.
#[must_use = "the activation top is cleared when this guard drops"]
pub struct ActivationGuard {
    _not_send: std::marker::PhantomData<*mut ()>,
}

impl Drop for ActivationGuard {
    fn drop(&mut self) {
        TOP.with(|t| t.set(SENTINEL));
    }
}

fn guard() -> ActivationGuard {
    ActivationGuard {
        _not_send: std::marker::PhantomData,
    }
}

/// Record the current activation's stack top, measured here and padded by
/// [`STACK_TOP_PAD`] to cover the caller's own frame (clamped to the stack
/// base). Must be the first call in every function invoked through
/// `WebAssembly.promising`; hold the returned guard for the activation's
/// full extent (it clears the registration on drop, including unwinds).
///
/// Entry glue that synchronously invokes a promising export from a JS frame
/// (where wasm may be live below) must save and restore the shared top
/// around the call using the `getTop`/`setTop` funcrefs registered on
/// `Module.__jspi` — the callee's suspension returns control to that JS
/// frame without any callee-side wasm running again, so no wasm-side hook
/// can restore the caller's top. JSPI resumption needs no hook: the top is
/// written back inside [`restore_stack`].
pub fn wasm_enter() -> ActivationGuard {
    ensure_init();
    let top = (stack_current() + STACK_TOP_PAD).min(stack_base());
    TOP.with(|t| t.set(top));
    guard()
}

/// [`wasm_enter`] with an exactly measured top (e.g. measured by entry glue
/// immediately before the promising call, `J.getSp()`).
pub fn wasm_enter_exact(top: usize) -> ActivationGuard {
    ensure_init();
    TOP.with(|t| t.set(top));
    guard()
}

/// The current activation's recorded stack top, if one was set.
pub fn stack_top() -> Option<usize> {
    let top = TOP.with(|t| t.get());
    (top != SENTINEL).then_some(top)
}

/// Save the current activation's live spill slice `[sp, top)` into the
/// JS-side registry, returning its save id. Must be called immediately
/// before a suspending import so the save is atomic with the suspension.
///
/// Panics if no stack top is recorded for this activation.
///
/// Hidden: backend-integration primitive (a future wasm-bindgen backend
/// wraps its own suspending imports with it); consumers use [`suspend_on`].
#[doc(hidden)]
pub fn save_stack() -> u32 {
    ensure_init();
    let top = TOP.with(|t| t.get());
    assert!(
        top != SENTINEL,
        "jspi: no stack top recorded for this activation; call \
         jspi::wasm_enter() first in every promising entry and hold its guard"
    );
    let sp = stack_current();
    debug_assert!(sp <= top, "jspi: stack pointer above recorded top");
    unsafe { jspi_save(sp, top) }
}

/// Restore the slice and stack pointer for `id` and write the activation
/// top back to the shared thread-local.
///
/// Contract for direct users ([`suspend`] already complies): this must be
/// the first thing the resumed activation does after its suspending import
/// returns, and `id` must arrive through engine-preserved storage (the
/// import's return value), never from memory written before the suspension.
/// `#[inline(always)]` is load-bearing: a real callee would allocate its
/// frame at the stale stack pointer and its epilogue would rewrite the
/// stack pointer after the restore.
///
/// Hidden: backend-integration primitive; consumers use [`suspend_on`].
#[doc(hidden)]
#[inline(always)]
pub fn restore_stack(id: u32) {
    let top = unsafe { jspi_restore(id) };
    TOP.with(|t| t.set(top));
}

/// Cumulative spill-slice copy load, for measuring the eager scheme's cost
/// under real workloads (see README: lazy/evicting optimization).
#[cfg(feature = "metrics")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Metrics {
    pub saves: u64,
    pub restores: u64,
    pub saved_bytes: u64,
    pub restored_bytes: u64,
}

#[cfg(feature = "metrics")]
pub fn metrics() -> Metrics {
    ensure_init();
    unsafe {
        Metrics {
            saves: jspi_metric(0) as u64,
            restores: jspi_metric(1) as u64,
            saved_bytes: jspi_metric(2) as u64,
            restored_bytes: jspi_metric(3) as u64,
        }
    }
}

#[cfg(feature = "metrics")]
pub fn reset_metrics() {
    ensure_init();
    unsafe { jspi_metrics_reset() }
}

const REJECTED_BIT: u32 = 1 << 31;
const INVALID_PID_BIT: u32 = 1 << 30;
const ID_MASK: u32 = !(REJECTED_BIT | INVALID_PID_BIT);

/// The awaited promise rejected. The rejection value is retained in the
/// JS-side registry (`Module.__jspi.rejections`, keyed by pid) for
/// consumer glue to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    pub pid: u32,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "jspi: awaited promise {} rejected", self.pid)
    }
}

impl std::error::Error for Rejected {}

/// Suspend the current activation until the JS-side registered promise
/// `pid` settles, with spill-slice save/restore. The promise must have been
/// registered in the shared registry (`Module.__jspi.promises`). Rejection
/// is returned as `Err` only after the stack is fully restored, so
/// unwinding (including drops during a propagated panic) always runs on a
/// healed stack.
///
/// Panics if JSPI is unavailable (see [`linked`]) or no stack top is
/// recorded. Must only be called on an activation entered through
/// `WebAssembly.promising`; suspension on a plain activation traps at the
/// engine level.
///
/// Hidden: raw-registry entry point (retained for future backends'
/// arbitrary-promise suspension); consumers use [`suspend_on`].
#[doc(hidden)]
pub fn suspend(pid: u32) -> Result<(), Rejected> {
    assert!(
        linked(),
        "jspi: suspension unavailable; link with -C link-args=-sJSPI and run \
         on a host with JSPI support"
    );
    let id = save_stack();
    let ret = unsafe { jspi_suspend(pid, id) };
    if ret & INVALID_PID_BIT != 0 {
        // returned synchronously: no suspension occurred, stack untouched
        panic!("jspi: suspend on unknown or already-consumed promise id {pid}");
    }
    #[cfg(not(jspi_disable_virtualization))]
    restore_stack(ret & ID_MASK);
    // cfg(jspi_disable_virtualization): restore skipped, proves the
    // corruption tests bite
    if ret & REJECTED_BIT != 0 {
        return Err(Rejected { pid });
    }
    Ok(())
}

struct DeferredInner {
    pid: u32,
    _not_send: PhantomData<*mut ()>,
}

impl Drop for DeferredInner {
    fn drop(&mut self) {
        unsafe { jspi_deferred_drop(self.pid) }
    }
}

/// A resolvable promise handle. Cloneable (reference-counted): the waiting
/// and resolving parties are typically different code holding the same
/// deferred. The registration is single-use — one [`suspend_on`] per
/// Deferred; construct a fresh one per park (construction is cheap, no
/// pooling). Dropping the last handle before suspension clears the
/// registration and disarms the timer so a stale timer cannot pin the host
/// event loop.
#[derive(Clone)]
pub struct Deferred {
    inner: Rc<DeferredInner>,
}

impl Deferred {
    /// Create a deferred; `timeout_ms` also resolves it at the deadline.
    /// Timeout resolution is indistinguishable from [`resolve`] — track
    /// deadlines consumer-side if the distinction matters.
    pub fn new(timeout_ms: Option<f64>) -> Deferred {
        ensure_init();
        let pid = unsafe { jspi_deferred_new(timeout_ms.unwrap_or(f64::NAN)) };
        Deferred {
            inner: Rc::new(DeferredInner {
                pid,
                _not_send: PhantomData,
            }),
        }
    }

    /// Resolve now, disarming any pending timeout. No-op if already settled
    /// (including by timeout) or dropped. Safe to call from plain
    /// (non-promising) host-callback entries; never runs wasm re-entrantly
    /// (resolution only schedules microtasks).
    pub fn resolve(&self) {
        unsafe { jspi_deferred_resolve(self.inner.pid) }
    }

    /// Whether two handles refer to the same deferred (`Rc::ptr_eq`
    /// semantics), e.g. to deregister a specific entry from a wait list.
    pub fn ptr_eq(a: &Deferred, b: &Deferred) -> bool {
        Rc::ptr_eq(&a.inner, &b.inner)
    }
}

/// Suspend the current activation until `d` settles, with spill-slice
/// save/restore per the crate invariants. Consumes the registration: a
/// second `suspend_on` for the same deferred panics.
///
/// Panics if JSPI is unavailable (see [`linked`]) or the activation has no
/// recorded stack top ([`wasm_enter`]). Must only be called on an activation
/// entered through `WebAssembly.promising`.
pub fn suspend_on(d: &Deferred) -> Result<(), Rejected> {
    suspend(d.inner.pid)
}

struct FiberData<T> {
    f: Box<dyn FnOnce() -> T>,
    slot: Rc<RefCell<Option<Result<T, Box<dyn Any + Send>>>>>,
    completion_pid: u32,
}

/// Handle to a spawned fiber; see [`spawn`]. Dropping detaches: the fiber
/// still runs to completion, and a panic payload is silently discarded
/// (surfaced only through [`join`](JoinHandle::join)).
#[must_use = "dropping a JoinHandle detaches the fiber"]
pub struct JoinHandle<T> {
    completion: Deferred,
    slot: Rc<RefCell<Option<Result<T, Box<dyn Any + Send>>>>>,
}

impl<T> JoinHandle<T> {
    /// Park the calling activation until the fiber completes. Propagates a
    /// fiber panic as `Err(payload)`. The caller needs a recorded stack top
    /// ([`wasm_enter`]) since this suspends.
    pub fn join(self) -> Result<T, Box<dyn Any + Send>> {
        suspend_on(&self.completion).expect("jspi: fiber completion rejected");
        self.slot
            .borrow_mut()
            .take()
            .expect("jspi: fiber completed without a result")
    }
}

/// Run `f` on a fresh suspendable activation ("fiber"), entered from the
/// host event loop through a `WebAssembly.promising`-wrapped trampoline.
/// The trampoline holds the [`ActivationGuard`] (exact pre-entry top,
/// measured by the glue) for the closure's extent and catches unwinds — a
/// panic never crosses the promising boundary; its payload is delivered
/// through [`JoinHandle::join`].
pub fn spawn<T: 'static>(f: impl FnOnce() -> T + 'static) -> JoinHandle<T> {
    ensure_init();
    let completion = Deferred::new(None);
    let slot = Rc::new(RefCell::new(None));
    extern "C" fn trampoline<T>(data: *mut c_void, top: usize) {
        let _jspi_stack = wasm_enter_exact(top);
        let data = unsafe { Box::from_raw(data as *mut FiberData<T>) };
        let FiberData {
            f,
            slot,
            completion_pid,
        } = *data;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        *slot.borrow_mut() = Some(result);
        unsafe { jspi_deferred_resolve(completion_pid) };
    }
    let data = Box::new(FiberData {
        f: Box::new(f),
        slot: slot.clone(),
        completion_pid: completion.inner.pid,
    });
    unsafe { jspi_spawn(trampoline::<T>, Box::into_raw(data) as *mut c_void) };
    JoinHandle { completion, slot }
}
