//! Test-only glue exercising crate internals a real consumer never touches:
//! rejectable/shareable raw registry promises (rejection and same-tick
//! double-wake tests), synchronous promising dispatch (sandwich test), and
//! plain event-loop calls (corruption test). Uses `#[doc(hidden)]` crate
//! items by design.

use std::ffi::c_void;

jspi::em_js_data!(
    __em_js__glue_promise_new,
    concat!(
        "()<::>{ ",
        jspi::jspi_js_prelude!(),
        " const pid = J.npromise++; const T = J.test ??= { resolvers: new Map(), rejecters: new Map(), promises: new Map() }; \
         const p = new Promise((r, j) => { T.resolvers.set(pid, r); T.rejecters.set(pid, j); }); \
         T.promises.set(pid, p); J.promises.set(pid, p); return pid; }"
    )
);

jspi::em_js_data!(
    __em_js__glue_promise_resolve,
    concat!(
        "(pid)<::>{ ",
        jspi::jspi_js_prelude!(),
        " J.test.resolvers.get(pid)(); }"
    )
);

jspi::em_js_data!(
    __em_js__glue_promise_reject,
    concat!(
        "(pid)<::>{ ",
        jspi::jspi_js_prelude!(),
        " J.test.rejecters.get(pid)(new Error('rejected by test')); }"
    )
);

// Register a second pid sharing pid's underlying promise, so two suspensions
// wake from the same resolution in the same tick.
jspi::em_js_data!(
    __em_js__glue_promise_share,
    concat!(
        "(pid)<::>{ ",
        jspi::jspi_js_prelude!(),
        " const pid2 = J.npromise++; J.promises.set(pid2, J.test.promises.get(pid)); return pid2; }"
    )
);

// Synchronously invoke a promising entry from a JS frame with wasm live
// below: the shared top must be saved/restored around the call, since the
// callee's suspension returns here without any callee-side wasm running.
jspi::em_js_data!(
    __em_js__glue_sandwich_dispatch,
    "(fnptr, data)<::>{ const t = wasmExports.__indirect_function_table; \
     const J = Module.__jspi; \
     const savedTop = J.getTop(); \
     WebAssembly.promising(t.get(fnptr))(data, J.getSp()); \
     J.setTop(savedTop); }"
);

// Call a plain (non-promising) export from the event loop: enters wasm with
// whatever stale stack pointer the last runner left.
jspi::em_js_data!(
    __em_js__glue_call_plain_later,
    "(fnptr, data)<::>{ setTimeout(() => { wasmExports.__indirect_function_table.get(fnptr)(data); }, 0); }"
);

#[link(wasm_import_module = "env")]
extern "C" {
    fn glue_promise_new() -> u32;
    fn glue_promise_resolve(pid: u32);
    fn glue_promise_reject(pid: u32);
    fn glue_promise_share(pid: u32) -> u32;
    fn glue_sandwich_dispatch(fnptr: extern "C" fn(*mut c_void, usize), data: *mut c_void);
    fn glue_call_plain_later(fnptr: extern "C" fn(*mut c_void), data: *mut c_void);
}

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__glue_promise_new.as_ptr(),
        __em_js__glue_promise_resolve.as_ptr(),
        __em_js__glue_promise_reject.as_ptr(),
        __em_js__glue_promise_share.as_ptr(),
        __em_js__glue_sandwich_dispatch.as_ptr(),
        __em_js__glue_call_plain_later.as_ptr(),
    ));
}

/// A raw registry promise handle with test-only reject/share capabilities.
#[derive(Clone, Copy)]
pub struct TestPromise(pub u32);

impl TestPromise {
    pub fn new() -> TestPromise {
        anchor();
        TestPromise(unsafe { glue_promise_new() })
    }

    pub fn resolve(&self) {
        unsafe { glue_promise_resolve(self.0) }
    }

    pub fn reject(&self) {
        unsafe { glue_promise_reject(self.0) }
    }

    /// A fresh pid waking from this promise's same resolution tick.
    pub fn share(&self) -> u32 {
        unsafe { glue_promise_share(self.0) }
    }

    pub fn wait(&self) -> Result<(), jspi::Rejected> {
        jspi::suspend(self.0)
    }
}

impl Default for TestPromise {
    fn default() -> Self {
        Self::new()
    }
}

/// Suspend the current activation for `ms` via a host timer.
pub fn sleep(ms: f64) {
    let d = jspi::Deferred::new(Some(ms));
    jspi::suspend_on(&d).unwrap();
}

extern "C" fn fnonce_trampoline(data: *mut c_void, top: usize) {
    let _jspi_stack = jspi::wasm_enter_exact(top);
    let f = unsafe { Box::from_raw(data as *mut Box<dyn FnOnce()>) };
    f();
}

/// Synchronously dispatch `f` as a promising activation from a JS frame,
/// while the calling activation's wasm frames are live below (the "sandwich"
/// pattern). Returns when `f` first suspends or completes.
pub fn sandwich_dispatch(f: impl FnOnce() + 'static) {
    anchor();
    let data: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe { glue_sandwich_dispatch(fnonce_trampoline, Box::into_raw(data) as *mut c_void) }
}

/// Run `f` later as a plain (non-suspendable) activation, entering wasm at
/// the stale stack pointer left by the last runner.
pub fn call_plain_later(f: impl FnOnce() + 'static) {
    anchor();
    extern "C" fn trampoline(data: *mut c_void) {
        let f = unsafe { Box::from_raw(data as *mut Box<dyn FnOnce()>) };
        f();
    }
    let data: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe { glue_call_plain_later(trampoline, Box::into_raw(data) as *mut c_void) }
}
