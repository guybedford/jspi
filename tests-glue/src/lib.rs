//! Reference consumer glue for the `jspi` primitives, demonstrating every
//! pattern a real consumer needs: fiber dispatch, a suspending sleep, a
//! resolvable/rejectable promise with post-restore value fetch, and plain
//! (non-blocking) host callbacks.
//!
//! All JS-side state hangs off the per-instance `Module` object
//! (`Module.__jspiTest`) — never `globalThis`: two instances in one JS
//! context would cross-wire shared state (empirically verified crash).

use std::ffi::c_void;

macro_rules! prelude {
    () => {
        "const T = Module.__jspiTest ??= { np: 1, promises: new Map(), resolvers: new Map(), rejecters: new Map(), results: new Map(), errors: new Map() };"
    };
}

// Fiber dispatch: promising-wrap a table funcref and enter it from the
// event loop. Runtime keepalive is held across the pending entry and its
// full activation (-sEXIT_RUNTIME=1 support), and a rejection (trap or
// escaped panic crossing the promising boundary) is rethrown on a fresh
// tick so an abandoned activation is fatal and loud, never silent.
jspi::em_js_data!(
    __em_js__glue_run_fiber,
    "(fnptr, data)<::>{ const entry = WebAssembly.promising(wasmExports.__indirect_function_table.get(fnptr)); \
     const ka = typeof runtimeKeepalivePush === 'function'; if (ka) runtimeKeepalivePush(); \
     setTimeout(() => { entry(data).finally(() => { if (ka) runtimeKeepalivePop(); }).catch((e) => setTimeout(() => { throw e; }, 0)); }, 0); }"
);

// Suspending sleep: unit return, plain awaited promise. Asyncify.handleAsync
// also provides runtime keepalive across the suspension.
jspi::em_js_data!(
    __em_js____asyncjs__glue_sleep,
    "(ms)<::>{ return Asyncify.handleAsync(async () => { await new Promise((r) => setTimeout(r, ms)); }); }"
);

jspi::em_js_data!(
    __em_js__glue_promise_new,
    concat!(
        "()<::>{ ",
        prelude!(),
        " const pid = T.np++; T.promises.set(pid, new Promise((res, rej) => { T.resolvers.set(pid, res); T.rejecters.set(pid, rej); })); return pid; }"
    )
);

jspi::em_js_data!(
    __em_js__glue_promise_resolve,
    concat!(
        "(pid, value)<::>{ ",
        prelude!(),
        " T.resolvers.get(pid)(value); }"
    )
);

jspi::em_js_data!(
    __em_js__glue_promise_reject,
    concat!(
        "(pid)<::>{ ",
        prelude!(),
        " T.rejecters.get(pid)(new Error('rejected by test')); }"
    )
);

// Register a second pid sharing pid's underlying promise, so two
// suspensions wake from the same resolution in the same microtask drain.
jspi::em_js_data!(
    __em_js__glue_promise_share,
    concat!(
        "(pid)<::>{ ",
        prelude!(),
        " const pid2 = T.np++; T.promises.set(pid2, T.promises.get(pid)); return pid2; }"
    )
);

// Suspending await: unit return (results are fetched by plain imports after
// the bracket restores), rejections caught into the registry — a JS
// exception thrown into the resume site would enter wasm as a foreign
// exception that cannot unwind.
jspi::em_js_data!(
    __em_js____asyncjs__glue_await,
    concat!(
        "(pid)<::>{ ",
        prelude!(),
        " const p = T.promises.get(pid); T.promises.delete(pid); \
         return Asyncify.handleAsync(async () => { try { T.results.set(pid, await p); } catch (e) { T.errors.set(pid, e); } }); }"
    )
);

jspi::em_js_data!(
    __em_js__glue_take_result,
    concat!(
        "(pid)<::>{ ",
        prelude!(),
        " const v = T.results.get(pid); T.results.delete(pid); return typeof v === 'number' ? v : -1; }"
    )
);

