//! The wasm side of the test suite: a set of `t_*` exports driven from
//! `tests/driver.cjs`. The driver owns all orchestration — it creates and
//! settles promises in `Module.__test`, wraps these exports with
//! `WebAssembly.promising`, and composes interleavings (non-LIFO wakes,
//! same-tick double wakes, overlapping parked entries, exclusive denial)
//! as genuine host-driven reentrancy.
//!
//! Exports return an i32 status: 0 ok, positive = assertion failure site,
//! negative = [`jspi::EnterError`] (-1 Nested, -2 Exclusive, -3 Parked).
//! An escaped panic reaches the driver as a promise rejection.
//!
//! Must be a lib crate: rustc internalizes `#[no_mangle]` statics when
//! compiling a bin, dropping the em_js data exports.

use std::cell::Cell;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

// Suspending await of a driver-registered promise: unit return (values are
// fetched by plain imports after the bracket restores), rejections caught
// into the registry — a JS exception thrown into the resume site would
// enter wasm as a foreign exception that cannot unwind.
jspi::em_js_data!(
    __em_js____asyncjs__t_await,
    "(pid)<::>{ const T = Module.__test; const p = T.promises.get(pid); T.promises.delete(pid); \
     return Asyncify.handleAsync(async () => { try { T.results.set(pid, await p); } catch (e) { T.errors.set(pid, e); } }); }"
);

jspi::em_js_data!(
    __em_js__t_take_result,
    "(pid)<::>{ const T = Module.__test; const v = T.results.get(pid); T.results.delete(pid); return typeof v === 'number' ? v : -1; }"
);

jspi::em_js_data!(
    __em_js__t_take_error,
    "(pid)<::>{ const T = Module.__test; if (T.errors.has(pid)) { T.errors.delete(pid); return 1; } return 0; }"
);

// Plain no-op import: the bracket's degradation to an identical-bytes
// no-op on a non-suspending call (including without -sJSPI).
jspi::em_js_data!(__em_js__t_noop, "()<::>{ }");

#[link(wasm_import_module = "env")]
unsafe extern "C-unwind" {
    #[link_name = "__asyncjs__t_await"]
    safe fn t_await_import(pid: u32);
    safe fn t_take_result(pid: u32) -> f64;
    safe fn t_take_error(pid: u32) -> i32;
    #[link_name = "t_noop"]
    safe fn t_noop_import();
}

// The `safe fn` import declarations above are where the blocking_call
// obligations are vouched for: genuine `__asyncjs__` suspending imports
// (or plain non-suspending calls), unit return, rejections never thrown
// into the resume site, no panic after the suspension point.
const T_AWAIT: extern "C-unwind" fn(u32) = t_await_import;
const T_NOOP: extern "C-unwind" fn() = t_noop_import;

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js____asyncjs__t_await.as_ptr(),
        __em_js__t_take_result.as_ptr(),
        __em_js__t_take_error.as_ptr(),
        __em_js__t_noop.as_ptr(),
    ));
}

/// Called from the module's `main` so the em_js statics and exports are
/// linked from the bin.
pub fn init() {
    anchor();
    std::hint::black_box((
        t_probe as usize,
        t_sleep_check as usize,
        t_park as usize,
        t_sequential as usize,
        t_value as usize,
        t_reject as usize,
        t_nested as usize,
        t_reentrant as usize,
        t_plain_denial as usize,
        t_noop_bracket as usize,
        t_grow as usize,
        t_scribble as usize,
        t_panic as usize,
        t_caught_panic as usize,
        t_park_during_unwind as usize,
        t_exclusive as usize,
        t_enter_check as usize,
        t_exclusive_check as usize,
    ));
}

fn await_pid(pid: u32) {
    unsafe { jspi::blocking_call(T_AWAIT, (pid,)) };
}

fn sleep_ms(ms: f64) {
    unsafe { jspi::sleep(Duration::from_secs_f64(ms / 1000.0)) };
}

fn enter_err(e: jspi::EnterError) -> i32 {
    match e {
        jspi::EnterError::Nested => -1,
        jspi::EnterError::Exclusive => -2,
        jspi::EnterError::Parked => -3,
    }
}

fn enter(f: impl FnOnce() -> i32 + Send + 'static) -> i32 {
    match unsafe { jspi::enter_promising(f) } {
        Ok(v) => v,
        Err(e) => enter_err(e),
    }
}

