//! The one suspending import the crate ships itself: [`sleep`] parks the
//! calling activation on a host timer via [`blocking_call`](crate::blocking_call).

use std::time::Duration;

// Suspending import: parks on a host timeout. Unit return, never rejects;
// `Asyncify.handleAsync` keeps the runtime alive across the suspension.
crate::em_js_data!(
    __em_js____asyncjs__jspi_sleep,
    "(ms)<::>{ return Asyncify.handleAsync(async () => { await new Promise((r) => setTimeout(r, ms)); }); }"
);

#[link(wasm_import_module = "env")]
unsafe extern "C-unwind" {
    #[link_name = "__asyncjs__jspi_sleep"]
    safe fn jspi_sleep_import(ms: f64);
}

const JSPI_SLEEP: extern "C-unwind" fn(f64) = jspi_sleep_import;

#[inline(never)]
fn anchor() {
    std::hint::black_box(__em_js____asyncjs__jspi_sleep.as_ptr());
}

/// Suspend the calling activation for `dur` on a host timer.
///
/// Must be called inside an [`enter_promising`](crate::enter_promising)
/// scope with no bracket in flight, like any other
/// [`blocking_call`](crate::blocking_call); misplacement is denied by
/// parity as a catchable panic.
///
/// # Safety
///
/// Inherits the [`blocking_call`](crate::blocking_call) contract: a
/// suspension point, sound only where the shared-activation invariants of
/// [`enter_promising`](crate::enter_promising) hold.
pub unsafe fn sleep(dur: Duration) {
    anchor();
    // SAFETY: `JSPI_SLEEP` is the `__asyncjs__` import above: a suspending
    // import that resolves on a host timeout, returns unit, never rejects,
    // and re-enters no wasm.
    unsafe {
        crate::blocking_call(JSPI_SLEEP, (dur.as_secs_f64() * 1000.0,));
    }
}