jspi::em_js_data!(
    __em_js__glue_take_error,
    concat!(
        "(pid)<::>{ ",
        prelude!(),
        " if (T.errors.has(pid)) { T.errors.delete(pid); return 1; } return 0; }"
    )
);

// Plain (non-promising) host callback: may resolve promises, must never
// block.
jspi::em_js_data!(
    __em_js__glue_call_plain_later,
    "(fnptr, data)<::>{ setTimeout(() => { wasmExports.__indirect_function_table.get(fnptr)(data); }, 0); }"
);

// No-op plain import: exercises the bracket's degradation to an
// identical-bytes no-op on a non-suspending call (including without -sJSPI).
jspi::em_js_data!(__em_js__glue_noop, "()<::>{ }");

#[link(wasm_import_module = "env")]
unsafe extern "C-unwind" {
    safe fn glue_promise_new() -> u32;
    safe fn glue_promise_resolve(pid: u32, value: f64);
    safe fn glue_promise_reject(pid: u32);
    safe fn glue_promise_share(pid: u32) -> u32;
    safe fn glue_take_result(pid: u32) -> f64;
    safe fn glue_take_error(pid: u32) -> i32;
    #[link_name = "__asyncjs__glue_await"]
    safe fn glue_await_import(pid: u32);
    #[link_name = "__asyncjs__glue_sleep"]
    safe fn glue_sleep_import(ms: f64);
    #[link_name = "glue_noop"]
    safe fn glue_noop_import();
    fn glue_run_fiber(fnptr: extern "C-unwind" fn(*mut c_void), data: *mut c_void);
    fn glue_call_plain_later(fnptr: extern "C" fn(*mut c_void), data: *mut c_void);
}

// Fn-pointer bindings of the suspending imports (a fn *item* does not
// unify with `BlockingArgs::Fn` through inference): the `safe fn` import
// declarations above are where their authors vouch for the blocking_call
// obligations — genuine `__asyncjs__` suspending imports (or plain
// non-suspending calls), unit return, rejections never thrown into the
// resume site, no panic after the suspension point.

/// Suspending await of a registered promise.
#[allow(non_upper_case_globals)]
pub const glue_await: extern "C-unwind" fn(u32) = glue_await_import;

/// Suspending sleep.
#[allow(non_upper_case_globals)]
pub const glue_sleep: extern "C-unwind" fn(f64) = glue_sleep_import;

/// Plain no-op import (non-suspending `blocking_call` degradation).
#[allow(non_upper_case_globals)]
pub const glue_noop: extern "C-unwind" fn() = glue_noop_import;

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__glue_run_fiber.as_ptr(),
        __em_js____asyncjs__glue_sleep.as_ptr(),
        __em_js__glue_promise_new.as_ptr(),
        __em_js__glue_promise_resolve.as_ptr(),
        __em_js__glue_promise_reject.as_ptr(),
        __em_js__glue_promise_share.as_ptr(),
        __em_js____asyncjs__glue_await.as_ptr(),
        __em_js__glue_take_result.as_ptr(),
        __em_js__glue_take_error.as_ptr(),
        __em_js__glue_call_plain_later.as_ptr(),
        __em_js__glue_noop.as_ptr(),
    ));
}

/// Ensure the glue's em_js statics are linked. Required before using the
/// raw fn-pointer consts directly if no other glue entry point has run
/// (every public wrapper anchors on its own).
pub fn init() {
    anchor();
}

/// Park the calling activation for `ms` via a host timer.
pub fn sleep(ms: f64) {
    jspi::blocking_call(glue_sleep, (ms,));
}

/// Park the calling activation until the registered promise `pid` settles.
/// Rejection is recorded in the JS registry; fetch with
/// [`TestPromise::take_error`].
pub fn await_pid(pid: u32) {
    jspi::blocking_call(glue_await, (pid,));
}