/// Probe bitmask, callable plain or promising: bit0 `in_promising`,
/// bit1 `exclusive_promising`, bit2 `jspi_enabled`.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_probe() -> i32 {
    (jspi::in_promising() as i32)
        | ((jspi::exclusive_promising() as i32) << 1)
        | ((jspi::jspi_enabled() as i32) << 2)
}

/// Basic park on the crate's own `jspi::sleep` with a live frame buffer.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_sleep_check(ms: f64) -> i32 {
    enter(move || {
        let buf = [0x5Au8; 128];
        black_box(&buf);
        sleep_ms(ms);
        if buf != [0x5Au8; 128] {
            return 1;
        }
        0
    })
}

fn recurse(depth: u32, pid: u32, pattern: u8) -> i32 {
    let mut buf = [0u8; 64];
    buf.fill(pattern ^ depth as u8);
    black_box(&mut buf);
    let r = if depth == 0 {
        await_pid(pid);
        0
    } else {
        recurse(depth - 1, pid, pattern)
    };
    if buf != [pattern ^ depth as u8; 64] {
        return 100 + depth as i32;
    }
    r
}

/// Park at `depth` patterned frames on promise `pid`; every frame is
/// verified on unwind. The driver composes these into overlapping,
/// non-LIFO, and same-tick scenarios.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_park(pid: u32, depth: u32, pattern: u32) -> i32 {
    enter(move || recurse(depth, pid, pattern as u8))
}

/// 50-style sequential save/call/restore cycles with per-iteration buffers.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_sequential(iters: u32) -> i32 {
    enter(move || {
        for i in 0..iters {
            let mut buf = [0u8; 96];
            buf.fill(i as u8 ^ 0x7B);
            black_box(&mut buf);
            sleep_ms(1.0);
            if buf != [i as u8 ^ 0x7B; 96] {
                return 1 + i as i32;
            }
        }
        0
    })
}

/// Await `pid` and return its resolved value, fetched by plain import
/// post-restore (-1 if absent or non-numeric).
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_value(pid: u32) -> f64 {
    unsafe {
        jspi::enter_promising(move || {
            await_pid(pid);
            t_take_result(pid)
        })
    }
    .unwrap_or(-999.0)
}

/// Await a promise the driver rejects: the rejection is recorded, returns
/// normally post-restore, and parking still works afterwards.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_reject(pid: u32) -> i32 {
    enter(move || {
        let buf = [0x51u8; 128];
        black_box(&buf);
        await_pid(pid);
        if t_take_error(pid) == 0 {
            return 1;
        }
        if buf != [0x51u8; 128] {
            return 2;
        }
        sleep_ms(1.0);
        if buf != [0x51u8; 128] {
            return 3;
        }
        0
    })
}

/// Nested scope denial from inside an open activation: Err(Nested) as a
/// value, system fully operational after (bracket still works).
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_nested() -> i32 {
    enter(|| {
        match unsafe { jspi::enter_promising(|| {}) } {
            Err(jspi::EnterError::Nested) => {}
            Err(_) => return 1,
            Ok(()) => return 2,
        }
        sleep_ms(1.0);
        0
    })
}

extern "C-unwind" fn reentrant_shim() {
    unsafe { jspi::blocking_call(T_NOOP, ()) };
}

/// A shim reaching back into blocking_call with a bracket already in
/// flight: denied pre-suspension as a catchable panic that unwinds through
/// the outer bracket, frames intact, system fully operational.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_reentrant() -> i32 {
    enter(|| {
        let buf = [0x24u8; 128];
        black_box(&buf);
        let r = catch_unwind(|| unsafe {
            jspi::blocking_call(reentrant_shim as extern "C-unwind" fn(), ())
        });
        if r.is_ok() {
            return 1;
        }
        if buf != [0x24u8; 128] {
            return 2;
        }
        sleep_ms(1.0);
        if buf != [0x24u8; 128] {
            return 3;
        }
        0
    })
}

/// Called plain by the driver (never promising-wrapped), including while
/// siblings are parked: a plain host activation is not promising
/// (`in_promising` false) and blocking_call from it is denied as a
/// catchable panic.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_plain_denial() -> i32 {
    if jspi::in_promising() {
        return 1;
    }
    let r = catch_unwind(|| unsafe { jspi::blocking_call(T_NOOP, ()) });
    if r.is_ok() {
        return 2;
    }
    0
}

/// Non-suspending bracket degradation (works without -sJSPI too).
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_noop_bracket() -> i32 {
    enter(|| {
        let buf = [0x69u8; 64];
        black_box(&buf);
        unsafe { jspi::blocking_call(T_NOOP, ()) };
        if buf != [0x69u8; 64] {
            return 1;
        }
        0
    })
}

/// Plain export forcing heap growth while siblings are parked: their
/// restores must use fresh heap views.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_grow(mib: u32) -> i32 {
    let big = vec![0x11u8; mib as usize * 1024 * 1024];
    black_box(&big[big.len() - 1]);
    0
}

/// Plain export with a large junk frame: scribbles down through parked
/// activations' live ranges (the corruption-proof scenario's weapon).
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_scribble() -> i32 {
    let mut junk = [0xEEu8; 16 * 1024];
    black_box(&mut junk);
    0
}

/// Park then panic: the escaped panic crosses the promising boundary as a
/// rejection the driver asserts on. `exclusive` != 0 enters exclusively —
/// the driver then verifies the unwind released the exclusive bit.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_panic(pid: u32, exclusive: u32) -> i32 {
    let f = move || -> i32 {
        let buf = [0xD7u8; 96];
        black_box(&buf);
        await_pid(pid);
        panic!("intentional test panic");
    };
    let r = if exclusive != 0 {
        unsafe { jspi::enter_promising_exclusive(f) }
    } else {
        unsafe { jspi::enter_promising(f) }
    };
    match r {
        Ok(v) => v,
        Err(e) => enter_err(e),
    }
}

thread_local! {
    static DROP_SAW_INTACT: Cell<bool> = const { Cell::new(false) };
}

struct DropCheck {
    buf: [u8; 96],
}

impl Drop for DropCheck {
    fn drop(&mut self) {
        DROP_SAW_INTACT.set(self.buf == [0xD7u8; 96]);
    }
}

/// Panic after a bracket returns, caught in the same activation: the
/// unwind runs drops against restored slices, and the activation survives
/// its own caught panic and parks again.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_caught_panic(pid: u32) -> i32 {
    enter(move || {
        DROP_SAW_INTACT.set(false);
        let r = catch_unwind(AssertUnwindSafe(|| {
            let dc = DropCheck { buf: [0xD7u8; 96] };
            black_box(&dc);
            await_pid(pid);
            panic!("intentional test panic");
        }));
        if r.is_ok() {
            return 1;
        }
        if !DROP_SAW_INTACT.get() {
            return 2;
        }
        sleep_ms(1.0);
        0
    })
}

struct ParkOnDrop;

impl Drop for ParkOnDrop {
    fn drop(&mut self) {
        sleep_ms(2.0);
        DROP_SAW_INTACT.set(true);
    }
}

/// A drop that parks while a panic is in flight: wasm-EH-in-flight + JSPI
/// interaction.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_park_during_unwind() -> i32 {
    enter(|| {
        DROP_SAW_INTACT.set(false);
        let r = catch_unwind(|| {
            let pod = ParkOnDrop;
            black_box(&pod);
            let buf = [0xC9u8; 96];
            black_box(&buf);
            panic!("intentional test panic");
        });
        if r.is_ok() {
            return 1;
        }
        if !DROP_SAW_INTACT.get() {
            return 2;
        }
        0
    })
}

/// Exclusive activation: holds the promising lock across its park (the
/// driver verifies denial of every other enter while parked, and release
/// on completion).
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_exclusive(pid: u32) -> i32 {
    match unsafe {
        jspi::enter_promising_exclusive(move || -> i32 {
            if !jspi::exclusive_promising() {
                return 1;
            }
            let buf = [0xE5u8; 128];
            black_box(&buf);
            await_pid(pid);
            if !jspi::exclusive_promising() {
                return 2;
            }
            if buf != [0xE5u8; 128] {
                return 3;
            }
            0
        })
    } {
        Ok(v) => v,
        Err(e) => enter_err(e),
    }
}

/// Attempt a plain enter, callable in any state: 0 = Ok, negative =
/// [`jspi::EnterError`].
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_enter_check() -> i32 {
    match unsafe { jspi::enter_promising(|| 0) } {
        Ok(v) => v,
        Err(e) => enter_err(e),
    }
}

/// Attempt an exclusive enter, callable in any state: 0 = Ok, negative =
/// [`jspi::EnterError`].
#[unsafe(no_mangle)]
pub extern "C-unwind" fn t_exclusive_check() -> i32 {
    match unsafe { jspi::enter_promising_exclusive(|| 0) } {
        Ok(v) => v,
        Err(e) => enter_err(e),
    }
}