/// A resolvable/rejectable JS promise handle in the test registry. The
/// registration is single-use: one [`wait`](TestPromise::wait) (or
/// [`await_pid`]) per pid.
#[derive(Clone, Copy)]
pub struct TestPromise(pub u32);

impl TestPromise {
    pub fn new() -> TestPromise {
        anchor();
        TestPromise(glue_promise_new())
    }

    /// Resolve with a value; fetch post-wait with [`take_result`](Self::take_result).
    pub fn resolve_with(&self, value: f64) {
        glue_promise_resolve(self.0, value);
    }

    pub fn resolve(&self) {
        self.resolve_with(0.0);
    }

    pub fn reject(&self) {
        glue_promise_reject(self.0);
    }

    /// A fresh pid waking from this promise's same resolution tick.
    pub fn share(&self) -> u32 {
        glue_promise_share(self.0)
    }

    /// Park until settled (resolution and rejection both return normally;
    /// rejection is recorded for [`take_error`](Self::take_error)).
    pub fn wait(&self) {
        await_pid(self.0);
    }

    /// The resolved value, fetched by plain import post-restore (-1 if
    /// absent or non-numeric). Consumes the entry.
    pub fn take_result(&self) -> f64 {
        glue_take_result(self.0)
    }

    /// Whether a rejection was recorded. Consumes the entry.
    pub fn take_error(&self) -> bool {
        glue_take_error(self.0) != 0
    }
}

impl Default for TestPromise {
    fn default() -> Self {
        Self::new()
    }
}

/// Run `f` on a fresh suspendable activation ("fiber"), entered from the
/// host event loop through a promising-wrapped trampoline whose first
/// statement is `unsafe { jspi::stack_root(...) }`. A panic escaping `f`
/// crosses the promising boundary as a rejection and is rethrown on a fresh
/// tick: fatal and loud.
pub fn run_fiber(f: impl FnOnce() + Send + 'static) {
    anchor();
    extern "C-unwind" fn trampoline(data: *mut c_void) {
        unsafe {
            jspi::stack_root(|| {
                let f = Box::from_raw(data as *mut Box<dyn FnOnce() + Send>);
                f();
            })
        }
    }
    let data: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    unsafe { glue_run_fiber(trampoline, Box::into_raw(data) as *mut c_void) }
}

/// [`run_fiber`] on the safe full-capture root ([`jspi::spawn`]): the
/// trampoline deliberately carries a fat entry frame (2 × `STACK_TOP_PAD`,
/// pattern-checked) — a `stack_root` contract violation, healed under
/// `spawn`'s save-to-base.
pub fn run_fiber_full(f: impl FnOnce() + Send + 'static) {
    anchor();
    extern "C-unwind" fn trampoline(data: *mut c_void) {
        let mut pad = [0u8; 2 * jspi::STACK_TOP_PAD];
        pad.fill(0x6C);
        std::hint::black_box(&mut pad);
        jspi::spawn(|| {
            let f = unsafe { Box::from_raw(data as *mut Box<dyn FnOnce() + Send>) };
            f();
        });
        assert_eq!(
            pad,
            [0x6Cu8; 2 * jspi::STACK_TOP_PAD],
            "run_fiber_full: fat entry frame corrupted"
        );
    }
    let data: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    unsafe { glue_run_fiber(trampoline, Box::into_raw(data) as *mut c_void) }
}

/// Run `f` later as a plain (non-suspendable) host-callback activation:
/// it may resolve promises but can never block (the parity counter denies
/// `blocking_call` from it with a catchable panic).
pub fn call_plain_later(f: impl FnOnce() + 'static) {
    anchor();
    extern "C" fn trampoline(data: *mut c_void) {
        let f = unsafe { Box::from_raw(data as *mut Box<dyn FnOnce()>) };
        f();
    }
    let data: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe { glue_call_plain_later(trampoline, Box::into_raw(data) as *mut c_void) }
}
